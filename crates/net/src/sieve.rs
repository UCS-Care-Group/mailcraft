// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sieve scripts (RFC 5228) an IMAP account's server keeps, read and written through
//! ManageSieve (RFC 5804, [`mailcraft_proto::managesieve`]): where Automatic Replies and rules
//! kept on the server live on a server that isn't Microsoft's.
//!
//! **Where the sign-in goes.** The ManageSieve server is the account's IMAP server, on port 4190,
//! or the host and port its owner named in Account Settings ([`server_for`]); the password or
//! token goes to those hosts and to no other ([`read`] and [`write`] refuse any other before a
//! connection is made), and a server's referral to another
//! one is never followed. The connection is STARTTLS (RFC 5804 §2.2): a server that doesn't offer
//! it is an error, never a reason to go on in the clear; only a server on this computer (the
//! tests' fake one) is spoken to without encryption. A sign-in token goes only where
//! [`crate::token_may_go_to`] lets it.
//!
//! **Nobody's script is written over unseen.** [`write`] reads the script it is to replace first
//! and writes only when it is still what MailCraft last wrote there (or there is none): a script
//! changed elsewhere since is handed back, untouched ([`Written::ChangedElsewhere`]). It never
//! makes another script inactive: when the owner has a script of their own active, MailCraft's is
//! written and left inactive, and the caller is told ([`Written::AnotherActive`]). What a server
//! sends is hostile, and bounded by the protocol client.
//!
//! **Why a server can't be had is said precisely** ([`Reach`]): nothing answers at its host and
//! port; something answers but says nothing (a ManageSieve server greets at once: one that waits
//! for encryption from the first byte, or a proxy, says nothing), which a TLS hello on a
//! connection of its own tells apart, with nothing of the owner's sent; something answers that
//! isn't ManageSieve; no STARTTLS; a failed handshake. None of it ever names a secret.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_i18n::{Arg, Text, said};
use mailcraft_model::{Account, Security, Server};
use mailcraft_proto::ProtoError;
use mailcraft_proto::managesieve::{ManageSieve, ScriptEntry};

use crate::transport::{self, Patience, Service, Stream};
use crate::{NetError, not_signed_in};

/// The port of ManageSieve (RFC 5804 §1.8).
pub const PORT: u16 = 4190;

/// How long a server has to greet once the connection is made: a ManageSieve server greets at
/// once, so one that says nothing for this long is waiting for something MailCraft doesn't send.
pub const GREETING_WAIT: Duration = Duration::from_secs(15);

/// What answered where a ManageSieve server was looked for, by its first words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LooksLike {
    Imap,
    Pop3,
    Smtp,
    Web,
    Ssh,
    /// An encrypted (TLS) server.
    Tls,
}

impl LooksLike {
    /// What answered, by the first bytes it sent: only these few words are looked at, and
    /// nothing of them is kept or shown.
    pub fn of(first: &[u8]) -> Option<LooksLike> {
        let starts = |word: &[u8]| first.len() >= word.len() && first.get(..word.len()).is_some_and(|f| f.eq_ignore_ascii_case(word));
        if transport::is_tls_record(first) {
            Some(LooksLike::Tls)
        } else if starts(b"* OK") || starts(b"* PREAUTH") || starts(b"* BYE") {
            Some(LooksLike::Imap)
        } else if starts(b"+OK") || starts(b"-ERR") {
            Some(LooksLike::Pop3)
        } else if starts(b"HTTP/") {
            Some(LooksLike::Web)
        } else if starts(b"SSH-") {
            Some(LooksLike::Ssh)
        } else if matches!(first, [b'2' | b'4' | b'5', b'0'..=b'9', b'0'..=b'9', b' ' | b'-', ..]) {
            Some(LooksLike::Smtp)
        } else {
            None
        }
    }

    fn said(self) -> Text {
        match self {
            LooksLike::Imap => said!("an IMAP server"),
            LooksLike::Pop3 => said!("a POP3 server"),
            LooksLike::Smtp => said!("an SMTP server"),
            LooksLike::Web => said!("a web server"),
            LooksLike::Ssh => said!("an SSH server"),
            LooksLike::Tls => said!("a server that encrypts from the first byte"),
        }
    }
}

