// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signatures (R4.5): rich text the owner ends their messages with, and the pictures (a logo) it
//! may hold.
//!
//! A signature is kept in the mailbox on this computer only; nothing keeps it in step with
//! Outlook, which offers no way to. Its pictures are kept with it, checked by their own bytes
//! (PNG, JPEG or GIF) and capped by size ([`MAX_PICTURE_BYTES`] each, [`MAX_PICTURES_BYTES`]
//! together, [`MAX_PICTURES`] in all), so every message it goes into stays light. A picture
//! stands in the text as a picture run ([`crate::rich::Run::picture`]); the bytes, the width it is
//! shown at and its description are here.
//!
//! Signatures written by earlier versions (`{name, text}`) open as rich text without formatting,
//! and every signature is written with its plain text as `text` too, so an earlier version still
//! reads its words.
//!
//! An HTML signature also keeps the owner's own HTML ([`Signature::html`]), at most [`MAX_HTML`]
//! bytes, as they wrote it: it is cleaned when a message goes out, never here. Its text is then a
//! stand-in, what a message being written shows of it. It has no pictures of its own: those it
//! shows are on the web, and the recipient's program decides whether to load them.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ModelError;
use crate::rich::{Paragraph, RichText, Run, Style, safe_link};

/// Most bytes one picture may have.
pub const MAX_PICTURE_BYTES: usize = 256 * 1024;
/// Most bytes a signature's pictures may have together.
pub const MAX_PICTURES_BYTES: usize = 512 * 1024;
/// Most pictures one signature holds.
pub const MAX_PICTURES: usize = 10;
/// Pictures above this many bytes together make every message noticeably heavier: the editor
/// says so, and lets the owner go on.
pub const HEAVY_BYTES: usize = 100 * 1024;
/// The widest a picture is shown, in pixels.
pub const MAX_WIDTH: u16 = 600;
/// Longest description of a picture, in characters.
pub const MAX_ALT: usize = 200;
/// Most characters of a signature's text.
pub const MAX_TEXT: usize = 10_000;
/// Most bytes of an HTML signature's own HTML.
pub const MAX_HTML: usize = 64 * 1024;

/// How a signature lays out its first picture against its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// Pictures stand in the text where they are.
    #[default]
    Inline,
    /// The first picture on the left, the text on its right (a two-cell table when sent).
    PictureLeft,
    /// The first picture above the text.
    PictureAbove,
}

impl Layout {
    pub fn parse(code: &str) -> Option<Layout> {
        match code.trim().to_ascii_lowercase().as_str() {
            "inline" => Some(Layout::Inline),
            "pictureleft" | "left" => Some(Layout::PictureLeft),
            "pictureabove" | "above" => Some(Layout::PictureAbove),
            _ => None,
        }
    }
}

/// A picture a signature holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Picture {
    /// Its id in the signature's text ([`Run::picture`]).
    pub id: u32,
    /// The file it came from, for the owner (`logo.png`).
    pub name: String,
    /// `image/png`, `image/jpeg` or `image/gif`, from its own bytes.
    pub mime: String,
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
    /// The width it is shown at, in pixels, at most [`MAX_WIDTH`]; `None` for its own (capped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
    /// What it shows, in words: for screen readers, for recipients who don't load pictures, and
    /// in plain text.
    pub alt: String,
}

/// The kind of picture `bytes` are, by their own first bytes: PNG, JPEG or GIF only.
pub fn picture_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// The details a signature can be made from (R4.5.4): kept so that changing one lays it out
/// again, until the owner edits the signature by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Details {
    pub name: String,
    pub title: String,
    pub organisation: String,
    pub phone: String,
    pub mobile: String,
    pub email: String,
    pub website: String,
    pub address: String,
    /// A hash of the text the details last made; the signature's text no longer having it means
    /// the owner edited it by hand.
    #[serde(skip_serializing_if = "is_zero")]
    pub made: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Longest a detail is kept, in characters.
pub const MAX_DETAIL: usize = 200;

