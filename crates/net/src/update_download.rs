// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Downloading a new version of MailCraft, for a copy that updates itself, and making sure it is
//! the project's own before anything is done with it. Nothing is installed here.
//!
//! A release is believed only by its own signature: `SHA256SUMS.txt.sig` is an ed25519
//! signature over `SHA256SUMS.txt` by one of the keys this build carries
//! ([`crate::update::RELEASE_KEYS`]: the one in use and a spare). Then the file for this copy is
//! downloaded whole (at most [`MAX_DOWNLOAD`]), carried on where it stopped when it was cut off
//! (`Range`), and taken only when its SHA-256 is the one the signed list gives for its name.
//! Anything else is refused and what came of it deleted.
//!
//! Everything from GitHub is as hostile as any server's answer: the list of files was checked to
//! be this release's on `github.com` ([`crate::update`]), the signature and the list of
//! checksums are size-capped and read strictly, and every address on the way, the redirects
//! too, must be HTTPS to one of GitHub's hosts for releases ([`HOSTS`]). The requests carry
//! nothing of the owner: the crate's fixed `User-Agent`, `Accept`, and a `Range` when carrying
//! on. An error names no address.

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use mailcraft_model::update::{Asset, Release, is_file_name};
use ring::signature::{ED25519, UnparsedPublicKey};

use crate::NetError;
use crate::http::{self, MAX_LONG_URL, StreamPatience, Streamed, Url};

/// Largest file downloaded (the largest package of 0.4.2 was 60 MB).
pub const MAX_DOWNLOAD: u64 = 400 * 1024 * 1024;
/// The release's list of checksums, and its signature.
pub const SUMS: &str = "SHA256SUMS.txt";
pub const SIGNATURE: &str = "SHA256SUMS.txt.sig";
/// The hosts a release's files are downloaded from: the release's own address, and the two
/// GitHub sends those to.
pub const HOSTS: [&str; 3] = ["github.com", "objects.githubusercontent.com", "release-assets.githubusercontent.com"];
/// Largest list of checksums and signature taken (0.4.2's list was 2 KB; a signature is 89 bytes).
const MAX_SUMS: u64 = 64 * 1024;
const MAX_SIGNATURE: u64 = 1024;
/// Most lines of the list read.
const MAX_LINES: usize = 1_000;
/// Most redirects followed on the way to one file.
const MAX_REDIRECTS: usize = 5;
/// What a file is called while it is coming.
const PART: &str = ".part";

/// Why there is no update to be had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// It isn't the project's, or isn't what the signed list says: what came of it is deleted,
    /// and nothing of it is ever installed.
    Refused(String),
    /// It couldn't be had this time (the network, the disk): what came is kept and carried on
    /// from the next time.
    Failed(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Refused(why) => write!(f, "the update was refused: {why}"),
            DownloadError::Failed(why) => write!(f, "the update couldn't be downloaded: {why}"),
        }
    }
}

fn refused(why: &str) -> DownloadError {
    DownloadError::Refused(why.to_string())
}

fn failed(why: &str) -> DownloadError {
    DownloadError::Failed(why.to_string())
}

/// What a network error is called, without the address or the host.
fn without_address(error: &NetError) -> DownloadError {
    if http::is_too_slow(error) {
        failed("GitHub took too long")
    } else if http::is_too_long(error) {
        refused("a file is larger than it may be")
    } else if error.is_unreachable() {
        failed("GitHub couldn't be reached")
    } else if let NetError::Config(why) = error
        && why.starts_with("the download couldn't be written")
    {
        failed(why)
    } else {
        failed("GitHub's answer was cut off or wasn't one")
    }
}

/// The checksums of a release's files, from its signed list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sums(Vec<(String, [u8; 32])>);

impl Sums {
    /// The SHA-256 the list gives for `name`.
    pub fn digest(&self, name: &str) -> Option<[u8; 32]> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, d)| *d)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The public keys this build carries ([`crate::update::RELEASE_KEYS`]); an empty or broken one
/// is none.
pub fn release_keys() -> Vec<[u8; 32]> {
    keys_of(&crate::update::RELEASE_KEYS)
}

