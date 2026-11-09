// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! gpg-agent on this computer, for OpenPGP keys that stay in GnuPG (on disk or on a smartcard):
//! MailCraft asks the agent to sign or decrypt, and the agent asks the owner for a PIN or
//! passphrase in its own pinentry. MailCraft never sees one (#30).
//!
//! [`connect`] finds the agent's socket by asking `gpgconf --list-dirs agent-socket`, run as a
//! program with its arguments and never through a shell (GnuPG, Gpg4win, GPG Suite; looked for
//! where each installs it, then on `PATH`), starts the agent with `gpgconf --launch gpg-agent`
//! when its socket isn't there yet, and opens it: a Unix domain socket, or on Windows (and
//! wherever the name holds such a file) Assuan's socket file, a port on the loopback address and
//! a nonce sent first. GnuPG not being installed is [`Unavailable::NoGnupg`], never a failure of
//! the program. The conversation itself is `mailcraft_assuan::GpgAgent`'s; each call waits for
//! the agent, perhaps while the owner types a PIN, so it is made away from the window's thread.
//!
//! Nothing here is used yet: signing (#30 part 5) and decrypting (part 6) will.

use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mailcraft_assuan::socket_file::{self, SocketFile};
use mailcraft_assuan::{GpgAgent, PinentryHints};

/// How long `gpgconf` may take before it is given up on.
const GPGCONF_TIMEOUT: Duration = Duration::from_secs(10);
/// Most of `gpgconf`'s output read.
const MAX_GPGCONF_OUTPUT: u64 = 64 * 1024;
/// How long one answer of the agent may take: long enough for the owner to find a smartcard and
/// type a PIN into the pinentry, which has its own timeout too.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long sending a line may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long connecting to the loopback port may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why there is no agent to talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// GnuPG isn't installed (no `gpgconf` was found, or it names no agent socket).
    NoGnupg,
    /// GnuPG is installed but its agent couldn't be reached: what went wrong, in words.
    NoAgent(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::NoGnupg => f.write_str("GnuPG isn't installed on this computer"),
            Unavailable::NoAgent(why) => write!(f, "GnuPG's agent couldn't be reached: {why}"),
        }
    }
}

/// A connection to gpg-agent: a Unix domain socket, or a loopback TCP connection.
pub(crate) enum AgentStream {
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
    Tcp(TcpStream),
}

impl AgentStream {
    fn set_timeouts(&self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => {
                s.set_read_timeout(Some(ANSWER_TIMEOUT))?;
                s.set_write_timeout(Some(WRITE_TIMEOUT))
            }
            AgentStream::Tcp(s) => {
                s.set_read_timeout(Some(ANSWER_TIMEOUT))?;
                s.set_write_timeout(Some(WRITE_TIMEOUT))
            }
        }
    }
}

impl Read for AgentStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.read(buf),
            AgentStream::Tcp(s) => s.read(buf),
        }
    }
}

impl Write for AgentStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.write(buf),
            AgentStream::Tcp(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.flush(),
            AgentStream::Tcp(s) => s.flush(),
        }
    }
}

/// Connect to this computer's gpg-agent, with the hints its pinentry needs to show on the owner's
/// screen (from this program's environment).
pub(crate) fn connect() -> Result<GpgAgent<AgentStream>, Unavailable> {
    let gpgconf = find_gpgconf(&gpgconf_candidates(&|name| std::env::var_os(name))).ok_or(Unavailable::NoGnupg)?;
    let socket = agent_socket(&gpgconf).ok_or(Unavailable::NoGnupg)?;
    if !socket.exists() {
        // The agent starts when it is first needed; `gpg` would start it, and so do we.
        let _ = run_gpgconf(&gpgconf, &["--launch", "gpg-agent"]);
    }
    let hints = PinentryHints::from_env(|name| std::env::var(name).ok());
    open(&socket, &hints)
}

/// Open the agent's socket at `socket` and greet it.
pub(crate) fn open(socket: &Path, hints: &PinentryHints) -> Result<GpgAgent<AgentStream>, Unavailable> {
    let stream = open_socket(socket).map_err(|e| Unavailable::NoAgent(format!("its socket couldn't be opened ({})", e.kind())))?;
    stream.set_timeouts().map_err(|e| Unavailable::NoAgent(format!("its socket couldn't be set up ({})", e.kind())))?;
    let mut agent = GpgAgent::open(stream).map_err(|e| Unavailable::NoAgent(e.to_string()))?;
    agent.set_pinentry_hints(hints).map_err(|e| Unavailable::NoAgent(e.to_string()))?;
    Ok(agent)
}

