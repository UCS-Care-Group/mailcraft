// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Connections: TCP with timeouts, and TLS (rustls, the `ring` provider, Mozilla's roots).
//!
//! Nothing here waits for ever. Connecting has a time limit, and so has the handshake; every
//! read and every write gives up after [`IO_TIMEOUT`] of silence; and a command with its answer
//! gets [`COMMAND_TIMEOUT`] and a second more for every [`BYTES_PER_SECOND`] bytes that moved (at
//! most [`MAX_EXTRA_SECONDS`] more), so that a server which lets a byte through now and then can't keep the thread either. A
//! connection can also be told to [`watch`](Stream::watch) a flag, and then stops at the next
//! read or write once the flag is set.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use mailcraft_model::{Security, Server};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use crate::NetError;

/// How long connecting may take, over all of a server's addresses.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long one read or write may take, so a dead server can't hang the thread.
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a command and its answer may take when nothing much moves...
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);
/// ...and how many bytes have to move for it to be given another second: a large message over a
/// slow line takes the time it takes.
const BYTES_PER_SECOND: u64 = 1024;
/// The most seconds that moved bytes add to a command's time, so that a server which keeps
/// sending can't keep the thread for ever.
const MAX_EXTRA_SECONDS: u64 = 1800;
/// How long a command may take when `moved` bytes have gone through it: its patience, and a second
/// more for every [`BYTES_PER_SECOND`] bytes, up to [`MAX_EXTRA_SECONDS`] more.
fn allowed_time(patience: Duration, moved: u64) -> Duration {
    patience.saturating_add(Duration::from_secs((moved / BYTES_PER_SECOND).min(MAX_EXTRA_SECONDS)))
}

/// How long the TLS handshake may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long saying goodbye may take: nobody waits for the answer.
const FAREWELL_TIMEOUT: Duration = Duration::from_secs(5);
/// Most addresses of one server that are tried.
const MAX_ADDRESSES: usize = 8;

/// What a read or a write fails with once the flag its connection [watches](Stream::watch) is set.
pub(crate) const STOPPED: &str = "Send/Receive was stopped";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Service {
    Imap,
    Smtp,
    /// A web server: the token endpoint of an OAuth sign-in, a name server that answers over HTTPS.
    Http,
}

/// How long a connection waits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Patience {
    /// For connecting, over all of the server's addresses.
    pub(crate) connect: Duration,
    /// For one read or write.
    pub(crate) io: Duration,
    /// For a command and its answer, before the bytes that moved are counted in.
    pub(crate) command: Duration,
}

impl Default for Patience {
    fn default() -> Self {
        Patience { connect: CONNECT_TIMEOUT, io: IO_TIMEOUT, command: COMMAND_TIMEOUT }
    }
}

enum Io {
    Plain(TcpStream),
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
}

/// A connection, encrypted or (to this machine, or before STARTTLS) not.
pub(crate) struct Stream<'a> {
    io: Io,
    /// Looked at before every read and write: once it is set, the connection is done for.
    cancel: Option<&'a AtomicBool>,
    /// How long a command and its answer may take, before the bytes that moved are counted in.
    patience: Duration,
    /// When the command under way was begun, and how many bytes went either way since.
    since: Instant,
    moved: u64,
    /// The last thing done was a read: the next write begins a new command.
    answered: bool,
}