impl Details {
    /// The details as kept: no control characters (the address keeps its line breaks), trimmed,
    /// at most [`MAX_DETAIL`] characters each.
    pub fn clean(self) -> Details {
        let one = |s: String| s.chars().filter(|c| !c.is_control()).take(MAX_DETAIL).collect::<String>().trim().to_string();
        let address = self.address.chars().filter(|c| *c == '\n' || !c.is_control()).take(MAX_DETAIL).collect::<String>().trim().to_string();
        Details {
            name: one(self.name),
            title: one(self.title),
            organisation: one(self.organisation),
            phone: one(self.phone),
            mobile: one(self.mobile),
            email: one(self.email),
            website: one(self.website),
            address,
            made: self.made,
        }
    }

    /// The text they make: the name in bold, the job title, the organisation, then each way of
    /// reaching the owner as a working link (`tel:`, `mailto:`, `https:`) and the address; the
    /// logo, when there is one, before it all.
    pub fn text(&self, logo: Option<u32>) -> RichText {
        let line = |runs: Vec<Run>| Paragraph::new(runs, Default::default());
        let plain = |text: &str| Run::new(text, Style::default());
        let linked = |text: &str, link: Option<String>| Run::new(text, Style { link, ..Style::default() });
        let mut out = Vec::new();
        if let Some(id) = logo {
            out.push(line(vec![Run::picture(id, Style::default())]));
        }
        if !self.name.is_empty() {
            out.push(line(vec![Run::new(&self.name, Style { bold: true, size: Some(12), ..Style::default() })]));
        }
        for text in [&self.title, &self.organisation] {
            if !text.is_empty() {
                out.push(line(vec![plain(text)]));
            }
        }
        for (label, number) in [("Phone: ", &self.phone), ("Mobile: ", &self.mobile)] {
            if !number.is_empty() {
                out.push(line(vec![plain(label), linked(number, tel(number))]));
            }
        }
        if !self.email.is_empty() {
            let link = (self.email.contains('@') && !self.email.contains(char::is_whitespace)).then(|| format!("mailto:{}", self.email));
            out.push(line(vec![linked(&self.email, link.as_deref().and_then(safe_link))]));
        }
        if !self.website.is_empty() {
            let lower = self.website.to_ascii_lowercase();
            let url = if lower.starts_with("https://") || lower.starts_with("http://") {
                self.website.clone()
            } else {
                format!("https://{}", self.website)
            };
            out.push(line(vec![linked(&self.website, safe_link(&url))]));
        }
        for part in self.address.lines().map(str::trim).filter(|l| !l.is_empty()) {
            out.push(line(vec![plain(part)]));
        }
        RichText::from_paragraphs(out)
    }
}

/// A telephone number as a `tel:` link: its digits, and a `+` in front when it has one; none
/// when there are too few digits to call.
fn tel(number: &str) -> Option<String> {
    let international = number.trim_start().starts_with('+');
    // `+44 (0)20…`: the (0) is dialled only from inside the country.
    let number = if international { number.replace("(0)", "") } else { number.to_string() };
    let digits: String = number.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 3 {
        return None;
    }
    let plus = if international { "+" } else { "" };
    safe_link(&format!("tel:{plus}{digits}"))
}

/// A fingerprint of a text that stays the same from one version of the program to the next
/// (FNV-1a over its paragraphs, runs and their formatting, written out), for telling whether the
/// owner has edited what the details made.
pub fn text_hash(text: &RichText) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut eat = |part: &str| {
        for b in part.bytes().chain([0xff]) {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in text.paragraphs() {
        eat(&format!("p{:?}{}", p.list, p.level));
        for r in &p.runs {
            let st = &r.style;
            eat(&r.text);
            eat(&format!(
                "{}{}{}{:?}{:?}{:?}{:?}{:?}",
                st.bold,
                st.italic,
                st.underline,
                st.size,
                st.link,
                st.color,
                st.font.map(|f| f.label()),
                r.picture
            ));
        }
    }
    h
}

/// Text, and at most a few pictures, the owner ends their messages with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub name: String,
    body: RichText,
    pictures: Vec<Picture>,
    pub layout: Layout,
    pub details: Option<Details>,
    /// The owner's own HTML, for an HTML signature.
    html: Option<String>,
}