/// The keys of `texts` (base64, 32 bytes each) that are keys.
pub fn keys_of(texts: &[&str]) -> Vec<[u8; 32]> {
    texts
        .iter()
        .filter_map(|text| mailcraft_charset::base64::STANDARD.decode(text.trim()).ok())
        .filter_map(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
        .collect()
}

/// A signature file: 64 bytes in base64 on one line (spaces and line endings around it allowed).
pub fn parse_signature(bytes: &[u8]) -> Option<[u8; 64]> {
    if bytes.len() > usize::try_from(MAX_SIGNATURE).unwrap_or(usize::MAX) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let decoded = mailcraft_charset::base64::STANDARD.decode(text).ok()?;
    <[u8; 64]>::try_from(decoded.as_slice()).ok()
}

/// A list of checksums as `sha256sum` writes it: `<64 hex digits>  <name>` (or ` *<name>`) a
/// line. A line that isn't one is passed over, and so is a name that won't do as a file's
/// ([`is_file_name`]); a name that comes twice makes it no list at all.
pub fn parse_sums(bytes: &[u8]) -> Option<Sums> {
    if bytes.len() > usize::try_from(MAX_SUMS).unwrap_or(usize::MAX) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut sums: Vec<(String, [u8; 32])> = Vec::new();
    for line in text.lines().take(MAX_LINES) {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let Some((hex, rest)) = line.split_once(' ') else { continue };
        let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*')).unwrap_or("");
        let (Some(digest), true) = (digest_of(hex), is_file_name(name)) else { continue };
        if sums.iter().any(|(n, _)| n == name) {
            return None;
        }
        sums.push((name.to_string(), digest));
    }
    Some(Sums(sums))
}

/// 64 hex digits as 32 bytes.
fn digest_of(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        let at = i.checked_mul(2)?;
        *byte = u8::from_str_radix(hex.get(at..at.checked_add(2)?)?, 16).ok()?;
    }
    Some(out)
}

/// The list of checksums, when `signature` is a signature over it by one of `keys`.
pub fn verify(sums: &[u8], signature: &[u8], keys: &[[u8; 32]]) -> Result<Sums, DownloadError> {
    if keys.is_empty() {
        return Err(refused("this build carries no release key"));
    }
    let signature = parse_signature(signature).ok_or_else(|| refused("the release's signature isn't one"))?;
    let signed = keys.iter().any(|key| UnparsedPublicKey::new(&ED25519, key).verify(sums, &signature).is_ok());
    if !signed {
        return Err(refused("the release isn't signed by MailCraft's release key"));
    }
    parse_sums(sums).ok_or_else(|| refused("the release's list of checksums isn't one"))
}

/// A file downloaded and checked: where it is, and the SHA-256 the signed list gives for it. That
/// digest, and never one worked out again from the file, is what it is checked against later:
/// the file may have been changed since.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub path: PathBuf,
    pub sha256: [u8; 32],
}

/// Where and how files may be downloaded from.
pub(crate) struct Reach<'a> {
    /// Whether an address may be asked.
    pub(crate) allowed: &'a dyn Fn(&Url) -> bool,
    /// For the large file, and for the small ones.
    pub(crate) large: StreamPatience,
    pub(crate) small: StreamPatience,
}

/// GitHub's hosts for releases, over HTTPS on the usual port.
fn on_github(url: &Url) -> bool {
    url.secure && matches!(url.port, 0 | 443) && HOSTS.iter().any(|host| url.host.eq_ignore_ascii_case(host))
}

/// Download file `name` of `release` into `dir` (the data folder's `updates/`), check it against
/// the release's signed list, and give where it is and the signed SHA-256. It is downloaded whole, carried on where an
/// earlier try stopped; one already there and checked is not downloaded again. Every other file
/// in `dir` is deleted. Blocks for as long as the download takes; call it on a background thread.
pub fn download(release: &Release, name: &str, dir: &Path) -> Result<Downloaded, DownloadError> {
    let reach = Reach {
        allowed: &on_github,
        large: StreamPatience { connect: Duration::from_secs(10), idle: Duration::from_secs(30), total: Duration::from_secs(3_600) },
        small: StreamPatience { connect: Duration::from_secs(10), idle: Duration::from_secs(10), total: Duration::from_secs(20) },
    };
    download_with(release, name, dir, &release_keys(), &reach)
}

