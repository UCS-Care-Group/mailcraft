// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! gpg-agent on this computer, for OpenPGP keys that stay in GnuPG (on disk or on a smartcard):
//! MailCraft asks the agent to sign or decrypt, and the agent asks the owner for a PIN or
//! passphrase in its own pinentry. MailCraft never sees one (#30).
//!
//! [`connect`] finds `gpgconf` (GnuPG, Gpg4win, GPG Suite: [`gpgconf_candidates`]), asks it
//! `--list-dirs agent-socket`, run as a program with its arguments and never through a shell,
//! and opens that socket: a Unix domain socket, or (on Windows, and wherever the name holds such
//! a file) Assuan's socket file, a port on the loopback address and a nonce sent first. When the
//! socket can't be opened (no agent yet, or one that went away and left its socket or port file
//! behind), `gpgconf --launch gpg-agent` starts one and the socket is tried once more.
//!
//! GnuPG not being installed is [`Unavailable::NoGnupg`]; installed but its `gpgconf` not
//! working is [`Unavailable::NotWorking`]; the agent not answering is [`Unavailable::NoAgent`].
//! None is a failure of the program. The conversation itself is `mailcraft_assuan::GpgAgent`'s;
//! the agent has [`SHORT_TIMEOUT`] for each answer, but a signature or a decryption, which waits
//! for the owner at the pinentry, has [`ANSWER_TIMEOUT`]; each call is made away from the
//! window's thread.
//!
//! Nothing here is used yet: signing (#30 part 5) and decrypting (part 6) will.

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use mailcraft_assuan::socket_file::{self, SocketFile};
use mailcraft_assuan::{GpgAgent, PinentryHints};

/// How long `gpgconf` may take before it is given up on.
const GPGCONF_TIMEOUT: Duration = Duration::from_secs(10);
/// Most of `gpgconf`'s output read.
const MAX_GPGCONF_OUTPUT: usize = 64 * 1024;
/// Longest socket name taken from `gpgconf`.
const MAX_SOCKET_NAME: usize = 4 * 1024;
/// How long the agent may take over an answer that doesn't wait for the owner.
pub(crate) const SHORT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a signature or a decryption may take: long enough for the owner to find a smartcard
/// and type a PIN into the pinentry, which has its own timeout too.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long sending a line may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long connecting to the loopback port may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Why there is no agent to talk to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// GnuPG isn't installed (no `gpgconf` was found).
    NoGnupg,
    /// GnuPG is installed, but its `gpgconf` didn't work: what went wrong, in words.
    NotWorking(String),
    /// GnuPG is installed but its agent couldn't be reached: what went wrong, in words.
    NoAgent(String),
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unavailable::NoGnupg => f.write_str("GnuPG isn't installed on this computer"),
            Unavailable::NotWorking(why) => write!(f, "GnuPG is installed but doesn't work: {why}"),
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
    fn set_read_timeout(&self, wait: Duration) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.set_read_timeout(Some(wait)),
            AgentStream::Tcp(s) => s.set_read_timeout(Some(wait)),
        }
    }

    fn set_write_timeout(&self, wait: Duration) -> io::Result<()> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.set_write_timeout(Some(wait)),
            AgentStream::Tcp(s) => s.set_write_timeout(Some(wait)),
        }
    }

    /// How long a read waits now.
    #[cfg(test)]
    pub(crate) fn read_timeout(&self) -> io::Result<Option<Duration>> {
        match self {
            #[cfg(unix)]
            AgentStream::Unix(s) => s.read_timeout(),
            AgentStream::Tcp(s) => s.read_timeout(),
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

/// The long wait while the owner is at the pinentry, the short one otherwise
/// (`mailcraft_assuan::OwnerWait`).
pub(crate) fn owner_wait(stream: &mut AgentStream, owner: bool) {
    let wait = if owner { ANSWER_TIMEOUT } else { SHORT_TIMEOUT };
    if let Err(e) = stream.set_read_timeout(wait) {
        log::warn!("gpg-agent's connection couldn't be given its wait: {}", e.kind());
    }
}

/// `gpgconf`, and how it is run.
pub(crate) struct Gpgconf {
    program: PathBuf,
    /// Arguments that go before each command's (none but in the tests, which run a script).
    prefix: Vec<OsString>,
    timeout: Duration,
}

impl Gpgconf {
    pub(crate) fn at(program: PathBuf) -> Gpgconf {
        Gpgconf { program, prefix: Vec::new(), timeout: GPGCONF_TIMEOUT }
    }

    #[cfg(test)]
    pub(crate) fn script(shell: PathBuf, script: PathBuf, timeout: Duration) -> Gpgconf {
        Gpgconf { program: shell, prefix: vec![script.into_os_string()], timeout }
    }

    /// Run `gpgconf` with `args` (a program and its arguments, never a shell), for at most its
    /// timeout. With `output`, the first line it writes (at most [`MAX_GPGCONF_OUTPUT`] bytes),
    /// read on a thread of its own and waited for no longer than the program; without, its
    /// output goes nowhere. `Err` says why it didn't work.
    pub(crate) fn run(&self, args: &[&str], output: bool) -> Result<Vec<u8>, String> {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix).args(args).stdin(Stdio::null()).stderr(Stdio::null());
        command.stdout(if output { Stdio::piped() } else { Stdio::null() });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window flashes up for it.
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|e| format!("gpgconf couldn't be started ({})", e.kind()))?;
        let (sender, line) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            let spawned = std::thread::Builder::new().name("gpgconf".into()).spawn(move || {
                let _ = sender.send(first_line(stdout));
            });
            if spawned.is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("gpgconf's output couldn't be read".into());
            }
        }
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("gpgconf didn't finish in time".into());
                }
            }
        };
        if !status.success() {
            return Err(match status.code() {
                Some(code) => format!("gpgconf ended with error {code}"),
                None => "gpgconf was stopped".into(),
            });
        }
        if !output {
            return Ok(Vec::new());
        }
        // A program gpgconf left running may hold its output open: the line is waited for no
        // longer than gpgconf itself could have taken.
        let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(200));
        line.recv_timeout(left).map_err(|_| "gpgconf's output didn't end".to_string())
    }

    /// The agent's socket, as `gpgconf --list-dirs agent-socket` names it.
    pub(crate) fn agent_socket(&self) -> Result<PathBuf, Unavailable> {
        let output = self.run(&["--list-dirs", "agent-socket"], true).map_err(Unavailable::NotWorking)?;
        socket_from_output(&output).ok_or_else(|| Unavailable::NotWorking("gpgconf names no agent socket".into()))
    }

    /// Start the agent if it isn't running (`gpgconf --launch gpg-agent`, which does nothing
    /// when it is).
    pub(crate) fn launch(&self) -> Result<(), String> {
        self.run(&["--launch", "gpg-agent"], false).map(drop)
    }
}

