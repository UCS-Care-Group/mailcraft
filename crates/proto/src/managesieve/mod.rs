// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A ManageSieve client (RFC 5804): the Sieve scripts (RFC 5228) a mail server keeps for its
//! owner, listed, read, checked, written, made active and removed, over any `Read + Write`
//! stream. STARTTLS (RFC 5804 §2.2) hands the stream back for the caller to wrap; sign-in is SASL
//! PLAIN (RFC 4616) with a password, or XOAUTH2 / OAUTHBEARER (RFC 7628) with an access token.
//!
//! A server may send its owner elsewhere (a `REFERRAL`, §1.3); that is never followed: it would
//! take the sign-in to a server the owner didn't name. A script is sent as a literal (`{n+}`),
//! byte for byte as it was given; a script's name goes in quotes, and one that couldn't (a control
//! character, a line break) is refused before anything is sent.
//!
//! What a server sends is hostile: lines, strings, responses and their counts are bounded
//! ([`parse`]), and the text of a script or of a script's name never reaches the log, only its
//! size.

mod parse;

use std::io::{Read, Write};

use mailcraft_charset::base64::STANDARD;

use crate::line::Wire;
use crate::{ProtoError, Secrets, for_trace, tidy, xoauth2};
use mailcraft_i18n::said;
use parse::{MAX_STRING, Token, read_response};

/// Most lines of data one command may bring (a listing of scripts, the capabilities).
const MAX_DATA_LINES: usize = 1_000;
/// Most extensions, sign-in mechanisms and scripts kept from one listing.
const MAX_LISTED: usize = 256;
/// Longest name of an extension, a mechanism or the server's software kept, in bytes.
const MAX_WORD: usize = 128;
/// Longest script name sent or kept, in bytes (RFC 5804 §1.6 asks servers to take at least 128
/// characters).
pub const MAX_NAME: usize = 512;
/// Longest string sent in quotes (RFC 5804 §4 caps a quoted string at 1024 octets); a longer one
/// goes as a literal.
const MAX_QUOTED: usize = 1_024;
/// Largest script sent or read, in bytes.
pub const MAX_SCRIPT: usize = MAX_STRING;
/// Most continuation rounds of one sign-in.
const MAX_SASL_ROUNDS: usize = 4;

/// What the server says it can do: its greeting, and its answer to CAPABILITY.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// The server's software, as it names itself (`IMPLEMENTATION`).
    pub implementation: String,
    /// The SASL mechanisms it takes, upper-cased (`SASL`).
    pub sasl: Vec<String>,
    /// The Sieve extensions its scripts may use, lower-cased (`SIEVE`): `fileinto`, `vacation`, ….
    pub extensions: Vec<String>,
    /// It offers STARTTLS (only ever before encryption is on).
    pub starttls: bool,
    /// The version of the protocol (`VERSION`): `1.0` for a server of RFC 5804, none for one
    /// from before it.
    pub version: Option<String>,
    /// How many redirects one message may be given (`MAXREDIRECTS`), when the server says.
    pub max_redirects: Option<u32>,
}

impl Capabilities {
    /// Whether scripts may use the extension `name` (`vacation`, `fileinto`, …).
    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(name))
    }

    /// Whether the server takes the SASL mechanism `name`.
    pub fn offers(&self, name: &str) -> bool {
        self.sasl.iter().any(|m| m.eq_ignore_ascii_case(name))
    }
}

/// One script of a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptEntry {
    pub name: String,
    /// It is the one the server runs on incoming mail (only one is, at most).
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Ok,
    No,
    Bye,
}

/// The end of a command's answer: `OK`, `NO` or `BYE`, with the response code in parentheses
/// and the server's words, when it gave them.
#[derive(Debug)]
struct Done {
    status: Status,
    /// The response code's name, upper-cased (`NONEXISTENT`, `QUOTA/MAXSIZE`, `REFERRAL`, …).
    code: Option<String>,
    text: String,
}

