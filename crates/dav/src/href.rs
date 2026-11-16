// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Addresses a server names, resolved against the collection they were found in, and held to the
//! account's DAV origin (scheme, host and port) before anything with credentials is built for them.
//!
//! A server is untrusted: an `href` that names another host would send the owner's password
//! there, or let a create land outside the collection. Every address this crate hands to a
//! request builder goes through [`Origin::resolve`]; nothing else makes one.

use crate::http::{EndpointError, MAX_ADDRESS};
use std::fmt;

/// Where an account's DAV server is: scheme, host and port. Two addresses are on the same origin
/// only when all three are equal (the host compared without regard to case, the default port of
/// the scheme written or not).
#[derive(Clone, PartialEq, Eq)]
pub struct Origin {
    secure: bool,
    host: String,
    port: u16,
}

/// Why an `href` was not followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HrefError {
    /// Empty, longer than 2,048 characters, with a control character, a backslash, a fragment, a
    /// bad percent escape or no sense as an address.
    Malformed,
    /// On another scheme, host or port than the account's DAV server.
    OffOrigin,
    /// `..` segments climb out of the root.
    Escapes,
    /// Not inside the collection it should be in.
    OffCollection,
}

impl fmt::Display for HrefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the server named an address that is not one",
            Self::OffOrigin => "the server named an address on another server",
            Self::Escapes => "the server named an address outside its own tree",
            Self::OffCollection => "the server named an address outside the collection",
        })
    }
}
impl std::error::Error for HrefError {}

/// Checks the characters of an address or `href` as text, before anything is read from it.
fn plain(text: &str) -> bool {
    !text.is_empty() && text.len() <= MAX_ADDRESS && !text.chars().any(|c| c.is_control() || c == '\\' || c == ' ')
}

impl Origin {
    /// The origin of an absolute `http` or `https` address (an address of at most 2,048
    /// characters, ASCII, no user info). What follows the authority is not looked at.
    pub fn parse(url: &str) -> Result<Self, EndpointError> {
        if url.len() > MAX_ADDRESS || url.chars().any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii() || c == '\\') {
            return Err(EndpointError::Malformed);
        }
        let (secure, rest) = match url.split_once("://") {
            Some((s, rest)) if s.eq_ignore_ascii_case("https") => (true, rest),
            Some((s, rest)) if s.eq_ignore_ascii_case("http") => (false, rest),
            _ => return Err(EndpointError::Malformed),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        Self::from_authority(secure, authority)
    }

