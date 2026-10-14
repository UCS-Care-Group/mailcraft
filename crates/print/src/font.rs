//! The fonts a print is set in: Inter (regular and semibold) and JetBrains Mono, the program's
//! own (`assets/fonts`, SIL Open Font License; see ATTRIBUTION.md).
//!
//! Measuring is done with the fonts' own advance widths, so that what is laid out here is what
//! the PDF shows. A character a font has no shape for is replaced by U+FFFD (or `?` where even
//! that is missing) when text is laid out ([`Font::clean`]), so the PDF and the preview agree on
//! what is printed.

use std::sync::OnceLock;

use ttf_parser::{Face, GlyphId};

/// Which of the fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontKind {
    Regular,
    Bold,
    Mono,
}

impl FontKind {
    pub const ALL: [FontKind; 3] = [FontKind::Regular, FontKind::Bold, FontKind::Mono];

    /// The font's PostScript name, as the PDF names it.
    pub fn postscript_name(self) -> &'static str {
        match self {
            FontKind::Regular => "Inter-Regular",
            FontKind::Bold => "Inter-SemiBold",
            FontKind::Mono => "JetBrainsMono-Regular",
        }
    }

    fn data(self) -> &'static [u8] {
        match self {
            FontKind::Regular => include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
            FontKind::Bold => include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
            FontKind::Mono => include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
        }
    }
}

/// A font read for measuring and for embedding.
pub struct Font {
    pub kind: FontKind,
    pub data: &'static [u8],
    face: Face<'static>,
    units: f32,
    replacement: char,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font").field("kind", &self.kind).finish()
    }
}

static FONTS: [OnceLock<Option<Font>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];

/// The font, read once. `None` only if the built-in font file couldn't be read, which a test
/// rules out; everything that measures then falls back to an estimate.
pub fn get(kind: FontKind) -> Option<&'static Font> {
    let slot = match kind {
        FontKind::Regular => &FONTS[0],
        FontKind::Bold => &FONTS[1],
        FontKind::Mono => &FONTS[2],
    };
    slot.get_or_init(|| Font::read(kind)).as_ref()
}

impl Font {
    fn read(kind: FontKind) -> Option<Font> {
        let data = kind.data();
        let face = Face::parse(data, 0).ok()?;
        let units = f32::from(face.units_per_em().max(16));
        let replacement = if face.glyph_index('\u{FFFD}').is_some() { '\u{FFFD}' } else { '?' };
        Some(Font { kind, data, face, units, replacement })
    }

    /// Whether the font has a shape for `c`.
    pub fn covers(&self, c: char) -> bool {
        self.face.glyph_index(c).is_some()
    }

    /// The glyph that prints `c` (the replacement's, or the empty glyph 0, when there is none).
    pub fn glyph(&self, c: char) -> u16 {
        self.face.glyph_index(c).or_else(|| self.face.glyph_index(self.replacement)).map_or(0, |g| g.0)
    }

    /// How wide glyph `g` is, in thousandths of the size (the PDF's unit for widths).
    pub fn glyph_width(&self, g: u16) -> f32 {
        f32::from(self.face.glyph_hor_advance(GlyphId(g)).unwrap_or(0)) * 1000.0 / self.units
    }

    /// How wide `text` is at `size` points.
    pub fn width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.glyph_width(self.glyph(c))).sum::<f32>() * size / 1000.0
    }

    /// The text with what can't be printed taken out: control characters go (a tab becomes a
    /// space), and a character without a shape becomes the replacement character.
    pub fn clean(&self, text: &str) -> String {
        text.chars()
            .filter_map(|c| match c {
                '\t' => Some(' '),
                c if c.is_control() => None,
                // Joiners, direction marks and the like have no width and no shape.
                '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' | '\u{00AD}' => None,
                c if self.covers(c) => Some(c),
                _ => Some(self.replacement),
            })
            .collect()
    }

    /// Above the baseline, in thousandths of the size.
    pub fn ascent(&self) -> f32 {
        f32::from(self.face.ascender()) * 1000.0 / self.units
    }

    /// Below the baseline (negative), in thousandths of the size.
    pub fn descent(&self) -> f32 {
        f32::from(self.face.descender()) * 1000.0 / self.units
    }

    /// The height of capitals, in thousandths of the size.
    pub fn cap_height(&self) -> f32 {
        f32::from(self.face.capital_height().unwrap_or(self.face.ascender().saturating_mul(7) / 10)) * 1000.0 / self.units
    }

    /// The box every glyph fits in, in thousandths of the size.
    pub fn bbox(&self) -> [f32; 4] {
        let r = self.face.global_bounding_box();
        let k = 1000.0 / self.units;
        [f32::from(r.x_min) * k, f32::from(r.y_min) * k, f32::from(r.x_max) * k, f32::from(r.y_max) * k]
    }

    pub fn glyph_count(&self) -> u16 {
        self.face.number_of_glyphs()
    }
}

/// How wide `text` is in `kind` at `size` (an estimate if the font couldn't be read).
pub fn width(kind: FontKind, text: &str, size: f32) -> f32 {
    match get(kind) {
        Some(f) => f.width(text, size),
        None => text.chars().count() as f32 * size * 0.55,
    }
}

/// `text` as it will be printed in `kind` (see [`Font::clean`]).
pub fn clean(kind: FontKind, text: &str) -> String {
    match get(kind) {
        Some(f) => f.clean(text),
        None => text.chars().filter(|c| !c.is_control()).collect(),
    }
}

/// `text` cut to fit `max` points at `size`, with an ellipsis when something was cut.
pub fn fit(kind: FontKind, text: &str, size: f32, max: f32) -> String {
    let text = clean(kind, text);
    if width(kind, &text, size) <= max {
        return text;
    }
    let ellipsis = "…";
    let room = max - width(kind, ellipsis, size);
    if room <= 0.0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0.0;
    for c in text.chars() {
        let w = width(kind, c.encode_utf8(&mut [0; 4]), size);
        if used + w > room {
            break;
        }
        used += w;
        out.push(c);
    }
    let trimmed = out.trim_end().to_string();
    trimmed + ellipsis
}