/// Where `gpgconf` may be, in the order it is looked for: where GnuPG's installers put it (a
/// program started from the Finder or the Start menu doesn't have their folders on its `PATH`),
/// then each folder of `PATH`. `var` reads one environment variable.
pub(crate) fn gpgconf_candidates(var: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let exe = if cfg!(windows) { "gpgconf.exe" } else { "gpgconf" };
    if cfg!(windows) {
        // GnuPG's own installer and Gpg4win's.
        for root in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(root) = var(root).filter(|r| !r.is_empty()) {
                let root = PathBuf::from(root);
                out.push(root.join("GnuPG").join("bin").join(exe));
                out.push(root.join("Gpg4win").join("bin").join(exe));
            }
        }
    } else {
        // GPG Suite, Homebrew (Apple silicon, then Intel), MacPorts, the system's own.
        for dir in ["/usr/local/MacGPG2/bin", "/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
            out.push(Path::new(dir).join(exe));
        }
    }
    if let Some(path) = var("PATH") {
        for dir in std::env::split_paths(&path).filter(|d| d.is_absolute()) {
            let candidate = dir.join(exe);
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
    }
    out
}

/// The first candidate that is a file.
pub(crate) fn find_gpgconf(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|c| c.is_file()).cloned()
}

/// The agent's socket, as `gpgconf --list-dirs agent-socket` names it.
fn agent_socket(gpgconf: &Path) -> Option<PathBuf> {
    let output = run_gpgconf(gpgconf, &["--list-dirs", "agent-socket"])?;
    socket_from_output(&output)
}

/// The first line of `gpgconf --list-dirs agent-socket`, unescaped (`gpgconf` writes `%3a` for a
/// colon, as in a Windows drive): an absolute name, or nothing.
pub(crate) fn socket_from_output(output: &[u8]) -> Option<PathBuf> {
    let line = output.split(|b| *b == b'\n').next()?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let mut bytes = Vec::with_capacity(line.len());
    let mut i = 0usize;
    while let Some(&b) = line.get(i) {
        let digit = |at: usize| line.get(at).and_then(|c| (*c as char).to_digit(16));
        if b == b'%'
            && let (Some(h), Some(l)) = (digit(i.saturating_add(1)), digit(i.saturating_add(2)))
        {
            bytes.push(((h << 4) | l) as u8);
            i = i.saturating_add(3);
            continue;
        }
        bytes.push(b);
        i = i.saturating_add(1);
    }
    let text = String::from_utf8(bytes).ok()?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    let path = PathBuf::from(text);
    path.is_absolute().then_some(path)
}

/// Run `gpgconf` with `args` (a program and its arguments, never a shell), for at most
/// [`GPGCONF_TIMEOUT`]; its output when it ends well.
fn run_gpgconf(gpgconf: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new(gpgconf);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for it.
        command.creation_flags(0x0800_0000);
    }
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::info!("gpgconf couldn't be started: {}", e.kind());
            return None;
        }
    };
    let stdout = child.stdout.take()?;
    let reader = std::thread::Builder::new()
        .name("gpgconf".into())
        .spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.take(MAX_GPGCONF_OUTPUT).read_to_end(&mut out);
            out
        })
        .ok();
    let deadline = Instant::now() + GPGCONF_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) | Err(_) => {
                log::warn!("gpgconf didn't finish in time");
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let output = reader.and_then(|r| r.join().ok());
    status.filter(|s| s.success()).and(output)
}

/// Open the socket at `path`: a Unix domain socket, or a socket file (a redirect, or the loopback
/// port and nonce), followed once.
fn open_socket(path: &Path) -> io::Result<AgentStream> {
    open_socket_at(path, true)
}

fn open_socket_at(path: &Path, follow: bool) -> io::Result<AgentStream> {
    let meta = std::fs::metadata(path)?;
    if meta.is_file() {
        if meta.len() > socket_file::MAX_FILE as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "socket file too big"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(socket_file::MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
        return match socket_file::parse(&bytes) {
            Ok(SocketFile::Loopback { port, nonce }) => {
                let mut stream = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), CONNECT_TIMEOUT)?;
                stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
                stream.write_all(&nonce)?;
                Ok(AgentStream::Tcp(stream))
            }
            Ok(SocketFile::Redirect(to)) if follow => open_socket_at(Path::new(&to), false),
            Ok(SocketFile::Redirect(_)) => Err(io::Error::new(io::ErrorKind::InvalidData, "a redirect to a redirect")),
            Err(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "not a socket file")),
        };
    }
    #[cfg(unix)]
    {
        Ok(AgentStream::Unix(std::os::unix::net::UnixStream::connect(path)?))
    }
    #[cfg(not(unix))]
    {
        Err(io::Error::new(io::ErrorKind::InvalidData, "not a socket file"))
    }
}