/// A ManageSieve session on a stream.
pub struct ManageSieve<S> {
    wire: Wire<S>,
    capabilities: Capabilities,
    secrets: Secrets,
    authenticated: bool,
}

/// Neither the stream nor anything secret is shown.
impl<S> std::fmt::Debug for ManageSieve<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManageSieve").field("capabilities", &self.capabilities).field("authenticated", &self.authenticated).finish_non_exhaustive()
    }
}

impl<S: Read + Write> ManageSieve<S> {
    /// Read the greeting: the server's capabilities, then `OK`.
    pub fn connect(stream: S) -> Result<Self, ProtoError> {
        let mut session =
            ManageSieve { wire: Wire::new(stream), capabilities: Capabilities::default(), secrets: Secrets::default(), authenticated: false };
        session.read_capabilities(true)?;
        Ok(session)
    }

    /// Go on over the stream [`starttls`](Self::starttls) handed back, once the caller wrapped it
    /// in TLS: the server says its capabilities again, and only what it says now counts (RFC
    /// 5804 §2.2).
    pub fn after_starttls(stream: S) -> Result<Self, ProtoError> {
        let mut session =
            ManageSieve { wire: Wire::new(stream), capabilities: Capabilities::default(), secrets: Secrets::default(), authenticated: false };
        session.read_capabilities(false)?;
        Ok(session)
    }

    /// What the server last said it can do.
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    /// Whether a sign-in went through.
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    /// The stream the session runs on, for its owner to set up between commands. Reading from it
    /// or writing to it would take the session apart.
    pub fn get_mut(&mut self) -> &mut S {
        self.wire.get_mut()
    }

    /// Ask for the capabilities again (after signing in, a server may offer more).
    pub fn refresh_capabilities(&mut self) -> Result<&Capabilities, ProtoError> {
        self.send(b"CAPABILITY\r\n", "CAPABILITY")?;
        self.read_capabilities(false)?;
        Ok(&self.capabilities)
    }

    /// Send STARTTLS and hand the stream back for the caller to wrap; then
    /// [`after_starttls`](Self::after_starttls).
    pub fn starttls(mut self) -> Result<S, ProtoError> {
        let (_, done) = self.command(b"STARTTLS\r\n", "STARTTLS")?;
        if done.status != Status::Ok {
            return Err(self.refused(&done));
        }
        // Anything after the go-ahead was slipped in before encryption started.
        if self.wire.has_pending() {
            return Err(ProtoError::Protocol(said!("the server sent data before the secure connection started")));
        }
        Ok(self.wire.into_inner())
    }

    /// Sign in with a password: SASL PLAIN, with an empty authorisation identity (the server
    /// takes the user's own).
    pub fn authenticate_plain(&mut self, user: &str, password: &str) -> Result<(), ProtoError> {
        if password.is_empty() {
            return Err(ProtoError::Auth(said!("enter a password to sign in")));
        }
        if user.is_empty() {
            return Err(ProtoError::Auth(said!("enter a user name to sign in")));
        }
        if user.contains('\0') || password.contains('\0') {
            return Err(ProtoError::Auth(said!("the user name or password has a character that can't be sent")));
        }
        if user.len().saturating_add(password.len()) > 32 * 1024 {
            return Err(ProtoError::Auth(said!("the user name or password is too long")));
        }
        if !self.capabilities.offers("PLAIN") {
            return Err(ProtoError::Auth(said!("the server offers no way to sign in with a password")));
        }
        self.secrets.add(password);
        let response = STANDARD.encode(format!("\0{user}\0{password}"));
        self.secrets.add(&response);
        self.sasl("PLAIN", &response, None).map_err(|e| asked_more(e, said!("the server asked for more than a user name and password")))
    }

    /// Sign in with an OAuth access token, as SASL XOAUTH2.
    pub fn authenticate_xoauth2(&mut self, user: &str, token: &str) -> Result<(), ProtoError> {
        let response = xoauth2(user, token)?;
        if !self.capabilities.offers("XOAUTH2") {
            return Err(ProtoError::Auth(said!("the server offers no way to sign in with an access token")));
        }
        self.secrets.add(token);
        self.secrets.add(&response);
        // A refusal comes as a challenge with the reasons; an empty answer asks for the verdict.
        self.sasl("XOAUTH2", &response, Some("")).map_err(|e| asked_more(e, said!("the server asked for more than a user name and access token")))
    }

