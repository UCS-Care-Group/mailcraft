// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailbox's tags (`GetTagRequest`), which are the owner's categories here: a name and a
//! colour for each. A message carries a tag by name (`tn`, [`crate::items`]), so a tag the list
//! doesn't have is one a name made and the server coloured as it pleased; a name this account's
//! list of categories doesn't have yet joins it in the server's colour, as a Microsoft account's
//! categories do.
//!
//! A tag's colour is a `rgb` field (`#RRGGBB`) when the server writes one, else its numbered
//! colour (`color`, 0 to 7) by our own colours for Zimbra's palette, else grey.

use mailcraft_model::{Category, Rgb};
use serde_json::json;

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::{json, soap};

/// Most tags taken of an answer.
pub const MAX_TAGS: usize = 200;

/// Our own colours for Zimbra's numbered tag colours (0 blue, 1 cyan, 2 green, 3 purple, 4 red,
/// 5 yellow, 6 pink, 7 grey).
const COLORS: [Rgb; 8] = [
    Rgb(0x00, 0x84, 0xC7),
    Rgb(0x43, 0xB3, 0xE3),
    Rgb(0x8D, 0xC6, 0x3F),
    Rgb(0x9C, 0x60, 0xE7),
    Rgb(0xEF, 0x2F, 0x2F),
    Rgb(0xFF, 0xC8, 0x00),
    Rgb(0xF5, 0x8E, 0xE1),
    Rgb(0x82, 0x82, 0x82),
];

/// The request for the mailbox's tags.
pub fn request(base: &str, token: &Secret) -> Result<HttpRequest, Error> {
    soap::request(base, "GetTagRequest", soap::MAIL, json!({}), Some(token))
}

/// A tag's colour: its `rgb` when there is one, its numbered colour's of ours otherwise, grey
/// when neither says one.
fn color(tag: &serde_json::Value) -> Rgb {
    Rgb::parse(&json::raw(tag.get("rgb")).unwrap_or_default())
        .or_else(|| json::number(tag.get("color")).and_then(|n| usize::try_from(n).ok()).and_then(|n| COLORS.get(n)).copied())
        .unwrap_or(mailcraft_model::GREY_CATEGORY)
}

/// The tags of a `GetTagResponse`, as categories: a name and a colour each, capped.
pub fn answer(response: &HttpResponse) -> Result<Vec<Category>, Error> {
    let answer = soap::answer("GetTagRequest", response)?;
    let tags = json::list(answer.get("tag"));
    if json::bad_list(answer.get("tag")) || tags.len() > MAX_TAGS {
        return Err(Error::Unreadable);
    }
    tags.iter()
        .map(|tag| {
            let name = json::raw(tag.get("name")).ok_or(Error::Unreadable)?;
            if name.trim().is_empty() || name.len() > json::MAX_TEXT || name.chars().any(char::is_control) {
                return Err(Error::Unreadable);
            }
            Ok(Category { name, color: color(tag) })
        })
        .collect()
}