/// [`download`] from a stand-in release server on this machine (`http://127.0.0.1:…`), trusting
/// `keys` rather than the build's: for the tests of the crates that sit on this one.
#[cfg(feature = "stand-in")]
pub fn download_from_stand_in(release: &Release, name: &str, dir: &Path, keys: &[[u8; 32]]) -> Result<Downloaded, DownloadError> {
    let loopback = |url: &Url| crate::transport::is_loopback(&url.host);
    let quick = StreamPatience { connect: Duration::from_secs(5), idle: Duration::from_secs(5), total: Duration::from_secs(20) };
    download_with(release, name, dir, keys, &Reach { allowed: &loopback, large: quick, small: quick })
}

pub(crate) fn download_with(release: &Release, name: &str, dir: &Path, keys: &[[u8; 32]], reach: &Reach<'_>) -> Result<Downloaded, DownloadError> {
    if keys.is_empty() {
        return Err(refused("this build carries no release key"));
    }
    if !is_file_name(name) {
        return Err(refused("that is no name of a release's file"));
    }
    let file_of = |wanted: &str| release.assets.iter().find(|a| a.name == wanted);
    let asset = file_of(name).ok_or_else(|| refused("the release has no file for this copy"))?;
    let sums = file_of(SUMS).ok_or_else(|| refused("the release has no list of checksums"))?;
    let signature = file_of(SIGNATURE).ok_or_else(|| refused("the release isn't signed"))?;
    if asset.size > MAX_DOWNLOAD {
        return Err(refused("the file is larger than an update may be"));
    }
    let sums_bytes = small(sums, MAX_SUMS, reach)?;
    let signature_bytes = small(signature, MAX_SIGNATURE, reach)?;
    let digest = verify(&sums_bytes, &signature_bytes, keys)?.digest(name).ok_or_else(|| refused("the signed list has no checksum for the file"))?;

    fs::create_dir_all(dir).map_err(|e| failed(&format!("the folder for updates couldn't be made: {}", e.kind())))?;
    let done = dir.join(name);
    let part = dir.join(format!("{name}{PART}"));
    tidy(dir, &[done.as_path(), part.as_path()]);
    if done.is_file() {
        if sha256(&done).is_some_and(|d| d == digest) {
            return Ok(Downloaded { path: done, sha256: digest });
        }
        let _ = fs::remove_file(&done);
    }
    match large(asset, &part, reach) {
        Ok(()) => {}
        Err(DownloadError::Refused(why)) => {
            let _ = fs::remove_file(&part);
            return Err(DownloadError::Refused(why));
        }
        Err(other) => return Err(other),
    }
    if sha256(&part) != Some(digest) {
        let _ = fs::remove_file(&part);
        return Err(refused("the file's checksum isn't the one the signed list gives"));
    }
    fs::rename(&part, &done).map_err(|e| failed(&format!("the download couldn't be put in place: {}", e.kind())))?;
    Ok(Downloaded { path: done, sha256: digest })
}

