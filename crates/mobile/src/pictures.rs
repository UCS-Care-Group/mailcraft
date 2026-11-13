// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures from the web, and a sender's picture. The one way out is
//! `mailcraft_net::pictures::fetch`: it tells the server nothing but the address (no cookies, no
//! referrer, no credentials), goes to public hosts only (checked on every redirect and against
//! the addresses connected to), follows at most five redirects and is capped in size and time.
//! What arrives must be a PNG, JPEG, GIF or WebP by its own bytes (a sender's picture may also be
//! an ICO). Whether the owner allowed a download, and when to wait (data saving), is the app's to
//! decide before it calls; nothing here is called by the core by itself.

use mailcraft_engine::store::Digest;
use mailcraft_model::auth_results;
use mailcraft_model::provider;

use crate::message::sniff;
use crate::records::{FetchedPicture, SenderFacts};
use crate::text::{self, MAX_ADDRESS};
use crate::{Core, CoreError};

/// Largest picture taken: the downloader's own cap, held again here.
const MAX_BYTES: usize = mailcraft_net::pictures::MAX_BYTES;
/// Largest sender's picture, which is small.
const MAX_SENDER_BYTES: usize = 1024 * 1024;
/// Longest web address that is asked for (the downloader refuses more of its own).
const MAX_URL: usize = 16 * 1024;
/// The size asked of Gravatar and Google, in pixels.
const SIZE: u32 = 96;

fn failed(reason: &str) -> CoreError {
    CoreError::Failed { reason: reason.to_string() }
}

/// Whether bytes begin as an ICO that holds a picture or more: a web site's icon.
fn is_icon(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\0\0\x01\0") && bytes.get(4..6).is_some_and(|count| count != b"\0\0")
}

fn download(url: &str, cap: usize, icons: bool) -> Result<FetchedPicture, CoreError> {
    if url.len() > MAX_URL {
        return Err(CoreError::BadRequest { reason: "that address is too long".to_string() });
    }
    let bytes = mailcraft_net::pictures::fetch(url).map_err(|e| CoreError::Failed { reason: e.to_string() })?;
    if bytes.len() > cap {
        return Err(failed("the picture is too large"));
    }
    if let Some(mime) = sniff(&bytes) {
        return Ok(FetchedPicture { mime: mime.to_string(), bytes });
    }
    if icons && is_icon(&bytes) {
        return Ok(FetchedPicture { mime: "image/x-icon".to_string(), bytes });
    }
    Err(failed("what came is not a picture that can be shown"))
}

/// Where a sender's picture may be had, in the order they are asked; none for what is no address.
fn sender_urls(address: &str) -> Vec<String> {
    let address = address.trim().to_lowercase();
    let plain = address.len() <= MAX_ADDRESS && !address.chars().any(|c| c.is_whitespace() || c.is_control());
    let Some((_, domain)) = address.rsplit_once('@').filter(|(name, _)| plain && !name.is_empty()) else { return Vec::new() };
    if !provider::is_domain(domain) {
        return Vec::new();
    }
    let mut urls = vec![format!("https://gravatar.com/avatar/{}?s={SIZE}&d=404", Digest::of(address.as_bytes()).hex())];
    let site = site(domain);
    // A customer's name on a hosting platform would wear the platform's icon, as a phish does.
    if !provider::is_mail_provider(site) && !provider::is_public_suffix(site) && !provider::on_hosting_platform(site) {
        urls.push(format!("https://favicon.im/{site}?larger=true&throw-error-on-404=true"));
        urls.push(format!("https://www.google.com/s2/favicons?domain={site}&sz={SIZE}"));
    }
    urls
}

/// The site a mail domain is part of, whose icon it wears: `mail.nutrient.io` sends for `nutrient.io`.
fn site(domain: &str) -> &str {
    let last = |labels: usize| domain.rmatch_indices('.').nth(labels - 1).and_then(|(dot, _)| domain.get(dot + 1..)).unwrap_or(domain);
    if provider::is_public_suffix(last(2)) { last(3) } else { last(2) }
}

/// Download the picture at `url`, an address a message gave, and hand it over as a picture. It
/// blocks for 30 seconds at most, so the app calls it off its main thread. It is for a message's
/// pictures only after the owner allowed them: nothing in the core calls it by itself.
#[uniffi::export]
pub fn fetch_picture(url: String) -> Result<FetchedPicture, CoreError> {
    download(&url, MAX_BYTES, false)
}

/// The places a picture of the sender `address` may be had, in the order they are asked: Gravatar
/// (told the SHA-256 of the address), then favicon.im and Google (told the sender's site alone,
/// never a mail provider's). None for what is not an address.
#[uniffi::export]
pub fn sender_picture_urls(address: String) -> Vec<String> {
    sender_urls(&address)
}

/// Download a sender's picture from one of [`sender_picture_urls`]: any other address is refused.
/// A sender's picture is small, and may be an ICO.
#[uniffi::export]
pub fn fetch_sender_picture(url: String) -> Result<FetchedPicture, CoreError> {
    // (The address's own part is not known here; every place is one of the three services.)
    let places = ["https://gravatar.com/avatar/", "https://favicon.im/", "https://www.google.com/s2/favicons?"];
    if !places.iter().any(|p| url.starts_with(p)) {
        return Err(CoreError::BadRequest { reason: "that is not a place sender pictures come from".to_string() });
    }
    download(&url, MAX_SENDER_BYTES, true)
}

#[uniffi::export]
impl Core {
    /// What the rules for a sender's picture (and a remembered sender's pictures) need to know of
    /// message `id`: nothing the sender wrote but their address.
    pub async fn sender_facts(&self, id: u64) -> Result<SenderFacts, CoreError> {
        self.call(move |s| {
            // (From the mailbox's own entry: the facts need the sender, the folder, the account and the
            // headers that are kept with the entry, never the body, which may be in the content folder.)
            let m = s.mailbox.messages.get(&id).ok_or_else(|| failed(s.interface_language.t("the message could not be read")))?;
            let address = text::clip(&m.from.email, MAX_ADDRESS);
            let junk = s.mailbox.in_junk(m.folder);
            let vouched = auth_results::vouched(s.mailbox.account(m.account), m);
            let known = s.knows(&address.trim().to_lowercase());
            Ok(SenderFacts { address, junk, vouched, known })
        })
        .await
    }
}