/// A signature as a file holds it (any version), before it is checked.
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Raw {
    name: String,
    /// The plain text of a signature from before R4.5 (or written beside `body` for them).
    text: String,
    #[serde(deserialize_with = "crate::sync::lenient")]
    body: Option<RichText>,
    #[serde(deserialize_with = "crate::sync::lenient_list")]
    pictures: Vec<Picture>,
    #[serde(deserialize_with = "crate::sync::lenient_or_default")]
    layout: Layout,
    #[serde(deserialize_with = "crate::sync::lenient")]
    details: Option<Details>,
    #[serde(deserialize_with = "crate::sync::lenient")]
    html: Option<String>,
}

/// A signature as it is written: its plain text as `text` too, for earlier versions.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Out<'a> {
    name: &'a str,
    text: String,
    body: &'a RichText,
    #[serde(skip_serializing_if = "<[Picture]>::is_empty")]
    pictures: &'a [Picture],
    #[serde(skip_serializing_if = "is_inline")]
    layout: Layout,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: &'a Option<Details>,
    #[serde(skip_serializing_if = "Option::is_none")]
    html: Option<&'a str>,
}

fn is_inline(l: &Layout) -> bool {
    *l == Layout::Inline
}

impl Serialize for Signature {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        Out {
            name: &self.name,
            text: self.plain(),
            body: &self.body,
            pictures: &self.pictures,
            layout: self.layout,
            details: &self.details,
            html: self.html.as_deref(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Signature, D::Error> {
        let raw = Raw::deserialize(d)?;
        let body = raw.body.unwrap_or_else(|| RichText::from_plain(&raw.text));
        let mut sig = Signature { name: raw.name, body, pictures: Vec::new(), layout: raw.layout, details: raw.details, html: None };
        // Each picture is checked as if it were added now; one that isn't a picture, or is over
        // the caps, is left out, and so is its place in the text.
        let mut seen = std::collections::HashSet::new();
        for p in raw.pictures {
            if seen.insert(p.id) && sig.check(&p.data, None).is_ok() {
                let mime = picture_mime(&p.data).unwrap_or_default().to_string();
                sig.pictures.push(Picture { mime, width: p.width.map(|w| w.clamp(1, MAX_WIDTH)), alt: clean_alt(&p.alt), ..p });
            }
        }
        sig.tidy();
        // HTML over the cap is left out, and the signature is its text; one with pictures can't be.
        if sig.pictures.is_empty() {
            sig.html = raw.html.filter(|h| h.len() <= MAX_HTML);
        }
        Ok(sig)
    }
}

/// A description as kept: one line, no control characters, at most [`MAX_ALT`] characters.
fn clean_alt(alt: &str) -> String {
    alt.chars().filter(|c| !c.is_control()).take(MAX_ALT).collect::<String>().trim().to_string()
}

impl Signature {
    /// A signature of plain text (as earlier versions kept them).
    pub fn plain_text(name: impl Into<String>, text: &str) -> Signature {
        let text: String = text.chars().take(MAX_TEXT).collect();
        Signature { name: name.into(), body: RichText::from_plain(&text), ..Signature::default() }
    }

    /// A signature from its parts (the text a message holds of it, say), checked as any other:
    /// pictures the text doesn't hold, or that aren't pictures within the caps, are left out.
    pub fn assemble(name: impl Into<String>, body: RichText, pictures: &[Picture], layout: Layout) -> Signature {
        let mut sig = Signature { name: name.into(), body, pictures: Vec::new(), layout, details: None, html: None };
        for p in pictures {
            if sig.picture(p.id).is_none()
                && let Ok(mime) = sig.check(&p.data, None)
            {
                sig.pictures.push(Picture { mime: mime.to_string(), ..p.clone() });
            }
        }
        sig.tidy();
        sig
    }

