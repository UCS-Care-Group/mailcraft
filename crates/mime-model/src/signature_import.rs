// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature imported as HTML its owner wrote, with the picture files it names kept beside it
//! (#R4.5).
//!
//! [`import`] takes the owner's document and the files it names and makes a
//! [`mailcraft_model::Signature`] of them: the document is kept byte for byte except each local
//! picture's `src`, which is written as the address the signature holds that picture under
//! ([`crate::rich::internal_cid`]); the picture's bytes are kept as one payload however many
//! places name it, and its file name (without folders) is the name it is shown by. The text kept
//! beside the HTML is the stand-in a message being written shows: the document read as rich text,
//! with a picture run in each place the picture really stands.
//!
//! Nothing here does I/O and nothing is fetched. An absolute `http:` or `https:` picture address
//! the HTML already has stays exactly as it is and adds no payload, under the reading pane's own
//! safe-address rule; every other scheme, a schemeless `//host` address, a `data:` picture, a
//! `cid:` reference and an absolute, drive-lettered, query-bearing, fragment-bearing or escaping
//! path is refused rather than bound to something it is not. A picture address is a relative
//! POSIX resource name: surrounding ASCII white space is trimmed, valid percent escapes are
//! decoded once into UTF-8, and the result is matched exactly (Unicode and case) against the
//! supplied assets' names. An asset's name is a filesystem label and is never percent-decoded; a
//! literal `%` in a file name is `%25` in the address.
//!
//! [`required_assets`] is the same reading without any file: the logical names a document needs, in
//! first-reference order, so an owner can be asked for each one before anything is read.
//!
//! [`picture_source`] is the same judgement of one address, for the signature once it is made: an
//! edit of its HTML may name a picture on the web or one of the signature's own by its `cid:`, and
//! nothing else names a picture that can be sent with it.
//!
//! The slice is deliberately narrow and refuses what it cannot keep: a picture the stand-in would
//! not hold where the owner put it (hidden by `display:none`, a `hidden` attribute, a size of two
//! pixels or less, or some layout this version does not read) is an error, not a picture quietly
//! dropped. Layout is always [`mailcraft_model::signature::Layout::Inline`], since an imported
//! document's own tables say where its pictures go. Quotas are the model's own
//! ([`MAX_HTML`](mailcraft_model::signature::MAX_HTML), [`MAX_TEXT`](mailcraft_model::signature::MAX_TEXT),
//! [`MAX_PICTURES`](mailcraft_model::signature::MAX_PICTURES), and the per-picture and total byte
//! caps), checked before anything is cloned and again as the document is written.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use mailcraft_html::{BlockKind, ImageSourceError, image_sources};
use mailcraft_model::rich::safe_link;
use mailcraft_model::signature::{Layout, MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES, MAX_PICTURES_BYTES, MAX_TEXT, picture_mime};
use mailcraft_model::{Signature, clean_name};

use crate::rich::{ImgRef, from_html_with_pictures, internal_cid};

/// Most bytes a supplied asset's name may have.
pub const MAX_ASSET_NAME: usize = 4096;

/// A picture file supplied beside the HTML: its relative resource name, and its bytes as they
/// are.
///
/// The name is a relative POSIX resource name, not a host path: folders are separated by `/`, no
/// component is empty or `..`, and it holds no backslash, `:`, `?`, `#`, NUL or other control
/// character. Leading or intermediate `.` components are normalized away. It is matched exactly
/// (Unicode and case), is never percent-decoded, and only its last component becomes the picture's
/// name in the signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub data: Vec<u8>,
}