impl<'a> Stream<'a> {
    fn over(io: Io) -> Stream<'a> {
        Stream { io, cancel: None, patience: COMMAND_TIMEOUT, since: Instant::now(), moved: 0, answered: false }
    }

    /// Stop at the next read or write once `cancel` is set (`None`: go on whatever happens, for
    /// a command that must not be left half done). A connection that stopped this way can't be
    /// used any more; its error says [`STOPPED`].
    pub(crate) fn watch(&mut self, cancel: Option<&'a AtomicBool>) {
        self.cancel = cancel;
    }

    /// Only a goodbye is still to be said, and it is not waited for long.
    pub(crate) fn leave(&mut self) {
        self.cancel = None;
        self.patience = FAREWELL_TIMEOUT;
        let tcp = match &self.io {
            Io::Plain(tcp) => tcp,
            Io::Tls(tls) => &tls.sock,
        };
        let _ = tcp.set_read_timeout(Some(FAREWELL_TIMEOUT));
        let _ = tcp.set_write_timeout(Some(FAREWELL_TIMEOUT));
    }

    /// Wait `io` at most for any one read or write from now on (never more than [`IO_TIMEOUT`],
    /// never for ever): for a connection kept open ([`crate::kept`]) and taken up again by a
    /// request with patience of its own.
    pub(crate) fn wait_for(&mut self, io: Duration) -> std::io::Result<()> {
        let tcp = match &self.io {
            Io::Plain(tcp) => tcp,
            Io::Tls(tls) => &tls.sock,
        };
        let io = Some(io.min(IO_TIMEOUT).max(Duration::from_millis(1)));
        tcp.set_read_timeout(io).and_then(|()| tcp.set_write_timeout(io))
    }

    /// What every read and write begins with: the flag, and the time the command has had.
    fn before(&mut self, writing: bool) -> std::io::Result<()> {
        if self.cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
            return Err(std::io::Error::other(STOPPED));
        }
        if writing && self.answered {
            (self.since, self.moved) = (Instant::now(), 0);
        }
        self.answered = !writing;
        let allowed = allowed_time(self.patience, self.moved);
        if self.since.elapsed() > allowed {
            return Err(ErrorKind::TimedOut.into());
        }
        Ok(())
    }
}

impl Read for Stream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.before(false)?;
        let n = match &mut self.io {
            Io::Plain(tcp) => tcp.read(buf),
            Io::Tls(tls) => tls.read(buf),
        }?;
        self.moved = self.moved.saturating_add(n as u64);
        Ok(n)
    }
}

impl Write for Stream<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.before(true)?;
        let n = match &mut self.io {
            Io::Plain(tcp) => tcp.write(buf),
            Io::Tls(tls) => tls.write(buf),
        }?;
        self.moved = self.moved.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.before(true)?;
        match &mut self.io {
            Io::Plain(tcp) => tcp.flush(),
            Io::Tls(tls) => tls.flush(),
        }
    }
}

/// The port to connect to: the server's own, or the usual one for its security when it names none.
pub(crate) fn port(server: &Server, service: Service) -> u16 {
    match (server.port, service, server.security) {
        (0, Service::Imap, Security::Tls) => 993,
        (0, Service::Imap, _) => 143,
        (0, Service::Smtp, Security::Tls) => 465,
        (0, Service::Smtp, _) => 587,
        (0, Service::Http, Security::Tls) => 443,
        (0, Service::Http, _) => 80,
        (port, _, _) => port,
    }
}

/// The host without the brackets an IPv6 address may be written in.
fn bare(host: &str) -> &str {
    let host = host.trim();
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host)
}

/// Whether `host` is this machine: `localhost`, or a loopback address (`127.0.0.1`, `::1`).
pub(crate) fn is_loopback(host: &str) -> bool {
    let host = bare(host);
    host.trim_end_matches('.').eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The server's host, once it is known that connecting won't expose the password.
pub(crate) fn checked_host(server: &Server) -> Result<String, NetError> {
    let host = bare(&server.host);
    if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(NetError::Config("the account's server has no usable name".into()));
    }
    if server.security == Security::None && !is_loopback(host) {
        return Err(NetError::Config(format!(
            "{host} is set to use no encryption, which would show the password to anyone on the network; choose TLS or STARTTLS"
        )));
    }
    Ok(host.to_string())
}

/// Connect to `server`: encrypted from the first byte when it asks for TLS, plain otherwise (the
/// caller then does STARTTLS and [`upgrade`]s, or the server is on this machine). Also returns
/// the host, for error messages and the certificate check. It waits as `patience` says, and for
/// nothing longer than a mail server is given.
pub(crate) fn open<'a>(server: &Server, service: Service, patience: &Patience) -> Result<(Stream<'a>, String), NetError> {
    let host = checked_host(server)?;
    log::debug!("connecting to {host}:{} ({service:?}, {})", port(server, service), server.security.code());
    let (connect, io) = (patience.connect.min(CONNECT_TIMEOUT), patience.io.min(IO_TIMEOUT));
    let tcp = dial(&host, port(server, service), server.security == Security::None, connect, io)?;
    let mut stream = if server.security == Security::Tls { secure(tcp, &host)? } else { Stream::over(Io::Plain(tcp)) };
    stream.patience = patience.command.min(COMMAND_TIMEOUT);
    Ok((stream, host))
}

