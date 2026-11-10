// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The little of HTTP this crate needs: one request (a GET, a form POSTed, or any other with a
//! body of text: [`request`]) over the crate's TLS connections, the answer read back within
//! limits, and the way text is written into URLs and forms. An OAuth sign-in trades its codes
//! for tokens with it, the mail exchangers of a domain are looked up with it, and Microsoft
//! Graph is asked with it.
//!
//! Only `https://` addresses are spoken to; `http://` is for a server on this machine (the
//! tests), the same rule as for mail servers. The answer is as hostile as a mail server's.
//! Redirects are not followed, and of a request and its answer nothing is logged but the
//! answer's status and size.

use std::io::{ErrorKind, Read, Write};
use std::time::{Duration, Instant};

use mailcraft_model::{Security, Server};
use mailcraft_proto::ProtoError;

use crate::NetError;
use crate::transport::{self, Service};

/// Largest answer body taken, unless the one who asks says otherwise ([`Request::max_body`]).
pub(crate) const MAX_BODY: usize = 64 * 1024;
/// Largest answer body anyone may ask for: a whole message of a Microsoft account (Microsoft's
/// own limit is 150 MB). Each caller asks for no more than its own answers need (#175).
pub(crate) const MAX_LARGE_BODY: usize = 160 * 1024 * 1024;
/// Most bytes the answer's status line and headers may take, and most lines.
const MAX_HEAD: usize = 48 * 1024;
const MAX_HEAD_LINES: usize = 200;
/// Longest address spoken to, and the longest that is followed when a server names the address
/// of what comes next ([`Url::parse_within`]).
const MAX_URL: usize = 2_000;
pub(crate) const MAX_LONG_URL: usize = 8_000;
/// The slowest a request's body may go up, in bytes a second: a large body is given more time
/// by this measure, a small one none.
const MIN_UPLOAD_RATE: usize = 16 * 1024;
/// Bytes of a body written at a time, between looks at the clock.
const UPLOAD_CHUNK: usize = 64 * 1024;
/// Most name=value pairs read from a query or a form.
const MAX_PAIRS: usize = 64;
/// What every request says of its sender: nothing about this machine.
const USER_AGENT: &str = "MailCraft";
/// Reads and writes interrupted in a row before giving up.
const MAX_INTERRUPTS: u32 = 1_000;
/// Longest text taken from a server into an error message.
const MAX_SHOWN: usize = 300;

/// Where a request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Url {
    /// Without the brackets of an IPv6 address.
    pub(crate) host: String,
    /// 0 for the usual one.
    pub(crate) port: u16,
    pub(crate) secure: bool,
    /// The path and the query: `/common/oauth2/v2.0/token`.
    pub(crate) path: String,
}

impl Url {
    /// Read `https://host[:port][/path]`. Anything else is refused, `http://` too unless the host is this machine.
    pub(crate) fn parse(text: &str) -> Result<Url, NetError> {
        Url::parse_within(text, MAX_URL)
    }

    /// [`Url::parse`] for an address of up to `max` bytes: one a server sent to be followed (the
    /// next page of a list) carries more than one written here.
    pub(crate) fn parse_within(text: &str, max: usize) -> Result<Url, NetError> {
        let bad = |why: &str| NetError::Config(format!("the server's address {why}"));
        let text = text.trim();
        if text.is_empty() || text.len() > max || !text.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(bad("isn't one"));
        }
        let (secure, rest) = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => (true, rest),
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (false, rest),
            _ => return Err(bad("must start with https://")),
        };
        let (authority, path) = match rest.find(['/', '?', '#']) {
            Some(at) => rest.split_at_checked(at).unwrap_or((rest, "")),
            None => (rest, ""),
        };
        // No user name, no fragment: nothing the addresses spoken to have.
        if authority.contains('@') || path.contains('#') {
            return Err(bad("isn't one"));
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => match v6.split_once(']') {
                Some((host, "")) => (host, None),
                Some((host, port)) => (host, Some(port.strip_prefix(':').ok_or_else(|| bad("isn't one"))?)),
                None => return Err(bad("isn't one")),
            },
            None => match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        let named = !host.is_empty() && host.len() <= 253 && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':'));
        let port = match port {
            Some(port) => port.parse::<u16>().ok().filter(|p| *p != 0),
            None => Some(0),
        };
        let (true, Some(port)) = (named, port) else { return Err(bad("isn't one")) };
        if !secure && !transport::is_loopback(host) {
            return Err(bad("must start with https://"));
        }
        let path = match path.chars().next() {
            Some('/') => path.to_string(),
            _ => format!("/{path}"),
        };
        Ok(Url { host: host.to_string(), port, secure, path })
    }

    /// What the `Host` header says: the name, and the port when it isn't the usual one.
    fn authority(&self) -> String {
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == 0 { host } else { format!("{host}:{}", self.port) }
    }

    /// Whether `other` is the same server spoken to in the same way: the same host, the same
    /// port, encrypted or not alike. What is sent to the one (a token) may then go to the other.
    pub(crate) fn same_server(&self, other: &Url) -> bool {
        let port = |url: &Url| match (url.port, url.secure) {
            (0, true) => 443,
            (0, false) => 80,
            (port, _) => port,
        };
        self.secure == other.secure && self.host.eq_ignore_ascii_case(&other.host) && port(self) == port(other)
    }
}

/// What a server answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
}

/// One request, with everything there is to say about it.
#[derive(Clone, Copy)]
pub(crate) struct Request<'a> {
    /// `GET`, `POST`, `PATCH`, `DELETE`.
    pub(crate) method: &'a str,
    pub(crate) headers: &'a [(&'a str, &'a str)],
    /// Text (JSON, base64) or bytes (a piece of an upload), with the `Content-Type` among the
    /// headers.
    pub(crate) body: Option<&'a [u8]>,
    /// How long connecting may take, and the answer. A large body is given more for going up.
    pub(crate) patience: Duration,
    /// Largest answer body taken (never more than [`MAX_LARGE_BODY`]).
    pub(crate) max_body: usize,
}

/// What a server answered to a [`Request`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    /// What its `Retry-After` header says, when it has one.
    pub(crate) retry_after: Option<String>,
    /// Its `Location` header (where a redirect leads), as the server wrote it.
    pub(crate) location: Option<String>,
    /// Its `WWW-Authenticate` headers (how it offers to be signed in to), as the server wrote them.
    pub(crate) challenges: Vec<String>,
    /// Its `X-EWS-Policy-Reason` header (why an organisation shuts Exchange Web Services).
    pub(crate) policy_reason: Option<String>,
}