/// Why no ManageSieve session could be had at all: where (`at`, the host and port), and what was
/// found there. Never anything secret: no sign-in was sent when any of these happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Nothing answers: the name doesn't resolve, the connection is refused, the network can't
    /// get there, or connecting took too long (`why`).
    NoAnswer { at: String, why: Text },
    /// The connection was made but the server said nothing in [`GREETING_WAIT`]. `tls`: it
    /// answered a TLS hello, so it waits for encryption from the first byte.
    Silent { at: String, tls: bool },
    /// The connection was made and closed at once, without a word.
    Closed { at: String },
    /// Something answered that isn't a ManageSieve server; what it looks like, when that is known.
    NotManageSieve { at: String, looks_like: Option<LooksLike> },
    /// A ManageSieve server that doesn't offer STARTTLS: nothing is sent to it unencrypted.
    NoStartTls { at: String },
    /// The secure connection failed (`why`: the handshake, the certificate).
    Tls { at: String, why: Text },
}

impl Reach {
    /// What happened, in a few words that name where: for a line, or after "weren't changed".
    pub fn said(&self) -> Text {
        match self {
            Reach::NoAnswer { at, why } => said!("nothing answers at {at} ({reason})", at, reason = Arg::said(why.clone())),
            Reach::Silent { at, tls: false } => said!("MailCraft connected to {at}, but the server said nothing", at),
            Reach::Silent { at, tls: true } => said!("{at} expects an encrypted connection from the start, which ManageSieve doesn't use", at),
            Reach::Closed { at } => said!("{at} closed the connection without a word", at),
            Reach::NotManageSieve { at, looks_like: None } => said!("something answers at {at}, but it isn't a ManageSieve server", at),
            Reach::NotManageSieve { at, looks_like: Some(what) } => {
                said!("something answers at {at}, but it isn't a ManageSieve server: it looks like {what}", at, what = Arg::said(what.said()))
            }
            Reach::NoStartTls { at } => said!("the ManageSieve server at {at} doesn't offer STARTTLS", at),
            Reach::Tls { at, why } => said!("couldn't make a secure connection to {at}: {reason}", at, reason = Arg::said(why.clone())),
        }
    }

    /// What the owner can do about it, in a sentence or two.
    pub fn advice(&self) -> Text {
        match self {
            Reach::NoAnswer { .. } => {
                said!("Check the Sieve server (Automatic Replies) in Account Settings, and that your mail server has ManageSieve switched on.")
            }
            Reach::Silent { tls: false, .. } => said!(
                "A ManageSieve server greets as soon as it is connected to. This port may expect an encrypted connection from the start (on Dovecot, ssl = yes on the sieve listener), which ManageSieve doesn't use: it starts unencrypted and switches to encryption with STARTTLS. Or a proxy in front of the server waits for something MailCraft doesn't send. Check the Sieve server (Automatic Replies) in Account Settings, or ask the server's administrator."
            ),
            Reach::Silent { tls: true, .. } => said!(
                "ManageSieve starts unencrypted and switches to encryption with STARTTLS, so MailCraft doesn't sign in there. On Dovecot, set ssl = no on the sieve listener (the connection is still encrypted, by STARTTLS), or name another port in Account Settings › Sieve server (Automatic Replies)."
            ),
            Reach::Closed { .. } | Reach::NotManageSieve { .. } => said!(
                "Check the Sieve server (Automatic Replies) in Account Settings: ManageSieve is usually on port 4190 of the incoming mail server."
            ),
            Reach::NoStartTls { .. } => said!(
                "MailCraft signs in only over an encrypted connection, so it didn't sign in there: your password would cross the network unencrypted. Ask the server's administrator to turn on TLS for ManageSieve."
            ),
            Reach::Tls { .. } => said!(
                "Nothing was sent to it. Check the Sieve server (Automatic Replies) in Account Settings, or ask the server's administrator about its certificate."
            ),
        }
    }
}

/// Why the server's Sieve scripts couldn't be read or written.
#[derive(Debug)]
pub enum SieveError {
    /// No ManageSieve session could be had at all, said precisely ([`Reach`]).
    Reach(Reach),
    /// Anything else: the sign-in refused, the account's settings, the server's answers.
    Net(NetError),
}

impl From<NetError> for SieveError {
    fn from(e: NetError) -> SieveError {
        SieveError::Net(e)
    }
}

impl From<ProtoError> for SieveError {
    fn from(e: ProtoError) -> SieveError {
        SieveError::Net(NetError::Proto(e))
    }
}

impl From<Reach> for SieveError {
    fn from(reach: Reach) -> SieveError {
        SieveError::Reach(reach)
    }
}

impl std::fmt::Display for SieveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SieveError::Reach(reach) => std::fmt::Display::fmt(&reach.said(), f),
            SieveError::Net(e) => std::fmt::Display::fmt(e, f),
        }
    }
}

impl std::error::Error for SieveError {}