/// [`open`] for a caller in more of a hurry: connecting may take `connect` at most, and one read
/// or write `io`.
pub(crate) fn open_within<'a>(server: &Server, service: Service, connect: Duration, io: Duration) -> Result<(Stream<'a>, String), NetError> {
    open(server, service, &Patience { connect, io, ..Patience::default() })
}

/// Wrap a connection in TLS after the server agreed to STARTTLS. It goes on as it was set up.
pub(crate) fn upgrade<'a>(stream: Stream<'a>, host: &str) -> Result<Stream<'a>, NetError> {
    let Stream { io, cancel, patience, .. } = stream;
    let mut secured = match io {
        Io::Plain(tcp) => secure(tcp, host)?,
        tls @ Io::Tls(_) => Stream::over(tls),
    };
    (secured.cancel, secured.patience) = (cancel, patience);
    Ok(secured)
}

/// A TCP connection with timeouts: `connect` for connecting, over all of the server's addresses,
/// and `io` for each read and write after. With `loopback_only`, whatever the name resolves to,
/// only addresses of this machine are connected to.
fn dial(host: &str, port: u16, loopback_only: bool, connect: Duration, io: Duration) -> Result<TcpStream, NetError> {
    let failed = |reason: &str| NetError::Connect { host: host.to_string(), reason: reason.to_string() };
    let addresses: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|_| failed("the name couldn't be looked up"))?
        .filter(|a| !loopback_only || a.ip().is_loopback())
        .take(MAX_ADDRESSES)
        .collect();
    let tcp = connect_first(&addresses, connect).map_err(failed)?;
    // No time at all would mean "wait for ever".
    let io = Some(io.max(Duration::from_millis(1)));
    // Without timeouts a silent server would hang the thread: no connection then.
    let ready = tcp.set_read_timeout(io).and_then(|()| tcp.set_write_timeout(io));
    if ready.is_err() {
        return Err(failed("the connection couldn't be set up"));
    }
    let _ = tcp.set_nodelay(true);
    Ok(tcp)
}

/// A TCP connection to the first of `addresses` that answers, all of them within `connect`, or why
/// there is none. Each address gets an equal share of the time left, so one that never answers
/// (an IPv6 address that goes nowhere, say) can't keep the next from being tried, and one that
/// fails at once hands its time on.
fn connect_first(addresses: &[SocketAddr], connect: Duration) -> Result<TcpStream, &'static str> {
    let deadline = Instant::now() + connect;
    let mut reason = "no address was found for the name";
    for (tried, address) in addresses.iter().enumerate() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            reason = "the server didn't answer";
            break;
        }
        let to_try = u32::try_from(addresses.len().saturating_sub(tried)).unwrap_or(u32::MAX).max(1);
        // No time at all would be refused rather than tried.
        let share = (left / to_try).max(Duration::from_millis(1));
        match TcpStream::connect_timeout(address, share) {
            Ok(tcp) => return Ok(tcp),
            Err(e) => {
                reason = match e.kind() {
                    ErrorKind::ConnectionRefused => "the server refused the connection",
                    ErrorKind::TimedOut | ErrorKind::WouldBlock => "the server didn't answer",
                    _ => "the network couldn't get there",
                }
            }
        }
    }
    Err(reason)
}

/// The TLS settings every connection shares: certificates must chain to one of Mozilla's roots
/// and match the server's name.
fn tls_config() -> Result<Arc<ClientConfig>, String> {
    #[cfg(test)]
    if let Some(config) = crate::tls_test_support::client_config() {
        return Ok(config);
    }
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_root_certificates(roots)
                .with_no_client_auth();
            Ok(Arc::new(config))
        })
        .clone()
}

