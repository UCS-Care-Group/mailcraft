// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The faces text is printed in, how wide each character is in them, and which glyphs of each a
//! document uses (only those are embedded).

use std::collections::HashMap;
use std::sync::Arc;

use skrifa::instance::{LocationRef, Size};
use skrifa::{FontRef, MetadataProvider, Tag};
use subsetter::GlyphRemapper;

/// The largest font file that is taken: a collection of Chinese, Japanese and Korean faces is
/// about twenty megabytes, and Arial Unicode about the same.
pub const MAX_FACE_BYTES: usize = 48 * 1024 * 1024;

/// The most faces a document is printed in.
pub const MAX_FACES: usize = 16;

/// A font face: a font file's bytes, and which face of it when it is a collection.
#[derive(Clone)]
pub struct Face {
    data: Arc<[u8]>,
    index: u32,
}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Face").field("bytes", &self.data.len()).field("index", &self.index).finish()
    }
}

impl Face {
    /// The face `index` of a font file, when it is one this crate can measure and embed: TrueType
    /// or CFF outlines, within [`MAX_FACE_BYTES`]. Variable CFF2 faces and bitmap-only faces are
    /// refused.
    pub fn new(data: impl Into<Arc<[u8]>>, index: u32) -> Option<Face> {
        let data: Arc<[u8]> = data.into();
        if data.len() > MAX_FACE_BYTES {
            return None;
        }
        let font = FontRef::from_index(&data, index).ok()?;
        let glyf = font.table_data(Tag::new(b"glyf")).is_some();
        let cff = font.table_data(Tag::new(b"CFF ")).is_some();
        if !glyf && !cff {
            return None;
        }
        if font.metrics(Size::unscaled(), LocationRef::default()).units_per_em == 0 {
            return None;
        }
        // Embedding is subsetting: a face the subsetter can't read is no use on paper.
        subsetter::subset(&data, index, &GlyphRemapper::new()).ok()?;
        Some(Face { data, index })
    }

    fn font(&self) -> Option<FontRef<'_>> {
        FontRef::from_index(&self.data, self.index).ok()
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }

    pub(crate) fn index(&self) -> u32 {
        self.index
    }
}

/// The faces a document may be printed in. `regular` draws everything no other face is asked
/// for; `bold` and `mono` are used where the text asks for them, else `regular` is made bold or
/// stands in; `fallbacks` draw the characters the others lack (Chinese, Arabic…), first first.
#[derive(Clone, Debug)]
pub struct Fonts {
    pub regular: Face,
    pub bold: Option<Face>,
    pub mono: Option<Face>,
    pub fallbacks: Vec<Face>,
}

/// Which of the document's faces a run of text asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Style {
    Regular,
    Bold,
    Mono,
}

/// What is known of one face while a document is laid out and written.
pub(crate) struct Used {
    pub face: Face,
    pub units_per_em: f32,
    pub ascent: f32,
    pub descent: f32,
    pub cap_height: f32,
    pub italic_angle: f32,
    pub bbox: [f32; 4],
    pub cff: bool,
    glyphs: HashMap<char, Option<(u16, f32)>>,
    widths: HashMap<u16, f32>,
    pub remapper: GlyphRemapper,
    /// For each new glyph id, the text it stands for (copy, search).
    pub text: HashMap<u16, String>,
    /// For each new glyph id, its advance in thousandths of the font size.
    pub advances: HashMap<u16, f32>,
}

impl Used {
    fn of(face: Face) -> Option<Used> {
        let (units_per_em, ascent, descent, cap_height, italic_angle, bbox, cff) = {
            let font = face.font()?;
            let m = font.metrics(Size::unscaled(), LocationRef::default());
            let upm = f32::from(m.units_per_em);
            let bbox = m.bounds.map_or([0.0, m.descent, upm, m.ascent], |b| [b.x_min, b.y_min, b.x_max, b.y_max]);
            let cff = font.table_data(Tag::new(b"glyf")).is_none();
            (upm, m.ascent, m.descent, m.cap_height.unwrap_or(m.ascent * 0.7), m.italic_angle, bbox, cff)
        };
        Some(Used {
            face,
            units_per_em,
            ascent,
            descent,
            cap_height,
            italic_angle,
            bbox,
            cff,
            glyphs: HashMap::new(),
            widths: HashMap::new(),
            remapper: GlyphRemapper::new(),
            text: HashMap::new(),
            advances: HashMap::new(),
        })
    }

