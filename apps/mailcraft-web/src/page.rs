// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the browser shell decides from text alone: which addresses the page opens, and what the
//! page's own address asks for. Kept apart from `web.rs` so that it is tested on the host.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

/// The longest address the page opens.
const MAX_LINK: usize = 8192;

/// The address a followed link opens, or `None`: `http`, `https`, `mailto` and `tel` only, with a
/// host for a web address (`mailcraft_html::is_safe_url`), and never a `cid:` (a picture of the
/// message, nowhere to go). It is opened with `noopener,noreferrer`, so the page it brings up
/// has no handle on this one. A web address is opened as `mailcraft_link` writes it, the same
/// text the link's tip shows the host of, so the browser can't map the host to another one.
pub fn web_link(url: &str) -> Option<String> {
    let url = url.trim_matches(' ');
    if url.len() > MAX_LINK || !mailcraft_html::is_safe_url(url) || mailcraft_html::is_embedded(url) {
        return None;
    }
    let (scheme, rest) = url.split_once(':')?;
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => mailcraft_link::WebLink::parse(url).map(mailcraft_link::WebLink::into_string),
        _ => Some(format!("{}:{}", scheme.to_ascii_lowercase(), mailcraft_link::encode_opaque(rest))),
    }
}

/// Whether the page's query (`?sample&webgl`) holds the flag `name`, as a whole parameter.
pub fn has_flag(query: &str, name: &str) -> bool {
    query.trim_start_matches('?').split('&').any(|p| p.split('=').next().is_some_and(|k| k == name))
}

/// Whether the page was started from a sample or an empty mailbox by its address
/// (`?sample`, `?empty`): a look around, which never touches the mailbox kept in the browser.
pub fn is_ephemeral(query: &str) -> bool {
    has_flag(query, "sample") || has_flag(query, "empty")
}

/// The file (next to `index.html`) of the face the browser build fetches when the interface is in
/// language `code`, for a script the bundled fonts lack and the browser build has no system face
/// for: Georgian only (#1010). `None`: the bundled fonts are enough, and nothing is fetched.
pub fn face_for(code: &str) -> Option<&'static str> {
    (code == "ka").then_some(GEORGIAN_FACE)
}

/// Noto Sans Georgian (`assets/fonts/`, copied next to `index.html` by Trunk).
pub const GEORGIAN_FACE: &str = "NotoSansGeorgian-Regular.ttf";

/// The most a fetched face may be: the Georgian face is 35 KB, so 1 MB leaves room and no more. A
/// server's answer larger than this is refused before it is checked as a font.
pub const FACE_MAX_BYTES: u64 = 1_000_000;

/// Whether a fetched face of `len` bytes is within [`FACE_MAX_BYTES`].
pub fn face_size_ok(len: u64) -> bool {
    len <= FACE_MAX_BYTES
}