/// Shake hands over `tcp` and check the server's certificate against `host`.
fn secure<'a>(mut tcp: TcpStream, host: &str) -> Result<Stream<'a>, NetError> {
    let failed = |reason: String| NetError::Tls { host: host.to_string(), reason };
    let config = tls_config().map_err(failed)?;
    let name = ServerName::try_from(host.to_string()).map_err(|_| failed("its name isn't one a certificate can be checked against".into()))?;
    let mut connection = ClientConnection::new(config, name).map_err(|e| failed(e.to_string()))?;
    // Shake hands now, so that a bad certificate is reported as one, and within a time limit: no
    // wait in it is longer than what is left of that. Afterwards the connection waits as it did
    // before (never for ever, whatever it was handed over with).
    let io = tcp.read_timeout().ok().flatten().unwrap_or(IO_TIMEOUT).min(IO_TIMEOUT);
    let waits = |tcp: &TcpStream, wait: Duration| {
        let wait = Some(wait.max(Duration::from_millis(1)));
        tcp.set_read_timeout(wait).and_then(|()| tcp.set_write_timeout(wait)).map_err(|_| failed("the connection couldn't be set up".into()))
    };
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    while connection.is_handshaking() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(failed("the server took too long".into()));
        }
        waits(&tcp, io.min(left))?;
        connection.complete_io(&mut tcp).map_err(|e| failed(handshake_problem(&e)))?;
    }
    waits(&tcp, io)?;
    Ok(Stream::over(Io::Tls(Box::new(StreamOwned::new(connection, tcp)))))
}

/// Why a handshake failed, in a few words.
fn handshake_problem(e: &std::io::Error) -> String {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => "the server took too long".into(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => "the server closed the connection".into(),
        // rustls says what is wrong with the certificate or the protocol.
        _ => e.to_string().chars().filter(|c| !c.is_control()).take(200).collect(),
    }
}

