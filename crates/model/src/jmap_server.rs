// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a JMAP account's server is ([`JmapServer`], held by [`crate::Account::jmap`]), and what
//! an address must be before anything of the owner's is sent to it.
//!
//! An address is checked as text. That takes no network library, so the engine and the browser
//! build can do it, and a host that is about to connect checks it again: a mailbox file is
//! hostile too, and it can say anything.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

/// The server of a [`crate::AccountKind::Jmap`] account (RFC 8620): where its session resource
/// is, and how the secret is sent. Plain settings: the secret itself (a password, or a bearer
/// token) is given by whoever holds the connection, and is never kept here.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapServer {
    /// The address of the server's session resource (`https://…`). It may carry the account's own
    /// address in its query, so it is never written to a log; [`JmapServer::check_session`] says
    /// whether it may be used.
    pub session: String,
    /// The secret is a bearer token. Otherwise it is the account's password, sent with the
    /// account's e-mail address as its login name (HTTP Basic).
    pub bearer: bool,
}

/// The address stays out of debug output, which may reach a log.
impl std::fmt::Debug for JmapServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapServer").field("session", &!self.session.is_empty()).field("bearer", &self.bearer).finish()
    }
}

impl JmapServer {
    /// Longest session address, in characters.
    pub const MAX_SESSION: usize = 2048;

    /// Whether `address` may be a session address: `Ok`, or why not, in words that follow the
    /// setting's name (`is empty`, `has a backslash in it`…).
    ///
    /// - `https://` only; plain `http://` only to a server on this computer (`localhost`, an
    ///   address in `127.0.0.0/8`, `[::1]`), which is for tests.
    /// - At most [`JmapServer::MAX_SESSION`] characters, and no control character, space or
    ///   backslash anywhere: a character a reader of the address could take differently from
    ///   another is refused rather than guessed at.
    /// - No user name or password in front of the server's name (what follows the name may hold an
    ///   `@`: an account's address in a query is no user name).
    /// - A server named by letters, digits, dots and dashes (an international name in its
    ///   `xn--` form), or by an IP address (an IPv6 one in brackets), and a port from 1 to 65535.
    ///
    /// Nothing is looked up and nothing is sent.
    pub fn check_session(address: &str) -> Result<(), &'static str> {
        accepted(address).map(|_| ())
    }

    /// Why this server's session address can't be used, when it can't ([`JmapServer::check_session`]).
    pub fn problem(&self) -> Option<&'static str> {
        Self::check_session(&self.session).err()
    }

    /// The name of the server the session address is on (`mail.example.org`), when the address may
    /// be used: for telling the owner which server it is, without the rest of the address.
    pub fn host(&self) -> Option<&str> {
        accepted(&self.session).ok()
    }
}

/// The server's name in `address` when `address` may be used, else why not.
fn accepted(address: &str) -> Result<&str, &'static str> {
    if address.is_empty() {
        return Err("is empty");
    }
    if address.chars().take(JmapServer::MAX_SESSION + 1).count() > JmapServer::MAX_SESSION {
        return Err("is longer than 2048 characters");
    }
    if address.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("has a control character or a space in it");
    }
    if address.contains('\\') {
        return Err("has a backslash in it");
    }
    let (https, rest) = if let Some(rest) = after_scheme(address, "https://") {
        (true, rest)
    } else if let Some(rest) = after_scheme(address, "http://") {
        (false, rest)
    } else {
        return Err("isn't an https:// address");
    };
    // The server's name, and its port, end at the first `/`, `?` or `#`.
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or_default();
    if authority.is_empty() {
        return Err("names no server");
    }
    if authority.contains('@') {
        return Err("carries a user name or a password: the secret is given apart from the address");
    }
    let (host, port) = split_authority(authority)?;
    if let Some(port) = port {
        // (`parse` alone would take a sign.)
        let digits = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
        if !digits || !port.parse::<u16>().is_ok_and(|n| n != 0) {
            return Err("has a port that isn't a number from 1 to 65535");
        }
    }
    if !https && !is_loopback(host) {
        return Err("is a plain http:// address to a server that isn't on this computer: use https://");
    }
    Ok(host)
}