/// Bytes up to the first line feed, or to the end, of at most [`MAX_GPGCONF_OUTPUT`].
fn first_line(mut stream: impl Read) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 1024];
    while out.len() < MAX_GPGCONF_OUTPUT && !out.contains(&b'\n') {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(chunk.get(..n).unwrap_or(&[])),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    out.truncate(MAX_GPGCONF_OUTPUT);
    out
}

/// Connect to this computer's gpg-agent, with the hints its pinentry needs to show on the owner's
/// screen (from this program's environment).
#[cfg_attr(test, expect(dead_code, reason = "the tests never run the computer's own gpgconf"))]
pub(crate) fn connect() -> Result<GpgAgent<AgentStream>, Unavailable> {
    let gpgconf = find_gpgconf(&gpgconf_candidates(&|name| std::env::var_os(name))).ok_or(Unavailable::NoGnupg)?;
    let hints = PinentryHints::from_env(|name| std::env::var(name).ok());
    connect_with(&Gpgconf::at(gpgconf), &hints)
}

/// Connect through `gpgconf`: `gpgconf --launch gpg-agent` first (it does nothing when the agent
/// runs, and otherwise starts it, so that a socket or port file an agent left behind isn't
/// opened, or found taken by another program), then the socket it names, and when that can't be
/// opened, once more after launching again.
pub(crate) fn connect_with(gpgconf: &Gpgconf, hints: &PinentryHints) -> Result<GpgAgent<AgentStream>, Unavailable> {
    let socket = gpgconf.agent_socket()?;
    if let Err(why) = gpgconf.launch() {
        log::info!("gpg-agent couldn't be started first: {why}");
    }
    match open(&socket, hints) {
        Ok(agent) => Ok(agent),
        Err(first) => {
            log::info!("gpg-agent didn't answer ({first}); starting it");
            if let Err(why) = gpgconf.launch() {
                log::warn!("gpg-agent couldn't be started: {why}");
                return Err(first);
            }
            open(&socket, hints)
        }
    }
}

