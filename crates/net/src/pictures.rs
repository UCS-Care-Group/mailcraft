// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures from the web, for a message whose owner asked to see its pictures.
//!
//! A message names these addresses, so whoever wrote the message chose them. Asking for one must
//! not become a way for a stranger to reach what only the owner can reach, nor tell the stranger
//! more than "somebody asked". [`fetch`] therefore
//!
//! - speaks only to `http://` and `https://` addresses, and refuses one with a user name in it;
//! - sends nothing that says who is asking: no cookie (none is ever kept), no credentials, no
//!   referrer, and the same plain user agent as everything else in this crate. Every request
//!   reads the same but for the address: what kinds of picture are taken, and that they are to
//!   come as they are, not packed;
//! - looks the host's name up itself and refuses the lot when any of its addresses is this
//!   machine, a private or link-local network, or anything else that is not the public internet
//!   ([`is_public`]); it then connects to the very addresses it checked, so a name can't answer
//!   one thing when it is checked and another when it is used;
//! - follows at most [`MAX_REDIRECTS`] redirects, each new address checked in the same way;
//! - takes at most [`MAX_BYTES`] for a picture, the answers of the redirects on the way counted
//!   in and whatever it unpacks to as well, and gives the whole of it [`PATIENCE`]. A server
//!   that says "not now" (429, 503) is asked once more after a moment; nothing else is tried
//!   twice.
//!
//! Unlike everything else in this crate, a picture may be asked for over plain `http://` from
//! anywhere: nothing secret travels with the request. That is why the connections are made here
//! and not by [`crate::transport`], which goes on refusing unencrypted connections to anything
//! but this machine. The answer is read by [`crate::http`], within its limits.
//!
//! The web is not as tidy as its standards, so a few things are taken as they come: an address
//! with a space or an accented letter in it is written the way a browser writes it (`%20`); a
//! host with several addresses is tried at one after the other with a moment's head start each,
//! so that an address this machine can't get to (IPv6 on a network without it) costs that
//! moment and not the picture; and an answer that comes packed (`Content-Encoding: gzip` or
//! `deflate`) although nobody asked for that is unpacked, within the same limit.
//!
//! What comes back is as hostile as the message was. Whether it is a picture at all is for
//! whoever decodes it to find out.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use mailcraft_proto::ProtoError;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use crate::NetError;
use crate::http::{self, Url};

/// Largest picture taken, in bytes; what the redirects on the way to it answered counts too.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Most redirects followed on the way to one picture.
pub const MAX_REDIRECTS: usize = 5;
/// How long one picture may take, from looking its host up to its last byte, redirects included.
pub const PATIENCE: Duration = Duration::from_secs(30);
/// How long connecting to one address may take.
const CONNECT: Duration = Duration::from_secs(8);
/// How long an address is given to answer before the next one of the host is tried beside it.
const HEAD_START: Duration = Duration::from_millis(300);
/// How long one read or write may take.
const IO: Duration = Duration::from_secs(10);
/// Most addresses of one host that are tried, and most that a lookup may give.
const MAX_ADDRESSES: usize = 4;
const MAX_LOOKED_UP: usize = 16;
/// What every request says of its sender: nothing about this machine or its owner. In the
/// browsers' form, because web firewalls (ModSecurity's rules among them) refuse a request whose
/// sender doesn't read like one, most often with 406 (#46).
const USER_AGENT: &str = "Mozilla/5.0 (compatible; MailCraft)";
/// The kinds of picture asked for. Servers that keep a picture in several kinds send a newer
/// one (WebP, AVIF) only to those who name it, so naming none of them gets the plain kinds.
/// Anything else is taken last rather than refused with 406 (#46): what comes is checked to be
/// a picture by its own bytes whatever it says it is.
const ACCEPT: &str = "image/png, image/jpeg, image/gif, image/*;q=0.5, */*;q=0.1";
/// How the picture is to come: as it is, not packed.
const ACCEPT_ENCODING: &str = "identity";
/// How much of an answer's beginning is kept, to read where a redirect leads.
const HEAD_KEPT: usize = 20 * 1024;
/// Writes interrupted in a row before giving up.
const MAX_INTERRUPTS: u32 = 100;
/// How long a server that says "not now" is left alone before it is asked the one more time:
/// what it says itself (`Retry-After`), within these.
const RETRY_WAIT: Duration = Duration::from_secs(1);
const MAX_RETRY_WAIT: Duration = Duration::from_secs(3);
/// Most ways of packing an answer may be packed in, one inside the other.
const MAX_CODINGS: usize = 2;

/// Why a picture was not downloaded. What each says reads on after "couldn't be downloaded:".
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PictureError {
    /// Not an `http://` or `https://` address, not an address at all, one with a user name in
    /// it, or one that is too long.
    #[error("its address isn't one that pictures are downloaded from")]
    Address,
    /// The address is this computer's, or of a network that is not the public internet.
    #[error("its address is on this computer or a private network, where mail may not reach")]
    NotPublic,
    #[error("its address led on to too many others")]
    TooManyRedirects,
    #[error("{}", status_text(*.0))]
    Status(u16),
    /// More than [`MAX_BYTES`], as the server says or as it turned out.
    #[error("it is larger than {} MB", MAX_BYTES / (1024 * 1024))]
    TooLarge,
    /// The whole of [`PATIENCE`] went by, or the server said nothing for too long.
    #[error("the server took too long")]
    TimedOut,
    /// It came packed in a way that isn't unpacked here, or what was packed is damaged.
    #[error("the server sent it packed in a way that can't be read")]
    Packed,
    #[error(transparent)]
    Net(#[from] NetError),
}

/// What a status that is neither a picture nor a redirect comes to, in words.
fn status_text(status: u16) -> String {
    match status {
        401 | 403 => format!("the server won't hand it out ({status})"),
        404 | 410 => format!("it is no longer there ({status})"),
        429 => format!("the server is being asked too often just now ({status})"),
        500..=599 => format!("the server has trouble of its own ({status})"),
        _ => format!("the server answered {status} instead of a picture"),
    }
}

/// Download the picture at `url`, an address a message gave, as the module's notes describe.
///
/// It blocks, for [`PATIENCE`] at most, so a host calls it on a background thread. The bytes are
/// whatever the server sent, [`MAX_BYTES`] of them at most: hostile, and not necessarily a picture.
pub fn fetch(url: &str) -> Result<Vec<u8>, PictureError> {
    fetch_with(url, &Reach::web(), &Limits::default())
}

/// How much one picture may cost.
#[derive(Clone, Copy, Debug)]
struct Limits {
    max_bytes: usize,
    patience: Duration,
    connect: Duration,
    /// What one address of a host gets before the next is tried beside it.
    head_start: Duration,
    /// How long a server that says "not now" is left alone, when it doesn't say.
    retry_wait: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: MAX_BYTES, patience: PATIENCE, connect: CONNECT, head_start: HEAD_START, retry_wait: RETRY_WAIT }
    }
}

/// A way of connecting that stands in for the system's, for the tests.
#[cfg(test)]
type Dial = Arc<dyn Fn(SocketAddr, Duration) -> std::io::Result<TcpStream> + Send + Sync>;

/// Where names lead, and where connections may go.
///
/// There is one of these outside the tests, [`Reach::web`]: the system's names, and the public
/// internet only. The tests have no internet and must not use it, so they alone can name
/// addresses of this machine that stand in for it (`stand_ins` does not exist in any other
/// build, and nothing at run time, no setting and no variable, can add to it), and they alone
/// can say how an address answers (`dial`: one that never does, without a network that has one).
struct Reach<'a> {
    /// The addresses of a host by its name, found in the time given; none when it has none.
    lookup: &'a dyn Fn(&str, u16, Duration) -> Vec<SocketAddr>,
    #[cfg(test)]
    stand_ins: &'a [SocketAddr],
    #[cfg(test)]
    dial: Option<Dial>,
}

impl Reach<'static> {
    fn web() -> Reach<'static> {
        Reach {
            lookup: &system_lookup,
            #[cfg(test)]
            stand_ins: &[],
            #[cfg(test)]
            dial: None,
        }
    }
}

impl Reach<'_> {
    fn allows(&self, address: &SocketAddr) -> bool {
        #[cfg(test)]
        if self.stand_ins.contains(address) {
            return true;
        }
        is_public(address.ip())
    }
}

/// Whether an address is one of the public internet's: not this machine, not a private,
/// carrier, link-local or site-local network, not multicast or broadcast, and not one of the
/// ranges set aside for documentation, benchmarks, relays and protocols. An IPv6 address that
/// carries an IPv4 one (mapped, or through NAT64) is judged by the one it carries.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match carried_v4(ip) {
            Some(inner) => is_public_v4(inner),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let set_aside = a == 0 // "this network"; 0.0.0.0 is this machine on some systems
        || a == 10 // private
        || (a == 100 && (64..128).contains(&b)) // shared by a carrier's customers
        || a == 127 // this machine
        || (a == 169 && b == 254) // link-local, and where clouds keep their metadata
        || (a == 172 && (16..32).contains(&b)) // private
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // protocol assignments; documentation
        || (a == 192 && b == 88 && c == 99) // relays to IPv6
        || (a == 192 && b == 168) // private
        || (a == 198 && (18..20).contains(&b)) // benchmarks
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224; // multicast, reserved, broadcast
    !set_aside
}

/// The IPv4 address an IPv6 one stands for: `::ffff:a.b.c.d`, or `64:ff9b::a.b.c.d` (NAT64).
fn carried_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return Some(mapped);
    }
    let [a, b, c, d, e, f, g, h] = ip.segments();
    let [g1, g2] = g.to_be_bytes();
    let [h1, h2] = h.to_be_bytes();
    ([a, b, c, d, e, f] == [0x64, 0xff9b, 0, 0, 0, 0]).then(|| Ipv4Addr::new(g1, g2, h1, h2))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let [a, b, ..] = ip.segments();
    // What is handed out to the internet at large is 2000::/3 (so not ::1, fc00::/7, fe80::/10,
    // ff00::/8), less what is set aside inside it.
    let handed_out = a & 0xe000 == 0x2000;
    let set_aside = (a == 0x2001 && b < 0x0200) // Teredo, benchmarks and other protocol assignments
        || (a == 0x2001 && b == 0x0db8) // documentation
        || a == 0x2002 // 6to4: an IPv4 address in disguise
        || (a == 0x3fff && b < 0x1000); // documentation
    handed_out && !set_aside
}

/// Run `work` on a thread of its own and wait `patience` for it; `None` when it took longer
/// (the thread is left to finish by itself).
fn within<T: Send + 'static>(patience: Duration, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("picture lookup".into()).spawn(move || {
        let _ = tx.send(work());
    });
    spawned.ok()?;
    rx.recv_timeout(patience).ok()
}

/// The system's answer to "where is `host`". The system takes its own time over a name whose
/// servers don't answer, so it is asked on a thread that can be left behind.
fn system_lookup(host: &str, port: u16, patience: Duration) -> Vec<SocketAddr> {
    let host = host.to_string();
    let look = move || (host.as_str(), port).to_socket_addrs().map(|found| found.take(MAX_LOOKED_UP).collect::<Vec<_>>()).unwrap_or_default();
    within(patience, look).unwrap_or_default()
}

fn problem(text: &str) -> PictureError {
    PictureError::Net(NetError::Proto(ProtoError::Io(text.into())))
}