/// Why a document and its picture files could not be made a signature. Says nothing of the
/// document's words, its addresses, a file's name or its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The signature's name is empty once cleaned.
    BadName,
    /// The HTML is longer than a signature's own HTML may be.
    HtmlTooLarge,
    /// The picture reader refused the HTML, for a reason of its own.
    Source(ImageSourceError),
    /// A supplied asset's name is not a relative POSIX resource name, or is over its cap.
    BadAssetName,
    /// Two supplied assets name the same resource once normalized.
    DuplicateAsset,
    /// A picture's address in the HTML is not a safe relative reference.
    BadReference,
    /// A picture's address names no supplied asset.
    MissingAsset,
    /// A supplied asset is not named by any picture.
    UnusedAsset,
    /// A signature holds at most [`MAX_PICTURES`] pictures.
    TooManyPictures,
    /// A supplied picture is over its own byte cap.
    PictureTooLarge,
    /// The supplied pictures are over their total byte cap.
    PicturesTooLarge,
    /// A supplied asset is not a PNG, JPEG or GIF by its own first bytes.
    UnsupportedPicture,
    /// Writing the pictures' addresses into the HTML made it longer than a signature's HTML may
    /// be.
    RewrittenHtmlTooLarge,
    /// The stand-in text the HTML makes is longer than a signature's text may be.
    TextTooLarge,
    /// The stand-in would not hold an imported picture where the owner put it, so the signature
    /// could not carry it: a hidden or otherwise unsupported picture layout.
    UnsupportedRepresentation,
    /// The model refused a step this importer had already checked.
    ModelRejected,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            ImportError::BadName => "a signature needs a non-empty name",
            ImportError::HtmlTooLarge => "the document is longer than a signature's HTML may be",
            ImportError::Source(e) => return write!(f, "the document's pictures could not be read: {e}"),
            ImportError::BadAssetName => "a picture's file name is not a relative resource name",
            ImportError::DuplicateAsset => "two pictures have the same file name",
            ImportError::BadReference => "a picture's address is not a safe relative reference",
            ImportError::MissingAsset => "a picture's address names no supplied file",
            ImportError::UnusedAsset => "a supplied file is named by no picture",
            ImportError::TooManyPictures => "a signature holds at most ten pictures",
            ImportError::PictureTooLarge => "a picture in a signature is too large",
            ImportError::PicturesTooLarge => "a signature's pictures are too large together",
            ImportError::UnsupportedPicture => "a picture must be a PNG, JPEG or GIF",
            ImportError::RewrittenHtmlTooLarge => "the document grows past what a signature's HTML may be",
            ImportError::TextTooLarge => "the document's text is longer than a signature's text may be",
            ImportError::UnsupportedRepresentation => "the document hides or drops a picture this version cannot keep",
            ImportError::ModelRejected => "the signature model refused the document",
        };
        f.write_str(text)
    }
}

impl std::error::Error for ImportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ImportError::Source(e) => Some(e),
            _ => None,
        }
    }
}