/// Most `WWW-Authenticate` headers kept of one answer, and the longest. An NTLM challenge (#252)
/// is Base64 of up to 16 KiB of target information (about 22,000 characters) and is never cut
/// or tidied; the whole head is bounded by [`MAX_HEAD`] in any case.
const MAX_CHALLENGES: usize = 16;
const MAX_CHALLENGE: usize = 24 * 1024;
/// The longest `X-EWS-Policy-Reason` kept.
const MAX_REASON: usize = 1_024;

/// `text` as one piece of a URL or a form: everything but letters, digits and `-._~` as `%XX`.
pub(crate) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// [`encode`] undone, and `+` read as a space. What isn't an escape stays as it is; what isn't UTF-8 becomes U+FFFD.
pub(crate) fn decode(text: &str) -> String {
    let hex = |b: Option<&u8>| b.and_then(|b| char::from(*b).to_digit(16)).and_then(|d| u8::try_from(d).ok());
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match (b, hex(bytes.get(i + 1)), hex(bytes.get(i + 2))) {
            (b'%', Some(high), Some(low)) => {
                out.push(high << 4 | low);
                i += 3;
            }
            (b'+', _, _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `name=value&name=value`, for a form body or a query.
pub(crate) fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(name, value)| format!("{}={}", encode(name), encode(value))).collect::<Vec<_>>().join("&")
}

/// The names and values of a query or a form.
pub(crate) fn pairs(query: &str) -> Vec<(String, String)> {
    let pair = |piece: &str| {
        let (name, value) = piece.split_once('=').unwrap_or((piece, ""));
        (decode(name), decode(value))
    };
    query.split('&').filter(|piece| !piece.is_empty()).take(MAX_PAIRS).map(pair).collect()
}

/// Server text made fit for an error message: one line, no control characters, not too long.
pub(crate) fn tidy(text: &str) -> String {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    let line = words.join(" ");
    if line.chars().count() <= MAX_SHOWN { line } else { line.chars().take(MAX_SHOWN).chain(['…']).collect() }
}

fn problem(text: &str) -> NetError {
    NetError::Proto(ProtoError::Io(text.into()))
}

fn nonsense(text: &str) -> NetError {
    NetError::Proto(ProtoError::Protocol(text.into()))
}

/// What an answer cut off by the server hanging up is called.
const CLOSED: &str = "the server closed the connection";

pub(crate) fn closed() -> NetError {
    problem(CLOSED)
}

/// Whether an error of [`exchange`] says that the server hung up (or never took the request).
pub(crate) fn is_closed(error: &NetError) -> bool {
    matches!(error, NetError::Proto(ProtoError::Io(why)) if why == CLOSED)
}

/// What an answer that didn't come in its time is called.
const TOO_SLOW: &str = "the server took too long to answer";
/// How what is said of an answer that is longer than was asked for begins.
const TOO_LONG: &str = "the server's answer is longer than";

fn too_slow() -> NetError {
    problem(TOO_SLOW)
}

/// Whether an error of [`read_reply`] says that the answer didn't come in the time it had.
pub(crate) fn is_too_slow(error: &NetError) -> bool {
    matches!(error, NetError::Proto(ProtoError::Io(why)) if why == TOO_SLOW)
}

/// Whether an error of [`read_reply`] says that the answer is longer than was asked for.
pub(crate) fn is_too_long(error: &NetError) -> bool {
    matches!(error, NetError::Proto(ProtoError::Protocol(why)) if why.starts_with(TOO_LONG))
}

fn io_problem(e: &std::io::Error) -> NetError {
    match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => too_slow(),
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe => closed(),
        _ => problem(&tidy(&e.to_string())),
    }
}

/// GET `url` (its query included) and read the answer, whatever its status. `headers` go along
/// (`Accept`, say). Connecting may take `patience`, and so may the answer.
pub(crate) fn get(url: &Url, headers: &[(&str, &str)], patience: Duration) -> Result<Answer, NetError> {
    send(url, "GET", headers, None, patience)
}

/// POST `fields` as a form to `url`, asking for JSON, and read the answer, whatever its status.
/// Connecting may take `patience`, and so may the answer.
pub(crate) fn post_form(url: &Url, fields: &[(&str, &str)], patience: Duration) -> Result<Answer, NetError> {
    let headers = [("Accept", "application/json"), ("Content-Type", "application/x-www-form-urlencoded")];
    send(url, "POST", &headers, Some(&form(fields)), patience)
}

/// One request on a connection of its own, and no more of the answer than the usual.
fn send(url: &Url, method: &str, headers: &[(&str, &str)], body: Option<&str>, patience: Duration) -> Result<Answer, NetError> {
    let reply = request(url, &Request { method, headers, body: body.map(str::as_bytes), patience, max_body: MAX_BODY })?;
    Ok(Answer { status: reply.status, body: reply.body })
}

/// One request on a connection of its own; the answer is read whatever its status.
///
/// Connecting may take [`Request::patience`], and so may everything after it together: the
/// request going out and the answer coming back. Only a body too large to go up in that time
/// at the slowest speed that is put up with gets more.
pub(crate) fn request(url: &Url, request: &Request<'_>) -> Result<Reply, NetError> {
    request_with(url, request, None)
}

/// [`request`] over a connection whose TLS settings are `tls` (`None`: the usual ones).
pub(crate) fn request_with(url: &Url, request: &Request<'_>, tls: Option<std::sync::Arc<rustls::ClientConfig>>) -> Result<Reply, NetError> {
    check_method(request.method)?;
    let mut stream = connect_with(url, request.patience, tls)?;
    exchange(&mut stream, url, request, false).map(|(reply, _)| reply)
}

/// A method is a short word in capitals, and nothing else is sent as one.
pub(crate) fn check_method(method: &str) -> Result<(), NetError> {
    if method.is_empty() || method.len() > 10 || !method.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(NetError::Config("that is no way to ask a web server".into()));
    }
    Ok(())
}

/// A new connection to `url`'s server, by the rules of every connection: TLS, or plain to this
/// machine alone. Connecting and every read or write after may take `patience`.
pub(crate) fn connect(url: &Url, patience: Duration) -> Result<transport::Stream<'static>, NetError> {
    connect_with(url, patience, None)
}

/// [`connect`] with TLS settings of the caller's (`None`: the usual ones).
pub(crate) fn connect_with(
    url: &Url,
    patience: Duration,
    tls: Option<std::sync::Arc<rustls::ClientConfig>>,
) -> Result<transport::Stream<'static>, NetError> {
    let server = Server { host: url.host.clone(), port: url.port, security: if url.secure { Security::Tls } else { Security::None } };
    let (stream, _) =
        transport::open_with(&server, Service::Http, &transport::Patience { connect: patience, io: patience, ..Default::default() }, tls)?;
    Ok(stream)
}