fn unreachable_host(host: &str, reason: &str) -> PictureError {
    PictureError::Net(NetError::Connect { host: host.to_string(), reason: reason.to_string() })
}

/// What an error of reading an answer comes to here.
fn reading(error: NetError) -> PictureError {
    if http::is_too_long(&error) {
        PictureError::TooLarge
    } else if http::is_too_slow(&error) {
        PictureError::TimedOut
    } else {
        PictureError::Net(error)
    }
}

/// What follows `scheme://` the way it is written on the wire. A message writes its addresses
/// as it likes: a space, a quote or a letter that isn't ASCII in the path or the query becomes
/// `%XX` (of its UTF-8), as a browser has it. The host is left as it is, and so is everything
/// that can't be part of an address at all (a control character): what reads the result
/// refuses those.
fn escaped(rest: &str) -> String {
    let at = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at_checked(at).unwrap_or((rest, ""));
    let mut out = String::with_capacity(rest.len().saturating_add(16));
    out.push_str(authority);
    for c in tail.chars() {
        if c == ' ' || matches!(c, '"' | '<' | '>' | '`') || (!c.is_ascii() && !c.is_control()) {
            let mut utf8 = [0u8; 4];
            for b in c.encode_utf8(&mut utf8).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Read `text` as the address of a picture: `http://` or `https://`, a host, a port perhaps, a
/// path. What follows a `#` is dropped (it is never sent).
fn address(text: &str) -> Result<Url, PictureError> {
    let text = text.trim().split('#').next().unwrap_or("");
    // (Longer than this it is no address, whatever is in it; and nothing longer is rewritten.)
    if text.len() > http::MAX_LONG_URL {
        return Err(PictureError::Address);
    }
    let (scheme, rest) = text.split_once("://").ok_or(PictureError::Address)?;
    let secure = match scheme {
        s if s.eq_ignore_ascii_case("https") => true,
        s if s.eq_ignore_ascii_case("http") => false,
        _ => return Err(PictureError::Address),
    };
    // `Url` reads an `https://` address to anywhere, and is as strict with it as is wanted of
    // both kinds: no user name, no white space or control characters, a real port.
    let mut url = Url::parse_within(&format!("https://{}", escaped(rest)), http::MAX_LONG_URL).map_err(|_| PictureError::Address)?;
    url.secure = secure;
    url.path = without_dots(&url.path);
    Ok(url)
}

/// A path (and its query) with its `.` and `..` steps taken, the way a browser takes them
/// before it asks: `/a/b/../c.png` is `/a/c.png`, and no step climbs above the top.
fn without_dots(path: &str) -> String {
    let (steps, query) = match path.find('?') {
        Some(at) => path.split_at_checked(at).unwrap_or((path, "")),
        None => (path, ""),
    };
    if !steps.split('/').any(|step| step == "." || step == "..") {
        return path.to_string();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut folder = false;
    for step in steps.split('/').skip(1) {
        folder = matches!(step, "." | "..");
        match step {
            "." => {}
            ".." => drop(kept.pop()),
            step => kept.push(step),
        }
    }
    // A path that ends in a step up or a step in place ends at a folder.
    format!("/{}{}{query}", kept.join("/"), if folder && !kept.is_empty() { "/" } else { "" })
}

/// The host as a request names it: with its port when that isn't the usual one.
fn authority(url: &Url) -> String {
    let host = if url.host.contains(':') { format!("[{}]", url.host) } else { url.host.clone() };
    if url.port == 0 { host } else { format!("{host}:{}", url.port) }
}

/// The address a redirect leads to: a whole one, or one written from where `from` is.
fn follow(from: &Url, location: &str) -> Result<Url, PictureError> {
    let location = location.trim();
    if location.is_empty() || location.len() > http::MAX_LONG_URL {
        return Err(PictureError::Address);
    }
    let scheme = if from.secure { "https" } else { "http" };
    let names_scheme = |text: &str| !text.is_empty() && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if location.split_once("://").is_some_and(|(before, _)| names_scheme(before)) {
        return address(location);
    }
    if location.starts_with("//") {
        return address(&format!("{scheme}:{location}"));
    }
    if location.starts_with('/') {
        return address(&format!("{scheme}://{}{location}", authority(from)));
    }
    // Beside where `from` is. A first step with a colon in it would be a scheme, not a path.
    let first = location.split(['/', '?']).next().unwrap_or("");
    if first.contains(':') {
        return Err(PictureError::Address);
    }
    let path = from.path.split('?').next().unwrap_or("/");
    let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
    address(&format!("{scheme}://{}{folder}/{location}", authority(from)))
}

/// What the headers of an answer say that matters here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Said {
    /// Where a redirect says it leads (`Location`).
    location: Option<String>,
    /// How the body is packed (`Content-Encoding`), in lower case; all of them when it says so
    /// more than once, in the order they were applied.
    coding: Option<String>,
    /// How many seconds the server asks to be left alone for (`Retry-After`, when it is a number).
    retry_after: Option<u64>,
}

/// The headers of the answer that `head` begins with.
fn said(head: &[u8]) -> Said {
    let mut lines = head.split(|b| *b == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    // "Go on" answers may come before the one that counts; `http` puts up with as many.
    for _ in 0..5 {
        let Some(status) = lines
            .next()
            .and_then(|line| String::from_utf8_lossy(line).split(' ').filter(|word| !word.is_empty()).nth(1).and_then(|s| s.parse::<u16>().ok()))
        else {
            break;
        };
        let mut said = Said::default();
        for line in lines.by_ref() {
            if line.is_empty() {
                break;
            }
            let line = String::from_utf8_lossy(line);
            let Some((name, value)) = line.split_once(':') else { continue };
            let (name, value) = (name.trim(), value.trim());
            if name.eq_ignore_ascii_case("location") {
                // The first one counts.
                said.location.get_or_insert_with(|| value.to_string());
            } else if name.eq_ignore_ascii_case("content-encoding") {
                let more = value.to_ascii_lowercase();
                said.coding = Some(match said.coding.take() {
                    Some(earlier) => format!("{earlier},{more}"),
                    None => more,
                });
            } else if name.eq_ignore_ascii_case("retry-after") {
                said.retry_after = value.parse::<u64>().ok().or(said.retry_after);
            }
        }
        if !(100..200).contains(&status) {
            return said;
        }
    }
    Said::default()
}

/// A body as it was before it was packed: `coding` is what its `Content-Encoding` says. No more
/// than `max` bytes come out, however little went in.
fn unpack(body: Vec<u8>, coding: &str, max: usize) -> Result<Vec<u8>, PictureError> {
    // Applied in the order they are written, so undone from the last.
    let codings: Vec<&str> = coding.split(',').map(str::trim).filter(|c| !c.is_empty() && *c != "identity").collect();
    if codings.len() > MAX_CODINGS {
        return Err(PictureError::Packed);
    }
    let mut body = body;
    for coding in codings.into_iter().rev() {
        body = match coding {
            // Some servers say so of a body they send as it is.
            "gzip" | "x-gzip" if !body.starts_with(&[0x1f, 0x8b]) => body,
            "gzip" | "x-gzip" => inflate(flate2::read::MultiGzDecoder::new(body.as_slice()), max)?,
            // "deflate" is zlib's wrapping by the book, and bare deflate from servers that took
            // the name at its word.
            "deflate" => match inflate(flate2::read::ZlibDecoder::new(body.as_slice()), max) {
                Err(PictureError::Packed) => inflate(flate2::read::DeflateDecoder::new(body.as_slice()), max)?,
                unpacked => unpacked?,
            },
            _ => return Err(PictureError::Packed),
        };
    }
    Ok(body)
}

/// Everything `packed` unpacks to, when that is no more than `max` bytes.
fn inflate(packed: impl Read, max: usize) -> Result<Vec<u8>, PictureError> {
    let mut out = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    // A few bytes can unpack to a great many: no more than the limit is ever taken out.
    match packed.take(limit).read_to_end(&mut out) {
        Ok(_) if out.len() > max => Err(PictureError::TooLarge),
        Ok(_) => Ok(out),
        Err(_) => Err(PictureError::Packed),
    }
}

/// The addresses in the order they are tried: as they were found, but with the two kinds (IPv6
/// and IPv4) taking turns, so that the second one tried is of the other kind when there is one.
fn in_turns(found: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let first_is_v6 = found.first().is_some_and(SocketAddr::is_ipv6);
    let (mut same, mut other): (std::collections::VecDeque<_>, std::collections::VecDeque<_>) =
        found.into_iter().partition(|address| address.is_ipv6() == first_is_v6);
    let mut out = Vec::with_capacity(same.len() + other.len());
    while !same.is_empty() || !other.is_empty() {
        out.extend(same.pop_front());
        out.extend(other.pop_front());
    }
    out
}

/// The addresses of `host` to connect to, every one of them checked.
fn addresses(host: &str, port: u16, reach: &Reach<'_>, deadline: Instant) -> Result<Vec<SocketAddr>, PictureError> {
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    if name == "localhost" || name.ends_with(".localhost") {
        return Err(PictureError::NotPublic);
    }
    let found = match host.parse::<IpAddr>() {
        // An address needs no looking up, and no resolver gets to read it its own way.
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => (reach.lookup)(host, port, deadline.saturating_duration_since(Instant::now())),
    };
    if found.is_empty() {
        return Err(unreachable_host(host, "the name couldn't be looked up"));
    }
    // One address that isn't public refuses them all: a name that points both at the internet
    // and into the owner's network is up to no good.
    if !found.iter().all(|address| reach.allows(address)) {
        return Err(PictureError::NotPublic);
    }
    Ok(in_turns(found).into_iter().take(MAX_ADDRESSES).collect())
}

/// A connection to the first of `addresses` that takes one, and to nothing else.
///
/// They are tried in their order, each on a thread of its own with a head start on the next:
/// one that answers at once is the only one tried, one that refuses makes way at once, and one
/// that says nothing (an address of a kind this machine has no way to) costs the head start and
/// no more. A thread that is left behind ends with its own time, and closes what it opened.
fn connect(addresses: &[SocketAddr], host: &str, reach: &Reach<'_>, limits: &Limits, deadline: Instant) -> Result<TcpStream, PictureError> {
    #[cfg(not(test))]
    let _ = reach;
    let mut reason = "no address was found for the name";
    let (tx, rx) = mpsc::channel::<std::io::Result<TcpStream>>();
    let mut waiting = addresses.iter().copied();
    let (mut tried, mut failed) = (0usize, 0usize);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            reason = "the server didn't answer";
            break;
        }
        let mut more = waiting.len() > 0;
        if more && let Some(address) = waiting.next() {
            let (tx, patience) = (tx.clone(), left.min(limits.connect));
            #[cfg(test)]
            let stand_in = reach.dial.clone();
            let attempt = move || {
                // The system's way of connecting (or, in the tests, the way they say).
                #[cfg(test)]
                let made = match &stand_in {
                    Some(stand_in) => stand_in(address, patience),
                    None => TcpStream::connect_timeout(&address, patience),
                };
                #[cfg(not(test))]
                let made = TcpStream::connect_timeout(&address, patience);
                // Nobody may be listening any more: then what was opened is closed here.
                let _ = tx.send(made);
            };
            tried += 1;
            if std::thread::Builder::new().name("picture connection".into()).spawn(attempt).is_err() {
                failed += 1;
                reason = "the network couldn't get there";
            }
            more = waiting.len() > 0;
        }
        if failed >= tried && !more {
            break;
        }
        // While there is another address to try, this one has its head start; the last ones
        // have all the time that is left.
        let wait = if more { left.min(limits.head_start) } else { left };
        match rx.recv_timeout(wait) {
            Ok(Ok(tcp)) => {
                let _ = tcp.set_nodelay(true);
                return Ok(tcp);
            }
            Ok(Err(e)) => {
                failed += 1;
                reason = match e.kind() {
                    ErrorKind::ConnectionRefused => "the server refused the connection",
                    ErrorKind::TimedOut | ErrorKind::WouldBlock => "the server didn't answer",
                    _ => "the network couldn't get there",
                };
                if failed >= tried && !more {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) if more => {}
            Err(_) => {
                reason = "the server didn't answer";
                break;
            }
        }
    }
    Err(unreachable_host(host, reason))
}

/// A socket that gives up when the picture's time is up: no read or write waits past it.
struct Timed {
    tcp: TcpStream,
    deadline: Instant,
}

impl Timed {
    fn ready(&self) -> std::io::Result<()> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ErrorKind::TimedOut.into());
        }
        // No time at all would mean "wait for ever".
        let wait = Some(left.min(IO).max(Duration::from_millis(1)));
        self.tcp.set_read_timeout(wait)?;
        self.tcp.set_write_timeout(wait)
    }
}