    fn from_authority(secure: bool, authority: &str) -> Result<Self, EndpointError> {
        if authority.is_empty() {
            return Err(EndpointError::Malformed);
        }
        if authority.contains('@') {
            return Err(EndpointError::Credentials);
        }
        let default = if secure { 443 } else { 80 };
        let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
            let (host, after) = inside.split_once(']').ok_or(EndpointError::Malformed)?;
            let port = match after {
                "" => default,
                _ => after.strip_prefix(':').and_then(parse_port).ok_or(EndpointError::Malformed)?,
            };
            (format!("[{}]", host.to_ascii_lowercase()), port)
        } else {
            match authority.split_once(':') {
                None => (authority.to_ascii_lowercase(), default),
                Some((host, port)) => (host.to_ascii_lowercase(), parse_port(port).ok_or(EndpointError::Malformed)?),
            }
        };
        // A name with its root dot is the same name: one form for every check that follows.
        let host = match host.strip_suffix('.') {
            Some(bare) if !host.starts_with('[') => bare.to_owned(),
            _ => host,
        };
        let name_ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']' | ':');
        if host.is_empty() || host.starts_with('.') || host.ends_with('.') || host.contains("..") || !host.chars().all(name_ok) {
            return Err(EndpointError::Malformed);
        }
        Ok(Self { secure, host, port })
    }

    /// `https`.
    pub fn secure(&self) -> bool {
        self.secure
    }
    /// The host, lower-cased (an IPv6 address in brackets).
    pub fn host(&self) -> &str {
        &self.host
    }
    /// The port, explicit or the scheme's.
    pub fn port(&self) -> u16 {
        self.port
    }
    /// `localhost` and the names under it (RFC 6761 §6.3), `127.0.0.0/8` or `[::1]`.
    pub fn is_loopback(&self) -> bool {
        let host = self.host.as_str();
        host == "localhost" || host.ends_with(".localhost") || host == "[::1]" || {
            let mut parts = host.split('.');
            parts.next() == Some("127")
                && parts.clone().count() == 3
                && parts.all(|p| !p.is_empty() && p.len() <= 3 && p.bytes().all(|b| b.is_ascii_digit()))
        }
    }

    /// `scheme://host[:port]`, the port left out when it is the scheme's.
    pub fn base(&self) -> String {
        let scheme = if self.secure { "https" } else { "http" };
        let default = if self.secure { 443 } else { 80 };
        if self.port == default { format!("{scheme}://{}", self.host) } else { format!("{scheme}://{}:{}", self.host, self.port) }
    }

    /// Resolves `href` (absolute, network-path, absolute-path or relative) against `base` (an
    /// address on this origin, normally the collection's), normalises it and requires it to be on
    /// this origin. The result is an absolute address of at most 2,048 characters, with dot
    /// segments removed, unreserved characters unescaped and the rest escaped upper-case.
    pub fn resolve(&self, base: &str, href: &str) -> Result<String, HrefError> {
        let href = href.trim_matches([' ', '\t', '\r', '\n']);
        if !plain(href) || href.contains('#') {
            return Err(HrefError::Malformed);
        }
        // A reference with a scheme but no `//` (`mailto:x`, `https:host/x`) is not a path.
        let first = href.split(['/', '?']).next().unwrap_or("");
        if first.contains(':') && !href.contains("://") {
            return Err(HrefError::Malformed);
        }
        let base_path = self.path_of(base)?;
        let tail = if let Some((scheme, rest)) = href.split_once("://") {
            let secure = match scheme.to_ascii_lowercase().as_str() {
                "https" => true,
                "http" => false,
                _ => return Err(HrefError::Malformed),
            };
            self.tail_of(secure, rest)?
        } else if let Some(rest) = href.strip_prefix("//") {
            self.tail_of(self.secure, rest)?
        } else if href.starts_with('/') {
            href.to_owned()
        } else if href.starts_with('?') {
            format!("{}{href}", base_path.split('?').next().unwrap_or("/"))
        } else {
            let dir = base_path.split('?').next().unwrap_or("/");
            let dir = dir.rsplit_once('/').map_or("", |(d, _)| d);
            format!("{dir}/{href}")
        };
        let (path, query) = match tail.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (tail.as_str(), None),
        };
        let mut out = self.base();
        out.push_str(&normal_path(path)?);
        if let Some(query) = query {
            out.push('?');
            out.push_str(&escape(query, true)?);
        }
        if out.len() > MAX_ADDRESS { Err(HrefError::Malformed) } else { Ok(out) }
    }

    /// An absolute address on **another** origin, normalised, for data only (discovery decides
    /// whether a host may be used at all). `https`, or loopback `http` only when this origin is loopback too; never a credentialed
    /// request from this crate. None for anything else, including addresses on this origin.
    pub fn foreign(&self, href: &str) -> Option<String> {
        let href = href.trim_matches([' ', '\t', '\r', '\n']);
        if !plain(href) || href.contains('#') || !href.contains("://") {
            return None;
        }
        let other = Origin::parse(href).ok()?;
        if &other == self || !(other.secure() || (other.is_loopback() && self.is_loopback())) {
            return None;
        }
        other.resolve(&other.base(), href).ok()
    }

    /// The part of an address after the authority (path and query), the address having to be on
    /// this origin.
    fn path_of(&self, url: &str) -> Result<String, HrefError> {
        let origin = Origin::parse(url).map_err(|_| HrefError::Malformed)?;
        if &origin != self {
            return Err(HrefError::OffOrigin);
        }
        let rest = url.split_once("://").map_or("", |(_, r)| r);
        Ok(match rest.find(['/', '?']) {
            Some(i) => {
                let tail = rest.get(i..).unwrap_or("/");
                if tail.starts_with('/') { tail.to_owned() } else { format!("/{tail}") }
            }
            None => "/".to_owned(),
        })
    }

    fn tail_of(&self, secure: bool, rest: &str) -> Result<String, HrefError> {
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, tail) = (rest.get(..end).unwrap_or(""), rest.get(end..).unwrap_or(""));
        let origin = Origin::from_authority(secure, authority).map_err(|_| HrefError::Malformed)?;
        if &origin != self {
            return Err(HrefError::OffOrigin);
        }
        Ok(if tail.is_empty() || tail.starts_with('?') { format!("/{tail}") } else { tail.to_owned() })
    }

    /// The normalised path of `url`, which must be on this origin (no query).
    pub fn path(&self, url: &str) -> Result<String, HrefError> {
        let full = self.resolve(&self.base(), url)?;
        let path = self.path_of(&full)?;
        Ok(path.split('?').next().unwrap_or("/").to_owned())
    }

    /// Whether `url` lies inside the collection `collection` (both on this origin, `url` below
    /// the collection's path, never the collection itself).
    pub fn is_within(&self, collection: &str, url: &str) -> bool {
        let (Ok(c), Ok(u)) = (self.path(collection), self.path(url)) else { return false };
        let c = if c.ends_with('/') { c } else { format!("{c}/") };
        let Some(rest) = u.strip_prefix(c.as_str()) else { return false };
        // A member has a name; an empty segment (`/cal//`) is the collection to a server that
        // merges slashes, so it is refused (a trailing slash after a name is allowed).
        let mut parts = rest.split('/').peekable();
        let mut named = false;
        while let Some(part) = parts.next() {
            if part.is_empty() {
                if parts.peek().is_some() {
                    return false;
                }
            } else {
                named = true;
            }
        }
        named
    }
}