/// Send `request` on `stream` and read its answer, whatever its status. With `keep` the
/// request asks for the connection to stay open ([`crate::kept`]), and what comes back says,
/// beside the answer, whether it may carry another request: only when the answer said where
/// it ends (by its length or in chunks), nothing came after it, and the server didn't say it
/// closes. Without `keep` the request says `Connection: close`, and the answer is never
/// followed by another.
pub(crate) fn exchange<S: Read + Write>(stream: &mut S, url: &Url, request: &Request<'_>, keep: bool) -> Result<(Reply, bool), NetError> {
    check_method(request.method)?;
    let started = Instant::now();
    let body: &[u8] = request.body.unwrap_or_default();
    let allowance = Duration::from_secs(u64::try_from(body.len() / MIN_UPLOAD_RATE).unwrap_or(u64::MAX));
    let patience = request.patience.saturating_add(allowance);
    let head = head_of(request.method, url, request.headers, request.body.map(<[u8]>::len), keep);
    // A header or a body may carry secrets: nothing of the request is logged.
    if body.len() <= UPLOAD_CHUNK {
        let mut all = head.into_bytes();
        all.extend_from_slice(body);
        write_all(stream, &all)?;
    } else {
        write_all(stream, head.as_bytes())?;
        // A server that takes its time over every piece is not waited on for ever.
        for piece in body.chunks(UPLOAD_CHUNK) {
            if started.elapsed() >= patience {
                return Err(too_slow());
            }
            write_all(stream, piece)?;
        }
    }
    stream.flush().map_err(|e| io_problem(&e))?;
    let (reply, reusable) = read_reply_kept(stream, started, patience, request.max_body.min(MAX_LARGE_BODY))?;
    log::debug!("{} answered {} ({} bytes)", url.host, reply.status, reply.body.len());
    Ok((reply, keep && reusable))
}

/// The request line and the headers of a request, up to the empty line. With `keep` it asks
/// for the connection to stay open after the answer; without, for it to close.
fn head_of(method: &str, url: &Url, headers: &[(&str, &str)], body: Option<usize>, keep: bool) -> String {
    // The address was checked when it was read: nothing in it can end the line early.
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\n", url.path, url.authority());
    for (name, value) in headers {
        // Nor can a header: a name that isn't one is left out, and a value loses its control
        // characters. Whether the connection stays open is this crate's to say, not a caller's.
        if !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !name.eq_ignore_ascii_case("connection") {
            let value: String = value.chars().filter(|c| !c.is_control()).collect();
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if let Some(length) = body {
        head.push_str(&format!("Content-Length: {length}\r\n"));
    }
    head.push_str(if keep { "Connection: keep-alive\r\n\r\n" } else { "Connection: close\r\n\r\n" });
    head
}

/// How long a [`stream`] may take: `connect` to connect, `idle` for any one read, and `total`
/// for all of it from the request on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StreamPatience {
    pub(crate) connect: Duration,
    pub(crate) idle: Duration,
    pub(crate) total: Duration,
}

/// What a [`stream`]ed answer said of itself, and how much of its body was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Streamed {
    pub(crate) status: u16,
    /// Its `Location` header (where a redirect leads), as the server wrote it.
    pub(crate) location: Option<String>,
    /// Its `Content-Range` header (`bytes 100-199/200`), as the server wrote it.
    pub(crate) content_range: Option<String>,
    /// Its `Content-Length`, when it gave one.
    pub(crate) length: Option<u64>,
    /// Bytes of the body written to the sink.
    pub(crate) written: u64,
}

/// A GET of `url` whose body, when the status is 200 or 206 and `accept` takes what the head
/// says, goes into `sink` as it comes, at most `max` bytes of it (an answer that says it has
/// more is refused before a byte is written). Any other answer is given back with its head
/// alone: its body is not read. An answer cut off halfway is an error, and what came of it
/// before is in the sink.
pub(crate) fn stream(
    url: &Url,
    headers: &[(&str, &str)],
    patience: StreamPatience,
    max: u64,
    accept: &dyn Fn(&Streamed) -> bool,
    sink: &mut dyn Write,
) -> Result<Streamed, NetError> {
    let server = Server { host: url.host.clone(), port: url.port, security: if url.secure { Security::Tls } else { Security::None } };
    let (mut stream, _) = transport::open_within(&server, Service::Http, patience.connect, patience.idle)?;
    let started = Instant::now();
    write_all(&mut stream, head_of("GET", url, headers, None, false).as_bytes())?;
    stream.flush().map_err(|e| io_problem(&e))?;
    let mut reader = Reader { stream: &mut stream, buf: Vec::new(), start: 0, ended: false, started, patience: patience.total };
    let mut head = read_head(&mut reader)?;
    for _ in 0..4 {
        if !(100..200).contains(&head.status) {
            break;
        }
        head = read_head(&mut reader)?;
    }
    let mut said =
        Streamed { status: head.status, location: head.location.clone(), content_range: head.content_range.clone(), length: head.length, written: 0 };
    if !matches!(head.status, 200 | 206) || !accept(&said) {
        return Ok(said);
    }
    let too_long = || nonsense(&format!("{TOO_LONG} {} KB", max / 1024));
    if head.chunked {
        // Every chunk but the last has at least one byte, so the limit also limits the rounds.
        let mut ended = false;
        for _ in 0..=max {
            let line = reader.line(256)?.ok_or_else(closed)?;
            let line = String::from_utf8_lossy(&line);
            let digits = line.split(';').next().unwrap_or("").trim();
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(nonsense("the server's answer is garbled"));
            }
            let size = u64::from_str_radix(digits, 16).unwrap_or(u64::MAX);
            if size == 0 {
                ended = true;
                break;
            }
            if size > max.saturating_sub(said.written) {
                return Err(too_long());
            }
            let poured = reader.pour(size, sink)?;
            said.written = said.written.saturating_add(poured);
            if poured < size {
                return Err(closed());
            }
            match reader.line(2)? {
                Some(rest) if rest.is_empty() => {}
                Some(_) => return Err(nonsense("the server's answer is garbled")),
                None => return Err(closed()),
            }
        }
        if !ended {
            return Err(too_long());
        }
    } else if let Some(length) = head.length {
        if length > max {
            return Err(too_long());
        }
        said.written = reader.pour(length, sink)?;
        if said.written < length {
            return Err(closed());
        }
    } else {
        said.written = reader.pour(max, sink)?;
        if reader.fill()? {
            return Err(too_long());
        }
    }
    log::debug!("{} answered {} ({} bytes)", url.host, said.status, said.written);
    Ok(said)
}

/// Not `write_all`: that retries an interrupted write for ever.
fn write_all<W: Write>(stream: &mut W, mut bytes: &[u8]) -> Result<(), NetError> {
    let mut interrupts = 0;
    while !bytes.is_empty() {
        match stream.write(bytes) {
            Ok(0) => return Err(closed()),
            Ok(n) => bytes = bytes.get(n..).unwrap_or(&[]),
            Err(e) if e.kind() == ErrorKind::Interrupted && interrupts < MAX_INTERRUPTS => interrupts += 1,
            Err(e) => return Err(io_problem(&e)),
        }
    }
    Ok(())
}

