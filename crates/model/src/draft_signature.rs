// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The HTML signature a draft was saved with ([`crate::Message::draft_signature`]).
//!
//! When an HTML signature goes into a draft, the draft's HTML holds a fragment made from it, and
//! the signature's pictures travel as inline parts bound to Content-IDs chosen for that draft.
//! Once the draft is saved and closed, nothing in its HTML says which part was the signature, so
//! a reopened draft cannot tell its signature from the owner's own words. A [`DraftSignature`]
//! keeps what is needed to know it again:
//!
//! - [`DraftSignature::signature`]: the signature as it was inserted (its name, the owner's
//!   original raw HTML, its layout, its text stand-in and its pictures), exactly as it was;
//! - [`DraftSignature::rendered`]: the exact signature fragment last embedded in the draft's HTML,
//!   after its picture references were bound to the draft's Content-IDs;
//! - [`DraftSignature::content_ids`]: the Content-ID used for each picture, in order:
//!   `content_ids()[i]` is the one of `signature().pictures()[i]`.
//!
//! It is data, not authority. A reader of a draft (the engine, later) may restore the signature
//! from it only when the draft's HTML really still contains [`DraftSignature::rendered`] and the
//! draft's inline parts under those Content-IDs still match the pictures; otherwise the owner has
//! changed the draft and the snapshot is to be ignored. This module only keeps and checks the
//! snapshot; it decides nothing about a draft.
//!
//! It is local: like [`crate::Message::follow_up`], it lives in the mailbox file with the
//! draft, or with the message waiting in the Outbox that the draft became (so that Undo Send,
//! Cancel Send and Open Draft give the signature back), and is never written into a message's
//! MIME, its headers or its bodies, nor handed to a host for a server
//! ([`crate::Message::for_server`]).
//!
//! A snapshot is checked whole, when it is made ([`DraftSignature::new`]) and when it is read
//! from a file: anything out of bounds or inconsistent makes no snapshot at all. Nothing is ever
//! normalised, trimmed or dropped from one, since a partly kept snapshot would describe a
//! signature that was never inserted.

use serde::{Deserialize, Serialize};

use crate::signature::{self, Picture, Signature};

/// Most bytes of a [`DraftSignature::rendered`] fragment: sixteen times what the owner's own
/// HTML may be ([`signature::MAX_HTML`]), room for what binding and laying it out adds.
pub const MAX_RENDERED: usize = signature::MAX_HTML.saturating_mul(16);

/// Most bytes of one Content-ID in [`DraftSignature::content_ids`].
pub const MAX_CONTENT_ID: usize = 1024;

/// The HTML signature a draft was saved with: see the [module](self).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftSignature {
    signature: Signature,
    rendered: String,
    content_ids: Vec<String>,
}

impl DraftSignature {
    /// A snapshot of `signature` as inserted, `rendered` as embedded in the draft and the
    /// Content-ID of each of its pictures, in order; `None` when any of it is out of bounds or
    /// inconsistent:
    ///
    /// - the signature is an HTML signature, its HTML within [`signature::MAX_HTML`] bytes;
    /// - its name is a cleaned name ([`crate::clean_name`] leaves it exactly as it is);
    /// - its text holds at most [`signature::MAX_TEXT`] characters;
    /// - its details, when it has them, are as [`signature::Details::clean`] leaves them;
    /// - its pictures are within [`signature::MAX_PICTURES`], each a PNG, JPEG or GIF by its own
    ///   bytes ([`signature::picture_mime`], which its `mime` names) of at most
    ///   [`signature::MAX_PICTURE_BYTES`], and at most [`signature::MAX_PICTURES_BYTES`] together;
    /// - `rendered` is not empty and at most [`MAX_RENDERED`] bytes;
    /// - there is one Content-ID for each picture (none for none), each different, not empty, at
    ///   most [`MAX_CONTENT_ID`] bytes, and of printable ASCII other than a space, `<`, `>` or
    ///   `"`.
    pub fn new(signature: Signature, rendered: String, content_ids: Vec<String>) -> Option<Self> {
        let snapshot = DraftSignature { signature, rendered, content_ids };
        snapshot.is_valid().then_some(snapshot)
    }