    /// Sign in with an OAuth access token, as SASL OAUTHBEARER (RFC 7628).
    pub fn authenticate_oauthbearer(&mut self, user: &str, token: &str) -> Result<(), ProtoError> {
        let response = oauthbearer(user, token)?;
        if !self.capabilities.offers("OAUTHBEARER") {
            return Err(ProtoError::Auth(said!("the server offers no way to sign in with an access token")));
        }
        self.secrets.add(token);
        self.secrets.add(&response);
        // A refusal comes as a challenge with the reasons; a lone ^A asks for the verdict (§3.2.3).
        self.sasl("OAUTHBEARER", &response, Some("AQ=="))
            .map_err(|e| asked_more(e, said!("the server asked for more than a user name and access token")))
    }

    /// Sign in with the access token, as whichever of OAUTHBEARER and XOAUTH2 the server takes
    /// (OAUTHBEARER, the standard one, first).
    pub fn authenticate_token(&mut self, user: &str, token: &str) -> Result<(), ProtoError> {
        if self.capabilities.offers("OAUTHBEARER") { self.authenticate_oauthbearer(user, token) } else { self.authenticate_xoauth2(user, token) }
    }

    /// `AUTHENTICATE "<mechanism>" "<initial response>"`. A challenge after it is answered once
    /// with `after_challenge` (when the mechanism has a way to ask for the verdict), else the
    /// exchange is cancelled with `"*"`.
    fn sasl(&mut self, mechanism: &str, response: &str, after_challenge: Option<&str>) -> Result<(), ProtoError> {
        let mut line = format!("AUTHENTICATE \"{mechanism}\" ").into_bytes();
        line.extend_from_slice(&string(response.as_bytes()));
        line.extend_from_slice(b"\r\n");
        self.send(&line, &format!("AUTHENTICATE \"{mechanism}\" ***"))?;
        let mut answered = false;
        for _ in 0..MAX_SASL_ROUNDS {
            let tokens = read_response(&mut self.wire)?;
            self.trace(&tokens);
            match classify(tokens)? {
                Response::Done(done) => {
                    // An `OK (SASL "…")` carries the server's last word of the exchange, which
                    // none of these mechanisms has a use for.
                    return match done.status {
                        Status::Ok => {
                            self.authenticated = true;
                            Ok(())
                        }
                        Status::No => Err(self.sign_in_failed(&done)),
                        Status::Bye => Err(self.ended(&done)),
                    };
                }
                Response::Data(_) => {
                    // A challenge: the reasons for a refusal, or a question there is no answer to.
                    let reply = match after_challenge {
                        Some(reply) if !answered => reply,
                        _ => "*",
                    };
                    let cancel = reply == "*";
                    answered = true;
                    let mut line = string(reply.as_bytes());
                    line.extend_from_slice(b"\r\n");
                    self.send(&line, "***")?;
                    if cancel {
                        let done = self.finish()?;
                        if done.status == Status::Bye {
                            return Err(self.ended(&done));
                        }
                        return Err(ProtoError::Auth(said!("the server asked for more than a sign-in has")));
                    }
                }
            }
        }
        Err(ProtoError::Protocol(said!("the server sent too many responses to one command")))
    }