/// A signature of `html`, the owner's own document, with `assets` its local pictures.
///
/// Each local picture address in `html` must name exactly one asset, and every asset must be
/// named by at least one picture; a name may be repeated, and the same asset may be reached by
/// more than one spelling of its address, but one asset is always one payload. The HTML's bytes
/// are kept except those addresses, and the model's caps are checked before anything is cloned.
/// See the module documentation for what is refused.
pub fn import(name: &str, html: &str, assets: &[Asset]) -> Result<Signature, ImportError> {
    let name = clean_name(name).ok_or(ImportError::BadName)?;
    if html.len() > MAX_HTML {
        return Err(ImportError::HtmlTooLarge);
    }
    if assets.len() > MAX_PICTURES {
        return Err(ImportError::TooManyPictures);
    }

    // Every asset before any of it is cloned: its normalized name, its kind, its own size and the
    // size they share.
    let mut names: Vec<String> = Vec::with_capacity(assets.len());
    let mut total = 0usize;
    for asset in assets {
        let normalized = asset_name(&asset.name).ok_or(ImportError::BadAssetName)?;
        if asset.data.len() > MAX_PICTURE_BYTES {
            return Err(ImportError::PictureTooLarge);
        }
        total = total.saturating_add(asset.data.len());
        if total > MAX_PICTURES_BYTES {
            return Err(ImportError::PicturesTooLarge);
        }
        if picture_mime(&asset.data).is_none() {
            return Err(ImportError::UnsupportedPicture);
        }
        if names.iter().any(|n| n == &normalized) {
            return Err(ImportError::DuplicateAsset);
        }
        names.push(normalized);
    }

    // The picture elements the tokenizer really reads, in source order, and what each names.
    let sources = image_sources(html).map_err(ImportError::Source)?;
    let mut plan: Vec<(Range<usize>, usize)> = Vec::with_capacity(sources.len());
    let mut referenced = vec![false; assets.len()];
    let mut order: Vec<usize> = Vec::new();
    for source in &sources {
        // A picture on the web keeps its address and adds nothing; nothing is fetched.
        let Some(normalized) = classify_reference(&source.src)? else { continue };
        let index = names.iter().position(|n| *n == normalized).ok_or(ImportError::MissingAsset)?;
        plan.push((source.value_range.clone(), index));
        if let Some(seen) = referenced.get_mut(index)
            && !*seen
        {
            *seen = true;
            order.push(index);
        }
    }
    if referenced.iter().any(|seen| !seen) {
        return Err(ImportError::UnusedAsset);
    }

    // The pictures, while the signature has no HTML yet; the text is the stand-in and comes last.
    let mut signature = Signature::plain_text(name, "");
    let mut assigned: Vec<Option<u32>> = vec![None; assets.len()];
    for &index in &order {
        let asset = assets.get(index).ok_or(ImportError::ModelRejected)?;
        let file = basename(names.get(index).map_or("", String::as_str));
        let id = signature.add_picture(file, asset.data.clone(), None, None).map_err(|_| ImportError::ModelRejected)?;
        if let Some(slot) = assigned.get_mut(index) {
            *slot = Some(id);
        }
    }

    // The document with only those source values written as this signature's own addresses: every
    // other byte, in order, and no range overlapping another.
    let mut rewritten = String::with_capacity(html.len());
    let mut at = 0usize;
    for (range, index) in &plan {
        if range.start < at || range.end > html.len() || range.start >= range.end {
            return Err(ImportError::ModelRejected);
        }
        rewritten.push_str(html.get(at..range.start).ok_or(ImportError::ModelRejected)?);
        let id = assigned.get(*index).copied().flatten().ok_or(ImportError::ModelRejected)?;
        let picture = signature.picture(id).ok_or(ImportError::ModelRejected)?;
        rewritten.push_str("cid:");
        rewritten.push_str(&internal_cid(picture));
        at = range.end;
        if rewritten.len() > MAX_HTML {
            return Err(ImportError::RewrittenHtmlTooLarge);
        }
    }
    rewritten.push_str(html.get(at..).ok_or(ImportError::ModelRejected)?);
    if rewritten.len() > MAX_HTML {
        return Err(ImportError::RewrittenHtmlTooLarge);
    }

    // Only the addresses this signature made stand in the text for a picture; a `cid:` the owner
    // wrote, if one got this far, is not one of them.
    let claimed: HashMap<String, u32> = signature.pictures().iter().map(|p| (internal_cid(p), p.id)).collect();
    let stand_in = from_html_with_pictures(&rewritten, &mut |img: &ImgRef| claimed.get(&img.cid).copied());
    let present: HashSet<u32> = stand_in.pictures().into_iter().collect();

    // Every imported picture must stand where the owner put it, in the text and as a picture the
    // reading pane really reads: one hidden by its style, its `hidden` attribute, a size of two
    // pixels or less or an unsupported layout is refused here rather than dropped by `tidy`.
    let blocks = mailcraft_html::parse_text_own(&rewritten, safe_link);
    let mut meta: HashMap<u32, (Option<u16>, String)> = HashMap::new();
    for block in mailcraft_html::images(&blocks) {
        if let BlockKind::Image { src, width, .. } = &block.kind
            && let Some(id) = src_cid(src).and_then(|cid| claimed.get(cid))
            && !meta.contains_key(id)
        {
            meta.insert(*id, (*width, block.spans.iter().map(|s| s.text.as_str()).collect()));
        }
    }
    for id in assigned.iter().flatten() {
        if !present.contains(id) || !meta.contains_key(id) {
            return Err(ImportError::UnsupportedRepresentation);
        }
    }
    if stand_in.len() > MAX_TEXT {
        return Err(ImportError::TextTooLarge);
    }

    // A picture's size and its description come from the first place it really stands, never from
    // markup that only looks like a picture.
    for id in assigned.iter().flatten() {
        let Some((width, alt)) = meta.get(id) else { continue };
        signature.set_picture(*id, Some(*width), Some(alt.as_str())).map_err(|_| ImportError::ModelRejected)?;
    }
    signature.set_body(stand_in).map_err(|_| ImportError::ModelRejected)?;
    if signature.pictures().len() != order.len() {
        return Err(ImportError::ModelRejected);
    }
    signature.layout = Layout::Inline;
    signature.set_html(Some(rewritten)).map_err(|_| ImportError::RewrittenHtmlTooLarge)?;

    // Every address the document now gives a picture has its payload beside it.
    if let Some(own) = signature.html() {
        for picture in signature.pictures() {
            if !own.contains(&format!("cid:{}", internal_cid(picture))) {
                return Err(ImportError::ModelRejected);
            }
        }
    }
    Ok(signature)
}

