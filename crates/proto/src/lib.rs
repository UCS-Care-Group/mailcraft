// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft mail protocols without sockets.
//!
//! [`smtp::Smtp`] and [`imap::Imap`] speak SMTP (RFC 5321, 3207, 4954) and IMAP4rev1 (RFC 3501,
//! 4315, 4959, 6154, 6851) over any `Read + Write` stream: a TLS socket on the desktop (`mailcraft-net`), a scripted
//! transcript in the tests. Everything a server sends is treated as hostile: lines, literals and
//! response counts are capped, and nothing here panics on junk.
//!
//! Passwords and access tokens never reach a log or an error text, even when a server echoes
//! them back.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod imap;
mod line;
pub mod smtp;

#[cfg(test)]
mod script;

/// Longest line a server may send.
pub const MAX_LINE: usize = 64 * 1024;
/// Largest counted block (an IMAP literal, so: the largest message) a server may send.
pub const MAX_LITERAL: usize = 64 * 1024 * 1024;
/// Longest user name and access token, together, that one sign-in may carry.
const MAX_XOAUTH2: usize = 32 * 1024;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ProtoError {
    #[error("connection problem: {0}")]
    Io(String),
    #[error("unexpected server response: {0}")]
    Protocol(String),
    /// The server refused the command (IMAP NO/BAD, SMTP 4xx/5xx); the text is the server's.
    #[error("the server said: {0}")]
    Rejected(String),
    #[error("sign-in failed: {0}")]
    Auth(String),
}

/// What SASL XOAUTH2 sends, in base64: `user=<user>^Aauth=Bearer <token>^A^A`, where `^A` is the
/// byte 0x01. A name or a token that would break out of that, or of the line it goes on, is refused.
pub(crate) fn xoauth2(user: &str, token: &str) -> Result<String, ProtoError> {
    if token.is_empty() {
        return Err(ProtoError::Auth("there is no access token to sign in with".into()));
    }
    let unsendable = |text: &str| text.bytes().any(|b| matches!(b, 0 | 1 | b'\r' | b'\n'));
    if unsendable(user) || unsendable(token) {
        return Err(ProtoError::Auth("the user name or access token has a character that can't be sent".into()));
    }
    if user.len().saturating_add(token.len()) > MAX_XOAUTH2 {
        return Err(ProtoError::Auth("the user name or access token is too long to be sent".into()));
    }
    Ok(mailcraft_charset::base64::STANDARD.encode(format!("user={user}\x01auth=Bearer {token}\x01\x01")))
}

/// What must never be shown: the password or access token and every form it was sent in.
#[derive(Clone, Default)]
pub(crate) struct Secrets(Vec<String>);