impl SieveError {
    /// Whether the server refused the sign-in itself ([`NetError::is_refused_sign_in`]).
    pub fn is_refused_sign_in(&self) -> bool {
        matches!(self, SieveError::Net(e) if e.is_refused_sign_in())
    }

    /// The same error with `secret` blanked out, should a server have written it back.
    fn without(self, secret: &str) -> SieveError {
        match self {
            SieveError::Net(e) => SieveError::Net(e.without(secret)),
            // (Made of MailCraft's own words, the host and port, and the handshake's.)
            reach @ SieveError::Reach(_) => reach,
        }
    }
}

/// `host:port` as it is shown: an IPv6 address in brackets.
fn at(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// What the server has: what its scripts may use, its scripts, and the text of the one asked for.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SieveState {
    /// The server's software, as it names itself.
    pub implementation: String,
    /// The Sieve extensions its scripts may use, lower-cased (`vacation`, `fileinto`, `date`, …).
    pub extensions: Vec<String>,
    /// Every script on the server, and which one is active.
    pub scripts: Vec<ScriptEntry>,
    /// The text of the script asked for; `None` when the server has none of that name.
    pub script: Option<String>,
}

impl SieveState {
    /// The name of the script the server runs on incoming mail, if any.
    pub fn active(&self) -> Option<&str> {
        self.scripts.iter().find(|s| s.active).map(|s| s.name.as_str())
    }

    /// Whether scripts may use the extension `name`.
    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(name))
    }
}

/// The names of scripts and their text are the owner's: only how many there are reaches a log.
impl std::fmt::Debug for SieveState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SieveState")
            .field("extensions", &self.extensions)
            .field("scripts", &self.scripts.len())
            .field("script", &self.script.as_ref().map(String::len))
            .finish_non_exhaustive()
    }
}

/// A script to write.
#[derive(Clone, Copy)]
pub struct ScriptChange<'a> {
    /// The script's name.
    pub name: &'a str,
    /// Its new text.
    pub text: &'a str,
    /// The SHA-256 digest ([`digest`]) of what MailCraft last wrote there; `None` when it never
    /// wrote one. The script is written only when the server's is still that (or is `text`
    /// already). Only the digest is kept, never the text.
    pub last_written: Option<&'a str>,
    /// Make it the active script, unless another one is.
    pub activate: bool,
}

/// What [`write`] did.
#[derive(Clone, PartialEq, Eq)]
pub enum Written {
    /// The script is on the server as it was given. `active`: it is the active one now.
    /// `warnings`: what the server had to say of it, if anything.
    Done { active: bool, warnings: Option<String> },
    /// The server's script isn't what MailCraft last wrote there: it was changed (or removed,
    /// `None`) elsewhere, and was left as it is.
    ChangedElsewhere { now: Option<String> },
    /// The script was written, but another one (`active`) is the active script, and stays so.
    AnotherActive { active: String, warnings: Option<String> },
}

/// The text of scripts stays out of debug output.
impl std::fmt::Debug for Written {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Written::Done { active, warnings } => f.debug_struct("Done").field("active", active).field("warnings", &warnings.is_some()).finish(),
            Written::ChangedElsewhere { now } => f.debug_struct("ChangedElsewhere").field("now", &now.as_ref().map(String::len)).finish(),
            Written::AnotherActive { warnings, .. } => f.debug_struct("AnotherActive").field("warnings", &warnings.is_some()).finish_non_exhaustive(),
        }
    }
}

/// The SHA-256 digest of a script, in lower-case hexadecimal: what is kept of the script MailCraft
/// last wrote, to know it again ([`ScriptChange::last_written`]).
pub fn digest(script: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, script.as_bytes());
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// The ManageSieve server of an IMAP account: the host and port the owner named in Account
/// Settings ([`Account::sieve_host`], [`Account::sieve_port`]), else its incoming server's host on
/// [`PORT`]; with STARTTLS (without encryption only when the IMAP server is on this computer and
/// has none, and so is this one).
pub fn server_for(account: &Account) -> Result<Server, NetError> {
    let incoming = account.incoming.as_ref().ok_or_else(|| NetError::Config(said!("the account has no incoming server")))?;
    let host = if account.sieve_host.trim().is_empty() { incoming.host.trim() } else { account.sieve_host.trim() };
    let plain = incoming.security == Security::None && transport::is_loopback(&incoming.host) && transport::is_loopback(host);
    let port = if account.sieve_port == 0 { PORT } else { account.sieve_port };
    Ok(Server { host: host.to_string(), port, security: if plain { Security::None } else { Security::StartTls } })
}

