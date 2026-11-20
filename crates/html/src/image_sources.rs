// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where the pictures of a document say their content comes from, as the one who wrote it did.
//!
//! This reader is for rebinding, not for showing: it gives each `<img>` element's `src` value in
//! source order, decoded, with the byte range of that value in the document it was read from, so
//! that a caller who keeps the document's bytes (a signature of the owner's, say) can replace just
//! those ranges with internal addresses and write every other byte back as it was. It reads no
//! address: nothing is fetched, resolved against anything, percent-decoded or judged safe, and a
//! value that is a relative path, a `data:` address or a character reference comes back as data.
//! It is neither a renderer nor a sanitizer, and it changes nothing.

use std::ops::Range;

use crate::entities::unescape;
use crate::tokenizer::{Token, Tokenizer, span_of};

/// Most pictures one document may name ([`image_sources`]); one that names more is refused rather
/// than read in part.
pub const MAX_IMAGE_SOURCES: usize = 256;

/// A picture's source: what its `src` says, and where that value sits in the document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSource {
    /// The value with character references decoded and nothing else read into it: a percent escape
    /// stays escaped, a relative path stays relative.
    pub src: String,
    /// The byte range of the value as written, in the document it was read from, its quotes not
    /// counted.
    pub value_range: Range<usize>,
    /// Whether a `<template>` holds the picture, as the writer of a signature reads the document:
    /// the writer leaves such a picture out of every message, so no file is needed for it. It is
    /// listed all the same, so that a caller who judges every address the document names judges
    /// this one too.
    pub in_template: bool,
}

/// Why a document's pictures could not be listed. Says nothing of the document's text or of the
/// addresses it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSourceError {
    /// The document is longer than [`crate::MAX_INPUT_WITH_PICTURES`] bytes.
    OverInput,
    /// The document names more pictures than [`MAX_IMAGE_SOURCES`].
    TooManySources,
    /// An `<img>` or `<base>` has more attributes than are read, so its source or address may be
    /// one that was dropped.
    AttributeOverflow,
    /// An `<img>` start tag is cut off by the end of the document.
    UnclosedTag,
    /// A source's text is not where the element it was read from says it is.
    BadRange,
    /// The document names a `<base href>`, which pictures' addresses would resolve against.
    UnsupportedBase,
}

impl std::fmt::Display for ImageSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ImageSourceError::OverInput => "the document is longer than is read",
            ImageSourceError::TooManySources => "the document names more pictures than are read",
            ImageSourceError::AttributeOverflow => "an img or base element has more attributes than are read",
            ImageSourceError::UnclosedTag => "an img start tag does not end",
            ImageSourceError::BadRange => "a picture's source is not where its element says it is",
            ImageSourceError::UnsupportedBase => "a base element sets the address pictures resolve against",
        };
        f.write_str(text)
    }
}

impl std::error::Error for ImageSourceError {}

/// The sources of the pictures in `html`, in source order: each `src` value as written (its byte
/// range) and decoded (its text). An element with no `src`, or with an empty one, names no picture
/// and gives none; one a `<template>` holds is given, marked [`ImageSource::in_template`]; where an
/// element has more than one `src`, the first wins, as it does for [`crate::parse`].
///
/// Refused, with the reason, rather than read in part: a document longer than
/// [`crate::MAX_INPUT_WITH_PICTURES`], one that names more than [`MAX_IMAGE_SOURCES`] pictures, one
/// whose `<img>` or `<base>` has more attributes than are read, one whose `<img>` start tag does
/// not end, and one that names a `<base href>`. Nothing is changed here: `html` is only read.
pub fn image_sources(html: &str) -> Result<Vec<ImageSource>, ImageSourceError> {
    if html.len() > crate::MAX_INPUT_WITH_PICTURES {
        return Err(ImageSourceError::OverInput);
    }
    let mut sources: Vec<ImageSource> = Vec::new();
    let mut tokens = Tokenizer::new(html);
    // For each `<img>`, whether the writer leaves it out for a `<template>` around it, by the
    // writer's own reading of where each element ends (a template inside an `svg` ends with it).
    let held = crate::quote::pictures_in_templates(html);
    let mut images = 0usize;
    while let Some(token) = tokens.next() {
        let Token::Open(tag) = &token else { continue };
        // A `<base href>` is an address to resolve against, which this reader does not guess at.
        if tag.name == "base" {
            // Attributes past those kept are dropped, so the address may be one not seen here:
            // refuse rather than guess which address the pictures would resolve against.
            if tag.attrs_overflowed() {
                return Err(ImageSourceError::AttributeOverflow);
            }
            if tag.attr("href").is_some_and(|href| !href.trim().is_empty()) {
                return Err(ImageSourceError::UnsupportedBase);
            }
        }
        if tag.name != "img" {
            continue;
        }
        // One the writer's reading does not place is not held, so that a file is asked for it.
        let in_template = held.get(images).copied().unwrap_or(false);
        images = images.saturating_add(1);
        // A start tag the end of the document cut off may hide its own end, so it is not read.
        if !tokens.tag_closed() {
            return Err(ImageSourceError::UnclosedTag);
        }
        if tag.attrs_overflowed() {
            return Err(ImageSourceError::AttributeOverflow);
        }
        let Some(raw) = tag.raw_attr("src") else { continue };
        if raw.is_empty() {
            continue;
        }
        // The value the tokenizer gives is a slice of the document it read, so its own address
        // says where it starts; the answer is checked against the document.
        let value_range = span_of(html, raw).ok_or(ImageSourceError::BadRange)?;
        // The value is a part of the start tag it was read from, before that tag's `>`.
        if value_range.end > tokens.pos().saturating_sub(1) {
            return Err(ImageSourceError::BadRange);
        }
        if sources.len() >= MAX_IMAGE_SOURCES {
            return Err(ImageSourceError::TooManySources);
        }
        sources.push(ImageSource { src: unescape(raw), value_range, in_template });
    }
    Ok(sources)
}