/// What follows `scheme` (`https://`, in any case) at the start of `address`.
fn after_scheme<'a>(address: &'a str, scheme: &str) -> Option<&'a str> {
    address.get(..scheme.len()).filter(|head| head.eq_ignore_ascii_case(scheme)).and_then(|_| address.get(scheme.len()..))
}

/// The server's name (an IPv6 address keeps its brackets) and the port after it, if any.
fn split_authority(authority: &str) -> Result<(&str, Option<&str>), &'static str> {
    const NOT_A_NAME: &str = "names a server that isn't letters, digits, dots and dashes, or an IP address";
    if authority.starts_with('[') {
        let close = authority.find(']').ok_or(NOT_A_NAME)?;
        let host = authority.get(..=close).ok_or(NOT_A_NAME)?;
        let inside = authority.get(1..close).ok_or(NOT_A_NAME)?;
        if inside.parse::<Ipv6Addr>().is_err() {
            return Err(NOT_A_NAME);
        }
        return match authority.get(close + 1..) {
            Some("") => Ok((host, None)),
            Some(after) => after.strip_prefix(':').map(|port| (host, Some(port))).ok_or(NOT_A_NAME),
            None => Err(NOT_A_NAME),
        };
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if host.is_empty() || host.len() > 253 || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
        return Err(NOT_A_NAME);
    }
    Ok((host, port))
}