    /// The scripts on the server, and which of them is active.
    pub fn list_scripts(&mut self) -> Result<Vec<ScriptEntry>, ProtoError> {
        let (data, done) = self.command(b"LISTSCRIPTS\r\n", "LISTSCRIPTS")?;
        if done.status != Status::Ok {
            return Err(self.refused(&done));
        }
        let mut scripts = Vec::new();
        for line in data {
            let mut parts = line.iter();
            let name = parts.next().and_then(Token::text).ok_or_else(not_managesieve)?;
            let active = match parts.next() {
                None => false,
                Some(word) if word.keyword().as_deref() == Some("ACTIVE") => true,
                Some(_) => return Err(not_managesieve()),
            };
            if parts.next().is_some() {
                return Err(not_managesieve());
            }
            // A name that couldn't be sent back is of no use: left out.
            if valid_name(&name) && scripts.len() < MAX_LISTED {
                scripts.push(ScriptEntry { name, active });
            }
        }
        Ok(scripts)
    }

    /// The text of the script `name`; `None` when the server has no script of that name.
    pub fn get_script(&mut self, name: &str) -> Result<Option<String>, ProtoError> {
        let mut line = b"GETSCRIPT ".to_vec();
        line.extend_from_slice(&name_string(name)?);
        line.extend_from_slice(b"\r\n");
        let (data, done) = self.command(&line, "GETSCRIPT …")?;
        match done.status {
            Status::Ok => {}
            Status::No if done.code.as_deref() == Some("NONEXISTENT") => return Ok(None),
            _ => return Err(self.refused(&done)),
        }
        let mut lines = data.into_iter();
        let script = match (lines.next(), lines.next()) {
            (Some(tokens), None) if tokens.len() == 1 => tokens.first().and_then(Token::text),
            _ => None,
        };
        script.map(Some).ok_or_else(|| ProtoError::Protocol(said!("the server didn't send the script")))
    }

    /// Whether the server would take a script of `size` bytes called `name` (HAVESPACE); `false`
    /// when it is over the owner's quota.
    pub fn have_space(&mut self, name: &str, size: usize) -> Result<bool, ProtoError> {
        let mut line = b"HAVESPACE ".to_vec();
        line.extend_from_slice(&name_string(name)?);
        line.extend_from_slice(format!(" {size}\r\n").as_bytes());
        let (_, done) = self.command(&line, &format!("HAVESPACE … {size}"))?;
        match done.status {
            Status::Ok => Ok(true),
            Status::No if done.code.as_deref().is_some_and(|c| c.starts_with("QUOTA")) => Ok(false),
            _ => Err(self.refused(&done)),
        }
    }

    /// Have the server check a script without keeping it (CHECKSCRIPT; a server from before RFC
    /// 5804 has none, and checks a script when it is written). Its warnings, when it has any; a
    /// script with errors is refused with the server's words.
    pub fn check_script(&mut self, script: &str) -> Result<Option<String>, ProtoError> {
        let mut line = b"CHECKSCRIPT ".to_vec();
        line.extend_from_slice(&script_literal(script)?);
        line.extend_from_slice(b"\r\n");
        let (_, done) = self.command(&line, &format!("CHECKSCRIPT ({} bytes)", script.len()))?;
        self.written(&done)
    }

    /// Write the script `name` (made anew, or in the place of the one of that name), as it is
    /// given. Its warnings, when the server has any; a script with errors is refused with the
    /// server's words, and nothing on the server changes.
    pub fn put_script(&mut self, name: &str, script: &str) -> Result<Option<String>, ProtoError> {
        let mut line = b"PUTSCRIPT ".to_vec();
        line.extend_from_slice(&name_string(name)?);
        line.push(b' ');
        line.extend_from_slice(&script_literal(script)?);
        line.extend_from_slice(b"\r\n");
        let (_, done) = self.command(&line, &format!("PUTSCRIPT … ({} bytes)", script.len()))?;
        self.written(&done)
    }

    /// Make `name` the script the server runs on incoming mail; the one that was active is not
    /// any more (it stays on the server).
    pub fn set_active(&mut self, name: &str) -> Result<(), ProtoError> {
        let mut line = b"SETACTIVE ".to_vec();
        line.extend_from_slice(&name_string(name)?);
        line.extend_from_slice(b"\r\n");
        self.simple(&line, "SETACTIVE …")
    }