#[cfg(test)]
#[path = "transport_dial_tests.rs"]
mod dial_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn server(host: &str, port: u16, security: Security) -> Server {
        Server { host: host.into(), port, security }
    }

    #[test]
    fn default_ports() {
        assert_eq!(port(&server("x", 0, Security::Tls), Service::Imap), 993);
        assert_eq!(port(&server("x", 0, Security::StartTls), Service::Imap), 143);
        assert_eq!(port(&server("x", 0, Security::None), Service::Imap), 143);
        assert_eq!(port(&server("x", 0, Security::Tls), Service::Smtp), 465);
        assert_eq!(port(&server("x", 0, Security::StartTls), Service::Smtp), 587);
        assert_eq!(port(&server("x", 0, Security::None), Service::Smtp), 587);
        assert_eq!(port(&server("x", 0, Security::Tls), Service::Http), 443);
        assert_eq!(port(&server("x", 0, Security::None), Service::Http), 80);
        assert_eq!(port(&server("x", 8443, Security::Tls), Service::Http), 8443);
        assert_eq!(port(&server("x", 2525, Security::Tls), Service::Smtp), 2525);
        assert_eq!(port(&server("x", 1143, Security::None), Service::Imap), 1143);
    }

    #[test]
    fn loopback_hosts() {
        for yes in ["localhost", "LOCALHOST", "localhost.", " 127.0.0.1 ", "127.8.9.10", "::1", "[::1]"] {
            assert!(is_loopback(yes), "{yes}");
        }
        for no in [
            "",
            "mail.example.org",
            "localhost.example.org",
            "notlocalhost",
            "192.168.1.10",
            "10.0.0.1",
            "0.0.0.0",
            "::",
            "[2001:db8::1]",
            "127.0.0.1.example.org",
            "::ffff:192.0.2.1",
        ] {
            assert!(!is_loopback(no), "{no}");
        }
    }

    #[test]
    fn plain_connections_only_to_this_machine() {
        assert_eq!(checked_host(&server("127.0.0.1", 0, Security::None)).as_deref(), Ok("127.0.0.1"));
        assert_eq!(checked_host(&server("[::1]", 0, Security::None)).as_deref(), Ok("::1"));
        assert_eq!(checked_host(&server(" mail.example.org ", 0, Security::Tls)).as_deref(), Ok("mail.example.org"));
        assert_eq!(checked_host(&server("mail.example.org", 0, Security::StartTls)).as_deref(), Ok("mail.example.org"));
        for (host, security) in [
            ("mail.example.org", Security::None),
            ("192.0.2.7", Security::None),
            ("", Security::Tls),
            ("  ", Security::None),
            ("two words", Security::Tls),
        ] {
            assert!(matches!(checked_host(&server(host, 0, security)), Err(NetError::Config(_))), "{host:?}");
        }
        // Refused before any name is looked up or any socket opened.
        let Err(NetError::Config(why)) = open(&server("mail.example.org", 0, Security::None), Service::Imap, &Patience::default()) else {
            panic!("an unencrypted connection was allowed")
        };
        assert!(why.contains("mail.example.org") && why.contains("no encryption"), "{why}");
    }

    #[test]
    fn a_closed_port_is_reported_not_waited_for() {
        // A port that was just free: nothing listens there.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let Err(NetError::Connect { host, reason }) = open(&server("127.0.0.1", port, Security::None), Service::Smtp, &Patience::default()) else {
            panic!("connected to a closed port")
        };
        assert_eq!(host, "127.0.0.1");
        assert!(!reason.is_empty());
    }

    #[test]
    fn the_tls_settings_build() {
        assert!(tls_config().is_ok());
        // A name no certificate could be for is refused before anything is sent.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        assert!(matches!(secure(tcp, "not a host name"), Err(NetError::Tls { .. })));
    }

    /// A server on this machine that does with its one connection what `serve` says.
    fn serving(serve: impl FnOnce(TcpStream) + Send + 'static) -> Server {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((peer, _)) = listener.accept() {
                serve(peer);
            }
        });
        server("127.0.0.1", port, Security::None)
    }

    /// Whatever comes is sent back, for as long as the client stays.
    fn echo(mut peer: TcpStream) {
        let mut buf = [0u8; 64];
        while let Ok(n) = peer.read(&mut buf) {
            if n == 0 || peer.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    }

    fn patience(io: u64, command: u64) -> Patience {
        Patience { io: Duration::from_millis(io), command: Duration::from_millis(command), ..Patience::default() }
    }

    #[test]
    fn a_silent_server_is_given_up_on() {
        // It takes the connection and never says a word.
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let quiet = serving(move |peer| {
            let _ = held.recv();
            drop(peer);
        });
        let (mut stream, _) = open(&quiet, Service::Imap, &patience(200, 60_000)).unwrap();
        let started = Instant::now();
        let error = stream.read(&mut [0u8; 16]).unwrap_err();
        assert!(matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        drop(hold);
        // Nobody is given more time than a mail server gets, whatever is asked for.
        let (stream, _) =
            open(&serving(echo), Service::Imap, &Patience { io: Duration::MAX, command: Duration::MAX, ..Patience::default() }).unwrap();
        assert_eq!(stream.patience, COMMAND_TIMEOUT);
        let Io::Plain(tcp) = &stream.io else { panic!("a plain connection was asked for") };
        assert_eq!((tcp.read_timeout().unwrap(), tcp.write_timeout().unwrap()), (Some(IO_TIMEOUT), Some(IO_TIMEOUT)));
    }

    #[test]
    fn a_server_that_drips_runs_out_of_time() {
        // A byte every 20 ms: no read waits long, and the answer never ends.
        let dripping = serving(|mut peer| {
            while peer.write_all(b"x").is_ok() {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let (mut stream, _) = open(&dripping, Service::Imap, &patience(2_000, 300)).unwrap();
        let started = Instant::now();
        let mut got = 0;
        let error = loop {
            match stream.read(&mut [0u8; 16]) {
                Ok(0) => panic!("the server went away"),
                Ok(n) => got += n,
                Err(e) => break e,
            }
        };
        assert_eq!(error.kind(), ErrorKind::TimedOut, "{error:?}");
        assert!(got > 0 && started.elapsed() < Duration::from_secs(3), "{got} bytes in {:?}", started.elapsed());
    }

    #[test]
    fn every_command_has_its_own_time_and_more_for_what_it_moves() {
        let (mut stream, _) = open(&serving(echo), Service::Imap, &patience(2_000, 150)).unwrap();
        let mut buf = [0u8; 8];
        // Three commands, each well within its time, together longer than one may take.
        for _ in 0..3 {
            stream.write_all(b"ping").unwrap();
            stream.read_exact(&mut buf[..4]).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        // One that is not answered in its time is given up on, however long reads may wait.
        stream.write_all(b"ping").unwrap();
        stream.read_exact(&mut buf[..4]).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(stream.read(&mut buf).unwrap_err().kind(), ErrorKind::TimedOut);
        // A second more for every kilobyte that moved: a large answer over a slow line is not
        // cut off for taking its time.
        let slow = serving(|mut peer| {
            let _ = peer.write_all(&[b'x'; 4 * 1024]);
            std::thread::sleep(Duration::from_millis(300));
            let _ = peer.write_all(b"done");
            let _ = peer.read(&mut [0u8; 1]);
        });
        let (mut stream, _) = open(&slow, Service::Imap, &patience(2_000, 150)).unwrap();
        let mut large = vec![0u8; 4 * 1024 + 4];
        stream.read_exact(&mut large).unwrap();
        assert!(large.ends_with(b"done"));
    }

    #[test]
    fn traffic_buys_a_command_only_so_much_more_time() {
        let base = Duration::from_secs(300);
        assert_eq!(allowed_time(base, 0), base);
        assert_eq!(allowed_time(base, 10 * 1024), base + Duration::from_secs(10));
        let most = base + Duration::from_secs(MAX_EXTRA_SECONDS);
        assert_eq!(allowed_time(base, u64::MAX), most);
        assert_eq!(allowed_time(base, 64 * 1024 * 1024 * 1024), most);
    }

    #[test]
    fn a_watched_connection_stops_when_it_is_told_to() {
        let cancel = AtomicBool::new(false);
        let (mut stream, _) = open(&serving(echo), Service::Imap, &Patience::default()).unwrap();
        stream.watch(Some(&cancel));
        let mut buf = [0u8; 8];
        stream.write_all(b"ping").unwrap();
        stream.read_exact(&mut buf[..4]).unwrap();
        // From the next read or write on, and it says why.
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(stream.read(&mut buf).unwrap_err().to_string(), STOPPED);
        assert_eq!(stream.write(b"ping").unwrap_err().to_string(), STOPPED);
        assert_eq!(stream.flush().unwrap_err().to_string(), STOPPED);
        // Told to go on whatever happens (for what must not be left half done), it does.
        stream.watch(None);
        stream.write_all(b"ping").unwrap();
        stream.read_exact(&mut buf[..4]).unwrap();
        assert_eq!(&buf[..4], b"ping");
        // A goodbye is said whatever the flag says, and is not waited for long.
        stream.watch(Some(&cancel));
        stream.leave();
        stream.write_all(b"bye!").unwrap();
        stream.read_exact(&mut buf[..4]).unwrap();
        let Io::Plain(tcp) = &stream.io else { panic!("a plain connection was asked for") };
        assert_eq!(
            (tcp.read_timeout().unwrap(), tcp.write_timeout().unwrap(), stream.patience),
            (Some(FAREWELL_TIMEOUT), Some(FAREWELL_TIMEOUT), FAREWELL_TIMEOUT)
        );
    }

    #[test]
    fn a_server_that_is_not_speaking_tls_is_a_tls_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut peer, _)) = listener.accept() {
                let _ = peer.write_all(b"* OK plain text, not a handshake\r\n");
                let _ = peer.read(&mut [0u8; 512]);
            }
        });
        // TLS to loopback is allowed; the handshake is what fails.
        let Err(NetError::Tls { host, reason }) = open(&server("localhost", port, Security::Tls), Service::Imap, &Patience::default()) else {
            panic!("a handshake with a plain-text server succeeded")
        };
        assert_eq!(host, "localhost");
        assert!(!reason.is_empty());
    }
}