/// Whether `host` (as `accepted` hands it over) is this computer: `localhost`, an IPv4 address in
/// `127.0.0.0/8` or `[::1]`. Nothing else is taken for it, whatever it may resolve to.
fn is_loopback(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    match host.strip_prefix('[').and_then(|inside| inside.strip_suffix(']')) {
        Some(inside) => inside.parse::<Ipv6Addr>().is_ok_and(|ip| ip.is_loopback()),
        None => host.parse::<Ipv4Addr>().is_ok_and(|ip| ip.is_loopback()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(address: &str) -> Result<(), &'static str> {
        JmapServer::check_session(address)
    }

    #[test]
    fn an_https_address_may_be_used() {
        for good in [
            "https://mail.example.org/jmap/session",
            "https://mail.example.org",
            "HTTPS://Mail.Example.org:8443/.well-known/jmap",
            // What follows the server's name may hold an @ (an account's address in a query) or a #.
            "https://mail.example.org/jmap/session?account=ada@example.org",
            "https://mail.example.org/jmap/session?account=ada%40example.org#top",
            "https://mail.example.org?x",
            "https://[2001:db8::1]/jmap",
            "https://[2001:db8::1]:8443/jmap",
            "https://192.0.2.7:8080/jmap",
            "https://xn--bcher-kva.example/jmap",
            // Whatever is not a control character, a space or a backslash is data after the name.
            "https://mail.example.org/jmap/session?name=Zoë",
        ] {
            assert_eq!(session(good), Ok(()), "{good}");
        }
        let server = JmapServer { session: "https://Mail.Example.org:8443/jmap/session?account=ada@example.org".into(), bearer: true };
        assert_eq!((server.problem(), server.host()), (None, Some("Mail.Example.org")));
        let v6 = JmapServer { session: "https://[2001:db8::1]:8443/jmap".into(), bearer: false };
        assert_eq!(v6.host(), Some("[2001:db8::1]"));
    }

    #[test]
    fn plain_http_is_for_this_computer_only() {
        for good in
            ["http://localhost/jmap", "http://LOCALHOST:8080/jmap/session", "http://127.0.0.1:8080/s", "http://127.1.2.3/s", "http://[::1]:3000/s"]
        {
            assert_eq!(session(good), Ok(()), "{good}");
        }
        let plain = "is a plain http:// address to a server that isn't on this computer: use https://";
        for bad in [
            "http://mail.example.org/jmap",
            "http://localhost.example.org/jmap",
            "http://127.0.0.1.example.org/jmap",
            "http://10.0.0.1/jmap",
            "http://192.168.1.2/jmap",
            "http://0.0.0.0/jmap",
            "http://[::ffff:127.0.0.1]/jmap",
            "http://[2001:db8::1]/jmap",
            // Spellings a browser reads as 127.0.0.1 are not taken for it.
            "http://0x7f.0.0.1/jmap",
            "http://2130706433/jmap",
            "http://127.1/jmap",
            "http://127.000.000.001/jmap",
            "http://localhost./jmap",
        ] {
            assert_eq!(session(bad), Err(plain), "{bad}");
        }
        // A user name is refused for what it is, whatever the server.
        assert_eq!(session("http://127.0.0.1@evil.example/jmap").map_err(|why| why.starts_with("carries")), Err(true));
    }

    #[test]
    fn what_cannot_be_a_session_address_says_why() {
        let name = "names a server that isn't letters, digits, dots and dashes, or an IP address";
        let port = "has a port that isn't a number from 1 to 65535";
        let control = "has a control character or a space in it";
        let user = "carries a user name or a password: the secret is given apart from the address";
        for (bad, why) in [
            ("", "is empty"),
            ("mail.example.org/jmap", "isn't an https:// address"),
            ("ftp://mail.example.org/", "isn't an https:// address"),
            ("file:///etc/passwd", "isn't an https:// address"),
            ("javascript:alert(1)", "isn't an https:// address"),
            ("//mail.example.org/jmap", "isn't an https:// address"),
            ("https:/mail.example.org/jmap", "isn't an https:// address"),
            ("https//mail.example.org/jmap", "isn't an https:// address"),
            ("nothttps://mail.example.org/", "isn't an https:// address"),
            ("https:", "isn't an https:// address"),
            ("https://", "names no server"),
            ("HTTPS://", "names no server"),
            ("http://", "names no server"),
            ("https:///jmap", "names no server"),
            ("https://?x=1", "names no server"),
            ("https://#top", "names no server"),
            ("https://ada@mail.example.org/jmap", user),
            ("https://ada:secret@mail.example.org/jmap", user),
            ("https://:@mail.example.org/jmap", user),
            ("https://mail.example.org@evil.example/jmap", user),
            ("https://mail.example.org:443@evil.example/jmap", user),
            ("https://mail.example.org\\@evil.example/jmap", "has a backslash in it"),
            ("https://mail.example.org\\jmap", "has a backslash in it"),
            ("https://mail.example.org/jmap\n", control),
            ("https://mail.example.org/jmap\r\nHost: evil.example", control),
            ("https://mail.example.org/jmap\0", control),
            ("https://mail.example.org/jmap\t", control),
            ("https://mail.example.org/\u{7f}", control),
            ("https://mail.example.org/\u{85}", control),
            ("https://mail.example.org/a b", control),
            ("https://mail.example.org/\u{a0}", control),
            ("https://mail.example.org/\u{2003}", control),
            (" https://mail.example.org/", control),
            ("https://mail.example.org/jmap ", control),
            ("https://mail_example.org/jmap", name),
            ("https://mail.exämple.org/jmap", name),
            ("https://%6dail.example.org/jmap", name),
            ("https://mail example.org/", control),
            ("https://[::1/jmap", name),
            ("https://[::1]x/jmap", name),
            ("https://[zzz]/jmap", name),
            ("https://[]/jmap", name),
            ("https://[::1%25eth0]/jmap", name),
            ("https://mail.example.org:0/jmap", port),
            ("https://mail.example.org:65536/jmap", port),
            ("https://mail.example.org:+80/jmap", port),
            ("https://mail.example.org:80a/jmap", port),
            ("https://mail.example.org:/jmap", port),
            ("https://mail.example.org:8080:9/jmap", name),
            ("https://[::1]:99999/jmap", port),
            ("https://:443/jmap", name),
        ] {
            assert_eq!(session(bad), Err(why), "{bad:?}");
        }
        // A server's name has its own limit.
        assert_eq!(session(&format!("https://{}.example/jmap", "a".repeat(260))), Err(name));
    }

    #[test]
    fn an_address_is_at_most_2048_characters() {
        let base = "https://mail.example.org/";
        assert_eq!(base.len(), 25);
        for filler in ["a", "é", "😀"] {
            let fits = format!("{base}{}", filler.repeat(JmapServer::MAX_SESSION - base.len()));
            let long = format!("{fits}{filler}");
            assert_eq!(fits.chars().count(), 2048);
            assert_eq!(session(&fits), Ok(()), "2048 characters of {filler}");
            assert_eq!(session(&long), Err("is longer than 2048 characters"), "2049 characters of {filler}");
        }
        // Only as far as the limit is looked at.
        assert_eq!(session(&format!("{base}{}", "a".repeat(10_000_000))), Err("is longer than 2048 characters"));
        assert_eq!(session(&"\u{0}".repeat(10_000_000)), Err("is longer than 2048 characters"));
    }

    #[test]
    fn the_setting_reads_writes_and_keeps_its_address_out_of_debug_output() {
        let server = JmapServer { session: "https://mail.example.org/jmap/session?account=ada@example.org".into(), bearer: true };
        let json = serde_json::to_string(&server).unwrap();
        assert_eq!(json, r#"{"session":"https://mail.example.org/jmap/session?account=ada@example.org","bearer":true}"#);
        assert_eq!(serde_json::from_str::<JmapServer>(&json).unwrap(), server);
        // What is not said is empty, and a password is the way to send the secret.
        let bare: JmapServer = serde_json::from_str("{}").unwrap();
        assert_eq!((bare.session.as_str(), bare.bearer, bare.problem(), bare.host()), ("", false, Some("is empty"), None));
        let shown = format!("{server:?} {server:#?}");
        assert!(!shown.contains('@') && !shown.contains("example.org") && shown.contains("bearer: true"), "{shown}");
    }

    proptest::proptest! {
        /// Whatever it is handed, it answers; and what it lets through is what it says it is.
        #[test]
        fn any_text_is_answered_and_an_accepted_one_is_sane(
            address in proptest::prelude::any::<String>(),
            host in "[a-zA-Z0-9.:@\\[\\]-]{0,24}",
            port in proptest::option::of(0u32..100_000),
            path in "[!-~]{0,30}",
        ) {
            let port = port.map(|n| format!(":{n}")).unwrap_or_default();
            let texts = [
                address.clone(),
                format!("https://{address}"),
                format!("http://localhost{address}"),
                format!("https://mail.example.org/{address}"),
                format!("https://{host}{port}/{path}"),
                format!("http://{host}{port}/{path}"),
                format!("https://mail.example.org{port}/{path}"),
            ];
            for text in texts {
                if JmapServer::check_session(&text).is_err() {
                    continue;
                }
                proptest::prop_assert!(text.chars().count() <= JmapServer::MAX_SESSION);
                proptest::prop_assert!(!text.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\'));
                let lower = text.to_ascii_lowercase();
                let rest = text.get(text.find("://").map_or(0, |at| at + 3)..).unwrap_or_default();
                let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
                proptest::prop_assert!(!authority.is_empty() && !authority.contains('@'));
                let server = JmapServer { session: text.clone(), bearer: false };
                let named = server.host().unwrap_or_default();
                proptest::prop_assert!(!named.is_empty() && authority.starts_with(named));
                // Plain http, only to this computer.
                proptest::prop_assert!(lower.starts_with("https://") || (lower.starts_with("http://") && is_loopback(named)));
            }
        }
    }
}