/// Delete every file in `dir` but `keep`: what is left of an older version's download.
fn tidy(dir: &Path, keep: &[&Path]) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten().take(1_000) {
        let path = entry.path();
        let is_file = entry.file_type().is_ok_and(|t| t.is_file());
        if is_file && !keep.contains(&path.as_path()) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// The SHA-256 of a file's bytes; `None` when it can't be read through. A copy that updates
/// itself checks a download with it again right before installing it.
pub fn sha256(path: &Path) -> Option<[u8; 32]> {
    sha256_of(&mut File::open(path).ok()?)
}

/// The SHA-256 of what is read from `reader`, to its end; `None` when it can't be read through.
/// For a file kept open from its check to its use, so that what was checked is what is used.
pub fn sha256_of(reader: &mut dyn Read) -> Option<[u8; 32]> {
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => context.update(buf.get(..n)?),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    <[u8; 32]>::try_from(context.finish().as_ref()).ok()
}

/// The address of a file, or of where a redirect leads, when it may be asked.
fn address(text: &str, reach: &Reach<'_>) -> Result<Url, DownloadError> {
    let url = Url::parse_within(text, MAX_LONG_URL).map_err(|_| refused("an address on the way isn't one"))?;
    if !(reach.allowed)(&url) {
        return Err(refused("an address on the way isn't GitHub's"));
    }
    Ok(url)
}

/// A GET of `asset`'s address, following redirects to allowed addresses only; the body goes into
/// `sink` when `accept` takes the answer's head.
fn get(
    asset: &Asset,
    range_from: Option<u64>,
    patience: StreamPatience,
    max: u64,
    reach: &Reach<'_>,
    accept: &dyn Fn(&Streamed) -> bool,
    sink: &mut dyn Write,
) -> Result<Streamed, DownloadError> {
    let mut url = address(&asset.url, reach)?;
    let range = range_from.map(|from| format!("bytes={from}-"));
    let mut headers = vec![("Accept", "application/octet-stream")];
    if let Some(range) = &range {
        headers.push(("Range", range.as_str()));
    }
    for _ in 0..=MAX_REDIRECTS {
        let said = http::stream(&url, &headers, patience, max, accept, sink).map_err(|e| without_address(&e))?;
        match said.status {
            301 | 302 | 303 | 307 | 308 => {
                let to = said.location.as_deref().ok_or_else(|| failed("GitHub sent a redirect that leads nowhere"))?;
                url = address(to, reach)?;
            }
            _ => return Ok(said),
        }
    }
    Err(failed("GitHub sent too many redirects"))
}

/// A small file of the release (its list of checksums, its signature), whole, in memory.
fn small(asset: &Asset, max: u64, reach: &Reach<'_>) -> Result<Vec<u8>, DownloadError> {
    let mut bytes = Vec::new();
    let said = get(asset, None, reach.small, max, reach, &|_| true, &mut bytes)?;
    match said.status {
        200 => Ok(bytes),
        404 => Err(refused("a file the release lists isn't there")),
        status => Err(failed(&format!("GitHub answered {status}"))),
    }
}

/// `bytes <from>-<last>/<total>` with `from` as asked and `last` the end of the whole.
fn range_carries_on(content_range: Option<&str>, from: u64) -> bool {
    let Some(rest) = content_range.and_then(|r| r.trim().strip_prefix("bytes ")) else { return false };
    let Some((span, total)) = rest.split_once('/') else { return false };
    let Some((first, last)) = span.split_once('-') else { return false };
    let number = |s: &str| if !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit()) { s.parse::<u64>().ok() } else { None };
    match (number(first), number(last), number(total)) {
        (Some(first), Some(last), Some(total)) => first == from && last.checked_add(1) == Some(total) && last >= first,
        _ => false,
    }
}