/// The logical pictures `html` needs, as the resource names its owner must supply, in the order
/// the document first names them.
///
/// Nothing is read, fetched or written: this is the document's own reading of its picture
/// addresses, with no files behind them. Each name is the relative POSIX resource name [`import`]
/// matches an [`Asset`] by — surrounding ASCII white space trimmed, valid percent escapes decoded
/// once into UTF-8, `.` components gone, and the rest exactly as written (Unicode and case). It is
/// a name, never a host path and never the address as it appears, so a caller that asks its owner
/// for one file per name must not decode or resolve a name again.
///
/// An absolute `http:` or `https:` address is kept as it is and asks for nothing, under
/// [`mailcraft_html::is_safe_url`]; every other address is read as a local reference and refused
/// exactly as [`import`] refuses it. A name longer than [`MAX_ASSET_NAME`] is refused here, before
/// a file of that name would be asked for.
///
/// The names are distinct, in first-reference order, and at most [`MAX_PICTURES`]: a document that
/// needs more local pictures than a signature holds is refused before any of them is read.
pub fn required_assets(html: &str) -> Result<Vec<String>, ImportError> {
    if html.len() > MAX_HTML {
        return Err(ImportError::HtmlTooLarge);
    }
    let sources = image_sources(html).map_err(ImportError::Source)?;
    let mut needed: Vec<String> = Vec::with_capacity(sources.len().min(MAX_PICTURES));
    for source in &sources {
        let Some(name) = classify_reference(&source.src)? else { continue };
        if name.len() > MAX_ASSET_NAME {
            return Err(ImportError::BadReference);
        }
        if needed.iter().any(|n| n == &name) {
            continue;
        }
        if needed.len() >= MAX_PICTURES {
            return Err(ImportError::TooManyPictures);
        }
        needed.push(name);
    }
    Ok(needed)
}

/// What one picture address in a signature's HTML stands for ([`picture_source`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureSource<'a> {
    /// An absolute `http:` or `https:` address whose spelling is safe
    /// ([`mailcraft_html::is_safe_url`]): it stays as it is and adds no payload. Nothing here
    /// fetches it.
    Web,
    /// A `cid:` address, with the Content-ID it names (its scheme gone): a picture the message
    /// carries, so one the signature must own ([`internal_cid`]).
    Cid(&'a str),
    /// A `data:` address: a picture written into the HTML, which the email-compatibility checker
    /// refuses (`OL-IMG-004`).
    Data,
    /// Anything else: a file's relative name or path, an absolute path, a `file:` address, a
    /// `//host` address, another scheme, or a web address whose spelling is not safe. [`import`]
    /// reads a relative one as the name of a file it was given; once the signature is made, no
    /// file is beside it, so it names no picture the signature can send.
    Local,
}

/// What `src`, a picture's address in a signature's HTML, stands for, judged as [`import`] judges
/// it: trimmed of ASCII white space, and its scheme read in any letter case. The one reading of a
/// signature's picture addresses, for the importer and for an edit of the HTML afterwards. Reads
/// nothing and fetches nothing.
pub fn picture_source(src: &str) -> PictureSource<'_> {
    let src = trim_ascii(src);
    if is_web(src) {
        return if mailcraft_html::is_safe_url(src) { PictureSource::Web } else { PictureSource::Local };
    }
    if let Some(cid) = after_scheme(src, "cid:") {
        return PictureSource::Cid(cid);
    }
    if after_scheme(src, "data:").is_some() {
        return PictureSource::Data;
    }
    PictureSource::Local
}