/// Open the agent's socket at `socket` and greet it.
pub(crate) fn open(socket: &Path, hints: &PinentryHints) -> Result<GpgAgent<AgentStream>, Unavailable> {
    let stream = open_socket(socket).map_err(|e| Unavailable::NoAgent(format!("its socket couldn't be opened ({})", e.kind())))?;
    let setup = |e: io::Error| Unavailable::NoAgent(format!("its socket couldn't be set up ({})", e.kind()));
    stream.set_read_timeout(SHORT_TIMEOUT).map_err(setup)?;
    stream.set_write_timeout(WRITE_TIMEOUT).map_err(setup)?;
    let mut agent = GpgAgent::open(stream).map_err(|e| Unavailable::NoAgent(e.to_string()))?;
    agent.set_owner_wait(owner_wait);
    agent.set_pinentry_hints(hints).map_err(|e| Unavailable::NoAgent(e.to_string()))?;
    Ok(agent)
}

/// Where `gpgconf` may be, in the order it is looked for (`var` reads one environment variable).
/// On Linux, the BSDs and macOS: each folder of `PATH` first, which is the owner's choice, then
/// where GnuPG's installers put it (a program started from the Finder doesn't have their folders
/// on its `PATH`). On Windows the other way round: where GnuPG and Gpg4win install it first, so
/// that a `gpgconf.exe` some other program put on `PATH` isn't run before them. Never a relative
/// folder of `PATH`, so never the current folder.
pub(crate) fn gpgconf_candidates(var: &dyn Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let exe = if cfg!(windows) { "gpgconf.exe" } else { "gpgconf" };
    let mut installed = Vec::new();
    if cfg!(windows) {
        // GnuPG's own installer and Gpg4win's.
        for root in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(root) = var(root).map(PathBuf::from).filter(|r| r.is_absolute()) {
                installed.push(root.join("GnuPG").join("bin").join(exe));
                installed.push(root.join("Gpg4win").join("bin").join(exe));
            }
        }
    } else {
        // GPG Suite, Homebrew (Apple silicon, then Intel), MacPorts, the system's own.
        for dir in ["/usr/local/MacGPG2/bin", "/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
            installed.push(Path::new(dir).join(exe));
        }
    }
    let on_path: Vec<PathBuf> =
        var("PATH").map(|path| std::env::split_paths(&path).filter(|d| d.is_absolute()).map(|d| d.join(exe)).collect()).unwrap_or_default();
    let (first, then) = if cfg!(windows) { (installed, on_path) } else { (on_path, installed) };
    let mut out: Vec<PathBuf> = Vec::new();
    for candidate in first.into_iter().chain(then) {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
}

/// The first candidate that is a file.
pub(crate) fn find_gpgconf(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|c| c.is_file()).cloned()
}

/// The first line of `gpgconf --list-dirs agent-socket`, unescaped (`gpgconf` writes `%3a` for a
/// colon, as in a Windows drive): an absolute name without control characters, or nothing. On
/// Linux, the BSDs and macOS the name is taken as the bytes it is; on Windows as UTF-8, with any
/// byte that isn't replaced (so the name won't be found, rather than another).
pub(crate) fn socket_from_output(output: &[u8]) -> Option<PathBuf> {
    let line = output.split(|b| *b == b'\n').next()?;
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.len() > MAX_SOCKET_NAME {
        return None;
    }
    let mut bytes = Vec::with_capacity(line.len());
    let mut i = 0usize;
    while let Some(&b) = line.get(i) {
        let digit = |at: usize| line.get(at).and_then(|c| char::from(*c).to_digit(16));
        if b == b'%'
            && let (Some(h), Some(l)) = (digit(i.saturating_add(1)), digit(i.saturating_add(2)))
        {
            bytes.push(u8::try_from((h << 4) | l).unwrap_or(b'?'));
            i = i.saturating_add(3);
            continue;
        }
        bytes.push(b);
        i = i.saturating_add(1);
    }
    if bytes.is_empty() || bytes.iter().any(|b| *b < 0x20 || *b == 0x7F) {
        return None;
    }
    #[cfg(unix)]
    let name = {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(bytes)
    };
    #[cfg(not(unix))]
    let name = OsString::from(String::from_utf8_lossy(&bytes).into_owned());
    let path = PathBuf::from(name);
    path.is_absolute().then_some(path)
}

/// Open the socket at `path`: a Unix domain socket, or a socket file (a redirect, or the loopback
/// port and nonce), followed once.
fn open_socket(path: &Path) -> io::Result<AgentStream> {
    open_socket_at(path, true)
}

fn open_socket_at(path: &Path, follow: bool) -> io::Result<AgentStream> {
    let meta = std::fs::metadata(path)?;
    if meta.is_file() {
        let max = u64::try_from(socket_file::MAX_FILE).unwrap_or(u64::MAX);
        if meta.len() > max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "socket file too big"));
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
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