    /// The glyph of `c` and its advance in ems; `None` when the face has no glyph for it.
    fn glyph(&mut self, c: char) -> Option<(u16, f32)> {
        if let Some(known) = self.glyphs.get(&c) {
            return *known;
        }
        let found = self.face.font().and_then(|font| {
            let gid = font.charmap().map(c)?;
            let gid = u16::try_from(gid.to_u32()).ok().filter(|g| *g != 0)?;
            let advance = font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(gid.into()).unwrap_or(0.0);
            Some((gid, advance / self.units_per_em))
        });
        self.glyphs.insert(c, found);
        found
    }

    /// The advance of glyph `gid` in ems.
    fn width(&mut self, gid: u16) -> f32 {
        if let Some(w) = self.widths.get(&gid) {
            return *w;
        }
        let w = self
            .face
            .font()
            .and_then(|font| font.glyph_metrics(Size::unscaled(), LocationRef::default()).advance_width(gid.into()))
            .map_or(0.5, |a| a / self.units_per_em);
        self.widths.insert(gid, w);
        w
    }

    /// The new glyph id of `gid` in the embedded subset, noting what text it stands for.
    pub fn use_glyph(&mut self, gid: u16, text: &str) -> u16 {
        let new = self.remapper.remap(gid);
        let advance = self.width(gid) * 1000.0;
        self.advances.entry(new).or_insert(advance);
        if !text.is_empty() {
            self.text.entry(new).or_insert_with(|| text.to_string());
        }
        new
    }
}

/// The faces of a document while it is laid out: the ones asked for first, then the fallbacks.
pub(crate) struct FaceSet {
    pub used: Vec<Used>,
    regular: usize,
    bold: Option<usize>,
    mono: Option<usize>,
    fallbacks: Vec<usize>,
}

/// One character placed in a face: which face, which glyph, how far it advances (ems), and
/// whether the face is standing in for a bold one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Placed {
    pub face: usize,
    pub gid: u16,
    pub advance: f32,
    pub fake_bold: bool,
}

impl FaceSet {
    pub fn new(fonts: &Fonts) -> Option<FaceSet> {
        let mut used = vec![Used::of(fonts.regular.clone())?];
        let add = |face: &Face, used: &mut Vec<Used>| {
            if used.len() >= MAX_FACES {
                return None;
            }
            let u = Used::of(face.clone())?;
            used.push(u);
            Some(used.len() - 1)
        };
        let bold = fonts.bold.as_ref().and_then(|f| add(f, &mut used));
        let mono = fonts.mono.as_ref().and_then(|f| add(f, &mut used));
        let fallbacks = fonts.fallbacks.iter().filter_map(|f| add(f, &mut used)).collect();
        Some(FaceSet { used, regular: 0, bold, mono, fallbacks })
    }

    /// `c` in the face `style` asks for, else the first fallback that has it, else the
    /// asked-for face's missing glyph.
    pub fn place(&mut self, c: char, style: Style) -> Placed {
        let (first, fake_bold) = match style {
            Style::Regular => (self.regular, false),
            Style::Bold => self.bold.map_or((self.regular, true), |b| (b, false)),
            Style::Mono => (self.mono.unwrap_or(self.regular), false),
        };
        let order = std::iter::once(first).chain(std::iter::once(self.regular)).chain(self.fallbacks.iter().copied());
        for face in order {
            if let Some(used) = self.used.get_mut(face)
                && let Some((gid, advance)) = used.glyph(c)
            {
                let fake = fake_bold || (style == Style::Bold && face != first);
                return Placed { face, gid, advance, fake_bold: fake };
            }
        }
        let advance = self.used.get_mut(first).map_or(0.5, |u| u.width(0));
        Placed { face: first, gid: 0, advance, fake_bold }
    }

    /// The ascent and descent (ems, descent negative) of the face `style` asks for.
    pub fn extent(&self, style: Style) -> (f32, f32) {
        let face = match style {
            Style::Regular => self.regular,
            Style::Bold => self.bold.unwrap_or(self.regular),
            Style::Mono => self.mono.unwrap_or(self.regular),
        };
        self.used.get(face).map_or((0.8, -0.2), |u| (u.ascent / u.units_per_em, u.descent / u.units_per_em))
    }
}