impl Read for Timed {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.ready()?;
        self.tcp.read(buf)
    }
}

impl Write for Timed {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.ready()?;
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.ready()?;
        self.tcp.flush()
    }
}

/// A connection, encrypted or not.
enum Line {
    Plain(Timed),
    Tls(Box<StreamOwned<ClientConnection, Timed>>),
}

impl Read for Line {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Line::Plain(plain) => plain.read(buf),
            Line::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Line::Plain(plain) => plain.write(buf),
            Line::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Line::Plain(plain) => plain.flush(),
            Line::Tls(tls) => tls.flush(),
        }
    }
}

/// The TLS settings, the same check as for mail: the system's check of the certificate, which
/// must also match the server's name (`crate::tls`).
fn tls_config() -> Result<Arc<ClientConfig>, String> {
    crate::tls::client_config(crate::tls::Purpose::Picture)
}

/// What went wrong in a handshake, in a few plain words: the text of the library says all of
/// it, with times to the second and every name a certificate has.
fn handshake_trouble(error: &std::io::Error) -> String {
    use rustls::{CertificateError as Cert, Error};
    match error.kind() {
        ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => return "the server closed the connection".into(),
        _ => {}
    }
    let plain = match error.get_ref().and_then(|inner| inner.downcast_ref::<Error>()) {
        Some(Error::InvalidCertificate(Cert::Expired | Cert::ExpiredContext { .. })) => "its certificate has run out",
        Some(Error::InvalidCertificate(Cert::NotValidYet | Cert::NotValidYetContext { .. })) => "its certificate isn't good yet",
        Some(Error::InvalidCertificate(Cert::NotValidForName | Cert::NotValidForNameContext { .. })) => "its certificate is for another name",
        Some(Error::InvalidCertificate(Cert::UnknownIssuer)) => "its certificate doesn't lead back to an authority that is trusted",
        Some(Error::InvalidCertificate(Cert::Revoked)) => "its certificate was taken back",
        Some(Error::InvalidCertificate(_)) => "its certificate isn't good",
        Some(Error::AlertReceived(_) | Error::PeerIncompatible(_)) => "the server and this program have no way of encrypting in common",
        _ => return error.to_string().chars().filter(|c| !c.is_control()).take(200).collect(),
    };
    plain.into()
}

/// Shake hands over `line` and check the server's certificate against `host`.
fn secure(mut line: Timed, host: &str) -> Result<Line, PictureError> {
    let failed = |reason: String| PictureError::Net(NetError::Tls { host: host.to_string(), reason });
    let config = tls_config().map_err(failed)?;
    let name = ServerName::try_from(host.to_string()).map_err(|_| failed("its name isn't one a certificate can be checked against".into()))?;
    let mut connection = ClientConnection::new(config, name).map_err(|e| failed(e.to_string()))?;
    // Now, so that a bad certificate is reported as one. Every wait in it ends with the picture's time.
    while connection.is_handshaking() {
        connection.complete_io(&mut line).map_err(|e| match e.kind() {
            ErrorKind::TimedOut | ErrorKind::WouldBlock => PictureError::TimedOut,
            _ => failed(handshake_trouble(&e)),
        })?;
    }
    Ok(Line::Tls(Box::new(StreamOwned::new(connection, line))))
}

/// Not `write_all`: that retries an interrupted write for ever.
fn send(line: &mut Line, mut bytes: &[u8]) -> Result<(), PictureError> {
    let failed = |e: &std::io::Error| match e.kind() {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => PictureError::TimedOut,
        _ => problem("the server closed the connection"),
    };
    let mut interrupts = 0;
    while !bytes.is_empty() {
        match line.write(bytes) {
            Ok(0) => return Err(problem("the server closed the connection")),
            Ok(n) => bytes = bytes.get(n..).unwrap_or(&[]),
            Err(e) if e.kind() == ErrorKind::Interrupted && interrupts < MAX_INTERRUPTS => interrupts += 1,
            Err(e) => return Err(failed(&e)),
        }
    }
    line.flush().map_err(|e| failed(&e))
}

/// A stream that keeps the beginning of what is read from it.
struct Kept<'a, R> {
    inner: &'a mut R,
    head: Vec<u8>,
}

impl<R: Read> Read for Kept<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        let room = HEAD_KEPT.saturating_sub(self.head.len());
        self.head.extend_from_slice(buf.get(..n.min(room)).unwrap_or(&[]));
        Ok(n)
    }
}

/// What one address answered.
struct Answer {
    status: u16,
    /// As it came, packed or not.
    body: Vec<u8>,
    said: Said,
}

/// One GET, on a connection of its own, to an address that was checked first.
fn ask(url: &Url, reach: &Reach<'_>, limits: &Limits, max_bytes: usize, deadline: Instant) -> Result<Answer, PictureError> {
    let port = match (url.port, url.secure) {
        (0, true) => 443,
        (0, false) => 80,
        (port, _) => port,
    };
    let addresses = addresses(&url.host, port, reach, deadline)?;
    let tcp = connect(&addresses, &url.host, reach, limits, deadline)?;
    let line = Timed { tcp, deadline };
    let mut line = if url.secure { secure(line, &url.host)? } else { Line::Plain(line) };
    // The address was checked when it was read: nothing in it can end a line early. And this is
    // all that is ever said: what to send, to whom, how, and that nothing more will be asked.
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nAccept: {ACCEPT}\r\nAccept-Encoding: {ACCEPT_ENCODING}\r\nConnection: close\r\n\r\n",
        url.path,
        authority(url)
    );
    send(&mut line, request.as_bytes())?;
    let mut kept = Kept { inner: &mut line, head: Vec::new() };
    let started = Instant::now();
    let reply = http::read_reply(&mut kept, started, deadline.saturating_duration_since(started), max_bytes).map_err(reading)?;
    // A refusal is worth seeing in Copy Diagnostics; the address never is.
    if reply.status >= 400 {
        log::info!("a picture's server answered {} ({} bytes)", reply.status, reply.body.len());
    } else {
        log::debug!("a picture's server answered {} ({} bytes)", reply.status, reply.body.len());
    }
    Ok(Answer { status: reply.status, body: reply.body, said: said(&kept.head) })
}

