//! `http(s)://` addresses: parsed, resolved against a base, and percent-encoding.
//!
//! Addresses reach this crate from the owner (the session's address) and from a server (the
//! session's `apiUrl`, `downloadUrl` and `uploadUrl`), so nothing here trusts them. They are bounded;
//! they hold no user name (`https://user:password@host/`); a host is a name of letters, digits, hyphens
//! and dots, or an IPv6 address in brackets; whatever in a path or a query is not printable ASCII is
//! percent-encoded, so that an address is sent as one line of plain ASCII; and what a parse refuses is
//! never echoed back, since an address can hold a person's address or a password.

use crate::{Error, Result};

/// Longest address parsed.
const MAX_URL: usize = 8192;
/// Longest host name (RFC 1035).
const MAX_HOST: usize = 253;

/// A parsed `http(s)://host[:port]/path?query` URL.
#[derive(Clone, PartialEq, Eq)]
pub struct Url {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Path and query, starting with `/`, in printable ASCII.
    pub path: String,
}

/// What may be shown of an address: the server it names and how long the rest is, never the path or the
/// query (they can hold a person's address).
impl std::fmt::Debug for Url {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Url").field("origin", &self.origin()).field("path_bytes", &self.path.len()).finish()
    }
}

pub fn default_port(https: bool) -> u16 {
    if https { 443 } else { 80 }
}

fn strip_prefix_ci<'a>(s: &'a str, p: &str) -> Option<&'a str> {
    s.get(..p.len()).filter(|h| h.eq_ignore_ascii_case(p)).and_then(|_| s.get(p.len()..))
}

/// A port: digits only, 1 to 65535.
fn port_of(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port != 0)
}

impl Url {
    pub fn parse(s: &str) -> Result<Url> {
        let s = s.trim();
        let bad = || Error::Protocol("not a web address".into());
        if s.len() > MAX_URL {
            return Err(bad());
        }
        let (https, rest) = if let Some(r) = strip_prefix_ci(s, "https://") {
            (true, r)
        } else if let Some(r) = strip_prefix_ci(s, "http://") {
            (false, r)
        } else {
            return Err(bad());
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at(end);
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6.split_once(']').ok_or_else(bad)?;
            if h.parse::<std::net::Ipv6Addr>().is_err() {
                return Err(bad());
            }
            let port = match after.strip_prefix(':') {
                Some(p) => port_of(p).ok_or_else(bad)?,
                None if after.is_empty() => default_port(https),
                None => return Err(bad()),
            };
            (h.to_string(), port)
        } else {
            let (h, port) = match authority.split_once(':') {
                Some((h, p)) => (h, port_of(p).ok_or_else(bad)?),
                None => (authority, default_port(https)),
            };
            let named = !h.is_empty() && h.len() <= MAX_HOST && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.'));
            if !named {
                return Err(bad());
            }
            (h.to_string(), port)
        };
        let tail = tail.split('#').next().unwrap_or("");
        let path = if tail.is_empty() {
            "/".to_string()
        } else if tail.starts_with('?') {
            format!("/{tail}")
        } else {
            tail.to_string()
        };
        // What is not printable ASCII (spaces, controls, anything beyond ASCII: every byte of its UTF-8) is
        // percent-encoded rather than sent raw, so that no line break, no blank and no byte of another
        // character set reaches a request line.
        let mut encoded = String::with_capacity(path.len());
        for c in path.chars() {
            if c.is_ascii_graphic() {
                encoded.push(c);
            } else {
                let mut utf8 = [0u8; 4];
                for b in c.encode_utf8(&mut utf8).bytes() {
                    encoded.push_str(&format!("%{b:02X}"));
                }
            }
        }
        Ok(Url { https, host: host.to_ascii_lowercase(), port, path: encoded })
    }

    /// A `Location` (or any reference) resolved against this URL.
    pub fn join(&self, location: &str) -> Result<Url> {
        let l = location.trim();
        // A reference with a scheme of its own is absolute (RFC 3986 §4.2): http and https are parsed,
        // any other is refused, rather than taken for a path.
        if let Some((scheme, _)) = l.split_once(':')
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        {
            return Url::parse(l);
        }
        let origin = self.origin();
        if let Some(rest) = l.strip_prefix("//") {
            return Url::parse(&format!("{}://{rest}", if self.https { "https" } else { "http" }));
        }
        if l.starts_with('/') {
            return Url::parse(&format!("{origin}{l}"));
        }
        // Only a query, or only a fragment (which is dropped): this path (RFC 3986 §5.2.2).
        let path = self.path.split('?').next().unwrap_or("/");
        if l.starts_with('?') {
            return Url::parse(&format!("{origin}{path}{l}"));
        }
        if l.starts_with('#') {
            return Ok(self.clone());
        }
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        Url::parse(&format!("{origin}{dir}/{l}"))
    }

    pub fn origin(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == default_port(self.https) { format!("{scheme}://{host}") } else { format!("{scheme}://{host}:{}", self.port) }
    }

