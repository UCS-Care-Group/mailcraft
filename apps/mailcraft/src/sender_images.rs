// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A sender's picture for the message list, when the owner turned that on in File › Options:
//! Gravatar's for the address, else the icon favicon.im has for its domain, else Google's.
//!
//! The window asks only about senders the owner knows or whose mail's pictures the owner lets
//! come, never about mail in Junk E-mail (`mailcraft_ui_egui::sender_images`). Here an address
//! becomes the places to ask, and each is downloaded and kept like a message's pictures
//! ([`picture_cache::picture`]): checked on the way, capped, and kept on disk so that it is not
//! asked for again at the next start.
//!
//! - **Gravatar** is told the SHA-256 of the address in lower case (its way of naming one), not
//!   the address, and answers 404 rather than a stand-in picture when it has none (`d=404`).
//! - **favicon.im** is told the domain of the sender's site alone ([`site`]: `nutrient.io` for
//!   `mail.nutrient.io`, which has no icon of its own), and answers 404 rather than a stand-in
//!   icon when it has none (`throw-error-on-404`). Not for a mail provider's own domain
//!   (`gmail.com`), whose icon says nothing of the sender, nor for a name on a hosting platform
//!   (`contoso.onmicrosoft.com`), which would wear the platform's icon.
//! - **Google's favicon service**, only when favicon.im had none or none that is shown (an SVG):
//!   it is told the same domain, and sends every icon as PNG. It has no documented terms, so it
//!   comes last; when it has no icon it answers 404 with a globe of its own, which, like any
//!   answer that isn't 200, is never shown.

use std::sync::{Arc, Weak};

use mailcraft_engine::store::Digest;
use mailcraft_model::provider;

use crate::picture_cache::{self, Cache};

/// Longest address looked up.
const MAX_ADDRESS: usize = 320;
/// The size asked of Gravatar and Google, in pixels: a list's avatar on a screen of three times
/// the density.
const SIZE: u32 = 96;

/// Where a picture of `address` may be had, in the order they are asked; none for what is no
/// address.
pub fn addresses(address: &str) -> Vec<String> {
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

/// The site a mail domain is part of, whose icon it wears: `mail.nutrient.io` sends for
/// `nutrient.io`, and `news.bbc.co.uk` for `bbc.co.uk`. A name made up for one recipient alone
/// (`u8f3a.spam.example`) is not told to anybody either.
fn site(domain: &str) -> &str {
    let last = |labels: usize| domain.rmatch_indices('.').nth(labels - 1).and_then(|(dot, _)| domain.get(dot + 1..)).unwrap_or(domain);
    // The last two labels, or three under a suffix anybody may have a name under.
    if provider::is_public_suffix(last(2)) { last(3) } else { last(2) }
}

/// Whether bytes begin as a sender's picture does: as a message's pictures do
/// ([`picture_cache::looks_like_a_picture`]), or as an ICO holding a picture or more, a web
/// site's icon, which only a sender's picture may be.
fn looks_like_a_picture(bytes: &[u8]) -> bool {
    picture_cache::looks_like_a_picture(bytes) || (bytes.starts_with(b"\0\0\x01\0") && bytes.get(4..6).is_some_and(|count| count != b"\0\0"))
}

/// The picture of `address`: from the first of [`addresses`] that has one, downloaded with
/// `fetch` (`mailcraft_net::pictures::fetch`), for as long as the window still wants it
/// (`wanted` can be upgraded). Once it doesn't, nothing more is asked: a request already on its
/// way goes on, and what it brings is not kept. On a background thread: it waits for the network.
pub fn look_up<E: std::fmt::Display>(
    address: &str,
    cache: Option<&Arc<Cache>>,
    wanted: &Weak<()>,
    fetch: impl Fn(&str) -> Result<Vec<u8>, E>,
) -> Result<Vec<u8>, String> {
    let turned_off = || Err("sender images were turned off".to_string());
    if wanted.strong_count() == 0 {
        return turned_off();
    }
    let urls = addresses(address);
    // What is kept, from any of them, before anything is asked: a place that had nothing is not
    // asked again at each start when a later one's picture is kept.
    let kept = urls.iter().find_map(|url| cache.and_then(|cache| cache.get_as(url, looks_like_a_picture)));
    if let Some(bytes) = kept {
        return Ok(bytes);
    }
    let mut why = "no picture is looked up for this address".to_string();
    for url in urls {
        if wanted.strong_count() == 0 {
            return turned_off();
        }
        match fetch(&url) {
            Ok(bytes) if looks_like_a_picture(&bytes) => {
                // Not kept when the window let go while it came (turned off, or cleared).
                if wanted.strong_count() == 0 {
                    return turned_off();
                }
                if let Some(cache) = cache {
                    cache.put_as(&url, &bytes, looks_like_a_picture);
                }
                return Ok(bytes);
            }
            // An icon in a kind that isn't shown (an SVG): the initials stay.
            Ok(_) => why = "what came is not a picture that is shown".to_string(),
            Err(e) => why = e.to_string(),
        }
    }
    Err(why)
}

#[cfg(test)]
#[path = "sender_images_tests.rs"]
mod tests;