/// Sign in to the ManageSieve server and read what it has: its capabilities, its scripts, and
/// the text of the script `name` (when there is one).
pub fn read(account: &Account, server: &Server, secret: &str, name: &str) -> Result<SieveState, SieveError> {
    read_with(account, server, secret, name, &Patience::default(), None)
}

/// [`read`], waiting as `patience` says, and giving up once `cancel` is set.
pub(crate) fn read_with(
    account: &Account,
    server: &Server,
    secret: &str,
    name: &str,
    patience: &Patience,
    cancel: Option<&AtomicBool>,
) -> Result<SieveState, SieveError> {
    let run = || {
        let mut sieve = open(account, server, secret, patience, cancel)?;
        let capabilities = sieve.capabilities().clone();
        let scripts = sieve.list_scripts()?;
        let script = if scripts.iter().any(|s| s.name == name) { sieve.get_script(name)? } else { None };
        let _ = sieve.logout();
        Ok(SieveState { implementation: capabilities.implementation, extensions: capabilities.extensions, scripts, script })
    };
    run().map_err(|e: SieveError| e.without(secret))
}

/// Write a script, if what is on the server is still what MailCraft last wrote there, and make
/// it active when asked and no other script is.
pub fn write(account: &Account, server: &Server, secret: &str, change: &ScriptChange<'_>) -> Result<Written, SieveError> {
    write_with(account, server, secret, change, &Patience::default(), None)
}

/// [`write`], waiting as `patience` says, and giving up once `cancel` is set.
pub(crate) fn write_with(
    account: &Account,
    server: &Server,
    secret: &str,
    change: &ScriptChange<'_>,
    patience: &Patience,
    cancel: Option<&AtomicBool>,
) -> Result<Written, SieveError> {
    let run = || {
        let mut sieve = open(account, server, secret, patience, cancel)?;
        let scripts = sieve.list_scripts()?;
        let there = scripts.iter().any(|s| s.name == change.name);
        let now = if there { sieve.get_script(change.name)? } else { None };
        // Unchanged since MailCraft wrote it, or already as it is to be.
        let ours = now.as_deref().map(digest).as_deref() == change.last_written || now.as_deref() == Some(change.text);
        if !ours {
            let _ = sieve.logout();
            return Ok(Written::ChangedElsewhere { now });
        }
        let warnings = if now.as_deref() == Some(change.text) { None } else { sieve.put_script(change.name, change.text)? };
        let active = scripts.iter().find(|s| s.active).map(|s| s.name.clone());
        let written = match active {
            Some(other) if other != change.name && change.activate => Written::AnotherActive { active: other, warnings },
            Some(other) if other == change.name => Written::Done { active: true, warnings },
            _ if change.activate => {
                sieve.set_active(change.name)?;
                Written::Done { active: true, warnings }
            }
            _ => Written::Done { active: false, warnings },
        };
        let _ = sieve.logout();
        Ok(written)
    };
    run().map_err(|e: SieveError| e.without(secret))
}

/// Leave the server with no active script, if `name` is the active one: MailCraft's own script
/// stops running, and nobody else's is touched. Whether it was active.
pub fn deactivate(account: &Account, server: &Server, secret: &str, name: &str) -> Result<bool, SieveError> {
    let run = || {
        let mut sieve = open(account, server, secret, &Patience::default(), None)?;
        let ours = sieve.list_scripts()?.iter().any(|s| s.active && s.name == name);
        if ours {
            sieve.deactivate()?;
        }
        let _ = sieve.logout();
        Ok(ours)
    };
    run().map_err(|e: SieveError| e.without(secret))
}