    pub fn body(&self) -> &RichText {
        &self.body
    }

    pub fn pictures(&self) -> &[Picture] {
        &self.pictures
    }

    pub fn picture(&self, id: u32) -> Option<&Picture> {
        self.pictures.iter().find(|p| p.id == id)
    }

    /// The owner's own HTML, when it is an HTML signature.
    pub fn html(&self) -> Option<&str> {
        self.html.as_deref()
    }

    /// Make it an HTML signature of `html` (its text is then a stand-in for it, set apart), or
    /// with `None` a rich one again, of its text. Refused over [`MAX_HTML`], and while it has
    /// pictures, which the HTML would lose.
    pub fn set_html(&mut self, html: Option<String>) -> Result<(), ModelError> {
        if let Some(h) = &html {
            if h.len() > MAX_HTML {
                return Err(ModelError::Invalid(format!("an HTML signature is at most {} KB", MAX_HTML / 1024)));
            }
            if !self.pictures.is_empty() {
                return Err(ModelError::Invalid(
                    "an HTML signature shows pictures from their web address only: remove this signature's pictures first".into(),
                ));
            }
        }
        self.html = html;
        Ok(())
    }

    /// Replace the text. Pictures it no longer holds are let go; a picture run naming a picture
    /// this signature doesn't have is left out. New words make an HTML signature rich text again.
    pub fn set_body(&mut self, body: RichText) -> Result<(), ModelError> {
        if body.len() > MAX_TEXT {
            return Err(ModelError::Invalid(format!("a signature holds at most {MAX_TEXT} characters")));
        }
        // The HTML no longer says what the text does: sent beside it, the two would disagree.
        if body != self.body {
            self.html = None;
        }
        self.body = body;
        self.tidy();
        Ok(())
    }

    /// The words alone, as a plain-text message carries them: a picture as its description in
    /// brackets, or nothing.
    pub fn plain(&self) -> String {
        self.body.to_plain_with(|id| self.picture(id).map(|p| p.alt.clone()))
    }

    /// Whether it was made from details and edited by hand since: the details no longer lay it
    /// out.
    pub fn hand_edited(&self) -> bool {
        self.details.as_ref().is_some_and(|d| d.made != 0 && d.made != text_hash(&self.body))
    }

    /// Make the text from `details` (cleaned), keeping the first picture as the logo, laid out
    /// beside the text (or above it, when the owner chose that); without one, in line.
    pub fn lay_out(&mut self, details: Details) -> Result<(), ModelError> {
        let mut details = details.clean();
        // The details make rich text.
        self.html = None;
        let logo = self.body.pictures().first().copied().or_else(|| self.pictures.first().map(|p| p.id));
        self.set_body(details.text(logo))?;
        self.layout = match (logo.is_some(), self.layout) {
            (false, _) => Layout::Inline,
            (true, Layout::Inline) => Layout::PictureLeft,
            (true, chosen) => chosen,
        };
        details.made = text_hash(&self.body);
        self.details = Some(details);
        Ok(())
    }

    /// Whether there is nothing in it.
    pub fn is_empty(&self) -> bool {
        self.body.is_empty() && self.pictures.is_empty() && self.html.is_none()
    }

    /// Bytes of its pictures together.
    pub fn picture_bytes(&self) -> usize {
        self.pictures.iter().map(|p| p.data.len()).sum()
    }