impl Secrets {
    pub(crate) fn add(&mut self, secret: &str) {
        if !secret.is_empty() && !self.0.iter().any(|s| s == secret) {
            self.0.push(secret.to_string());
            // Longest first, so a secret inside another one can't leave a piece behind.
            self.0.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    /// `text` with every secret replaced by `***`.
    pub(crate) fn hide(&self, text: &str) -> String {
        let mut out = text.to_string();
        for s in &self.0 {
            if out.contains(s.as_str()) {
                out = out.replace(s.as_str(), "***");
            }
        }
        out
    }
}

/// What the wire trace shows of a protocol line, either way: its leading keywords (a tag, a
/// reply code, a command or status word, a sequence set, an extension's name) and how long the
/// rest is. Whatever follows the first character that is no keyword's (lower case, `@`, a
/// quote, a bracket of a list) is left out: a user name, an address in `MAIL FROM` or `RCPT TO`,
/// a folder's name, a server's "ada@example.org authenticated" or "<bob@example.org> unknown",
/// the flags and headers of a message. Secrets are hidden before this ([`Secrets::hide`]).
pub(crate) fn for_trace(line: &str) -> String {
    const MAX_KEPT: usize = 120;
    let line = line.trim_end();
    let keyword = |c: char| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '*' | '+' | '-' | '.' | ':' | ',' | '=' | '_' | '[' | ']');
    let end = line.char_indices().find(|(_, c)| !keyword(*c) && *c != ' ').map_or(line.len(), |(at, _)| at);
    let mut kept = line.get(..end).unwrap_or_default();
    let opens = line.get(end..).is_some_and(|rest| rest.starts_with(['<', '(', '"', '{', '\\']));
    if end < line.len() && !kept.ends_with(' ') && !opens {
        // A word cut short is left out, unless it is the first one (`250-` of `250-mx.example`).
        if let Some(space) = kept.trim_end().rfind(' ') {
            kept = kept.get(..space).unwrap_or_default();
        }
    }
    let kept: String = kept.trim_end().chars().take(MAX_KEPT).collect();
    let left = line.get(kept.len()..).unwrap_or_default().trim_start().len();
    match (kept.is_empty(), left) {
        (_, 0) => kept,
        (true, _) => format!("({left} bytes)"),
        (false, _) => format!("{kept} … ({left} bytes)"),
    }
}

/// Server text made fit for an error message: one line, no control characters, not too long.
pub(crate) fn tidy(text: &str) -> String {
    const MAX_CHARS: usize = 300;
    let mut out = String::new();
    let mut space = false;
    for (n, c) in text.trim().chars().enumerate() {
        if n >= MAX_CHARS {
            out.push('…');
            break;
        }
        if c.is_control() || c.is_whitespace() {
            space = !out.is_empty();
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_trace_keeps_keywords_and_no_address_name_or_text() {
        // (Line, what the trace shows.)
        for (line, shown) in [
            ("A0001 OK ada@example.org authenticated (Success)\r\n", "A0001 OK … (39 bytes)"),
            ("A0002 LOGIN ***", "A0002 LOGIN ***"),
            ("MAIL FROM:<ada@example.org> SIZE=120", "MAIL FROM: … (26 bytes)"),
            ("RCPT TO:<bob@example.org>", "RCPT TO: … (17 bytes)"),
            ("550 5.1.1 <bob@example.org>: Recipient address rejected", "550 5.1.1 … (45 bytes)"),
            ("250-mx.example.org at your service", "250- … (30 bytes)"),
            ("250-AUTH LOGIN PLAIN XOAUTH2", "250-AUTH LOGIN PLAIN XOAUTH2"),
            ("A0003 SELECT \"Letters from Grace\"", "A0003 SELECT … (20 bytes)"),
            ("* LIST (\\HasNoChildren) \"/\" Grace", "* LIST … (26 bytes)"),
            ("* 12 FETCH (UID 40 FLAGS (\\Seen) BODY[HEADER.FIELDS (SUBJECT)] \"Subject: Quarterly figures\")", "* 12 FETCH … (81 bytes)"),
            ("* 3 EXISTS", "* 3 EXISTS"),
            ("UID FETCH 1:40,42 (UID FLAGS)", "UID FETCH 1:40,42 … (11 bytes)"),
            ("hello there", "(11 bytes)"),
            ("", ""),
        ] {
            let traced = for_trace(line);
            assert_eq!(traced, shown, "{line:?}");
            for private in ["ada", "bob", "Grace", "Quarterly", "mx.example"] {
                assert!(!traced.contains(private), "{line:?}: {traced}");
            }
        }
        assert!(for_trace(&format!("A0001 OK {}", "X ".repeat(500))).len() < 160);
        assert_eq!(for_trace("ünïcödé ä"), "(14 bytes)");
    }

    #[test]
    fn secrets_are_hidden_longest_first() {
        let mut s = Secrets::default();
        s.add("hunter2");
        s.add("hunter2-and-more");
        s.add("");
        assert_eq!(s.hide("no hunter2-and-more, no hunter2, nothing"), "no ***, no ***, nothing");
        assert_eq!(Secrets::default().hide("plain"), "plain");
    }

    #[test]
    fn the_xoauth2_response_is_user_and_bearer_token_between_control_a() {
        let decoded = |payload: String| mailcraft_charset::base64::STANDARD.decode(payload).unwrap();
        assert_eq!(decoded(xoauth2("ada@example.org", "tok.en-123").unwrap()), b"user=ada@example.org\x01auth=Bearer tok.en-123\x01\x01");
        // The example of the mechanism's description.
        assert_eq!(
            xoauth2("someuser@example.com", "ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg").unwrap(),
            "dXNlcj1zb21ldXNlckBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciB5YTI5LnZGOWRmdDRxbVRjMk52YjNSbGNrQmhkSFJoZG1semRHRXVZMjl0Q2cBAQ=="
        );
        assert_eq!(decoded(xoauth2("ädä", "t").unwrap()), "user=ädä\x01auth=Bearer t\x01\x01".as_bytes());
        // Nothing that could end the name, the token or the line early.
        for (user, token) in [("ada\x01auth=Bearer other", "t"), ("ada", "t\x01"), ("ada\r\n", "t"), ("ada", "t\nQUIT"), ("a\0", "t"), ("ada", "\0")]
        {
            assert!(matches!(xoauth2(user, token), Err(ProtoError::Auth(_))), "{user:?} {token:?}");
        }
        assert!(matches!(xoauth2("ada", ""), Err(ProtoError::Auth(_))));
        assert!(xoauth2("ada", &"t".repeat(MAX_XOAUTH2 - 3)).is_ok());
        let err = xoauth2("ada", &"t".repeat(MAX_XOAUTH2)).unwrap_err();
        assert!(matches!(&err, ProtoError::Auth(why) if why.len() < 100), "{err:?}");
    }

    #[test]
    fn server_text_is_tidied() {
        assert_eq!(tidy("  5.7.8\tBad\r\n credentials \u{7}"), "5.7.8 Bad credentials");
        let long = tidy(&"é".repeat(1000));
        assert_eq!(long.chars().count(), 301);
        assert!(long.ends_with('…'));
        assert_eq!(tidy(""), "");
    }
}
