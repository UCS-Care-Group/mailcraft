// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's own handling of links: what a web address is, where it really goes, and how it
//! is written once it has been checked.
//!
//! [`WebLink::parse`] reads an `http` or `https` address from RFC 3986's shape and is stricter
//! than a browser: whatever a browser would read in more than one way is refused (a backslash,
//! a control character, a host with a space, a percent sign or a full stop from another
//! script in it, no `//`). What it accepts it writes the way a browser would, and that string,
//! [`WebLink::as_str`], is what the link tip shows and what is handed to the system's opener,
//! so that the host shown is the host reached.
//!
//! The host is lower-cased ASCII. An international label is written in Punycode (RFC 3492) as
//! `xn--…`, so that a letter from another script that looks like ours shows. There is no
//! UTS #46 mapping or Unicode normalisation (no tables, no dependency): characters it would map
//! (compatibility forms such as `ﬁ` or a full-width `Ａ`, or a letter written as base plus
//! combining mark) are written as they are, so they give a different `xn--` host from the
//! composed or plain form, and the label shown is the label launched. A zero-width non-joiner
//! or joiner is accepted only where IDNA2008 allows one (RFC 5892 appendix A.1, A.2: after a
//! virama, or between joining Arabic-script letters). An IPv4 address in any of
//! the forms a browser reads (`2130706433`, `0x7f.1`) is written as dotted decimal.
//!
//! Everything is bounded (an address of at most [`MAX_LINK`] bytes; a host of at most 253) and
//! nothing panics.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod encode;
mod host;
pub mod punycode;
#[cfg(test)]
mod tests;

use encode::Part;
pub use encode::encode_component;

/// The longest address read; a longer one is no link.
pub const MAX_LINK: usize = 64 * 1024;

/// A web address that has been read and written out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebLink {
    text: String,
    host: String,
    https: bool,
}

impl WebLink {
    /// Read `input` as an `http` or `https` address with a host. `None` when it is not one.
    pub fn parse(input: &str) -> Option<WebLink> {
        if input.len() > MAX_LINK {
            return None;
        }
        let input = input.trim_matches(' ');
        if input.chars().any(|c| c.is_control() || c == '\\') {
            return None;
        }
        let (scheme, rest) = input.split_once(':')?;
        let https = if scheme.eq_ignore_ascii_case("https") {
            true
        } else if scheme.eq_ignore_ascii_case("http") {
            false
        } else {
            return None;
        };
        let rest = rest.strip_prefix("//")?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, tail) = rest.split_at_checked(end)?;
        let (userinfo, hostport) = match authority.rfind('@') {
            Some(at) => (Some(authority.get(..at)?), authority.get(at + 1..)?),
            None => (None, authority),
        };
        let (host, port) = split_host(hostport)?;
        let port = match port {
            Some(p) if p == if https { 443 } else { 80 } => None,
            other => other,
        };

        let mut text = String::with_capacity(input.len() + 16);
        text.push_str(if https { "https://" } else { "http://" });
        if let Some(userinfo) = userinfo {
            encode::push(&mut text, userinfo, Part::UserInfo);
            text.push('@');
        }
        text.push_str(&host);
        if let Some(port) = port {
            text.push(':');
            text.push_str(&port.to_string());
        }
        push_tail(&mut text, tail);
        Some(WebLink { text, host, https })
    }

    /// The address as it is shown and launched.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The address as it is shown and launched, owned.
    pub fn into_string(self) -> String {
        self.text
    }

    /// The host it goes to: lower-case ASCII, an IPv6 address in brackets. Never empty.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether the address is `https`.
    pub fn is_https(&self) -> bool {
        self.https
    }
}

/// The host and the port of `name[:port]` or `[v6][:port]`.
fn split_host(hostport: &str) -> Option<(String, Option<u16>)> {
    let (host, port) = match hostport.strip_prefix('[') {
        Some(v6) => {
            let (inner, after) = v6.split_once(']')?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':')?),
            };
            (host::ipv6(inner)?, port)
        }
        None => match hostport.split_once(':') {
            Some((name, port)) => (host::domain(name)?, Some(port)),
            None => (host::domain(hostport)?, None),
        },
    };
    let port = match port {
        None | Some("") => None,
        Some(digits) => {
            // Leading zeros are allowed, however many; the value must fit.
            let mut value: u32 = 0;
            for b in digits.bytes() {
                if !b.is_ascii_digit() {
                    return None;
                }
                value = value * 10 + u32::from(b - b'0');
                if value > u32::from(u16::MAX) {
                    return None;
                }
            }
            Some(u16::try_from(value).ok()?)
        }
    };
    Some((host, port))
}

/// The path, query and fragment of an address (`tail` starts at the first `/`, `?` or `#`, or is
/// empty), written out with dot segments resolved.
fn push_tail(out: &mut String, tail: &str) {
    let (before, fragment) = match tail.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (tail, None),
    };
    let (path, query) = match before.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (before, None),
    };
    push_path(out, path);
    if let Some(query) = query {
        out.push('?');
        encode::push(out, query, Part::Query);
    }
    if let Some(fragment) = fragment {
        out.push('#');
        encode::push(out, fragment, Part::Fragment);
    }
}

/// `.` or `..` written as a segment, also with its dots as `%2e`.
fn dots(segment: &str) -> Option<usize> {
    let lower = segment.to_ascii_lowercase().replace("%2e", ".");
    match lower.as_str() {
        "." => Some(1),
        ".." => Some(2),
        _ => None,
    }
}

fn push_path(out: &mut String, path: &str) {
    let parts: Vec<&str> = path.strip_prefix('/').unwrap_or(path).split('/').collect();
    let mut segments: Vec<String> = Vec::with_capacity(parts.len());
    for (i, segment) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        match dots(segment) {
            Some(up) => {
                if up == 2 {
                    segments.pop();
                }
                if last {
                    segments.push(String::new());
                }
            }
            None => {
                let mut s = String::with_capacity(segment.len());
                encode::push(&mut s, segment, Part::Path);
                segments.push(s);
            }
        }
    }
    out.push('/');
    out.push_str(&segments.join("/"));
}

/// The rest of a `mailto:` or `tel:` address written so that it holds no space, quote or other
/// character a launcher could read as more than one argument.
pub fn encode_opaque(rest: &str) -> String {
    let mut out = String::with_capacity(rest.len());
    encode::push(&mut out, rest, Part::Opaque);
    out
}