    /// The signature as it was inserted.
    pub fn signature(&self) -> &Signature {
        &self.signature
    }

    /// The exact signature fragment last embedded in the draft's HTML.
    pub fn rendered(&self) -> &str {
        &self.rendered
    }

    /// The Content-ID of each of [`Signature::pictures`], in the same order.
    pub fn content_ids(&self) -> &[String] {
        &self.content_ids
    }

    fn is_valid(&self) -> bool {
        let sig = &self.signature;
        let html_ok = sig.html().is_some_and(|h| h.len() <= signature::MAX_HTML);
        let name_ok = crate::clean_name(&sig.name).as_deref() == Some(sig.name.as_str());
        let text_ok = sig.body().len() <= signature::MAX_TEXT;
        let details_ok = sig.details.as_ref().is_none_or(|d| d.clone().clean() == *d);
        let rendered_ok = !self.rendered.is_empty() && self.rendered.len() <= MAX_RENDERED;
        html_ok
            && name_ok
            && text_ok
            && details_ok
            && pictures_ok(sig.pictures())
            && rendered_ok
            && content_ids_ok(&self.content_ids, sig.pictures().len())
    }
}

/// Within the caps a signature's pictures are kept under, each the kind its own bytes say.
fn pictures_ok(pictures: &[Picture]) -> bool {
    let mut total = 0usize;
    pictures.len() <= signature::MAX_PICTURES
        && pictures.iter().all(|p| {
            total = total.saturating_add(p.data.len());
            p.data.len() <= signature::MAX_PICTURE_BYTES && signature::picture_mime(&p.data) == Some(p.mime.as_str())
        })
        && total <= signature::MAX_PICTURES_BYTES
}

/// One distinct, bounded, printable Content-ID for each of `pictures` pictures.
fn content_ids_ok(ids: &[String], pictures: usize) -> bool {
    let well_formed =
        |id: &String| !id.is_empty() && id.len() <= MAX_CONTENT_ID && id.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"'));
    let mut seen = std::collections::HashSet::new();
    ids.len() == pictures && ids.iter().all(|id| well_formed(id) && seen.insert(id.as_str()))
}

impl std::fmt::Debug for DraftSignature {
    /// Sizes and counts only: never the name, the HTML, the text or the pictures' bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftSignature")
            .field("html_bytes", &self.signature.html().map_or(0, str::len))
            .field("text_chars", &self.signature.body().len())
            .field("pictures", &self.signature.pictures().len())
            .field("picture_bytes", &self.signature.picture_bytes())
            .field("rendered_bytes", &self.rendered.len())
            .field("content_ids", &self.content_ids.len())
            .finish()
    }
}

/// A snapshot as a file holds it, before it is checked.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    signature: serde_json::Value,
    rendered: String,
    content_ids: Vec<String>,
}

impl<'de> Deserialize<'de> for DraftSignature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<DraftSignature, D::Error> {
        use serde::de::Error;
        let refused = || D::Error::custom("not a draft's signature snapshot");
        let raw = Raw::deserialize(d)?;
        // A signature's own reader leaves out pictures that are bad, twice there or not in its
        // text, tidies its descriptions and widths, and drops the runs of pictures it left out:
        // a snapshot whose signature it changed in any of these ways is not the signature that
        // was inserted. The pictures and the text as written are read on their own and must be
        // what the signature kept.
        let raw_pictures: Vec<Picture> = match raw.signature.get("pictures") {
            None => Vec::new(),
            Some(pictures @ serde_json::Value::Array(_)) => crate::load_limit::from_value(pictures).map_err(|_| refused())?,
            Some(_) => return Err(refused()),
        };
        let raw_body: Option<crate::RichText> = match raw.signature.get("body") {
            None => None,
            Some(body) => Some(crate::load_limit::from_value(body).map_err(|_| refused())?),
        };
        let signature: Signature = crate::load_limit::from_value(&raw.signature).map_err(|_| refused())?;
        if signature.pictures().len() != raw_pictures.len() || signature.pictures() != raw_pictures.as_slice() {
            return Err(refused());
        }
        if raw_body.is_some_and(|body| body != *signature.body()) {
            return Err(refused());
        }
        DraftSignature::new(signature, raw.rendered, raw.content_ids).ok_or_else(refused)
    }
}