/// A stream with the bytes read ahead of what was asked for.
struct Reader<'a, R> {
    stream: &'a mut R,
    /// Bytes read from the stream and not handed out yet (from `start`).
    buf: Vec<u8>,
    start: usize,
    ended: bool,
    /// When the request went out, and how long the answer may take from then.
    started: Instant,
    patience: Duration,
}

impl<R: Read> Reader<'_, R> {
    fn pending(&self) -> &[u8] {
        self.buf.get(self.start..).unwrap_or(&[])
    }

    /// Read some more into the buffer; `false` at the end of the stream.
    fn fill(&mut self) -> Result<bool, NetError> {
        if self.ended {
            return Ok(false);
        }
        if self.started.elapsed() >= self.patience {
            return Err(too_slow());
        }
        if self.start > 0 {
            self.buf.drain(..self.start.min(self.buf.len()));
            self.start = 0;
        }
        let mut chunk = [0u8; 4096];
        let mut interrupts = 0;
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    // A stream that claims more than the buffer holds is lying.
                    self.buf.extend_from_slice(chunk.get(..n.min(chunk.len())).unwrap_or(&[]));
                    return Ok(true);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted && interrupts < MAX_INTERRUPTS => interrupts += 1,
                // A server that hangs up without TLS's goodbye has ended its answer all the same;
                // whether the answer is whole is for its reader to say.
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(io_problem(&e)),
            }
        }
        self.ended = true;
        Ok(false)
    }

    /// The next line without its line ending, no longer than `max`; `None` when the stream ends first.
    fn line(&mut self, max: usize) -> Result<Option<Vec<u8>>, NetError> {
        loop {
            if let Some(end) = self.pending().iter().position(|b| *b == b'\n') {
                let line = self.pending().get(..end).unwrap_or(&[]);
                let line = line.strip_suffix(b"\r").unwrap_or(line).to_vec();
                self.start = self.start.saturating_add(end).saturating_add(1);
                return if line.len() > max { Err(nonsense("the server's answer has a line that is too long")) } else { Ok(Some(line)) };
            }
            // The limit is on the line itself; a CR may still come on top.
            if self.pending().len() > max.saturating_add(1) {
                return Err(nonsense("the server's answer has a line that is too long"));
            }
            if !self.fill()? {
                return Ok(None);
            }
        }
    }

    /// Exactly `n` bytes (the caller has checked that `n` is within limits).
    fn take(&mut self, n: usize) -> Result<Vec<u8>, NetError> {
        while self.pending().len() < n {
            if !self.fill()? {
                return Err(closed());
            }
        }
        let out = self.pending().get(..n).unwrap_or(&[]).to_vec();
        self.start = self.start.saturating_add(n);
        Ok(out)
    }

    /// Up to `n` bytes on into `sink`, as they come; how many were written (fewer only when the
    /// stream ended first).
    fn pour(&mut self, n: u64, sink: &mut dyn Write) -> Result<u64, NetError> {
        let mut written = 0u64;
        while written < n {
            if self.pending().is_empty() && !self.fill()? {
                break;
            }
            let left = usize::try_from(n.saturating_sub(written)).unwrap_or(usize::MAX);
            let pending = self.pending();
            let piece = pending.get(..left.min(pending.len())).unwrap_or(&[]);
            let taken = piece.len();
            sink.write_all(piece).map_err(|e| NetError::Config(format!("the download couldn't be written: {}", tidy(&e.to_string()))))?;
            self.start = self.start.saturating_add(taken);
            written = written.saturating_add(u64::try_from(taken).unwrap_or(u64::MAX));
        }
        Ok(written)
    }

    /// Everything up to the end of the stream, when that is no more than `max`.
    fn rest(&mut self, max: usize) -> Result<Vec<u8>, NetError> {
        while self.pending().len() <= max && self.fill()? {}
        if self.pending().len() > max {
            return Err(too_long(max));
        }
        let out = self.pending().to_vec();
        self.start = self.buf.len();
        Ok(out)
    }
}

fn too_long(max: usize) -> NetError {
    nonsense(&format!("{TOO_LONG} {} KB", max / 1024))
}

/// How the answer says its body ends, and what else is kept of its headers.
#[derive(Default)]
struct Head {
    status: u16,
    length: Option<u64>,
    chunked: bool,
    retry_after: Option<String>,
    location: Option<String>,
    challenges: Vec<String>,
    policy_reason: Option<String>,
    content_range: Option<String>,
    /// The server closes the connection after this answer: it said so, it speaks HTTP/1.0, or
    /// the way the body ends is not one that leaves the connection fit for more (a coding
    /// other than chunked last, or both a length and chunks, which a server never sends).
    close: bool,
}

/// [`read_reply`] with a body of no more than the usual size.
#[cfg(test)]
pub(crate) fn read_answer<R: Read>(stream: &mut R, started: Instant, patience: Duration) -> Result<Answer, NetError> {
    let reply = read_reply(stream, started, patience, MAX_BODY)?;
    Ok(Answer { status: reply.status, body: reply.body })
}

/// Read one HTTP/1.x answer: the status, the body by its `Content-Length`, in chunks, or up to
/// the end of the stream (`max_body` bytes of it at most), and what the answer says of when to
/// come back. It has `patience` from `started` to arrive.
pub(crate) fn read_reply<R: Read>(stream: &mut R, started: Instant, patience: Duration, max_body: usize) -> Result<Reply, NetError> {
    read_reply_kept(stream, started, patience, max_body).map(|(reply, _)| reply)
}

/// [`read_reply`], and whether the connection may carry another request after it: only when
/// the answer's body ended where the answer said it would (its length, its last chunk, or no
/// body by its status), not a byte came after it, and the server didn't say it closes. Never
/// for a body read up to the end of the stream.
pub(crate) fn read_reply_kept<R: Read>(stream: &mut R, started: Instant, patience: Duration, max_body: usize) -> Result<(Reply, bool), NetError> {
    let mut reader = Reader { stream, buf: Vec::new(), start: 0, ended: false, started, patience };
    let mut head = read_head(&mut reader)?;
    // "Go on" before the answer proper (100 Continue): never asked for, tolerated a few times.
    for _ in 0..4 {
        if !(100..200).contains(&head.status) {
            break;
        }
        head = read_head(&mut reader)?;
    }
    // Still "go on": the answer proper may be on its way, and must not be read as the next one's.
    if (100..200).contains(&head.status) {
        return Err(nonsense("the server didn't answer"));
    }
    let (body, delimited) = if matches!(head.status, 204 | 304) {
        (Vec::new(), true)
    } else if head.chunked {
        (read_chunks(&mut reader, max_body)?, true)
    } else if let Some(length) = head.length {
        let length = usize::try_from(length).ok().filter(|n| *n <= max_body).ok_or_else(|| too_long(max_body))?;
        (reader.take(length)?, true)
    } else {
        (reader.rest(max_body)?, false)
    };
    let reusable = delimited && !head.close && !reader.ended && reader.pending().is_empty();
    let reply = Reply {
        status: head.status,
        body,
        retry_after: head.retry_after,
        location: head.location,
        challenges: head.challenges,
        policy_reason: head.policy_reason,
    };
    Ok((reply, reusable))
}