    /// Leave the server with no active script: no script runs on incoming mail. The scripts stay.
    pub fn deactivate(&mut self) -> Result<(), ProtoError> {
        self.simple(b"SETACTIVE \"\"\r\n", "SETACTIVE \"\"")
    }

    /// Remove the script `name` (a server refuses to remove the active one).
    pub fn delete_script(&mut self, name: &str) -> Result<(), ProtoError> {
        let mut line = b"DELETESCRIPT ".to_vec();
        line.extend_from_slice(&name_string(name)?);
        line.extend_from_slice(b"\r\n");
        self.simple(&line, "DELETESCRIPT …")
    }

    /// LOGOUT. A server that hangs up without answering has logged out all the same.
    pub fn logout(mut self) -> Result<(), ProtoError> {
        match self.command(b"LOGOUT\r\n", "LOGOUT") {
            Ok(_) | Err(ProtoError::Io(_)) => Ok(()),
            // The answer to LOGOUT is BYE, or OK and then BYE: either is a goodbye.
            Err(ProtoError::Protocol(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn simple(&mut self, line: &[u8], shown: &str) -> Result<(), ProtoError> {
        let (_, done) = self.command(line, shown)?;
        if done.status == Status::Ok { Ok(()) } else { Err(self.refused(&done)) }
    }

    /// The answer to PUTSCRIPT or CHECKSCRIPT: its warnings, or why it was refused.
    fn written(&self, done: &Done) -> Result<Option<String>, ProtoError> {
        match done.status {
            Status::Ok if done.code.as_deref() == Some("WARNINGS") => Ok(Some(tidy(&self.secrets.hide(&done.text)))),
            Status::Ok => Ok(None),
            _ => Err(self.refused(done)),
        }
    }

    /// The capabilities, a line each, and then `OK` (a greeting may be `BYE` instead).
    fn read_capabilities(&mut self, greeting: bool) -> Result<(), ProtoError> {
        let mut capabilities = Capabilities::default();
        for _ in 0..MAX_DATA_LINES {
            let tokens = read_response(&mut self.wire)?;
            self.trace(&tokens);
            let line = match classify(tokens) {
                Ok(Response::Data(line)) => line,
                Ok(Response::Done(done)) => {
                    return match done.status {
                        Status::Ok => {
                            self.capabilities = capabilities;
                            Ok(())
                        }
                        Status::No if greeting => {
                            Err(ProtoError::Protocol(said!("the server greeted with a refusal: {reason}", reason = tidy(&done.text))))
                        }
                        Status::No => Err(self.refused(&done)),
                        Status::Bye => Err(self.ended(&done)),
                    };
                }
                Err(_) if greeting => return Err(ProtoError::Protocol(said!("the server didn't greet like a ManageSieve server"))),
                Err(e) => return Err(e),
            };
            let mut parts = line.iter();
            let name = parts.next().and_then(Token::text).ok_or_else(not_managesieve)?.to_ascii_uppercase();
            let value = parts.next().and_then(Token::text).unwrap_or_default();
            let words = || value.split_ascii_whitespace().filter(|w| w.len() <= MAX_WORD).take(MAX_LISTED);
            match name.as_str() {
                "IMPLEMENTATION" => capabilities.implementation = tidy(&value).chars().take(MAX_WORD).collect(),
                "SASL" => capabilities.sasl = words().map(str::to_ascii_uppercase).collect(),
                "SIEVE" => capabilities.extensions = words().map(str::to_ascii_lowercase).collect(),
                "STARTTLS" => capabilities.starttls = true,
                "VERSION" => capabilities.version = Some(tidy(&value).chars().take(MAX_WORD).collect()),
                "MAXREDIRECTS" => capabilities.max_redirects = value.trim().parse().ok(),
                // NOTIFY, LANGUAGE, OWNER (the user's name: not kept), UNAUTHENTICATE, and those
                // of later extensions.
                _ => {}
            }
        }
        Err(ProtoError::Protocol(said!("the server sent too many responses to one command")))
    }

    /// Send a command and read its answer: the lines of data, and how it ended. `BYE` is an
    /// error; so is a referral, which is never followed.
    fn command(&mut self, line: &[u8], shown: &str) -> Result<(Vec<Vec<Token>>, Done), ProtoError> {
        self.send(line, shown)?;
        let mut data = Vec::new();
        loop {
            let tokens = read_response(&mut self.wire)?;
            self.trace(&tokens);
            match classify(tokens)? {
                Response::Done(done) if done.status == Status::Bye => return Err(self.ended(&done)),
                Response::Done(done) if done.code.as_deref() == Some("REFERRAL") => return Err(referral()),
                Response::Done(done) => return Ok((data, done)),
                Response::Data(line) => {
                    if data.len() >= MAX_DATA_LINES {
                        return Err(ProtoError::Protocol(said!("the server sent too many responses to one command")));
                    }
                    data.push(line);
                }
            }
        }
    }

    /// Read to the end of a command's answer (after a cancelled sign-in).
    fn finish(&mut self) -> Result<Done, ProtoError> {
        for _ in 0..MAX_DATA_LINES {
            let tokens = read_response(&mut self.wire)?;
            self.trace(&tokens);
            if let Response::Done(done) = classify(tokens)? {
                return Ok(done);
            }
        }
        Err(ProtoError::Protocol(said!("the server sent too many responses to one command")))
    }

    /// Write `line`; the log gets `shown` instead (never a script, a name or a secret).
    fn send(&mut self, line: &[u8], shown: &str) -> Result<(), ProtoError> {
        log::trace!("C: {}", for_trace(&self.secrets.hide(shown)));
        self.wire.write(line)?;
        self.wire.flush()
    }

    /// What the log is told of a response: its status and code, or how many parts of data it had.
    fn trace(&self, tokens: &[Token]) {
        if log::log_enabled!(log::Level::Trace) {
            let shown = match tokens.first() {
                Some(Token::Atom(word)) => {
                    let code = match (tokens.get(1), tokens.get(2)) {
                        (Some(Token::Open), Some(Token::Atom(code))) => format!(" ({code})"),
                        _ => String::new(),
                    };
                    format!("{}{code}", for_trace(&word.to_ascii_uppercase()))
                }
                _ => format!("({} parts)", tokens.len()),
            };
            log::trace!("S: {shown}");
        }
    }

    /// The error for an answer that says no.
    fn refused(&self, done: &Done) -> ProtoError {
        // Hidden before it is shortened: a secret cut in half would no longer be recognised.
        let text = tidy(&self.secrets.hide(&done.text));
        let text = match (&done.code, text.is_empty()) {
            (_, false) => text,
            (Some(code), true) => tidy(code),
            (None, true) => return ProtoError::Rejected(said!("the command was refused")),
        };
        ProtoError::Rejected(text.into())
    }

    fn sign_in_failed(&self, done: &Done) -> ProtoError {
        match self.refused(done) {
            ProtoError::Rejected(text) => ProtoError::Auth(text),
            other => other,
        }
    }

    /// The server said goodbye in the middle of things.
    fn ended(&self, done: &Done) -> ProtoError {
        if done.code.as_deref() == Some("REFERRAL") {
            return referral();
        }
        let text = tidy(&self.secrets.hide(&done.text));
        if text.is_empty() {
            ProtoError::Io(said!("the server ended the session"))
        } else {
            ProtoError::Io(said!("the server ended the session: {reason}", reason = text))
        }
    }
}

/// A sign-in error said in words of the mechanism's own, for a server that asked for more than it.
fn asked_more(error: ProtoError, words: mailcraft_i18n::Text) -> ProtoError {
    match error {
        ProtoError::Auth(text) if text == said!("the server asked for more than a sign-in has") => ProtoError::Auth(words),
        other => other,
    }
}

enum Response {
    Done(Done),
    Data(Vec<Token>),
}

/// A response is the end of the command's answer when it starts with `OK`, `NO` or `BYE`, and a
/// line of data when it starts with a string.
fn classify(tokens: Vec<Token>) -> Result<Response, ProtoError> {
    let status = match tokens.first() {
        Some(Token::Str(_)) => return Ok(Response::Data(tokens)),
        Some(word) => match word.keyword().as_deref() {
            Some("OK") => Status::Ok,
            Some("NO") => Status::No,
            Some("BYE") => Status::Bye,
            _ => return Err(not_managesieve()),
        },
        None => return Err(not_managesieve()),
    };
    let mut rest = tokens.into_iter().skip(1).peekable();
    let mut code = None;
    if rest.next_if_eq(&Token::Open).is_some() {
        code = Some(rest.next().and_then(|t| t.keyword()).ok_or_else(not_managesieve)?);
        // The code's own arguments (a referral's address, a tag, SASL's last data) are of no
        // use here; nothing may be nested in them.
        loop {
            match rest.next() {
                Some(Token::Close) => break,
                Some(Token::Open) | None => return Err(not_managesieve()),
                Some(_) => {}
            }
        }
    }
    // The server's words: a string, as the grammar has it, or the atoms of a careless server.
    let text = rest
        .filter_map(|t| match t {
            Token::Str(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
            Token::Atom(word) => Some(word),
            Token::Open | Token::Close => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok(Response::Done(Done { status, code, text }))
}

fn not_managesieve() -> ProtoError {
    ProtoError::Protocol(said!("the server sent something that isn't a ManageSieve response"))
}

fn referral() -> ProtoError {
    ProtoError::Rejected(said!("the server sent MailCraft to another server for the Sieve scripts, which it doesn't follow"))
}

/// Whether `name` can be a script's name (RFC 5804 §1.6): not empty, not too long, and none of
/// the control characters, line or paragraph separators that are not allowed in one.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && !name.chars().any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
}

/// A script's name in quotes, or why it can't be sent.
fn name_string(name: &str) -> Result<Vec<u8>, ProtoError> {
    if !valid_name(name) {
        return Err(ProtoError::Protocol(said!("\"{name}\" can't be used as a script name", name = tidy(name))));
    }
    Ok(string(name.as_bytes()))
}

/// A script as a literal, or why it can't be sent.
fn script_literal(script: &str) -> Result<Vec<u8>, ProtoError> {
    if script.len() > MAX_SCRIPT {
        return Err(ProtoError::Protocol(said!("the script is larger than the {kb} KB MailCraft sends", kb = MAX_SCRIPT / 1024)));
    }
    Ok(literal(script.as_bytes()))
}

/// A string as the client sends it: in quotes (with `"` and `\` escaped) when it is short and has
/// no line break or NUL, else as a literal of the bytes as they are.
pub(crate) fn string(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() <= MAX_QUOTED && !bytes.iter().any(|b| matches!(b, b'\r' | b'\n' | 0)) {
        let mut out = Vec::with_capacity(bytes.len().saturating_add(8));
        out.push(b'"');
        for &b in bytes {
            if matches!(b, b'"' | b'\\') {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.push(b'"');
        out
    } else {
        literal(bytes)
    }
}

/// A literal the server doesn't have to agree to first (`{n+}`, RFC 5804 §4).
fn literal(bytes: &[u8]) -> Vec<u8> {
    let mut out = format!("{{{}+}}\r\n", bytes.len()).into_bytes();
    out.extend_from_slice(bytes);
    out
}

/// What SASL OAUTHBEARER sends (RFC 7628 §3.1), in base64: the GS2 header with the user, then
/// the bearer token, between `^A`s. A name or a token that would break out of that is refused.
pub(crate) fn oauthbearer(user: &str, token: &str) -> Result<String, ProtoError> {
    // The same checks as XOAUTH2's: no ^A, line break or NUL in either, and not too long.
    xoauth2(user, token)?;
    // In the GS2 header `,` and `=` are written `=2C` and `=3D` (RFC 5801 §4).
    let user = user.replace('=', "=3D").replace(',', "=2C");
    Ok(STANDARD.encode(format!("n,a={user},\x01auth=Bearer {token}\x01\x01")))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod limits_tests;
