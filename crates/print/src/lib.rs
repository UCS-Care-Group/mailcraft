// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Printing (#32): messages laid out on pages and written as a PDF, which the host prints or
//! saves. A message's display blocks (`mailcraft-html`) are laid out as the reading pane shows
//! them, in the light colours they were written in, under a header with its subject, sender,
//! recipients and date. Text is drawn in the faces given ([`Fonts`]), each embedded as the subset
//! of glyphs the document uses; a character none of them has prints as the face's missing glyph.
//! Pictures attached to the message (`cid:`) and written into it (`data:`) are printed; nothing is
//! fetched. Nothing here does I/O, so it builds for the browser too.
//!
//! Everything is capped: pages, pictures and their pixels, nesting.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod font;
mod layout;
mod pdf;
mod picture;

use std::sync::Arc;

pub use font::{Face, Fonts, MAX_FACE_BYTES, MAX_FACES};
pub use mailcraft_html::Block;

/// The faces MailCraft bundles (`assets/fonts`, ATTRIBUTION.md): Inter, regular and semibold,
/// and JetBrains Mono. The window draws its own text in the same bytes.
pub static INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
pub static INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
pub static INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");
pub static JETBRAINS_MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// The bundled faces to print in, with `fallbacks` behind them.
pub fn bundled(fallbacks: Vec<Face>) -> Option<Fonts> {
    Some(Fonts { regular: Face::new(INTER_REGULAR, 0)?, bold: Face::new(INTER_SEMIBOLD, 0), mono: Face::new(JETBRAINS_MONO, 0), fallbacks })
}

/// The most pages a document may have; what is past them is left out.
pub const MAX_PAGES: usize = 2000;

/// A sheet of paper, in points (1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    pub width: f32,
    pub height: f32,
}

impl Paper {
    /// ISO A4, 210 × 297 mm.
    pub const A4: Paper = Paper { width: 595.28, height: 841.89 };
    /// US Letter, 8.5 × 11 in.
    pub const LETTER: Paper = Paper { width: 612.0, height: 792.0 };

    /// The paper of a region (an ISO 3166 code, any case): Letter in the United States, Canada,
    /// Mexico and the other countries that use it, A4 everywhere else.
    pub fn for_region(region: &str) -> Paper {
        const LETTER_REGIONS: &[&str] = &["US", "CA", "MX", "PH", "CL", "CO", "VE", "GT", "PR", "CR", "PA", "DO", "SV", "NI", "BZ"];
        if LETTER_REGIONS.iter().any(|r| r.eq_ignore_ascii_case(region.trim())) { Paper::LETTER } else { Paper::A4 }
    }
}

/// One thing to print: a message, with its header lines.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    /// The heading over it: a message's subject.
    pub title: String,
    /// Label and value lines under the heading (From, Sent, To, Cc, Attachments), in their order.
    pub fields: Vec<(String, String)>,
    /// The body.
    pub blocks: Vec<Block>,
    /// The pictures attached to the message, by their content id without `<>` (any case), for
    /// the body's `cid:` pictures.
    pub pictures: Vec<(String, Arc<[u8]>)>,
}

/// How the pages look.
#[derive(Clone, Debug)]
pub struct Options {
    pub paper: Paper,
    /// The document's title in its properties.
    pub title: String,
    /// The line at the foot of each page, with `{n}` for its number and `{m}` for the count;
    /// empty for none.
    pub page_label: String,
}

impl Default for Options {
    fn default() -> Options {
        Options { paper: Paper::A4, title: String::new(), page_label: "{n} / {m}".into() }
    }
}

/// Why no PDF came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The regular face can't be read.
    Font,
    /// There is nothing to print.
    Empty,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Font => f.write_str("the font to print in can't be read"),
            Error::Empty => f.write_str("there is nothing to print"),
        }
    }
}

impl std::error::Error for Error {}

/// `docs` laid out on pages, each starting on a page of its own, as a PDF.
pub fn pdf(docs: &[Doc], fonts: &Fonts, options: &Options) -> Result<Vec<u8>, Error> {
    if docs.is_empty() {
        return Err(Error::Empty);
    }
    let mut faces = font::FaceSet::new(fonts).ok_or(Error::Font)?;
    let mut pictures = picture::Pictures::default();
    let pages = layout::pages(docs, &mut faces, &mut pictures, options);
    Ok(pdf::write(&pages, &mut faces, &pictures, options))
}

/// The text of each page of `docs` as it is laid out, line by line: what [`pdf`] puts on paper,
/// without the paper. For tests and for agents that want to check a print.
pub fn page_text(docs: &[Doc], fonts: &Fonts, options: &Options) -> Result<Vec<String>, Error> {
    if docs.is_empty() {
        return Err(Error::Empty);
    }
    let mut faces = font::FaceSet::new(fonts).ok_or(Error::Font)?;
    let mut pictures = picture::Pictures::default();
    let pages = layout::pages(docs, &mut faces, &mut pictures, options);
    Ok(pages.iter().map(layout::Page::text).collect())
}

#[cfg(test)]
mod tests;