fn read_head<R: Read>(reader: &mut Reader<'_, R>) -> Result<Head, NetError> {
    let not_http = || nonsense("the server didn't answer in HTTP");
    let line = reader.line(MAX_HEAD)?.ok_or_else(closed)?;
    let line = String::from_utf8_lossy(&line);
    // `HTTP/1.1 200 OK`.
    let mut words = line.split(' ').filter(|w| !w.is_empty());
    let (version, status) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let status = status.parse::<u16>().ok().filter(|s| status.len() == 3 && (100..600).contains(s));
    let (true, Some(status)) = (version.starts_with("HTTP/1."), status) else { return Err(not_http()) };
    // Only HTTP/1.1 keeps a connection open unless told otherwise.
    let mut head = Head { status, close: !version.starts_with("HTTP/1.1"), ..Default::default() };
    let mut seen = line.len();
    for _ in 0..MAX_HEAD_LINES {
        let line = reader.line(MAX_HEAD)?.ok_or_else(closed)?;
        if line.is_empty() {
            return Ok(head);
        }
        seen = seen.saturating_add(line.len());
        if seen > MAX_HEAD {
            break;
        }
        let line = String::from_utf8_lossy(&line);
        let Some((name, value)) = line.split_once(':') else { return Err(not_http()) };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            // Two lengths that differ are no length at all.
            let length = value.parse::<u64>().ok().filter(|n| head.length.is_none_or(|earlier| earlier == *n)).ok_or_else(not_http)?;
            head.length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            head.chunked = value.rsplit(',').next().is_some_and(|last| last.trim().eq_ignore_ascii_case("chunked"));
            // A body whose coding doesn't end in chunks runs to the end of the connection.
            head.close |= !head.chunked;
        } else if name.eq_ignore_ascii_case("connection") {
            head.close |= value.split(',').any(|token| token.trim().eq_ignore_ascii_case("close"));
        } else if name.eq_ignore_ascii_case("retry-after") {
            head.retry_after = Some(tidy(value));
        } else if name.eq_ignore_ascii_case("location") {
            head.location = Some(value.to_string());
        } else if name.eq_ignore_ascii_case("www-authenticate") {
            if head.challenges.len() < MAX_CHALLENGES {
                head.challenges.push(value.trim().chars().filter(|c| !c.is_control()).take(MAX_CHALLENGE).collect());
            }
        } else if name.eq_ignore_ascii_case("x-ews-policy-reason") {
            head.policy_reason = Some(tidy(value).chars().take(MAX_REASON).collect());
        } else if name.eq_ignore_ascii_case("content-range") {
            head.content_range = Some(value.to_string());
        }
        // A length beside chunks: the chunks are read, and the connection is not trusted after.
        head.close |= head.chunked && head.length.is_some();
    }
    Err(nonsense("the server's answer has too many headers"))
}