/// The host without brackets and the dot that may end it, in lower case: how two names of one
/// server are compared.
fn host_key(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Whether `server` is the host of the account's incoming server, or the ManageSieve host its
/// owner named: the places its sign-in may go.
fn is_mail_server(account: &Account, server: &Server) -> bool {
    let wanted = host_key(&server.host);
    let named = host_key(&account.sieve_host);
    let incoming = account.incoming.as_ref().map(|incoming| host_key(&incoming.host)).unwrap_or_default();
    account.incoming.is_some() && !wanted.is_empty() && (wanted == incoming || wanted == named)
}

/// Connect, make the connection secure, and sign in.
fn open<'a>(
    account: &Account,
    server: &Server,
    secret: &str,
    patience: &Patience,
    cancel: Option<&'a AtomicBool>,
) -> Result<ManageSieve<Stream<'a>>, SieveError> {
    if !is_mail_server(account, server) {
        return Err(NetError::Config(said!("Sieve scripts are only read from and written to the account's own mail server")).into());
    }
    crate::sync::token_may_go_to(account, server, "imap.gmail.com")?;
    let user = account.login();
    if user.is_empty() {
        return Err(NetError::Config(said!("the account has no user name or e-mail address to sign in with")).into());
    }
    // Without a token or password there is nothing to show the server: no connection is made.
    if account.oauth.is_some() && secret.is_empty() {
        return Err(not_signed_in(account).into());
    }
    if account.oauth.is_none() && secret.is_empty() {
        return Err(NetError::Config(said!("enter a password to sign in")).into());
    }
    let port = transport::port(server, Service::ManageSieve);
    let (mut stream, host) = transport::open(server, Service::ManageSieve, patience).map_err(|e| match e {
        NetError::Connect { host, reason } => Reach::NoAnswer { at: at(&host, port), why: reason }.into(),
        other => SieveError::Net(other),
    })?;
    let at = at(&host, port);
    stream.watch(cancel);
    let first = greeting(&mut stream, server, &at, patience)?;
    let mut sieve = ManageSieve::connect(stream).map_err(|e| match e {
        // What came isn't a ManageSieve greeting, and didn't begin like one.
        ProtoError::Protocol(_) | ProtoError::Io(_) if !greets_like_managesieve(&first) => {
            Reach::NotManageSieve { at: at.clone(), looks_like: LooksLike::of(&first) }.into()
        }
        other => SieveError::from(other),
    })?;
    if server.security == Security::StartTls {
        // Never carry on in the clear when encryption was asked for.
        if !sieve.capabilities().starttls {
            return Err(Reach::NoStartTls { at }.into());
        }
        let plain = sieve.starttls()?;
        let secured = transport::upgrade(plain, &host).map_err(|e| match e {
            NetError::Tls { reason, .. } => Reach::Tls { at: at.clone(), why: reason }.into(),
            other => SieveError::Net(other),
        })?;
        sieve = ManageSieve::after_starttls(secured)?;
    }
    if account.oauth.is_some() {
        sieve.authenticate_token(user, secret)?;
    } else {
        sieve.authenticate_plain(user, secret)?;
    }
    Ok(sieve)
}

/// The first bytes the server sent, waiting [`GREETING_WAIT`] at most (or less, as `patience`
/// says), left for the protocol to read. A server that says nothing in that time, closes the
/// connection, or begins a TLS record is no ManageSieve server MailCraft can use: which of those,
/// said precisely; a silent one is sent a TLS hello on a connection of its own to tell whether
/// it waits for encryption from the first byte.
fn greeting(stream: &mut Stream<'_>, server: &Server, at: &str, patience: &Patience) -> Result<Vec<u8>, SieveError> {
    let wait = patience.io.min(GREETING_WAIT);
    let mut first = [0u8; 16];
    match stream.peek_first(&mut first, wait) {
        Ok(0) => Err(Reach::Closed { at: at.to_string() }.into()),
        Ok(n) => {
            let first = first.get(..n).unwrap_or_default();
            if transport::is_tls_record(first) {
                return Err(Reach::NotManageSieve { at: at.to_string(), looks_like: Some(LooksLike::Tls) }.into());
            }
            Ok(first.to_vec())
        }
        Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
            let answer = transport::answer_to_hello(server, Service::ManageSieve, patience.connect, wait);
            Err(match LooksLike::of(&answer) {
                Some(LooksLike::Tls) => Reach::Silent { at: at.to_string(), tls: true },
                Some(what) => Reach::NotManageSieve { at: at.to_string(), looks_like: Some(what) },
                None => Reach::Silent { at: at.to_string(), tls: false },
            }
            .into())
        }
        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {
            Err(Reach::Closed { at: at.to_string() }.into())
        }
        // (Send/Receive was stopped, or the system's own words.)
        Err(e) => Err(ProtoError::Io(Text::verbatim(e.to_string().chars().filter(|c| !c.is_control()).take(200).collect::<String>())).into()),
    }
}

/// Whether the first bytes a server sent begin like a ManageSieve greeting: a string (in quotes,
/// or counted) or the word that ends one. Then a greeting that goes wrong after is said as the
/// server's own trouble, not as something else answering.
fn greets_like_managesieve(first: &[u8]) -> bool {
    let word = |w: &[u8]| first.len() >= w.len() && first.get(..w.len()).is_some_and(|f| f.eq_ignore_ascii_case(w));
    matches!(first.first(), Some(b'"' | b'{')) || word(b"OK") || word(b"NO") || word(b"BYE")
}