/// [`fetch`], with the reach and the limits said.
fn fetch_with(url: &str, reach: &Reach<'_>, limits: &Limits) -> Result<Vec<u8>, PictureError> {
    let deadline = Instant::now() + limits.patience;
    let mut at = address(url)?;
    let mut left = limits.max_bytes;
    let (mut redirects, mut asked_again) = (0, false);
    // The address itself, then as many redirects as are followed, and one asking again: every
    // turn but the last of these is one or the other, so the turns are counted.
    loop {
        let answer = ask(&at, reach, limits, left, deadline)?;
        match answer.status {
            // What came packed (nobody asked for that) is unpacked, to no more than may be taken.
            200 => {
                return match &answer.said.coding {
                    Some(coding) => unpack(answer.body, coding, left),
                    None => Ok(answer.body),
                };
            }
            301 | 302 | 303 | 307 | 308 => {
                if redirects >= MAX_REDIRECTS {
                    return Err(PictureError::TooManyRedirects);
                }
                redirects += 1;
                let location = answer.said.location.ok_or(PictureError::Status(answer.status))?;
                at = follow(&at, &location)?;
                left = left.saturating_sub(answer.body.len());
            }
            // "Not now": once more, after as long as it says or a moment, if there is the time.
            status @ (429 | 503) if !asked_again => {
                asked_again = true;
                let wait = answer.said.retry_after.map_or(limits.retry_wait, Duration::from_secs).min(MAX_RETRY_WAIT);
                if deadline.saturating_duration_since(Instant::now()) <= wait {
                    return Err(PictureError::Status(status));
                }
                std::thread::sleep(wait);
                left = left.saturating_sub(answer.body.len());
            }
            status => return Err(PictureError::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use proptest::prelude::*;

    use super::*;
    use crate::fake::FakeHttp;

    /// Something a server could send as a picture (whether it is one is not this crate's business).
    fn picture() -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend((0..600u32).map(|i| (i % 251) as u8));
        bytes
    }

    fn answer(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn redirect(to: &str) -> Vec<u8> {
        answer("302 Found", &format!("Location: {to}\r\n"), b"moved")
    }

    /// The fake's own address, where the tests allow connections to go.
    fn at(server: &FakeHttp) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], server.port))
    }

    /// The path a request asks for.
    fn path_of(request: &str) -> &str {
        request.split(' ').nth(1).unwrap_or("")
    }

    /// Limits for tests: the same sizes, less waiting.
    fn quick() -> Limits {
        Limits { patience: Duration::from_secs(5), connect: Duration::from_secs(3), ..Limits::default() }
    }

    /// For addresses that are refused by how they are written: no name is ever looked up.
    fn no_names(host: &str, _: u16, _: Duration) -> Vec<SocketAddr> {
        panic!("{host} was looked up")
    }

    #[test]
    fn a_picture_is_fetched_and_the_request_says_nothing_of_its_sender() {
        let server = FakeHttp::answering(answer("200 OK", "Content-Type: image/png\r\nSet-Cookie: who=you; Path=/\r\n", &picture()));
        let allowed = [at(&server)];
        let names = |host: &str, _: u16, _: Duration| if host.trim_end_matches('.') == "pictures.example" { allowed.to_vec() } else { Vec::new() };
        let reach = Reach { lookup: &names, stand_ins: &allowed, dial: None };

        // By its address, and by a name that leads there; plain `http` to a name is fine for a picture.
        assert_eq!(fetch_with(&server.url("/img/hero.png?w=600&sig=abc#top"), &reach, &quick()), Ok(picture()));
        assert_eq!(fetch_with(" HTTP://pictures.example/logo.png ", &reach, &quick()), Ok(picture()));
        assert_eq!(fetch_with("http://pictures.example./logo.png", &reach, &quick()), Ok(picture()));

        let sent = server.request(0);
        assert!(sent.starts_with("GET /img/hero.png?w=600&sig=abc HTTP/1.1\r\n"), "{sent}");
        assert!(sent.ends_with("\r\n\r\n"), "a GET has no body: {sent}");
        // Exactly these headers, and so no cookie, no credentials, no referrer, nothing of this machine.
        let headers: Vec<(String, String)> =
            sent.lines().skip(1).filter_map(|l| l.split_once(": ")).map(|(name, value)| (name.to_ascii_lowercase(), value.to_string())).collect();
        assert_eq!(
            headers,
            [
                ("host", format!("127.0.0.1:{}", server.port).as_str()),
                ("user-agent", "Mozilla/5.0 (compatible; MailCraft)"),
                ("accept", "image/png, image/jpeg, image/gif, image/*;q=0.5, */*;q=0.1"),
                ("accept-encoding", "identity"),
                ("connection", "close"),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
        // Every request reads the same but for what it asks for, and of whom.
        let rest = |n: usize| server.request(n).lines().skip(2).map(str::to_string).collect::<Vec<_>>();
        assert!(rest(0) == rest(1) && rest(1) == rest(2), "{:?}", rest(1));
        // The cookie the first answer set is not sent with the next request, to the same server.
        for n in 0..3 {
            let lower = server.request(n).to_ascii_lowercase();
            for never in ["cookie", "authorization", "referer", "origin", "from:", "who=you"] {
                assert!(!lower.contains(never), "{never}: {lower}");
            }
        }
        assert!(server.request(1).contains("\r\nHost: pictures.example\r\n"));
        assert_eq!(server.asked(), 3);
    }

    #[test]
    fn redirects_are_followed_so_many_times_and_no_further() {
        let server = FakeHttp::start(|_, request| {
            let host = request.lines().find_map(|l| l.strip_prefix("Host: ")).unwrap_or("").to_string();
            match path_of(request) {
                // Written every way a server writes them, and of every kind there is.
                "/one" => answer("301 Moved Permanently", "Set-Cookie: seen=1\r\nlocation:  /two?x=1  \r\n", b""),
                "/two?x=1" => answer("307 Temporary Redirect", &format!("Location: http://{host}/dir/three\r\n"), b""),
                "/dir/three" => redirect("four.png#frag"),
                "/dir/four.png" => answer("200 OK", "", &picture()),
                "/same-host" => answer("303 See Other", &format!("Location: //{host}/dir/four.png\r\n"), b""),
                "/shop/permanent" => answer("308 Permanent Redirect", "LOCATION:../dir/./four.png?size=2x\r\n", b""),
                "/dir/four.png?size=2x" => answer("200 OK", "", &picture()),
                // Where it leads is written as loosely as a message writes an address.
                "/spaced" => answer("302 Found", "Location: /dir/autumn sale/hero é.png?name=a b\r\n", b""),
                "/dir/autumn%20sale/hero%20%C3%A9.png?name=a%20b" => answer("200 OK", "", &picture()),
                "/hop-0" => redirect("/hop-1"),
                "/hop-1" => redirect("/hop-2"),
                "/hop-2" => redirect("/one"),
                "/hop-too-many" => redirect("/hop-0"),
                "/loop" => redirect("/loop"),
                "/nowhere" => answer("302 Found", "", b""),
                "/empty" => answer("302 Found", "Location: \r\n", b""),
                "/not-modified" => answer("304 Not Modified", "Location: /dir/four.png\r\n", b""),
                "/gone" => answer("404 Not Found", "", b"no such picture"),
                _ => answer("500 Oops", "", b""),
            }
        });
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        let get = |path: &str| fetch_with(&server.url(path), &reach, &quick());

        assert_eq!(get("/one"), Ok(picture()));
        assert_eq!(server.asked(), 4);
        // What the first answer set as a cookie went nowhere.
        assert!((0..4).all(|n| !server.request(n).to_ascii_lowercase().contains("cookie")));
        assert_eq!(get("/same-host"), Ok(picture()));
        assert_eq!(get("/spaced"), Ok(picture()));
        // One written from where the first is, with steps up and in place: taken before it is asked for.
        assert_eq!(get("/shop/permanent"), Ok(picture()));
        // Five redirects are followed: these three and the two it takes from there.
        assert_eq!(MAX_REDIRECTS, 5);
        let before = server.asked();
        assert_eq!(get("/hop-1"), Ok(picture()));
        assert_eq!(server.asked() - before, 6);
        // A sixth is one too many, and a loop ends the same way: after six requests.
        let before = server.asked();
        assert_eq!(get("/hop-0"), Err(PictureError::TooManyRedirects));
        assert_eq!(server.asked() - before, 6);
        let before = server.asked();
        assert_eq!(get("/hop-too-many"), Err(PictureError::TooManyRedirects));
        assert_eq!(server.asked() - before, 6);
        let before = server.asked();
        assert_eq!(get("/loop"), Err(PictureError::TooManyRedirects));
        assert_eq!(server.asked() - before, 6);
        // A redirect that doesn't say where, and answers that are neither a picture nor a redirect.
        assert_eq!(get("/nowhere"), Err(PictureError::Status(302)));
        assert_eq!(get("/empty"), Err(PictureError::Address));
        assert_eq!(get("/not-modified"), Err(PictureError::Status(304)));
        assert_eq!(get("/gone"), Err(PictureError::Status(404)));
        assert_eq!(get("/anything-else"), Err(PictureError::Status(500)));
    }

    #[test]
    fn a_redirect_is_checked_like_the_address_it_came_from() {
        // A second server on this machine that the tests were not told about: as far as the
        // check goes it is the owner's own machine, and must never hear from us.
        let inside = FakeHttp::answering(answer("200 OK", "", b"the owner's router"));
        let inside_port = inside.port;
        let server = FakeHttp::start(move |_, request| {
            let me = request.lines().find_map(|l| l.strip_prefix("Host: ")).and_then(|h| h.rsplit(':').next()).unwrap_or("").to_string();
            let to = match path_of(request) {
                "/0" => format!("http://127.0.0.1:{inside_port}/admin"),
                // This server's own port, by names for this machine rather than its address.
                "/1" => format!("http://localhost:{me}/x.png"),
                "/2" => format!("http://[::1]:{me}/x.png"),
                "/3" => format!("http://[::ffff:127.0.0.1]:{me}/x.png"),
                "/4" => "http://10.0.0.1/x.png".into(),
                "/5" => "https://192.168.1.1/x.png".into(),
                "/6" => "http://169.254.169.254/latest/meta-data/".into(),
                "/7" => "http://[fd00::1]/x.png".into(),
                "/8" => "http://0.0.0.0/x.png".into(),
                "/9" => "https://intranet.example/x.png".into(),
                "/10" => "http://both.example/x.png".into(),
                "/11" => "ftp://pictures.example/x.png".into(),
                "/12" => "file:///etc/passwd".into(),
                "/13" => "javascript:alert(1)".into(),
                "/14" => "data:image/png;base64,AAAA".into(),
                "/15" => "//owner:secret@pictures.example/x.png".into(),
                "/16" => "http://pictures.example:0/x.png".into(),
                _ => "/missing".into(),
            };
            redirect(&to)
        });
        let allowed = [at(&server)];
        let looked_up = Mutex::new(Vec::new());
        let names = |host: &str, port: u16, _: Duration| {
            looked_up.lock().unwrap().push(host.to_string());
            match host {
                "intranet.example" => vec![SocketAddr::from(([10, 1, 2, 3], port))],
                // An address of the internet and one of this machine: refused whole, and neither is tried.
                "both.example" => vec![SocketAddr::from(([93, 184, 216, 34], port)), SocketAddr::from(([127, 0, 0, 1], inside_port))],
                _ => Vec::new(),
            }
        };
        let reach = Reach { lookup: &names, stand_ins: &allowed, dial: None };
        for n in 0..=16 {
            let wanted = if n <= 10 { PictureError::NotPublic } else { PictureError::Address };
            assert_eq!(fetch_with(&server.url(&format!("/{n}")), &reach, &quick()), Err(wanted), "/{n}");
        }
        // Each of them cost one request, to the server that sent the redirect, and nothing else.
        assert_eq!(server.asked(), 17);
        assert_eq!(inside.asked(), 0);
        assert_eq!(*looked_up.lock().unwrap(), ["intranet.example", "both.example"]);
    }

    #[test]
    fn addresses_that_are_not_public_are_refused_before_anything_is_sent() {
        // With the reach every other build has, a server on this machine is out of reach, though
        // it is there and would answer. (Only addresses here: the tests look no real name up.)
        let server = FakeHttp::answering(answer("200 OK", "", &picture()));
        let port = server.port;
        for local in [
            format!("http://127.0.0.1:{port}/a.png"),
            format!("http://localhost:{port}/a.png"),
            format!("http://LOCALHOST.:{port}/a.png"),
            format!("http://mail.localhost:{port}/a.png"),
            format!("http://127.8.9.10:{port}/a.png"),
            format!("http://[::1]:{port}/a.png"),
            format!("http://[::ffff:127.0.0.1]:{port}/a.png"),
            format!("http://[::ffff:7f00:1]:{port}/a.png"),
            format!("http://0.0.0.0:{port}/a.png"),
            format!("https://127.0.0.1:{port}/a.png"),
        ] {
            assert_eq!(fetch(&local), Err(PictureError::NotPublic), "{local}");
        }
        assert_eq!(server.asked(), 0);

        // Nor is a name looked up for an address that is refused by how it is written.
        let reach = Reach { lookup: &no_names, stand_ins: &[], dial: None };
        for (url, wanted) in [
            ("http://10.0.0.1/a.png", PictureError::NotPublic),
            ("http://10.255.255.255:8080/a.png", PictureError::NotPublic),
            ("http://172.16.0.1/a.png", PictureError::NotPublic),
            ("http://172.31.255.254/a.png", PictureError::NotPublic),
            ("http://192.168.0.1/a.png", PictureError::NotPublic),
            ("http://100.64.0.1/a.png", PictureError::NotPublic),
            ("http://169.254.169.254/latest/meta-data/iam/", PictureError::NotPublic),
            ("http://224.0.0.251/a.png", PictureError::NotPublic),
            ("http://255.255.255.255/a.png", PictureError::NotPublic),
            ("http://[fe80::1]/a.png", PictureError::NotPublic),
            ("http://[fc00::1]/a.png", PictureError::NotPublic),
            ("http://[fd12:3456:789a::1]/a.png", PictureError::NotPublic),
            ("http://[::]/a.png", PictureError::NotPublic),
            ("http://[64:ff9b::a00:1]/a.png", PictureError::NotPublic),
            ("http://[2001:db8::1]/a.png", PictureError::NotPublic),
            ("http://[2002:c0a8:101::1]/a.png", PictureError::NotPublic),
            ("https://localhost/a.png", PictureError::NotPublic),
            ("", PictureError::Address),
            ("pictures.example/a.png", PictureError::Address),
            ("//pictures.example/a.png", PictureError::Address),
            ("/a.png", PictureError::Address),
            ("ftp://pictures.example/a.png", PictureError::Address),
            ("file:///C:/Users/owner/secret.png", PictureError::Address),
            ("\\\\fileserver\\share\\a.png", PictureError::Address),
            ("cid:logo@example.org", PictureError::Address),
            ("data:image/png;base64,AAAA", PictureError::Address),
            ("javascript:alert(1)", PictureError::Address),
            ("https://owner:secret@pictures.example/a.png", PictureError::Address),
            ("https://owner@pictures.example/a.png", PictureError::Address),
            ("https://pictures.example:0/a.png", PictureError::Address),
            ("https://pictures.example:99999/a.png", PictureError::Address),
            ("https://pictures example/a.png", PictureError::Address),
            ("https://pictures.example /a.png", PictureError::Address),
            ("https://pictures.example/a\tb.png", PictureError::Address),
            ("https://pictures.example/a\u{0}b.png", PictureError::Address),
            ("https://pictures.example/a\u{7f}b.png", PictureError::Address),
            ("https://pictures.example/a\u{85}b.png", PictureError::Address),
            ("https://pictures.example/a.png\r\nX-Injected: 1", PictureError::Address),
            ("https://pictures.example/a.png?x=1\nHost: evil.example", PictureError::Address),
            ("https://pictüres.example/a.png", PictureError::Address),
            ("https://fe80::1%eth0/a.png", PictureError::Address),
            ("https://[fe80::1%25eth0]/a.png", PictureError::Address),
            ("https://", PictureError::Address),
            ("https:///a.png", PictureError::Address),
        ] {
            assert_eq!(fetch_with(url, &reach, &quick()), Err(wanted), "{url:?}");
        }
        let long = format!("https://pictures.example/{}", "a".repeat(http::MAX_LONG_URL));
        assert_eq!(fetch_with(&long, &reach, &quick()), Err(PictureError::Address));

        // By name: every address a name has must be public; a name with none is no host.
        let names = |host: &str, port: u16, _: Duration| match host {
            "router.example" => vec![SocketAddr::from(([192, 168, 1, 1], port))],
            "sly.example" => vec![SocketAddr::from(([93, 184, 216, 34], port)), SocketAddr::from(([10, 0, 0, 7], port))],
            "six.example" => vec![SocketAddr::new("fe80::1".parse().unwrap(), port)],
            _ => Vec::new(),
        };
        let reach = Reach { lookup: &names, stand_ins: &[], dial: None };
        for host in ["router.example", "sly.example", "six.example"] {
            assert_eq!(fetch_with(&format!("https://{host}/a.png"), &reach, &quick()), Err(PictureError::NotPublic), "{host}");
        }
        assert!(matches!(
            fetch_with("https://nowhere.example/a.png", &reach, &quick()),
            Err(PictureError::Net(NetError::Connect { host, .. })) if host == "nowhere.example"
        ));
    }

    #[test]
    fn what_is_public_and_what_is_not() {
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        for public in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "100.63.255.255",
            "100.128.0.0",
            "172.15.255.255",
            "172.32.0.0",
            "192.0.1.1",
            "192.167.255.255",
            "198.17.255.255",
            "198.20.0.0",
            "223.255.255.255",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2a00:1450:4001:81b::200e",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
        ] {
            assert!(is_public(ip(public)), "{public}");
        }
        for private in [
            "0.0.0.0",
            "0.1.2.3",
            "10.0.0.0",
            "10.255.255.255",
            "100.64.0.0",
            "100.127.255.255",
            "127.0.0.1",
            "127.255.255.255",
            "169.254.0.1",
            "169.254.169.254",
            "172.16.0.0",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.0.0",
            "192.168.255.255",
            "198.18.0.0",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.7",
            "224.0.0.1",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::7f00:1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:192.168.1.1",
            "64:ff9b::7f00:1",
            "64:ff9b::a00:1",
            "64:ff9b:1::808:808",
            "100::1",
            "2001::1",
            "2001:0:4136:e378:8000:63bf:3fff:fdd2",
            "2001:2::1",
            "2001:db8::1",
            "2002:808:808::1",
            "3fff::1",
            "fc00::1",
            "fd00::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
            "4000::1",
        ] {
            assert!(!is_public(ip(private)), "{private}");
        }
    }

    #[test]
    fn a_picture_that_is_too_large_is_refused() {
        let small = Limits { max_bytes: 10_000, ..quick() };
        let server = FakeHttp::start(|_, request| {
            let body = |n: usize| "x".repeat(n);
            match path_of(request) {
                "/fits" => answer("200 OK", "", body(10_000).as_bytes()),
                "/by-length" => answer("200 OK", "", body(10_001).as_bytes()),
                "/in-chunks" => {
                    let chunk = format!("1000\r\n{}\r\n", body(0x1000));
                    format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{chunk}{chunk}{chunk}0\r\n\r\n").into_bytes()
                }
                "/to-the-end" => format!("HTTP/1.1 200 OK\r\n\r\n{}", body(10_001)).into_bytes(),
                // Claimed, and never sent: refused on the claim.
                "/claimed" => format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\npng", MAX_BYTES + 1).into_bytes(),
                "/absurd" => b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\npng".to_vec(),
                // What the redirects on the way answer counts too.
                "/heavy-1" => answer("302 Found", "Location: /heavy-2\r\n", body(6_000).as_bytes()),
                "/heavy-2" => answer("302 Found", "Location: /heavy-3\r\n", body(3_000).as_bytes()),
                "/heavy-3" => answer("200 OK", "", body(1_001).as_bytes()),
                "/light-1" => answer("302 Found", "Location: /light-2\r\n", body(6_000).as_bytes()),
                "/light-2" => answer("200 OK", "", body(4_000).as_bytes()),
                _ => answer("404 Not Found", "", b""),
            }
        });
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        let get = |path: &str, limits: &Limits| fetch_with(&server.url(path), &reach, limits);

        assert_eq!(get("/fits", &small).map(|bytes| bytes.len()), Ok(10_000));
        for path in ["/by-length", "/in-chunks", "/to-the-end"] {
            assert_eq!(get(path, &small), Err(PictureError::TooLarge), "{path}");
        }
        assert_eq!(get("/light-1", &small).map(|bytes| bytes.len()), Ok(4_000));
        assert_eq!(get("/heavy-1", &small), Err(PictureError::TooLarge));
        // The size every other build goes by, and what the owner is told of a picture past it.
        assert_eq!(Limits::default().max_bytes, MAX_BYTES);
        assert_eq!(get("/claimed", &quick()), Err(PictureError::TooLarge));
        assert_eq!(PictureError::TooLarge.to_string(), "it is larger than 8 MB");
        assert!(matches!(get("/absurd", &quick()), Err(PictureError::Net(NetError::Proto(ProtoError::Protocol(_))))));
    }

    /// `bytes` packed the way `Content-Encoding: gzip` has it.
    fn gzipped(bytes: &[u8]) -> Vec<u8> {
        let mut packer = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        packer.write_all(bytes).unwrap();
        packer.finish().unwrap()
    }

    /// The same for `deflate`: in zlib's wrapping, or bare.
    fn deflated(bytes: &[u8], wrapped: bool) -> Vec<u8> {
        if wrapped {
            let mut packer = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            packer.write_all(bytes).unwrap();
            packer.finish().unwrap()
        } else {
            let mut packer = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            packer.write_all(bytes).unwrap();
            packer.finish().unwrap()
        }
    }

    /// An answer whose body goes out in chunks of `size` bytes.
    fn chunked(headers: &str, body: &[u8], size: usize) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 200 OK\r\n{headers}Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n").into_bytes();
        for chunk in body.chunks(size.max(1)) {
            out.extend(format!("{:x};note=ignored\r\n", chunk.len()).bytes());
            out.extend_from_slice(chunk);
            out.extend(b"\r\n");
        }
        out.extend(b"0\r\nX-Trailer: 1\r\n\r\n");
        out
    }

    /// What a WebP picture begins with (whether the rest is one is the decoder's to say).
    fn webp() -> Vec<u8> {
        let mut bytes = b"RIFF\x24\x02\0\0WEBPVP8 ".to_vec();
        bytes.extend((0..540u32).map(|i| (i % 241) as u8));
        bytes
    }

    #[test]
    fn answers_as_the_web_sends_them_chunked_packed_and_of_newer_kinds() {
        let big = {
            let mut bytes = picture();
            bytes.extend((0..70_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8));
            bytes
        };
        let (png, large) = (picture(), big.clone());
        let server = FakeHttp::start(move |_, request| {
            match path_of(request) {
                // In chunks, as most servers that make their pictures on the way send them.
                "/chunks.png" => chunked("Content-Type: image/png\r\n", &png, 97),
                "/big-chunks.png" => chunked("", &large, 8_192),
                "/one-byte-chunks.png" => chunked("", &png, 1),
                // Packed, though the request says "as it is": servers that pack everything.
                "/gzip.png" => answer("200 OK", "Content-Encoding: gzip\r\nVary: Accept-Encoding\r\n", &gzipped(&png)),
                "/x-gzip.png" => answer("200 OK", "content-encoding:  X-GZIP \r\n", &gzipped(&png)),
                "/gzip-in-chunks.png" => chunked("Content-Encoding: gzip\r\n", &gzipped(&large), 1_000),
                "/deflate.png" => answer("200 OK", "Content-Encoding: deflate\r\n", &deflated(&png, true)),
                "/bare-deflate.png" => answer("200 OK", "Content-Encoding: deflate\r\n", &deflated(&png, false)),
                "/twice.png" => answer("200 OK", "Content-Encoding: gzip\r\nContent-Encoding: deflate\r\n", &deflated(&gzipped(&png), true)),
                "/twice-in-one.png" => answer("200 OK", "Content-Encoding: deflate, gzip\r\n", &gzipped(&deflated(&png, true))),
                // Says it is packed and isn't, or says nothing at all of it.
                "/not-really.png" => answer("200 OK", "Content-Encoding: gzip\r\n", &png),
                "/identity.png" => answer("200 OK", "Content-Encoding: identity\r\n", &png),
                "/empty-coding.png" => answer("200 OK", "Content-Encoding: \r\n", &png),
                // Packed in ways that are not unpacked here, or damaged in the packing.
                "/brotli.png" => answer("200 OK", "Content-Encoding: br\r\n", b"\x0b\x02\x80png\x03"),
                "/zstd.png" => answer("200 OK", "Content-Encoding: zstd\r\n", b"\x28\xb5\x2f\xfd\x04\x58\x19\0\0png"),
                "/thrice.png" => answer("200 OK", "Content-Encoding: gzip, gzip, gzip\r\n", &gzipped(&gzipped(&gzipped(&png)))),
                "/cut.png" => {
                    let packed = gzipped(&large);
                    answer("200 OK", "Content-Encoding: gzip\r\n", &packed[..packed.len() / 2])
                }
                "/garbled.png" => answer("200 OK", "Content-Encoding: gzip\r\n", b"\x1f\x8b\x08\0\0\0\0\0\0\xff\xff\xff\xff\xff\xff\xff\xff"),
                "/garbled-deflate.png" => answer("200 OK", "Content-Encoding: deflate\r\n", b"\xff\xff\xff\xff not packed at all"),
                // A kind nobody asked for by name: handed on, for the decoder to make what it can of.
                "/hero.webp" => answer("200 OK", "Content-Type: image/webp\r\nVary: Accept\r\n", &webp()),
                "/hero.jpg" => answer("200 OK", "Content-Type: image/webp\r\n", &webp()),
                "/hero.avif" => answer("200 OK", "Content-Type: image/avif\r\n", b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf"),
                // (#46) A web firewall: 406 to a sender that doesn't read like a browser, or that
                // won't take whatever it has.
                "/guarded.png" => {
                    let lower = request.to_ascii_lowercase();
                    if lower.contains("user-agent: mozilla/5.0") && lower.contains("*/*") {
                        answer("200 OK", "Content-Type: application/octet-stream\r\n", &png)
                    } else {
                        answer("406 Not Acceptable", "", b"")
                    }
                }
                _ => answer("404 Not Found", "", b""),
            }
        });
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        let get = |path: &str| fetch_with(&server.url(path), &reach, &quick());

        for path in [
            "/chunks.png",
            "/one-byte-chunks.png",
            "/gzip.png",
            "/x-gzip.png",
            "/deflate.png",
            "/bare-deflate.png",
            "/twice.png",
            "/twice-in-one.png",
        ] {
            assert_eq!(get(path), Ok(picture()), "{path}");
        }
        for path in ["/not-really.png", "/identity.png", "/empty-coding.png"] {
            assert_eq!(get(path), Ok(picture()), "{path}");
        }
        for path in ["/big-chunks.png", "/gzip-in-chunks.png"] {
            assert_eq!(get(path).map(|bytes| bytes == big), Ok(true), "{path}");
        }
        for path in ["/brotli.png", "/zstd.png", "/thrice.png", "/cut.png", "/garbled.png", "/garbled-deflate.png"] {
            assert_eq!(get(path), Err(PictureError::Packed), "{path}");
        }
        assert_eq!(get("/guarded.png"), Ok(picture()));
        assert_eq!(get("/hero.webp"), Ok(webp()));
        assert_eq!(get("/hero.jpg"), Ok(webp()));
        assert!(get("/hero.avif").is_ok_and(|bytes| bytes.get(4..12) == Some(&b"ftypavif"[..])));
        // Nobody is ever told that packing, or a newer kind of picture, would be welcome.
        for n in 0..server.asked() {
            let request = server.request(n).to_ascii_lowercase();
            let headers = request.split_once("\r\n").map_or("", |(_, headers)| headers);
            assert!(headers.contains("accept-encoding: identity\r\n") && !headers.contains("gzip") && !headers.contains("webp"), "{request}");
        }
    }

    #[test]
    fn what_a_packed_answer_unpacks_to_is_bounded() {
        // A few hundred bytes that unpack to twenty million: no more than the limit is taken out.
        let bomb = gzipped(&vec![0u8; 20_000_000]);
        assert!(bomb.len() < 30_000, "{}", bomb.len());
        let (once, twice) = (bomb.clone(), gzipped(&bomb));
        let fits = gzipped(&vec![7u8; 10_000]);
        let server = FakeHttp::start(move |_, request| match path_of(request) {
            "/bomb" => answer("200 OK", "Content-Encoding: gzip\r\n", &once),
            "/bomb-in-a-bomb" => answer("200 OK", "Content-Encoding: gzip, gzip\r\n", &twice),
            "/fits" => answer("200 OK", "Content-Encoding: gzip\r\n", &fits),
            // What the redirect on the way answered counts against what it may unpack to.
            "/after-a-redirect" => answer("302 Found", "Location: /fits\r\n", &[b'x'; 500]),
            _ => answer("404 Not Found", "", b""),
        });
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        let get = |path: &str, max_bytes: usize| fetch_with(&server.url(path), &reach, &Limits { max_bytes, ..quick() });

        let started = Instant::now();
        assert_eq!(get("/bomb", MAX_BYTES), Err(PictureError::TooLarge));
        assert_eq!(get("/bomb-in-a-bomb", MAX_BYTES), Err(PictureError::TooLarge));
        assert_eq!(get("/bomb", 40_000), Err(PictureError::TooLarge));
        assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());
        assert_eq!(get("/fits", 10_000).map(|bytes| bytes.len()), Ok(10_000));
        assert_eq!(get("/fits", 9_999), Err(PictureError::TooLarge));
        assert_eq!(get("/after-a-redirect", 10_500).map(|bytes| bytes.len()), Ok(10_000));
        assert_eq!(get("/after-a-redirect", 10_499), Err(PictureError::TooLarge));
        // The same holds of each way of unpacking by itself.
        let zeros = vec![0u8; 100_000];
        assert_eq!(unpack(gzipped(&zeros), "gzip", 99_999), Err(PictureError::TooLarge));
        assert_eq!(unpack(deflated(&zeros, true), "deflate", 99_999), Err(PictureError::TooLarge));
        assert_eq!(unpack(deflated(&zeros, false), "deflate", 99_999), Err(PictureError::TooLarge));
        assert_eq!(unpack(gzipped(&zeros), "gzip", 100_000).map(|bytes| bytes.len()), Ok(100_000));
        assert_eq!(unpack(Vec::new(), "gzip", 10), Ok(Vec::new()));
        assert_eq!(unpack(b"as it is".to_vec(), " identity , ", 10), Ok(b"as it is".to_vec()));
    }

    #[test]
    fn a_server_that_says_not_now_is_asked_once_more() {
        let brief = Limits { retry_wait: Duration::from_millis(30), ..quick() };
        let server = FakeHttp::start(|n, request| match (path_of(request), n) {
            // Too many at once, then fine: what a host says to the fourth picture of one message.
            ("/busy.png", 0) => answer("429 Too Many Requests", "", b"slow down"),
            ("/busy.png", _) => answer("200 OK", "", &picture()),
            ("/unavailable.png", 2) => answer("503 Service Unavailable", "Retry-After: 0\r\n", b""),
            ("/unavailable.png", _) => answer("200 OK", "", &picture()),
            ("/always-busy.png", _) => answer("429 Too Many Requests", "Retry-After: 0\r\n", b""),
            // It asks for longer than a picture is worth, or for a day that is no number.
            ("/come-back-tomorrow.png", _) => answer("503 Service Unavailable", "Retry-After: 86400\r\n", b""),
            ("/come-back-on.png", _) => answer("503 Service Unavailable", "Retry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n", b""),
            // Other refusals are refusals: asking again changes nothing.
            ("/forbidden.png", _) => answer("403 Forbidden", "Retry-After: 0\r\n", b""),
            ("/broken.png", _) => answer("500 Internal Server Error", "", b""),
            // One asking again for the whole way, redirects and all.
            ("/moved.png", 15) => answer("429 Too Many Requests", "Retry-After: 0\r\n", b""),
            ("/moved.png", _) => redirect("/there.png"),
            ("/there.png", 17) => answer("503 Service Unavailable", "Retry-After: 0\r\n", b""),
            _ => answer("404 Not Found", "", b""),
        });
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        let get = |path: &str, limits: &Limits| fetch_with(&server.url(path), &reach, limits);

        assert_eq!(get("/busy.png", &brief), Ok(picture()));
        assert_eq!(get("/unavailable.png", &brief), Ok(picture()));
        assert_eq!(server.asked(), 4);
        // Twice is all: a server that goes on saying so is believed.
        assert_eq!(get("/always-busy.png", &brief), Err(PictureError::Status(429)));
        assert_eq!(server.asked(), 6);
        // No longer than a few seconds is waited, whatever it asks for; and not at all when
        // the picture's time would be up first.
        let started = Instant::now();
        assert_eq!(get("/come-back-on.png", &brief), Err(PictureError::Status(503)));
        assert_eq!(server.asked(), 8);
        let hurried = Limits { patience: Duration::from_millis(1_500), ..brief };
        assert_eq!(get("/come-back-tomorrow.png", &hurried), Err(PictureError::Status(503)));
        assert_eq!(server.asked(), 9);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        for (path, status) in [("/forbidden.png", 403), ("/broken.png", 500), ("/missing.png", 404)] {
            let before = server.asked();
            assert_eq!(get(path, &brief), Err(PictureError::Status(status)));
            assert_eq!(server.asked() - before, 1, "{path}");
        }
        assert_eq!(server.asked(), 12);
        // Requests 12 to 14 never come to "/moved.png" at 15: line the count up, then go.
        for _ in 0..3 {
            let _ = get("/missing.png", &brief);
        }
        assert_eq!(get("/moved.png", &brief), Err(PictureError::Status(503)));
        assert_eq!(server.asked(), 18);
        // What the owner is told of each.
        for (status, said) in [
            (403, "the server won't hand it out (403)"),
            (404, "it is no longer there (404)"),
            (410, "it is no longer there (410)"),
            (429, "the server is being asked too often just now (429)"),
            (503, "the server has trouble of its own (503)"),
            (304, "the server answered 304 instead of a picture"),
        ] {
            assert_eq!(PictureError::Status(status).to_string(), said);
        }
    }

    #[test]
    fn an_address_that_never_answers_makes_way_for_the_next() {
        let server = FakeHttp::answering(answer("200 OK", "", &picture()));
        let good = at(&server);
        // Addresses of a kind this machine has no way to: they are never really connected to
        // (the stand-in for the system's way of connecting sits on them until its time is up).
        let dead: Vec<SocketAddr> = ["[2001:db8:dead::1]:80", "[2001:db8:dead::2]:80", "203.0.113.9:80"].iter().map(|a| a.parse().unwrap()).collect();
        let dialled = Arc::new(Mutex::new(Vec::new()));
        let (seen, dead_ones) = (dialled.clone(), dead.clone());
        let dial: Dial = Arc::new(move |address, patience| {
            seen.lock().unwrap().push(address);
            if dead_ones.contains(&address) {
                std::thread::sleep(patience);
                return Err(ErrorKind::TimedOut.into());
            }
            TcpStream::connect_timeout(&address, patience)
        });
        let mut allowed = dead.clone();
        allowed.push(good);
        let limits = Limits { head_start: Duration::from_millis(80), connect: Duration::from_millis(1_500), ..quick() };
        let found = Mutex::new(Vec::new());
        let names = |_: &str, _: u16, _: Duration| found.lock().unwrap().clone();
        let reach = Reach { lookup: &names, stand_ins: &allowed, dial: Some(dial) };
        let get = |order: &[SocketAddr]| {
            *found.lock().unwrap() = order.to_vec();
            dialled.lock().unwrap().clear();
            let started = Instant::now();
            let result = fetch_with("http://pictures.example/a.png", &reach, &limits);
            (result, started.elapsed(), dialled.lock().unwrap().clone())
        };

        // The one that answers is first: the others are never tried.
        let (result, took, tried) = get(&[good, dead[0], dead[2]]);
        assert_eq!((result, tried), (Ok(picture()), vec![good]));
        assert!(took < Duration::from_millis(1_000), "{took:?}");
        // An IPv6 address that doesn't answer comes first, as on a network that has names for
        // IPv6 and no way there: it costs its head start, not the eight seconds it would be given.
        let (result, took, tried) = get(&[dead[0], good]);
        assert_eq!((result, tried), (Ok(picture()), vec![dead[0], good]));
        assert!(took >= Duration::from_millis(80) && took < Duration::from_millis(1_000), "{took:?}");
        // Two of them first: the kinds take turns, so the second one tried is the one that answers.
        let (result, took, tried) = get(&[dead[0], dead[1], good]);
        assert_eq!((result, tried), (Ok(picture()), vec![dead[0], good]));
        assert!(took < Duration::from_millis(1_000), "{took:?}");
        // Dead ones of both kinds before it: each has its head start, and then it is the good one's turn.
        let (result, took, tried) = get(&[dead[2], dead[0], dead[1], good]);
        assert_eq!((result, tried), (Ok(picture()), vec![dead[2], dead[0], good]));
        assert!(took >= Duration::from_millis(160) && took < Duration::from_millis(1_300), "{took:?}");
        // None of them answers: given up on when the last of them has had its time, with a reason.
        let (result, took, tried) = get(&[dead[0], dead[2]]);
        assert!(
            matches!(&result, Err(PictureError::Net(NetError::Connect { host, reason })) if host == "pictures.example" && reason == "the server didn't answer"),
            "{result:?}"
        );
        assert_eq!(tried.len(), 2);
        assert!(took >= Duration::from_millis(1_400) && took < Duration::from_millis(4_000), "{took:?}");
        // No more than four addresses of a host are tried, whatever its name gives.
        let many: Vec<SocketAddr> = (0..12).map(|n| SocketAddr::from(([203, 0, 113, 100 + n], 80))).collect();
        let mut allowed_many = many.clone();
        allowed_many.push(good);
        let refused: Dial = Arc::new(|_, _| Err(ErrorKind::ConnectionRefused.into()));
        let names = |_: &str, _: u16, _: Duration| many.clone();
        let result = fetch_with("http://pictures.example/a.png", &Reach { lookup: &names, stand_ins: &allowed_many, dial: Some(refused) }, &limits);
        assert!(
            matches!(&result, Err(PictureError::Net(NetError::Connect { reason, .. })) if reason == "the server refused the connection"),
            "{result:?}"
        );

        // The order they are tried in.
        let v = |text: &str| -> SocketAddr { text.parse().unwrap() };
        let (a4, b4, c4, a6, b6) =
            (v("93.184.216.34:443"), v("93.184.216.35:443"), v("93.184.216.36:443"), v("[2606:4700::1]:443"), v("[2606:4700::2]:443"));
        assert_eq!(in_turns(vec![a6, b6, a4, b4, c4]), [a6, a4, b6, b4, c4]);
        assert_eq!(in_turns(vec![a4, b4, a6, b6]), [a4, a6, b4, b6]);
        assert_eq!(in_turns(vec![a4, b4, c4]), [a4, b4, c4]);
        assert_eq!(in_turns(vec![a6]), [a6]);
        assert!(in_turns(Vec::new()).is_empty());
    }

    /// A server on this machine that does with its one connection what `serve` says.
    fn serving(serve: impl FnOnce(TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((peer, _)) = listener.accept() {
                serve(peer);
            }
        });
        address
    }

    #[test]
    fn a_slow_server_is_given_up_on() {
        let hurried = Limits { patience: Duration::from_millis(400), ..Limits::default() };
        assert_eq!(PictureError::TimedOut.to_string(), "the server took too long");
        // It takes the connection and the request, and never says a word.
        let (hold, held) = std::sync::mpsc::channel::<()>();
        let silent = serving(move |peer| {
            let _ = held.recv();
            drop(peer);
        });
        let started = Instant::now();
        let result = fetch_with(&format!("http://{silent}/a.png"), &Reach { lookup: &no_names, stand_ins: &[silent], dial: None }, &hurried);
        assert_eq!(result, Err(PictureError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
        drop(hold);

        // A byte every 20 ms, for ever: no single read waits long, and the picture never ends.
        let dripping = serving(|mut peer| {
            let _ = peer.write_all(b"HTTP/1.1 200 OK\r\n\r\n");
            while peer.write_all(b"x").is_ok() {
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let result = fetch_with(&format!("http://{dripping}/a.png"), &Reach { lookup: &no_names, stand_ins: &[dripping], dial: None }, &hurried);
        assert_eq!(result, Err(PictureError::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());

        // Redirects don't each get their time anew: the picture has one allowance.
        let server = FakeHttp::start(|n, _| {
            std::thread::sleep(Duration::from_millis(250));
            redirect(&format!("/{}", n + 1))
        });
        let allowed = [at(&server)];
        let started = Instant::now();
        let result = fetch_with(&server.url("/0"), &Reach { lookup: &no_names, stand_ins: &allowed, dial: None }, &hurried);
        assert!(result.is_err() && result != Err(PictureError::TooManyRedirects), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());

        // A name whose lookup hangs is left behind when the time is up.
        let started = Instant::now();
        assert_eq!(within(Duration::from_millis(100), || std::thread::sleep(Duration::from_secs(3))), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(within(Duration::from_secs(5), || 7), Some(7));
        let stuck = |_: &str, _: u16, patience: Duration| {
            within(patience, || std::thread::sleep(Duration::from_secs(3))).map(|()| Vec::new()).unwrap_or_default()
        };
        let started = Instant::now();
        let result = fetch_with("https://stuck.example/a.png", &Reach { lookup: &stuck, stand_ins: &[], dial: None }, &hurried);
        assert!(matches!(result, Err(PictureError::Net(NetError::Connect { .. }))), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_closed_port_and_a_server_that_hangs_up_are_errors() {
        // A port that was just free: nothing listens there.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let result = fetch_with(&format!("http://{closed}/a.png"), &Reach { lookup: &no_names, stand_ins: &[closed], dial: None }, &quick());
        assert!(matches!(&result, Err(PictureError::Net(NetError::Connect { host, .. })) if host == "127.0.0.1"), "{result:?}");
        // It answers the door and leaves.
        let rude = serving(drop);
        let result = fetch_with(&format!("http://{rude}/a.png"), &Reach { lookup: &no_names, stand_ins: &[rude], dial: None }, &quick());
        assert!(matches!(result, Err(PictureError::Net(NetError::Proto(ProtoError::Io(_))))), "{result:?}");
        // `https` to a server that speaks no TLS is a failed handshake, not a picture.
        let plain = serving(|mut peer| {
            let _ = peer.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\npng");
            let _ = peer.read(&mut [0u8; 512]);
        });
        let result = fetch_with(&format!("https://{plain}/a.png"), &Reach { lookup: &no_names, stand_ins: &[plain], dial: None }, &quick());
        assert!(matches!(&result, Err(PictureError::Net(NetError::Tls { host, .. })) if host == "127.0.0.1"), "{result:?}");
    }

    #[test]
    fn hostile_answers_are_errors_or_bytes_and_never_more() {
        let answers: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"\x00\xff\xfe garbage \x80".to_vec(),
            b"220 smtp.example.org ESMTP\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: many\r\n\r\npng".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 50\r\n\r\ncut short".to_vec(),
            b"HTTP/1.1 302 Found\r\nLocation: \xff\xfe\x00\r\n\r\n".to_vec(),
            b"HTTP/1.1 302 Found\r\nLocation: http://\r\n\r\n".to_vec(),
            format!("HTTP/1.1 302 Found\r\nLocation: /{}\r\n\r\n", "a".repeat(12_000)).into_bytes(),
            format!("HTTP/1.1 200 OK\r\n{}\r\npng", "X-Filler: yes\r\n".repeat(400)).into_bytes(),
            format!("HTTP/1.1 200 OK\r\nX-Filler: {}\r\n\r\npng", "x".repeat(40_000)).into_bytes(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\npng\r\n0\r\n\r\n".to_vec(),
            b"HTTP/1.1 999 What\r\n\r\n".to_vec(),
        ];
        let count = answers.len();
        let server = FakeHttp::start(move |n, _| answers[n % count].clone());
        let allowed = [at(&server)];
        let reach = Reach { lookup: &no_names, stand_ins: &allowed, dial: None };
        for n in 0..count {
            let result = fetch_with(&server.url("/a.png"), &reach, &quick());
            assert!(result.is_err(), "answer {n}: {:?}", result.map(|bytes| bytes.len()));
        }
        // What is not a picture and says 200 is handed on as it came: telling is the decoder's work.
        let page = FakeHttp::answering(answer("200 OK", "Content-Type: text/html\r\n", b"<html><script>alert(1)</script></html>"));
        let allowed = [at(&page)];
        let bytes = fetch_with(&page.url("/a.png"), &Reach { lookup: &no_names, stand_ins: &allowed, dial: None }, &quick()).unwrap();
        assert!(bytes.starts_with(b"<html>"));
    }

    #[test]
    fn addresses_are_read_and_redirects_are_resolved() {
        let read = |text: &str| address(text).map(|url| (url.secure, url.host, url.port, url.path));
        assert_eq!(read("https://img.example/a/b.png?w=2"), Ok((true, "img.example".into(), 0, "/a/b.png?w=2".into())));
        assert_eq!(read("http://IMG.example:8080"), Ok((false, "IMG.example".into(), 8080, "/".into())));
        assert_eq!(read("hTTp://[2001:4860::1]:81/x#y"), Ok((false, "2001:4860::1".into(), 81, "/x".into())));
        assert_eq!(read("https://img.example?x=1"), Ok((true, "img.example".into(), 0, "/?x=1".into())));

        let from = address("https://img.example:8443/shop/2026/a.png?size=2").unwrap();
        let to = |location: &str| {
            follow(&from, location).map(|url| format!("{}://{}{}", if url.secure { "https" } else { "http" }, authority(&url), url.path))
        };
        assert_eq!(to("http://cdn.example/b.png").as_deref(), Ok("http://cdn.example/b.png"));
        assert_eq!(to("HTTPS://cdn.example:444/b.png#x").as_deref(), Ok("https://cdn.example:444/b.png"));
        assert_eq!(to("//cdn.example/b.png").as_deref(), Ok("https://cdn.example/b.png"));
        assert_eq!(to("/b.png?u=http://x.example/").as_deref(), Ok("https://img.example:8443/b.png?u=http://x.example/"));
        assert_eq!(to("b.png").as_deref(), Ok("https://img.example:8443/shop/2026/b.png"));
        assert_eq!(to("?size=3").as_deref(), Ok("https://img.example:8443/shop/2026/?size=3"));
        assert_eq!(
            follow(&address("http://[::1]:81").unwrap(), "x.png").map(|url| (authority(&url), url.path)),
            Ok(("[::1]:81".into(), "/x.png".into()))
        );
        for bad in [
            "",
            "  ",
            "ftp://cdn.example/b.png",
            "javascript:alert(1)",
            "mailto:a@b.example",
            "data:,x",
            "b.png\r\nHost: evil",
            "http://a b/",
            "//a@b/c",
        ] {
            assert_eq!(to(bad), Err(PictureError::Address), "{bad:?}");
        }

        // What a message writes loosely is written the way the wire has it; what is written
        // that way already stays as it is.
        let path = |text: &str| address(text).map(|url| url.path);
        assert_eq!(path("https://img.example/autumn sale/hero image.png?t=a b"), Ok("/autumn%20sale/hero%20image.png?t=a%20b".into()));
        assert_eq!(
            path("https://img.example/caf\u{e9}/\u{65e5}\u{672c}.png?q=\u{1f600}"),
            Ok("/caf%C3%A9/%E6%97%A5%E6%9C%AC.png?q=%F0%9F%98%80".into())
        );
        assert_eq!(path("https://img.example/a%20b/%C3%A9.png?x=%7B1%7D&y=100%"), Ok("/a%20b/%C3%A9.png?x=%7B1%7D&y=100%".into()));
        assert_eq!(path("https://img.example/a\"b<c>d`e.png"), Ok("/a%22b%3Cc%3Ed%60e.png".into()));
        assert_eq!(
            path("https://img.example/o.gif?u=1|2&d={x}&e=[y]&f=a+b&g=~!$'()*,;:@^\\"),
            Ok("/o.gif?u=1|2&d={x}&e=[y]&f=a+b&g=~!$'()*,;:@^\\".into())
        );
        assert_eq!(path("https://img.example?name=a b"), Ok("/?name=a%20b".into()));
        // Steps up and in place are taken first, as a browser takes them; never above the top.
        assert_eq!(path("https://img.example/a/b/../c/./d.png?x=../y"), Ok("/a/c/d.png?x=../y".into()));
        assert_eq!(path("https://img.example/../../etc/passwd"), Ok("/etc/passwd".into()));
        assert_eq!(path("https://img.example/a/b/.."), Ok("/a/".into()));
        assert_eq!(path("https://img.example/a/./"), Ok("/a/".into()));
        assert_eq!(path("https://img.example/.."), Ok("/".into()));
        assert_eq!(path("https://img.example/a/..."), Ok("/a/...".into()));
        assert_eq!(path("https://img.example/a//b/.hidden/x..png"), Ok("/a//b/.hidden/x..png".into()));
        // The host is never rewritten: one that isn't plain is no host.
        for bad in
            ["https://img example/a.png", "https://b\u{fc}cher.example/a.png", "https://img.example\u{a0}/a.png", "https://\"img\".example/a.png"]
        {
            assert_eq!(address(bad), Err(PictureError::Address), "{bad:?}");
        }
        assert_eq!(to("sale 2026/hero \u{e9}.png").as_deref(), Ok("https://img.example:8443/shop/2026/sale%202026/hero%20%C3%A9.png"));
        assert_eq!(to("../../b.png").as_deref(), Ok("https://img.example:8443/b.png"));
        assert_eq!(to("./b.png?x=1").as_deref(), Ok("https://img.example:8443/shop/2026/b.png?x=1"));
        // Rewriting never makes an address longer than an address may be.
        let long = format!("https://img.example/{}", "\u{e9}".repeat(http::MAX_LONG_URL / 4));
        assert_eq!(address(&long), Err(PictureError::Address));

        let head = |text: &str| said(text.as_bytes()).location;
        assert_eq!(head("HTTP/1.1 302 Found\r\nLocation: /b.png\r\n\r\nbody").as_deref(), Some("/b.png"));
        assert_eq!(head("HTTP/1.0 301\nX: 1\nLOCATION:   https://cdn.example/b.png  \n\n").as_deref(), Some("https://cdn.example/b.png"));
        assert_eq!(
            head("HTTP/1.1 100 Continue\r\nLocation: /wrong\r\n\r\nHTTP/1.1 302 Found\r\nLocation: /right\r\n\r\n").as_deref(),
            Some("/right")
        );
        // The first one counts; one in the body is no header.
        assert_eq!(head("HTTP/1.1 302 Found\r\nLocation: /first\r\nLocation: /second\r\n\r\n").as_deref(), Some("/first"));
        assert_eq!(head("HTTP/1.1 302 Found\r\n\r\nLocation: /in-the-body\r\n"), None);
        assert_eq!(head("HTTP/1.1 302 Found\r\nX-Location: /no\r\n\r\n"), None);
        assert_eq!(head(""), None);
        assert_eq!(head("nonsense"), None);

        // The rest of what an answer says that matters: how it is packed, and when to come back.
        let all = |text: &str| said(text.as_bytes());
        assert_eq!(
            all("HTTP/1.1 503 Busy\r\nRetry-After: 2\r\ncontent-encoding: GZip\r\nLocation: /x\r\n\r\nRetry-After: 9\r\n"),
            Said { location: Some("/x".into()), coding: Some("gzip".into()), retry_after: Some(2) }
        );
        assert_eq!(
            all("HTTP/1.1 200 OK\r\nContent-Encoding: deflate\r\nContent-Encoding: gzip , br\r\n\r\n").coding.as_deref(),
            Some("deflate,gzip , br")
        );
        assert_eq!(all("HTTP/1.1 429 Slow\r\nRetry-After: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n").retry_after, None);
        assert_eq!(all("HTTP/1.1 429 Slow\r\nRetry-After: -3\r\nRetry-After: 99999999999999999999999\r\n\r\n").retry_after, None);
        // What a "go on" said is not what the answer says.
        assert_eq!(all("HTTP/1.1 100 Continue\r\nContent-Encoding: gzip\r\nRetry-After: 5\r\n\r\nHTTP/1.1 200 OK\r\n\r\n"), Said::default());
        assert_eq!(all("HTTP/1.1 200 OK\r\nX-Content-Encoding: gzip\r\nno colon here\r\n\r\n"), Said::default());
    }

    #[test]
    fn a_handshake_that_fails_says_why_in_a_few_words() {
        use rustls::{AlertDescription, CertificateError, Error};
        let through = |error: Error| handshake_trouble(&std::io::Error::new(ErrorKind::InvalidData, error));
        assert_eq!(through(Error::InvalidCertificate(CertificateError::Expired)), "its certificate has run out");
        assert_eq!(through(Error::InvalidCertificate(CertificateError::NotValidForName)), "its certificate is for another name");
        assert_eq!(
            through(Error::InvalidCertificate(CertificateError::UnknownIssuer)),
            "its certificate doesn't lead back to an authority that is trusted"
        );
        assert_eq!(through(Error::InvalidCertificate(CertificateError::BadEncoding)), "its certificate isn't good");
        assert_eq!(
            through(Error::AlertReceived(AlertDescription::HandshakeFailure)),
            "the server and this program have no way of encrypting in common"
        );
        assert_eq!(handshake_trouble(&ErrorKind::ConnectionReset.into()), "the server closed the connection");
        // Anything else in the library's words: one line, and not much of it.
        let other = handshake_trouble(&std::io::Error::other(format!("odd\r\n{}", "x".repeat(900))));
        assert!(other.starts_with("oddx") && other.chars().count() == 200, "{other}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Whatever a server sends, the answer is an error or bytes within the limit.
        #[test]
        fn arbitrary_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400), after_head in any::<bool>()) {
            static SERVER: std::sync::OnceLock<(FakeHttp, Arc<Mutex<Vec<u8>>>, AtomicUsize)> = std::sync::OnceLock::new();
            let (server, next, asked) = SERVER.get_or_init(|| {
                let next: Arc<Mutex<Vec<u8>>> = Default::default();
                let from = next.clone();
                (FakeHttp::start(move |_, _| from.lock().unwrap().clone()), next, AtomicUsize::new(0))
            });
            let mut answer = if after_head { b"HTTP/1.1 302 Found\r\nLocation: ".to_vec() } else { Vec::new() };
            answer.extend_from_slice(&bytes);
            *next.lock().unwrap() = answer;
            let allowed = [at(server)];
            let limits = Limits { max_bytes: 200, patience: Duration::from_secs(5), connect: Duration::from_secs(3), retry_wait: Duration::ZERO, ..Limits::default() };
            // A redirect could name a host: none has an address here.
            let nowhere = |_: &str, _: u16, _: Duration| Vec::new();
            let url = server.url(&format!("/{}", asked.fetch_add(1, Ordering::Relaxed)));
            if let Ok(body) = fetch_with(&url, &Reach { lookup: &nowhere, stand_ins: &allowed, dial: None }, &limits) {
                prop_assert!(body.len() <= 200);
            }
        }

        #[test]
        fn any_text_is_an_address_or_not(text in "\\PC{0,60}", tail in "[a-z0-9:/?#@\\[\\].%-]{0,40}", location in "\\PC{0,40}|[a-z0-9:/?#@\\[\\].%-]{0,40}") {
            let _ = address(&text);
            let _ = address(&tail);
            let _ = said(text.as_bytes());
            let other = format!("ftp://{tail}");
            prop_assert!(address(&other).is_err());
            // Whatever a message writes after the scheme: an address that is plain on the wire
            // (nothing in it can end a line, or be read as anything but its path), or none.
            for scheme in ["https://", "http://"] {
                if let Ok(url) = address(&format!("{scheme}{text}")) {
                    prop_assert!(url.path.starts_with('/') && url.path.bytes().all(|b| b.is_ascii_graphic()), "{:?}", url.path);
                    prop_assert!(!url.host.is_empty() && url.host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':')), "{:?}", url.host);
                    prop_assert!(!url.path.split('?').next().unwrap_or("").split('/').any(|step| step == "." || step == ".."), "{:?}", url.path);
                }
            }
            for scheme in ["https://", "http://", "HTTP://"] {
                if let Ok(url) = address(&format!("{scheme}{tail}")) {
                    prop_assert!(!url.host.is_empty() && !url.host.contains('@') && url.path.starts_with('/') && !url.path.contains('#'));
                    prop_assert_eq!(url.secure, scheme == "https://");
                    // Wherever a redirect points, what comes of it is an address like any other, or nothing.
                    if let Ok(next) = follow(&url, &location) {
                        prop_assert!(!next.host.is_empty() && next.path.starts_with('/') && next.path.bytes().all(|b| b.is_ascii_graphic()));
                    }
                }
            }
        }

        /// Whatever a server sends as a packed body, and however it says it is packed: an error,
        /// or no more bytes than may be taken.
        #[test]
        fn anything_unpacks_to_an_error_or_within_the_limit(
            bytes in proptest::collection::vec(any::<u8>(), 0..300),
            coding in "(gzip|x-gzip|deflate|identity|br|zstd|,| |[a-z]{0,5}){0,5}",
            well_formed in 0u8..4,
            max in 0usize..600,
        ) {
            // Junk as it is, and junk that was really packed, cut short or not.
            let body = match well_formed {
                1 => gzipped(&bytes),
                2 => deflated(&bytes, true),
                3 => gzipped(&bytes).into_iter().take(bytes.len() / 2 + 6).collect(),
                _ => bytes.clone(),
            };
            for coding in [coding.as_str(), "gzip", "deflate", "gzip, deflate"] {
                if let Ok(out) = unpack(body.clone(), coding, max) {
                    prop_assert!(out.len() <= max.max(body.len()), "{} bytes out of {coding:?}", out.len());
                }
            }
            if bytes.len() <= max {
                prop_assert_eq!(unpack(gzipped(&bytes), "gzip", max), Ok(bytes.clone()));
                prop_assert_eq!(unpack(deflated(&bytes, true), "deflate", max), Ok(bytes.clone()));
            }
            // And whatever the headers of an answer are, they are read or not.
            let mut head = b"HTTP/1.1 200 OK\r\nContent-Encoding: ".to_vec();
            head.extend_from_slice(&bytes);
            let _ = said(&head);
            let _ = said(&bytes);
        }

        #[test]
        fn every_address_is_public_or_not(v4 in any::<u32>(), v6 in any::<u128>()) {
            let v4 = Ipv4Addr::from(v4);
            // Private, loopback and link-local by the standard library's own reckoning are never public here.
            if v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() || v4.is_documentation() {
                prop_assert!(!is_public(IpAddr::V4(v4)));
            }
            // And an IPv6 address that carries an IPv4 one is public exactly when that one is.
            prop_assert_eq!(is_public(IpAddr::V6(v4.to_ipv6_mapped())), is_public(IpAddr::V4(v4)));
            let v6 = Ipv6Addr::from(v6);
            if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || v6.is_unique_local() || v6.is_unicast_link_local() {
                prop_assert!(!is_public(IpAddr::V6(v6)));
            }
        }
    }
}