/// The large file into `part`: carried on from what is there, or from the start.
fn large(asset: &Asset, part: &Path, reach: &Reach<'_>) -> Result<(), DownloadError> {
    let cap = asset.size.min(MAX_DOWNLOAD);
    let mut have = fs::metadata(part).map(|m| m.len()).unwrap_or(0);
    if have > cap {
        let _ = fs::remove_file(part);
        have = 0;
    }
    if have == cap && cap > 0 {
        return Ok(());
    }
    // Once from where it stopped; and, when the server won't carry on, once from the start.
    for _ in 0..2 {
        let from = (have > 0).then_some(have);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(part)
            .map_err(|e| failed(&format!("the download couldn't be written: {}", e.kind())))?;
        let mut sink = BufWriter::new(file);
        let accept = |said: &Streamed| match from {
            None => said.status == 200,
            Some(from) => said.status == 206 && range_carries_on(said.content_range.as_deref(), from),
        };
        let got = get(asset, from, reach.large, cap.saturating_sub(have), reach, &accept, &mut sink);
        let flushed = sink.flush();
        let said = got?;
        flushed.map_err(|e| failed(&format!("the download couldn't be written: {}", e.kind())))?;
        let carried_on = from.is_some() && said.status == 206 && range_carries_on(said.content_range.as_deref(), have);
        match said.status {
            200 if from.is_none() => return Ok(()),
            206 if carried_on => return Ok(()),
            // Not carried on (the whole file again, a range not to be had, another range): from the start.
            200 | 206 | 416 if from.is_some() => {
                let _ = fs::remove_file(part);
                have = 0;
            }
            404 => return Err(refused("a file the release lists isn't there")),
            status => return Err(failed(&format!("GitHub answered {status}"))),
        }
    }
    Err(failed("GitHub didn't send the file"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU16, Ordering};
    use std::sync::{Arc, Mutex};

    use mailcraft_model::update::Version;
    use proptest::prelude::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    use super::*;
    use crate::fake::FakeHttp;

    const NAME: &str = "mailcraft-9.9.9-linux-x86_64.AppImage";

    fn key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn public(pair: &Ed25519KeyPair) -> [u8; 32] {
        pair.public_key().as_ref().try_into().unwrap()
    }

    /// The signature file as `cargo xtask release-key sign` writes it.
    fn signature(pair: &Ed25519KeyPair, bytes: &[u8]) -> Vec<u8> {
        format!("{}\n", mailcraft_charset::base64::STANDARD.encode(pair.sign(bytes).as_ref())).into_bytes()
    }

    fn hex(digest: &[u8]) -> String {
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sums_for(files: &[(&str, &[u8])]) -> Vec<u8> {
        files
            .iter()
            .map(|(name, bytes)| format!("{}  {name}\n", hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())))
            .collect::<String>()
            .into_bytes()
    }

    /// A stand-in release: what each path answers, the file served whole or from a `Range`.
    #[derive(Clone)]
    struct Stand {
        sums: Vec<u8>,
        signature: Vec<u8>,
        file: Vec<u8>,
        /// Bytes of the file sent before the answer is cut off.
        cut_at: Option<usize>,
        /// Ranges are not honoured (the whole file instead).
        no_ranges: bool,
        /// The file comes by way of a redirect to this address (`{port}` filled in).
        redirect: Option<String>,
    }

    struct Served {
        server: FakeHttp,
        stand: Arc<Mutex<Stand>>,
    }

    fn ok(status: &str, extra: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn serve(stand: Stand) -> Served {
        let stand = Arc::new(Mutex::new(stand));
        let port = Arc::new(AtomicU16::new(0));
        let (s, p) = (stand.clone(), port.clone());
        let server = FakeHttp::start(move |_, request| {
            let stand = s.lock().unwrap().clone();
            let path = request.split(' ').nth(1).unwrap_or("").to_string();
            let range = request.lines().find_map(|l| l.strip_prefix("Range: bytes=")).and_then(|r| r.trim_end_matches('-').parse::<usize>().ok());
            match path.as_str() {
                "/dl/SHA256SUMS.txt" => ok("200 OK", "", &stand.sums),
                "/dl/SHA256SUMS.txt.sig" => ok("200 OK", "", &stand.signature),
                _ => serve_file(&stand, &path, range, p.load(Ordering::Relaxed)),
            }
        });
        port.store(server.port, Ordering::Relaxed);
        Served { server, stand }
    }

    fn serve_file(stand: &Stand, path: &str, range: Option<usize>, port: u16) -> Vec<u8> {
        if path == format!("/dl/{NAME}") {
            if let Some(to) = &stand.redirect {
                let to = to.replace("{port}", &port.to_string());
                return format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\n\r\n").into_bytes();
            }
        } else if path != format!("/cdn/{NAME}") {
            return ok("404 Not Found", "", b"");
        }
        let file = &stand.file;
        let (status, extra, body) = match range {
            Some(from) if !stand.no_ranges && from < file.len() => {
                ("206 Partial Content", format!("Content-Range: bytes {from}-{}/{}\r\n", file.len() - 1, file.len()), &file[from..])
            }
            Some(_) if !stand.no_ranges => ("416 Range Not Satisfiable", String::new(), &file[..0]),
            _ => ("200 OK", String::new(), &file[..]),
        };
        match stand.cut_at {
            Some(cut) if cut < body.len() => {
                let mut out = format!("HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
                out.extend_from_slice(&body[..cut]);
                out
            }
            _ => ok(status, &extra, body),
        }
    }

    fn release(server: &FakeHttp, size: u64) -> Release {
        let asset = |name: &str, size: u64| Asset { name: name.to_string(), size, url: server.url(&format!("/dl/{name}")) };
        Release {
            version: Version::parse("9.9.9").unwrap(),
            page: String::new(),
            security: false,
            assets: vec![asset(SUMS, 100), asset(SIGNATURE, 89), asset(NAME, size)],
        }
    }

    fn loopback(url: &Url) -> bool {
        crate::transport::is_loopback(&url.host)
    }

    fn reach() -> Reach<'static> {
        let quick = StreamPatience { connect: Duration::from_secs(2), idle: Duration::from_secs(2), total: Duration::from_secs(5) };
        Reach { allowed: &loopback, large: quick, small: quick }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailcraft-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// A release whose file is `file`, signed with `pair`, served as `adjust` says.
    fn signed(pair: &Ed25519KeyPair, file: &[u8], adjust: impl FnOnce(&mut Stand)) -> Served {
        let sums = sums_for(&[(NAME, file), ("mailcraft-9.9.9-windows-x64.msi", b"other")]);
        let mut stand = Stand { signature: signature(pair, &sums), sums, file: file.to_vec(), cut_at: None, no_ranges: false, redirect: None };
        adjust(&mut stand);
        serve(stand)
    }

    #[test]
    fn a_signed_release_is_downloaded_and_checked() {
        let pair = key();
        let file = b"the new version".repeat(1000);
        let r = signed(&pair, &file, |_| {});
        let dir = scratch("ok");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("mailcraft-9.9.8-linux-x86_64.AppImage.part"), b"an older one").unwrap();
        let got = download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(got.path, dir.join(NAME));
        assert_eq!(fs::read(&got.path).unwrap(), file);
        // The digest given is the signed list's, not one worked out from the file afterwards:
        // the file changed once it is in place doesn't change it.
        let signed_digest = parse_sums(&r.stand.lock().unwrap().sums).unwrap().digest(NAME).unwrap();
        assert_eq!(got.sha256, signed_digest);
        fs::write(&got.path, b"changed after the download").unwrap();
        assert_eq!(got.sha256, signed_digest);
        assert_ne!(sha256(&got.path), Some(got.sha256));
        fs::write(&got.path, &file).unwrap();
        // What was there of another version is gone; nothing else is left.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // Asked again: the list and its signature, and not the file.
        let before = r.server.asked();
        download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(r.server.asked(), before + 2);
        // The requests carry nothing of the owner.
        for n in 0..r.server.asked() {
            let sent = r.server.request(n);
            let headers: Vec<&str> = sent.split("\r\n").skip(1).take_while(|l| !l.is_empty()).filter(|l| !l.starts_with("Host:")).collect();
            assert_eq!(headers, ["User-Agent: MailCraft", "Accept: application/octet-stream", "Connection: close"], "{sent}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_spare_key_is_taken_and_any_other_refused() {
        let (in_use, spare, stranger) = (key(), key(), key());
        let file = b"new".to_vec();
        let keys = [public(&in_use), public(&spare)];
        let r = signed(&spare, &file, |_| {});
        let dir = scratch("spare");
        assert!(download_with(&release(&r.server, 3), NAME, &dir, &keys, &reach()).is_ok());
        let r = signed(&stranger, &file, |_| {});
        let dir = scratch("stranger");
        assert_eq!(
            download_with(&release(&r.server, 3), NAME, &dir, &keys, &reach()),
            Err(refused("the release isn't signed by MailCraft's release key"))
        );
        assert!(!dir.join(NAME).exists() && !dir.join(format!("{NAME}{PART}")).exists());
        // No key at all: nothing is even asked.
        let r = signed(&in_use, &file, |_| {});
        assert_eq!(download_with(&release(&r.server, 3), NAME, &dir, &[], &reach()), Err(refused("this build carries no release key")));
        assert_eq!(r.server.asked(), 0);
        // The build's own placeholders are no keys.
        assert!(keys_of(&["", "  ", "not base64", "AAAA"]).is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_signature_or_a_wrong_checksum_is_refused_and_deleted() {
        let pair = key();
        let file = b"the new version".to_vec();
        let dir = scratch("bad");
        // A signature over something else; one that isn't one; a list changed after signing.
        let cases: Vec<(Box<dyn FnOnce(&mut Stand)>, &str)> = vec![
            (Box::new(|s: &mut Stand| s.signature = signature(&key(), b"else")), "the release isn't signed by MailCraft's release key"),
            (Box::new(|s: &mut Stand| s.signature = b"not a signature".to_vec()), "the release's signature isn't one"),
            (Box::new(|s: &mut Stand| s.sums.extend_from_slice(b"00  evil\n")), "the release isn't signed by MailCraft's release key"),
            (Box::new(|s: &mut Stand| s.file = b"the evil version".to_vec()), "the file's checksum isn't the one the signed list gives"),
        ];
        for (adjust, why) in cases {
            let r = signed(&pair, &file, adjust);
            let size = r.stand.lock().unwrap().file.len() as u64;
            assert_eq!(download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()), Err(refused(why)));
            assert!(!dir.join(NAME).exists() && !dir.join(format!("{NAME}{PART}")).exists(), "{why}");
        }
        // The signed list has no line for the file.
        let sums = sums_for(&[("mailcraft-9.9.9-windows-x64.msi", b"x")]);
        let r = signed(&pair, &file, |s| {
            s.signature = signature(&pair, &sums);
            s.sums = sums.clone();
        });
        assert_eq!(
            download_with(&release(&r.server, 15), NAME, &dir, &[public(&pair)], &reach()),
            Err(refused("the signed list has no checksum for the file"))
        );
        // A file that was there and was changed since is downloaded afresh.
        let r = signed(&pair, &file, |_| {});
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(NAME), b"tampered").unwrap();
        let got = download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(fs::read(got.path).unwrap(), file);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_release_without_what_it_needs_is_refused() {
        let pair = key();
        let r = signed(&pair, b"x", |_| {});
        let dir = scratch("missing");
        for (without, why) in
            [(SIGNATURE, "the release isn't signed"), (SUMS, "the release has no list of checksums"), (NAME, "the release has no file for this copy")]
        {
            let mut release = release(&r.server, 1);
            release.assets.retain(|a| a.name != without);
            assert_eq!(download_with(&release, NAME, &dir, &[public(&pair)], &reach()), Err(refused(why)));
        }
        let release = release(&r.server, MAX_DOWNLOAD + 1);
        assert_eq!(download_with(&release, NAME, &dir, &[public(&pair)], &reach()), Err(refused("the file is larger than an update may be")));
        assert_eq!(download_with(&release, "../x", &dir, &[public(&pair)], &reach()), Err(refused("that is no name of a release's file")));
        assert_eq!(r.server.asked(), 0);
    }

    #[test]
    fn a_file_larger_than_it_says_is_refused() {
        let pair = key();
        let file = b"0123456789".repeat(10);
        let r = signed(&pair, &file, |_| {});
        let dir = scratch("large");
        assert_eq!(download_with(&release(&r.server, 50), NAME, &dir, &[public(&pair)], &reach()), Err(refused("a file is larger than it may be")));
        assert!(!dir.join(format!("{NAME}{PART}")).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_off_download_carries_on_where_it_stopped() {
        let pair = key();
        let file: Vec<u8> = (0..50_000u32).map(|n| (n % 251) as u8).collect();
        let r = signed(&pair, &file, |s| s.cut_at = Some(20_000));
        let dir = scratch("cut");
        let size = file.len() as u64;
        let first = download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach());
        assert!(matches!(first, Err(DownloadError::Failed(_))), "{first:?}");
        let part = dir.join(format!("{NAME}{PART}"));
        assert_eq!(fs::metadata(&part).unwrap().len(), 20_000);
        // The next try asks for the rest, and gets it.
        r.stand.lock().unwrap().cut_at = None;
        let got = download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(fs::read(&got.path).unwrap(), file);
        let last = r.server.request(r.server.asked() - 1);
        assert!(last.contains("Range: bytes=20000-"), "{last}");
        assert!(!part.exists());
        // A server that won't carry on sends the whole again: taken from the start.
        let _ = fs::remove_dir_all(&dir);
        let r = signed(&pair, &file, |s| {
            s.cut_at = Some(10_000);
            s.no_ranges = true;
        });
        assert!(download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()).is_err());
        r.stand.lock().unwrap().cut_at = None;
        let got = download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(fs::read(&got.path).unwrap(), file);
        // A part as long as the whole file that is not the file.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let r = signed(&pair, &file, |_| {});
        fs::write(&part, vec![0u8; file.len()]).unwrap();
        // Refused, deleted, and fine the next time.
        assert_eq!(
            download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()),
            Err(refused("the file's checksum isn't the one the signed list gives"))
        );
        assert!(download_with(&release(&r.server, size), NAME, &dir, &[public(&pair)], &reach()).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn redirects_go_to_allowed_places_only() {
        let pair = key();
        let file = b"by way of a redirect".to_vec();
        let r = signed(&pair, &file, |s| s.redirect = Some(format!("http://127.0.0.1:{{port}}/cdn/{NAME}")));
        let dir = scratch("redirect");
        let got = download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach()).unwrap();
        assert_eq!(fs::read(got.path).unwrap(), file);
        // Somewhere else: refused, and nothing asked of it.
        let _ = fs::remove_dir_all(&dir);
        for elsewhere in ["https://evil.example/x", "http://192.0.2.1/x", "javascript:alert(1)", "/relative", ""] {
            r.stand.lock().unwrap().redirect = Some(elsewhere.to_string());
            let got = download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach());
            assert!(matches!(got, Err(DownloadError::Refused(_))), "{elsewhere}: {got:?}");
        }
        // One that goes round and round.
        r.stand.lock().unwrap().redirect = Some(format!("http://127.0.0.1:{{port}}/dl/{NAME}"));
        assert_eq!(
            download_with(&release(&r.server, file.len() as u64), NAME, &dir, &[public(&pair)], &reach()),
            Err(failed("GitHub sent too many redirects"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_githubs_hosts_over_https_are_allowed() {
        for yes in [
            "https://github.com/o/r/releases/download/v1.0.0/x",
            "https://objects.githubusercontent.com/github-production-release-asset/1?x=y",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1?sig=a%2Fb",
            "https://GitHub.com:443/x",
        ] {
            assert!(on_github(&Url::parse_within(yes, MAX_LONG_URL).unwrap()), "{yes}");
        }
        for no in [
            "https://github.com.evil.example/x",
            "https://evil.example/github.com",
            "https://github.com:8443/x",
            "https://api.github.com/x",
            "https://raw.githubusercontent.com/x",
            "http://127.0.0.1/x",
        ] {
            assert!(!on_github(&Url::parse_within(no, MAX_LONG_URL).unwrap()), "{no}");
        }
    }

    #[test]
    fn the_signature_and_the_list_are_read_strictly() {
        let pair = key();
        let sig = signature(&pair, b"x");
        assert!(parse_signature(&sig).is_some());
        assert!(parse_signature(b"  \r\n").is_none());
        assert!(parse_signature(&[b'A'; 2000]).is_none());
        assert!(parse_signature(mailcraft_charset::base64::STANDARD.encode([0u8; 63]).as_bytes()).is_none());
        let digest = "ab".repeat(32);
        let sums = format!(
            "{digest}  {NAME}\r\n{digest} *binary.msi\nnot a line\n{digest}  ../evil\n{}  short\n{digest}  \n{digest}  with space\n",
            "ab".repeat(31)
        );
        let read = parse_sums(sums.as_bytes()).unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read.digest(NAME), Some([0xab; 32]));
        assert!(read.digest("binary.msi").is_some() && read.digest("../evil").is_none());
        // A name twice is no list; nor is one too long.
        assert!(parse_sums(format!("{digest}  a\n{digest}  a\n").as_bytes()).is_none());
        assert!(parse_sums(&vec![b'a'; 70_000]).is_none());
        assert!(parse_sums(&[0xff, 0xfe]).is_none());
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(sums in proptest::collection::vec(any::<u8>(), 0..600), sig in proptest::collection::vec(any::<u8>(), 0..200), text in "\\PC{0,300}", range in "\\PC{0,40}") {
            let key = [7u8; 32];
            let _ = parse_signature(&sig);
            let _ = parse_sums(&sums);
            let _ = parse_sums(text.as_bytes());
            prop_assert!(verify(&sums, &sig, &[key]).is_err() || parse_sums(&sums).is_some());
            let _ = range_carries_on(Some(&range), 5);
            let _ = keys_of(&[text.as_str()]);
        }
    }
}