impl fmt::Debug for Origin {
    /// Nothing: the server's address is a setting, and is not logged.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Origin(..)")
    }
}

fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|p| *p != 0)
}

fn hex(v: u8) -> char {
    char::from(b"0123456789ABCDEF".get(usize::from(v & 15)).copied().unwrap_or(b'0'))
}

fn unhex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Escapes one path segment (or a query when `query`): an escape of an unreserved character is
/// undone, any other escape is kept upper-case, and what may not stand bare is escaped.
fn escape(text: &str, query: bool) -> Result<String, HrefError> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hi = bytes.get(i + 1).copied().and_then(unhex);
            let lo = bytes.get(i + 2).copied().and_then(unhex);
            let (Some(hi), Some(lo)) = (hi, lo) else { return Err(HrefError::Malformed) };
            let v = hi * 16 + lo;
            // An encoded slash, backslash or NUL is read as a separator by some servers: refused
            // in a path, so that what is "within" a collection is within it for them too.
            if !query && matches!(v, b'/' | b'\\' | 0) {
                return Err(HrefError::Malformed);
            }
            if v.is_ascii_alphanumeric() || matches!(v, b'-' | b'.' | b'_' | b'~') {
                out.push(char::from(v));
            } else {
                out.push('%');
                out.push(hex(v >> 4));
                out.push(hex(v));
            }
            i += 3;
            continue;
        }
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' | b':' | b'@')
            || (query && matches!(b, b'/' | b'?'));
        if keep {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(hex(b >> 4));
            out.push(hex(b));
        }
        i += 1;
    }
    Ok(out)
}

/// RFC 3986 §5.2.4 `remove_dot_segments` on the path, empty segments kept (`/a//b` stays). `..`
/// above the root is an error, never clamped.
fn normal_path(path: &str) -> Result<String, HrefError> {
    let mut kept: Vec<String> = Vec::new();
    let rest = path.strip_prefix('/').unwrap_or(path);
    let segments: Vec<&str> = rest.split('/').collect();
    let last = segments.len().saturating_sub(1);
    for (i, raw) in segments.iter().enumerate() {
        let segment = escape(raw, false)?;
        // A path parameter does not hide a dot segment (`..;x` is `..` to some servers).
        let base = segment.split(';').next().unwrap_or("");
        if segment.contains(';') && (base == "." || base == "..") {
            return Err(HrefError::Escapes);
        }
        match segment.as_str() {
            "." => {
                if i == last {
                    kept.push(String::new());
                }
            }
            ".." => {
                if kept.pop().is_none() {
                    return Err(HrefError::Escapes);
                }
                if i == last {
                    kept.push(String::new());
                }
            }
            _ => kept.push(segment),
        }
    }
    Ok(format!("/{}", kept.join("/")))
}

/// The origin of an account's DAV server **as the owner set it** (the settings, or an address the
/// owner typed). Only this can be paired with a credential ([`crate::request::Endpoint::new`]); an
/// [`Origin`] taken from anything a server said (a redirect, an `href`, an SRV answer) can only
/// make an anonymous endpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct TrustedOrigin(Origin);

impl TrustedOrigin {
    /// From the account's own setting: `https`, or loopback `http` for tests. Never call this with
    /// an address a server supplied.
    pub fn from_account_settings(url: &str) -> Result<Self, EndpointError> {
        let origin = Origin::parse(url)?;
        if origin.secure() || origin.is_loopback() { Ok(Self(origin)) } else { Err(EndpointError::NotEncrypted) }
    }
    /// The origin.
    pub fn origin(&self) -> &Origin {
        &self.0
    }
}

impl fmt::Debug for TrustedOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TrustedOrigin(..)")
    }
}

#[cfg(test)]
#[path = "href_tests.rs"]
mod tests;