    /// The whole address again.
    pub fn full(&self) -> String {
        format!("{}{}", self.origin(), self.path)
    }

    /// Whether the address is on this computer: `localhost`, or an IP address that is a loopback one
    /// (`127.0.0.0/8` written as four numbers, `::1`). The only place plain `http` is allowed to go to.
    pub fn is_loopback(&self) -> bool {
        self.host == "localhost" || self.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }
}

/// Percent-encode everything but RFC 3986's unreserved characters.
pub fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Undo [`pct`]: each `%XX` becomes its byte, what is then not UTF-8 is replaced, and a `%` that starts
/// no escape stays as it is.
pub fn pct_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    while let Some(&b) = bytes.get(at) {
        let escaped = if b == b'%' {
            bytes
                .get(at.saturating_add(1)..at.saturating_add(3))
                .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        } else {
            None
        };
        match escaped {
            Some(byte) => {
                out.push(byte);
                at = at.saturating_add(3);
            }
            None => {
                out.push(b);
                at = at.saturating_add(1);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn urls_and_encodings() {
        let u = Url::parse("https://Api.Example.com/jmap/session?x=1#frag").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("api.example.com", 443, "/jmap/session?x=1"));
        assert_eq!(u.join("/api/").unwrap().full(), "https://api.example.com/api/");
        assert_eq!(u.join("upload/").unwrap().full(), "https://api.example.com/jmap/upload/");
        assert_eq!(u.join("http://other:8080/x").unwrap().full(), "http://other:8080/x");
        for bad in ["ftp://x", "https://", "https://user@host/", "https://h:0/", "https://[::1/"] {
            assert!(Url::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(Url::parse("http://[::1]:8080").unwrap().origin(), "http://[::1]:8080");
        assert_eq!(pct("a b/é"), "a%20b%2F%C3%A9");
    }

    #[test]
    fn references_resolve_against_the_base() {
        let base = Url::parse("https://h.example:8443/a/b/session?account=x").unwrap();
        assert_eq!(base.join("//other.example/p").unwrap().full(), "https://other.example/p");
        assert_eq!(base.join("/p?q=1").unwrap().full(), "https://h.example:8443/p?q=1");
        assert_eq!(base.join("c").unwrap().full(), "https://h.example:8443/a/b/c");
        assert_eq!(base.join(" https://x.example ").unwrap().full(), "https://x.example/");
        // A query alone, or a fragment alone, keeps the path.
        assert_eq!(base.join("?y=2").unwrap().full(), "https://h.example:8443/a/b/session?y=2");
        assert_eq!(base.join("#top").unwrap().full(), "https://h.example:8443/a/b/session?account=x");
        // The base's scheme is the reference's when it names none.
        let plain = Url::parse("http://127.0.0.1:8080/s").unwrap();
        assert_eq!(plain.join("//127.0.0.1:9/p").unwrap().full(), "http://127.0.0.1:9/p");
        assert_eq!(Url::parse("HTTPS://H.Example").unwrap().full(), "https://h.example/");
        assert_eq!(Url::parse("https://h.example?a=1").unwrap().path, "/?a=1");
        // Another scheme is no path; a colon further into a path is none of a scheme's.
        for other in ["ftp://h.example/x", "javascript:alert(1)", "mailto:a@b.example", "c:d", "https:x"] {
            assert!(base.join(other).is_err(), "{other}");
        }
        assert_eq!(base.join("a/b:c").unwrap().full(), "https://h.example:8443/a/b/a/b:c");
        assert_eq!(base.join("./c:d").unwrap().full(), "https://h.example:8443/a/b/./c:d");
    }

    #[test]
    fn hostile_addresses_are_refused_without_being_echoed() {
        for bad in [
            "https://user:secret@host.example/",
            "https://host.example\\@evil.example/",
            "https://good.example\\.evil.example/",
            "https://good.example\\path",
            "https://ho st.example/",
            "https://host.example:99999/",
            "https://host.example:/",
            "https://host.example:+443/",
            "https://host.example:4 43/",
            "https://host.example:80:80/",
            "https://::1/",
            "https://[]/",
            "https://[::1]x/",
            "https://[evil.example]/",
            "https://[localhost]/",
            "https://[::1%25eth0]/",
            "https://\u{7}host/",
            "https://h\u{f6}st.example/",
            "https://host_name.example/",
            "https://%65vil.example/",
            "https://host.example%2f@evil.example/",
            "javascript:alert(1)",
            "",
        ] {
            let err = Url::parse(bad).unwrap_err().to_string();
            assert!(!err.contains("secret") && !err.contains("host"), "{bad}: {err}");
        }
        let long = format!("https://h.example/{}", "a".repeat(MAX_URL));
        assert!(Url::parse(&long).is_err());
        let long_host = format!("https://{}.example/", "a".repeat(MAX_HOST));
        assert!(Url::parse(&long_host).is_err());
        // A path or query is plain ASCII on one line: controls, blanks and what is beyond ASCII are
        // encoded whole, however many bytes their UTF-8 takes.
        let u = Url::parse("https://h.example/a b\r\nc\u{85}d?e=f g\u{3000}h\u{1F600}").unwrap();
        assert_eq!(u.path, "/a%20b%0D%0Ac%C2%85d?e=f%20g%E3%80%80h%F0%9F%98%80");
        assert!(u.full().bytes().all(|b| b.is_ascii_graphic()));
        // The name of a host in a form some resolvers take (a trailing dot, upper case) is kept as it is.
        assert_eq!(Url::parse("https://H.Example./x").unwrap().host, "h.example.");
        // Brackets hold an IPv6 address and nothing else.
        assert_eq!(Url::parse("http://[0:0:0:0:0:0:0:1]/").unwrap().host, "0:0:0:0:0:0:0:1");
        assert!(Url::parse("http://[0:0:0:0:0:0:0:1]/").unwrap().is_loopback());
    }

    #[test]
    fn only_this_computer_is_loopback() {
        for yes in ["http://localhost/", "http://127.0.0.1:8080/", "http://127.9.9.9/", "http://[::1]/", "https://LOCALHOST/"] {
            assert!(Url::parse(yes).unwrap().is_loopback(), "{yes}");
        }
        for no in [
            "http://127.0.0.1.evil.example/",
            "http://localhost.evil.example/",
            "http://10.0.0.1/",
            "http://128.0.0.1/",
            "http://[::2]/",
            "http://127.1/",
            "http://example.org/",
        ] {
            assert!(!Url::parse(no).unwrap().is_loopback(), "{no}");
        }
    }

    #[test]
    fn what_is_shown_of_an_address_is_its_server() {
        let u = Url::parse("https://jmap.example.org/jmap/session?account=ada%40example.org").unwrap();
        let shown = format!("{u:?}");
        assert!(shown.contains("https://jmap.example.org") && !shown.contains("ada") && !shown.contains("session"), "{shown}");
    }

    #[test]
    fn escapes_are_undone() {
        assert_eq!(pct_decode("ada%40example.org"), "ada@example.org");
        assert_eq!(pct_decode("a%20b%2Fc%C3%A9"), "a b/cé");
        assert_eq!(pct_decode("100%"), "100%");
        assert_eq!(pct_decode("%4"), "%4");
        assert_eq!(pct_decode("%zz%+1%-1"), "%zz%+1%-1", "only two hexadecimal digits make an escape");
        assert_eq!(pct_decode("%ff%fe"), "\u{fffd}\u{fffd}", "what is no UTF-8 is replaced");
        assert_eq!(pct_decode(&pct("any text é\n%41")), "any text é\n%41");
    }

    /// Strings that are nearly addresses, so that parsing is tried on more than text that never starts like one.
    fn address_like() -> impl Strategy<Value = String> {
        let scheme = prop_oneof![Just("https://"), Just("http://"), Just("HTTPS://"), Just("ftp://"), Just("")];
        let host = prop_oneof![
            "[a-zA-Z0-9.-]{0,20}",
            Just("localhost".to_string()),
            Just("[::1]".to_string()),
            Just("[fe80::1]".to_string()),
            "[ -~]{0,20}",
            ".{0,10}",
        ];
        let port = prop_oneof![Just(String::new()), "[0-9]{1,6}".prop_map(|p| format!(":{p}")), ".{0,5}".prop_map(|p| format!(":{p}"))];
        let tail = prop_oneof![Just(String::new()), "/[ -~]{0,30}", ".{0,30}", "[?#/][ -~\u{80}-\u{3000}]{0,20}"];
        (scheme, host, port, tail).prop_map(|(scheme, host, port, tail)| format!("{scheme}{host}{port}{tail}"))
    }

    proptest! {
        #[test]
        fn any_address_parses_or_is_refused(s in prop_oneof![address_like(), ".{0,300}"], rel in prop_oneof![address_like(), ".{0,100}"]) {
            if let Ok(u) = Url::parse(&s) {
                prop_assert!(!u.host.is_empty() && u.port != 0 && u.path.starts_with('/'));
                prop_assert!(u.full().bytes().all(|b| b.is_ascii_graphic()));
                // What was parsed parses again to the same address.
                prop_assert_eq!(Url::parse(&u.full()).ok(), Some(u.clone()));
                let _ = u.is_loopback();
                if let Ok(joined) = u.join(&rel) {
                    prop_assert!(joined.full().bytes().all(|b| b.is_ascii_graphic()));
                    prop_assert_eq!(Url::parse(&joined.full()).ok(), Some(joined));
                }
            }
            let _ = pct(&s);
            prop_assert_eq!(pct_decode(&pct(&s)), s);
        }
    }

    #[test]
    fn the_generator_reaches_addresses_that_parse() {
        use proptest::strategy::ValueTree;
        let mut runner = proptest::test_runner::TestRunner::default();
        let strategy = address_like();
        let parsed = (0..500).filter(|_| Url::parse(&strategy.new_tree(&mut runner).unwrap().current()).is_ok()).count();
        assert!(parsed > 50, "only {parsed} of 500 parsed: the property above would test too little");
    }
}