/// What a picture address names, classified once for both [`import`] and [`required_assets`]:
/// `None` for an absolute `http:` or `https:` address whose safe spelling stays as it is
/// ([`PictureSource::Web`]), or the relative POSIX resource name the address is matched by.
///
/// The address is trimmed of ASCII white space before it is judged, as the importer has always
/// judged it. Every other refusal is [`ImportError::BadReference`], and this reads nothing.
fn classify_reference(src: &str) -> Result<Option<String>, ImportError> {
    match picture_source(src) {
        PictureSource::Web => Ok(None),
        // A web address whose spelling is not safe is refused, never read as a file's name.
        _ if is_web(trim_ascii(src)) => Err(ImportError::BadReference),
        _ => reference_name(src).map(Some).ok_or(ImportError::BadReference),
    }
}

/// What follows `scheme` (any letter case) at the start of `src`.
fn after_scheme<'a>(src: &'a str, scheme: &str) -> Option<&'a str> {
    src.get(..scheme.len()).filter(|part| part.eq_ignore_ascii_case(scheme)).and_then(|_| src.get(scheme.len()..))
}

/// Whether a picture address is one to the web, whose safe spelling stays as it is.
fn is_web(url: &str) -> bool {
    ["http:", "https:"].iter().any(|scheme| url.get(..scheme.len()).is_some_and(|part| part.eq_ignore_ascii_case(scheme)))
}

/// `s` without the ASCII white space around it.
fn trim_ascii(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii_whitespace())
}

/// An asset's name as it is matched: relative, with `.` components gone and nothing to escape it.
fn asset_name(name: &str) -> Option<String> {
    if name.is_empty() || name.len() > MAX_ASSET_NAME || !name_chars_ok(name) {
        return None;
    }
    normalize_components(name)
}

/// A picture address, read as a relative resource name: trimmed, percent-decoded once into UTF-8,
/// then normalized as an asset's name is.
fn reference_name(src: &str) -> Option<String> {
    let trimmed = trim_ascii(src);
    if trimmed.is_empty() || trimmed.starts_with('/') {
        return None;
    }
    let decoded = percent_decode(trimmed)?;
    if decoded.is_empty() || !name_chars_ok(&decoded) {
        return None;
    }
    normalize_components(&decoded)
}

/// Characters a resource name may not hold: controls (NUL among them), a backslash, a colon (a
/// scheme or a drive), a query or a fragment.
fn name_chars_ok(name: &str) -> bool {
    !name.chars().any(|c| c.is_control() || matches!(c, '\\' | ':' | '?' | '#'))
}

/// `path` with its `.` components gone: no component may be empty and none may climb out.
fn normalize_components(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" => return None,
            "." => {}
            ".." => return None,
            part => parts.push(part),
        }
    }
    let joined = parts.join("/");
    (!joined.is_empty()).then_some(joined)
}

/// `s` with each valid percent escape decoded once, as UTF-8. An escape that is malformed, that
/// does not decode to UTF-8, or that hides a path separator is refused.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b != b'%' {
            out.push(b);
            i += 1;
            continue;
        }
        let high = hex_value(*bytes.get(i + 1)?)?;
        let low = hex_value(*bytes.get(i + 2)?)?;
        let value = high.checked_mul(16)?.checked_add(low)?;
        if matches!(value, b'/' | b'\\') {
            return None;
        }
        out.push(value);
        i += 3;
    }
    String::from_utf8(out).ok()
}

/// The value of one hexadecimal digit.
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The last component of a normalized resource name: the name a picture is shown by.
fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// An internal `cid:` address's Content-ID, without its scheme.
fn src_cid(src: &str) -> Option<&str> {
    let src = src.trim_start();
    src.get(..4).filter(|scheme| scheme.eq_ignore_ascii_case("cid:")).and_then(|_| src.get(4..))
}