/// A body sent in chunks: `<size in hex>[;extension]`, that many bytes, a line ending; a chunk
/// of no bytes ends it. No more than `max` bytes of it are taken.
fn read_chunks<R: Read>(reader: &mut Reader<'_, R>, max: usize) -> Result<Vec<u8>, NetError> {
    let broken = || nonsense("the server's answer is garbled");
    let mut body = Vec::new();
    // Every chunk but the last has at least one byte, so the body's limit also limits the rounds.
    for _ in 0..=max {
        let line = reader.line(256)?.ok_or_else(closed)?;
        let line = String::from_utf8_lossy(&line);
        let digits = line.split(';').next().unwrap_or("").trim();
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(broken());
        }
        // A size beyond u64 is too long like any other that is.
        let size = u64::from_str_radix(digits, 16).ok().and_then(|n| usize::try_from(n).ok()).unwrap_or(usize::MAX);
        if size == 0 {
            // Trailers, if any, up to an empty line or the end of the stream.
            for _ in 0..MAX_HEAD_LINES {
                match reader.line(MAX_HEAD)? {
                    Some(trailer) if !trailer.is_empty() => {}
                    _ => return Ok(body),
                }
            }
            return Err(broken());
        }
        if size > max.saturating_sub(body.len()) {
            return Err(too_long(max));
        }
        body.extend_from_slice(&reader.take(size)?);
        match reader.line(2)? {
            Some(rest) if rest.is_empty() => {}
            Some(_) => return Err(broken()),
            None => return Err(closed()),
        }
    }
    Err(too_long(max))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    const SOON: Duration = Duration::from_secs(5);

    fn answer(bytes: &[u8]) -> Result<Answer, NetError> {
        read_answer(&mut &bytes[..], Instant::now(), SOON)
    }

    /// The answer's status and body as text.
    fn read(text: &str) -> (u16, String) {
        let answer = answer(text.as_bytes()).unwrap();
        (answer.status, String::from_utf8(answer.body).unwrap())
    }

    fn protocol(result: Result<Answer, NetError>) -> String {
        match result {
            Err(NetError::Proto(ProtoError::Protocol(why))) => why,
            other => panic!("expected an answer that makes no sense, got {other:?}"),
        }
    }

    #[test]
    fn addresses() {
        let token = url("https://login.microsoftonline.com/common/oauth2/v2.0/token");
        assert_eq!(token, Url { host: "login.microsoftonline.com".into(), port: 0, secure: true, path: "/common/oauth2/v2.0/token".into() });
        assert_eq!(token.authority(), "login.microsoftonline.com");
        assert_eq!(url(" HTTPS://Example.org:8443 "), Url { host: "Example.org".into(), port: 8443, secure: true, path: "/".into() });
        assert_eq!(url("https://example.org:8443/a?b=c").authority(), "example.org:8443");
        assert_eq!(url("https://example.org?b=c").path, "/?b=c");
        // Plain HTTP only to this machine.
        assert_eq!(url("http://127.0.0.1:8080/token"), Url { host: "127.0.0.1".into(), port: 8080, secure: false, path: "/token".into() });
        assert_eq!(url("http://localhost/token").port, 0);
        let v6 = url("http://[::1]:8080/token");
        assert_eq!((v6.host.as_str(), v6.port, v6.authority().as_str()), ("::1", 8080, "[::1]:8080"));
        assert_eq!(url("https://[2001:db8::1]/x").host, "2001:db8::1");
        for bad in [
            "",
            "login.example.org/token",
            "http://login.example.org/token",
            "http://192.0.2.7/token",
            "http://localhost.example.org/token",
            "ftp://localhost/token",
            "file:///etc/passwd",
            "https://",
            "https:///token",
            "https://user:secret@example.org/token",
            "https://example.org/token#fragment",
            "https://example.org:0/token",
            "https://example.org:99999/token",
            "https://example.org:port/token",
            "https://exam ple.org/token",
            "https://example.org/to\r\nken",
            "https://exämple.org/token",
            "https://[::1/token",
            "https://[::1]8080/token",
            "https://example.org\\token",
        ] {
            assert!(matches!(Url::parse(bad), Err(NetError::Config(_))), "{bad:?}");
        }
        assert!(matches!(Url::parse(&format!("https://example.org/{}", "a".repeat(MAX_URL))), Err(NetError::Config(_))));
    }

    #[test]
    fn text_in_urls_and_forms() {
        assert_eq!(encode("AZaz09-._~"), "AZaz09-._~");
        assert_eq!(encode("a b+c&d=e/f?g#h%i"), "a%20b%2Bc%26d%3De%2Ff%3Fg%23h%25i");
        assert_eq!(encode("é\r\n"), "%C3%A9%0D%0A");
        assert_eq!(decode("a%20b+c%2Bd%C3%A9"), "a b c+dé");
        // What isn't an escape stays; what isn't UTF-8 doesn't break anything.
        assert_eq!(decode("100%, %zz %4"), "100%, %zz %4");
        assert_eq!(decode("%FF%"), "\u{FFFD}%");
        assert_eq!(form(&[("scope", "a b"), ("x&y", "1=2"), ("empty", "")]), "scope=a%20b&x%26y=1%3D2&empty=");
        assert_eq!(
            pairs("code=a%2Fb&state=xyz&&flag&=nameless&q=1=2"),
            [("code", "a/b"), ("state", "xyz"), ("flag", ""), ("", "nameless"), ("q", "1=2")].map(|(n, v)| (n.to_string(), v.to_string()))
        );
        assert!(pairs("").is_empty());
        assert_eq!(pairs(&"a=1&".repeat(1_000)).len(), MAX_PAIRS);
    }

    #[test]
    fn server_text_for_error_messages() {
        assert_eq!(tidy("  AADSTS70008:\tThe grant\r\n has expired. \u{7}"), "AADSTS70008: The grant has expired.");
        let long = tidy(&"é".repeat(1_000));
        assert_eq!(long.chars().count(), MAX_SHOWN + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn answers_with_a_length() {
        assert_eq!(
            read("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}and more"),
            (200, "{\"a\":1}".into())
        );
        assert_eq!(read("HTTP/1.1 400 Bad Request\r\ncontent-length:2\r\n\r\nno"), (400, "no".into()));
        // Bare line feeds, a status without words, a body of nothing.
        assert_eq!(read("HTTP/1.0 200\nContent-Length: 2\n\nok"), (200, "ok".into()));
        assert_eq!(read("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), (200, String::new()));
        assert_eq!(read("HTTP/1.1 204 No Content\r\n\r\n"), (204, String::new()));
        // The same length twice is one length.
        assert_eq!(read("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nok"), (200, "ok".into()));
        // "Go on" before the answer.
        assert_eq!(read("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"), (200, "ok".into()));
    }

    #[test]
    fn answers_up_to_the_end_of_the_stream() {
        assert_eq!(read("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"a\":1}"), (200, "{\"a\":1}".into()));
        assert_eq!(read("HTTP/1.1 500 Oops\r\n\r\n"), (500, String::new()));
        let full = format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(MAX_BODY));
        assert_eq!(answer(full.as_bytes()).unwrap().body.len(), MAX_BODY);
    }

    #[test]
    fn answers_in_chunks() {
        let chunked = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert_eq!(read(&format!("{chunked}4\r\n{{\"a\"\r\n3;ext=1\r\n:1}}\r\n0\r\n\r\n")), (200, "{\"a\":1}".into()));
        assert_eq!(read(&format!("{chunked}A\r\n0123456789\r\n0\r\nTrailer: x\r\n\r\n")), (200, "0123456789".into()));
        assert_eq!(read(&format!("{chunked}0\r\n\r\n")), (200, String::new()));
        // Chunks win over a length; the end of the stream may stand in for the last empty line.
        assert_eq!(read("HTTP/1.1 200 OK\r\nContent-Length: 99\r\nTransfer-Encoding: gzip, Chunked\r\n\r\n2\r\nok\r\n0\r\n"), (200, "ok".into()));
        for garbled in ["zz\r\nok\r\n0\r\n\r\n", "\r\nok", "2\r\nokay\r\n0\r\n\r\n", "-2\r\nok\r\n", "2 \r\nok\r\n0x0\r\n"] {
            assert_eq!(protocol(answer(format!("{chunked}{garbled}").as_bytes())), "the server's answer is garbled", "{garbled:?}");
        }
        for cut in ["", "5\r\nab", "2\r\nok", "2\r\nok\r\n"] {
            let result = answer(format!("{chunked}{cut}").as_bytes());
            assert!(matches!(result, Err(NetError::Proto(ProtoError::Io(_)))), "{cut:?}: {result:?}");
        }
    }

    #[test]
    fn answers_that_are_too_long_are_refused_unread() {
        let claimed = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\nxx", MAX_BODY + 1);
        assert!(protocol(answer(claimed.as_bytes())).contains("longer than 64 KB"));
        let claimed = "HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999\r\n\r\nxx";
        assert!(matches!(answer(claimed.as_bytes()), Err(NetError::Proto(ProtoError::Protocol(_)))));
        let endless = format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(MAX_BODY + 1));
        assert!(protocol(answer(endless.as_bytes())).contains("longer than 64 KB"));
        for size in ["10001", "ffffffffffffffff", "fffffffffffffffffffffffffffff"] {
            let chunk = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{size}\r\nxx");
            assert!(protocol(answer(chunk.as_bytes())).contains("longer than 64 KB"), "{size}");
        }
        // Chunks that are only too much together.
        let chunk = format!("8000\r\n{}\r\n", "x".repeat(0x8000));
        let full = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}{chunk}0\r\n\r\n");
        assert_eq!(answer(full.as_bytes()).unwrap().body.len(), MAX_BODY);
        let drip = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}{chunk}1\r\nx\r\n0\r\n\r\n");
        assert!(protocol(answer(drip.as_bytes())).contains("longer than 64 KB"));
        // Headers without end, and one header that never ends.
        let headers = format!("HTTP/1.1 200 OK\r\n{}\r\nok", "X-Filler: yes\r\n".repeat(MAX_HEAD_LINES + 1));
        assert!(matches!(answer(headers.as_bytes()), Err(NetError::Proto(ProtoError::Protocol(_)))));
        let header = format!("HTTP/1.1 200 OK\r\nX-Filler: {}\r\n\r\nok", "x".repeat(MAX_HEAD * 2));
        assert!(matches!(answer(header.as_bytes()), Err(NetError::Proto(ProtoError::Protocol(_)))));
    }

    #[test]
    fn answers_that_are_not_http_are_refused() {
        for junk in [
            "* OK IMAP4rev1 ready\r\n",
            "220 smtp.example.org ESMTP\r\n",
            "HTTP/2 200\r\n\r\n",
            "HTTP/1.1 OK\r\n\r\n",
            "HTTP/1.1 20 OK\r\n\r\n",
            "HTTP/1.1 2000 OK\r\n\r\n",
            "HTTP/1.1 999 What\r\n\r\n",
            "HTTP/1.1 +20 OK\r\n\r\n",
            "\r\n\r\n",
            "HTTP/1.1 200 OK\r\nno colon here\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: many\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\nokk",
            "<html>not found</html>\n",
        ] {
            assert!(matches!(answer(junk.as_bytes()), Err(NetError::Proto(ProtoError::Protocol(_)))), "{junk:?}");
        }
        // A server that hangs up early.
        for cut in ["", "HTTP/1.1 200 OK", "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab"] {
            assert_eq!(answer(cut.as_bytes()), Err(NetError::Proto(ProtoError::Io("the server closed the connection".into()))), "{cut:?}");
        }
    }

    /// A stream that fails once what it has is read.
    struct Failing(Vec<u8>, ErrorKind);

    impl Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0.is_empty() {
                return Err(std::io::Error::from(self.1));
            }
            let n = self.0.len().min(buf.len()).min(3);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn stream_problems_end_the_reading() {
        let now = Instant::now();
        let head = b"HTTP/1.1 200 OK\r\n\r\n{\"a\":1}".to_vec();
        // TLS torn down without a goodbye is the end of a body that runs to the end.
        assert_eq!(read_answer(&mut Failing(head.clone(), ErrorKind::UnexpectedEof), now, SOON).unwrap().body, b"{\"a\":1}");
        let slow = read_answer(&mut Failing(head.clone(), ErrorKind::TimedOut), now, SOON);
        assert_eq!(slow, Err(NetError::Proto(ProtoError::Io("the server took too long to answer".into()))));
        assert!(matches!(read_answer(&mut Failing(head.clone(), ErrorKind::Interrupted), now, SOON), Err(NetError::Proto(ProtoError::Io(_)))));
        // An answer that isn't finished when its time is up.
        let late = read_answer(&mut &head[..], now, Duration::ZERO);
        assert_eq!(late, Err(NetError::Proto(ProtoError::Io("the server took too long to answer".into()))));
    }

    #[test]
    fn longer_addresses_and_the_same_server() {
        let long = format!("https://graph.example.org/v1.0/me/contacts?$skiptoken={}", "a".repeat(MAX_URL));
        assert!(matches!(Url::parse(&long), Err(NetError::Config(_))));
        assert_eq!(Url::parse_within(&long, MAX_LONG_URL).unwrap().host, "graph.example.org");
        let longer = format!("https://graph.example.org/{}", "a".repeat(MAX_LONG_URL));
        assert!(matches!(Url::parse_within(&longer, MAX_LONG_URL), Err(NetError::Config(_))));

        let graph = url("https://graph.example.org/v1.0/me");
        for same in ["https://graph.example.org/other?x=1", "https://GRAPH.Example.ORG:443/", "https://graph.example.org:443"] {
            assert!(graph.same_server(&url(same)), "{same}");
        }
        for other in [
            "https://graph.example.org:8443/v1.0/me",
            "https://graph.example.org.evil.example/v1.0/me",
            "https://evil.example/graph.example.org",
            "https://graph.example.org./v1.0/me",
            "https://xgraph.example.org/v1.0/me",
            "https://example.org/v1.0/me",
        ] {
            assert!(!graph.same_server(&url(other)), "{other}");
        }
        // Encrypted or not is part of it, and so is the port.
        let local = url("http://127.0.0.1:8080/a");
        assert!(local.same_server(&url("http://127.0.0.1:8080/b")));
        for other in ["https://127.0.0.1:8080/b", "http://127.0.0.1:8081/b", "http://localhost:8080/b", "http://127.0.0.1/b"] {
            assert!(!local.same_server(&url(other)), "{other}");
        }
        assert!(url("http://localhost/a").same_server(&url("http://LOCALHOST:80/b")));
    }

    #[test]
    fn larger_answers_for_those_who_ask() {
        let body = "x".repeat(MAX_BODY + 1);
        let reply = |answer: &str, max: usize| read_reply(&mut answer.as_bytes(), Instant::now(), SOON, max);
        let too_long = |result: Result<Reply, NetError>| match result {
            Err(NetError::Proto(ProtoError::Protocol(why))) => why,
            other => panic!("expected an answer that is too long, got {:?}", other.map(|reply| reply.body.len())),
        };
        // By its length, in chunks, and up to the end of the stream.
        let chunked = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len());
        for answer in [format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()), chunked, format!("HTTP/1.1 200 OK\r\n\r\n{body}")]
        {
            assert_eq!(reply(&answer, MAX_BODY + 1).unwrap().body.len(), MAX_BODY + 1);
            assert_eq!(reply(&answer, MAX_LARGE_BODY).unwrap().body.len(), MAX_BODY + 1);
            assert_eq!(too_long(reply(&answer, MAX_BODY)), "the server's answer is longer than 64 KB");
            assert_eq!(too_long(reply(&answer, 2 * 1024)), "the server's answer is longer than 2 KB");
            assert!(reply(&answer, 0).is_err());
        }
    }

    #[test]
    fn an_answer_over_16_mb_comes_whole_to_one_who_asks_for_it() {
        use crate::fake::FakeHttp;

        // A whole message of a Microsoft account may be larger than 16 MB (#175).
        let size = 17 * 1024 * 1024 + 3;
        let mut answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n").into_bytes();
        answer.resize(answer.len() + size, b'm');
        let server = FakeHttp::answering(answer);
        let at = url(&server.url("/v1.0/me/messages/AAMk/$value"));
        let ask = |max_body: usize| request(&at, &Request { method: "GET", headers: &[], body: None, patience: SOON, max_body });
        assert_eq!(ask(MAX_LARGE_BODY).unwrap().body.len(), size);
        assert_eq!(ask(size).unwrap().body.len(), size);
        assert_eq!(ask(usize::MAX).unwrap().body.len(), size, "within the ceiling");
        // Everyone else still has their own limit.
        assert!(matches!(ask(16 * 1024 * 1024), Err(NetError::Proto(ProtoError::Protocol(why))) if why.contains("longer than 16384 KB")));
        assert!(matches!(ask(MAX_BODY), Err(NetError::Proto(ProtoError::Protocol(why))) if why.contains("longer than 64 KB")));
    }

    #[test]
    fn when_to_come_back_is_kept() {
        let reply = |headers: &str| {
            let answer = format!("HTTP/1.1 429 Too Many Requests\r\n{headers}Content-Length: 2\r\n\r\n{{}}");
            read_reply(&mut answer.as_bytes(), Instant::now(), SOON, MAX_BODY).unwrap()
        };
        assert_eq!(reply("Retry-After: 30\r\n"), Reply { status: 429, body: b"{}".to_vec(), retry_after: Some("30".into()), ..Default::default() });
        assert_eq!(reply("retry-after:  Wed, 21 Oct 2026 07:28:00 GMT \r\n").retry_after.as_deref(), Some("Wed, 21 Oct 2026 07:28:00 GMT"));
        assert_eq!(reply("X-Other: 1\r\n").retry_after, None);
        // Cut to size and to one line, like any text from a server.
        let long = reply(&format!("Retry-After: 1\u{7}2 {}\r\n", "9".repeat(2_000))).retry_after.unwrap();
        assert!(long.starts_with("1 2 9") && long.chars().count() <= MAX_SHOWN + 1, "{long}");
    }

    #[test]
    fn requests_with_a_body_and_a_say_in_how_much_comes_back() {
        use crate::fake::{FakeHttp, json_answer};

        let server = FakeHttp::answering(json_answer("200 OK", &"y".repeat(3 * MAX_BODY)));
        let at = url(&server.url("/v1.0/me/sendMail?x=1"));
        let headers = [("Authorization", "Bearer tok\r\nen"), ("Content-Type", "text/plain"), ("Bad Name", "x"), ("", "y")];
        let ask = |method: &str, body: Option<&str>, max_body: usize| {
            request(&at, &Request { method, headers: &headers, body: body.map(str::as_bytes), patience: SOON, max_body })
        };

        let reply = ask("POST", Some("aGVsbG8="), 4 * MAX_BODY).unwrap();
        assert_eq!((reply.status, reply.body.len(), reply.retry_after), (200, 3 * MAX_BODY, None));
        let sent = server.request(0);
        assert!(sent.starts_with("POST /v1.0/me/sendMail?x=1 HTTP/1.1\r\n") && sent.ends_with("\r\n\r\naGVsbG8="), "{sent}");
        for header in [
            "Authorization: Bearer token\r\n",
            "Content-Type: text/plain\r\n",
            "Content-Length: 8\r\n",
            "User-Agent: MailCraft\r\n",
            "Connection: close\r\n",
        ] {
            assert!(sent.contains(header), "{header:?}: {sent}");
        }
        assert!(!sent.contains("Bad Name") && !sent.contains("\r\n: y"), "{sent}");

        // No more comes back than was asked for, or than anyone may ask for.
        assert!(matches!(ask("GET", None, MAX_BODY), Err(NetError::Proto(ProtoError::Protocol(why))) if why.contains("longer than 64 KB")));
        assert_eq!(ask("PATCH", Some(""), usize::MAX).unwrap().body.len(), 3 * MAX_BODY);
        // A body of nothing still says its length; no body says nothing of one.
        assert!(server.request(2).starts_with("PATCH ") && server.request(2).contains("Content-Length: 0\r\n"));
        assert!(ask("DELETE", None, 4 * MAX_BODY).is_ok());
        assert!(server.request(3).starts_with("DELETE ") && !server.request(3).to_lowercase().contains("content-length"));

        // A body that goes up in pieces arrives whole.
        let large = "z".repeat(3 * UPLOAD_CHUNK + 5);
        assert!(ask("POST", Some(&large), 4 * MAX_BODY).is_ok());
        let sent = server.request(4);
        assert!(sent.contains(&format!("Content-Length: {}\r\n", large.len())) && sent.ends_with(&large));

        // What is no way of asking is not sent at all.
        for bad in ["", "get", "Post", "GET /evil HTTP/1.1\r\nX-Evil:", "G E T", "ELEVENCHARS"] {
            assert!(matches!(ask(bad, None, MAX_BODY), Err(NetError::Config(_))), "{bad:?}");
        }
        assert_eq!(server.asked(), 5);
    }

    proptest! {
        #[test]
        fn arbitrary_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
            let _ = answer(&bytes);
            let mut after_head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            after_head.extend_from_slice(&bytes);
            let _ = answer(&after_head);
        }

        #[test]
        fn answer_shaped_noise_never_panics(
            status in "(HTTP/1\\.[01] )?[0-9]{0,4}( [A-Za-z ]{0,10})?",
            headers in proptest::collection::vec(("(Content-Length|Transfer-Encoding|X-Other)?", "[:;]? ?", "(chunked|[0-9a-f]{0,6}|-1|gzip)"), 0..6),
            body in proptest::collection::vec(prop_oneof![Just("0\r\n".to_string()), Just("\r\n".to_string()), "[0-9a-f]{1,3}\r\n", "[ -~]{0,20}"], 0..12),
        ) {
            let headers: String = headers.iter().map(|(name, sep, value)| format!("{name}{sep}{value}\r\n")).collect();
            let text = format!("{status}\r\n{headers}\r\n{}", body.concat());
            if let Ok(answer) = answer(text.as_bytes()) {
                prop_assert!(answer.body.len() <= MAX_BODY);
            }
        }

        #[test]
        fn any_text_survives_a_form(name in "\\PC{0,20}", value in any::<String>()) {
            let written = form(&[(name.as_str(), value.as_str())]);
            prop_assert!(written.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%=".contains(&b)));
            prop_assert_eq!(pairs(&written), vec![(name, value)]);
        }

        #[test]
        fn any_text_is_an_address_or_not(text in "\\PC{0,60}", tail in "[a-z0-9:/?#@\\[\\].-]{0,30}") {
            let _ = Url::parse(&text);
            for scheme in ["https://", "http://"] {
                if let Ok(url) = Url::parse(&format!("{scheme}{tail}")) {
                    prop_assert!(url.secure || transport::is_loopback(&url.host));
                    prop_assert!(url.path.starts_with('/'));
                }
            }
        }

        #[test]
        fn decoding_never_panics(text in "[%+0-9a-fA-F\\PC]{0,40}") {
            let _ = decode(&text);
            let _ = pairs(&text);
            let _ = tidy(&text);
        }
    }
}