    /// Whether `data` may be added (or kept, `replacing` one of its own): a picture of a kind
    /// that is kept, within every cap. Why not, in words, when it may not.
    fn check(&self, data: &[u8], replacing: Option<u32>) -> Result<&'static str, ModelError> {
        let mime = picture_mime(data)
            .ok_or_else(|| ModelError::Invalid("a signature's pictures are PNG, JPEG or GIF (many mail programs can't show other kinds)".into()))?;
        if data.len() > MAX_PICTURE_BYTES {
            return Err(ModelError::Invalid(format!("a picture in a signature is at most {} KB", MAX_PICTURE_BYTES / 1024)));
        }
        let others: Vec<&Picture> = self.pictures.iter().filter(|p| Some(p.id) != replacing).collect();
        if others.len() >= MAX_PICTURES {
            return Err(ModelError::Invalid(format!("a signature holds at most {MAX_PICTURES} pictures")));
        }
        let total = others.iter().map(|p| p.data.len()).sum::<usize>().saturating_add(data.len());
        if total > MAX_PICTURES_BYTES {
            return Err(ModelError::Invalid(format!("a signature's pictures are at most {} KB together", MAX_PICTURES_BYTES / 1024)));
        }
        Ok(mime)
    }

    /// Add a picture at text position `at` (the end when it is past it): its id.
    pub fn add_picture(&mut self, name: &str, data: Vec<u8>, alt: Option<&str>, at: Option<usize>) -> Result<u32, ModelError> {
        if self.html.is_some() {
            return Err(ModelError::Invalid("an HTML signature shows pictures from their web address: switch it to rich text to add one".into()));
        }
        let mime = self.check(&data, None)?;
        let id = self.pictures.iter().map(|p| p.id).max().map_or(1, |n| n.saturating_add(1));
        let name: String = name.chars().filter(|c| !c.is_control()).take(255).collect();
        // By default the file's name without its extension describes it.
        let alt = alt.map_or_else(|| name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem).replace(['_', '-'], " "), str::to_string);
        let at = at.unwrap_or(usize::MAX).min(self.body.len());
        let mut text = self.body.clone();
        let fragment = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(id, Style::default())], Default::default())]);
        text.insert_rich(at, &fragment)?;
        self.pictures.push(Picture { id, name, mime: mime.to_string(), data: Arc::new(data), width: None, alt: clean_alt(&alt) });
        self.body = text;
        Ok(id)
    }

    /// Change the width a picture is shown at (`Some(None)`: its own) and its description.
    pub fn set_picture(&mut self, id: u32, width: Option<Option<u16>>, alt: Option<&str>) -> Result<(), ModelError> {
        let p = self.pictures.iter_mut().find(|p| p.id == id).ok_or_else(|| ModelError::Invalid(format!("no picture {id} in this signature")))?;
        if let Some(width) = width {
            p.width = width.map(|w| w.clamp(1, MAX_WIDTH));
        }
        if let Some(alt) = alt {
            p.alt = clean_alt(alt);
        }
        Ok(())
    }

    /// Take a picture out, and its place in the text.
    pub fn remove_picture(&mut self, id: u32) -> Result<(), ModelError> {
        if self.picture(id).is_none() {
            return Err(ModelError::Invalid(format!("no picture {id} in this signature")));
        }
        let paragraphs = self
            .body
            .paragraphs()
            .iter()
            .map(|p| Paragraph::new(p.runs.iter().filter(|r| r.picture != Some(id)).cloned().collect(), p.list).at_level(p.level))
            .collect();
        self.body = RichText::from_paragraphs(paragraphs);
        self.tidy();
        Ok(())
    }

    /// The text and the pictures agree: no run names a picture that isn't here, and no picture is
    /// kept that the text doesn't hold.
    fn tidy(&mut self) {
        let held: std::collections::HashSet<u32> = self.pictures.iter().map(|p| p.id).collect();
        if self.body.pictures().iter().any(|id| !held.contains(id)) {
            let paragraphs = self
                .body
                .paragraphs()
                .iter()
                .map(|p| {
                    let runs = p.runs.iter().filter(|r| r.picture.is_none_or(|id| held.contains(&id))).cloned().collect();
                    Paragraph::new(runs, p.list).at_level(p.level)
                })
                .collect();
            self.body = RichText::from_paragraphs(paragraphs);
        }
        let used: std::collections::HashSet<u32> = self.body.pictures().into_iter().collect();
        self.pictures.retain(|p| used.contains(&p.id));
    }
}

#[cfg(test)]
mod tests;
