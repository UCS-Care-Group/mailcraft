// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Laid-out pages written as a PDF: each face as a CID font of the glyphs used, pictures as
//! image XObjects, links as link annotations.

use pdf_writer::types::{ActionType, AnnotationType, CidFontType, FontFlags, LineCapStyle, SystemInfo, TextRenderingMode, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use crate::Options;
use crate::font::FaceSet;
use crate::layout::{Op, Page};
use crate::picture::{Data, Pictures, deflate};
use mailcraft_html::{LineStyle, Rgb};

struct Ids(i32);

impl Ids {
    fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

fn rgb(c: Rgb) -> (f32, f32, f32) {
    (f32::from(c.0) / 255.0, f32::from(c.1) / 255.0, f32::from(c.2) / 255.0)
}

/// The two bytes of each glyph id, as `Identity-H` reads them.
fn codes(glyphs: &[u16]) -> Vec<u8> {
    glyphs.iter().flat_map(|g| g.to_be_bytes()).collect()
}

/// A six-letter subset tag for face `n`, as PDF names subset fonts.
fn tag(n: usize) -> String {
    let mut s = String::with_capacity(7);
    let mut v = n;
    for _ in 0..6 {
        s.push(char::from(b'A' + (v % 26) as u8));
        v /= 26;
    }
    s.push('+');
    s
}

pub(crate) fn write(pages: &[Page], faces: &mut FaceSet, pictures: &Pictures, options: &Options) -> Vec<u8> {
    let mut pdf = Pdf::new();
    let mut ids = Ids(0);
    let catalog = ids.next();
    let tree = ids.next();
    let info = ids.next();
    let paper = options.paper;

    let shown: Vec<bool> =
        (0..faces.used.len()).map(|n| pages.iter().any(|p| p.ops.iter().any(|op| matches!(op, Op::Text { face, .. } if *face == n)))).collect();
    let font_refs: Vec<Option<Ref>> = shown.iter().map(|s| s.then(|| ids.next())).collect();
    let picture_refs: Vec<Ref> = pictures.list.iter().map(|_| ids.next()).collect();

    let mut page_refs = Vec::new();
    for page in pages {
        let page_ref = ids.next();
        let content_ref = ids.next();
        page_refs.push(page_ref);
        let mut content = Content::new();
        let mut annotations = Vec::new();
        let mut faces_used = Vec::new();
        let mut pictures_used = Vec::new();
        for op in &page.ops {
            match op {
                Op::Text { x, y, face, size, glyphs, color, italic, fake_bold, .. } => {
                    let (r, g, b) = rgb(*color);
                    let name = format!("F{face}");
                    if !faces_used.contains(face) {
                        faces_used.push(*face);
                    }
                    content.begin_text();
                    content.set_fill_rgb(r, g, b);
                    if *fake_bold {
                        content.set_stroke_rgb(r, g, b);
                        content.set_line_width(size * 0.035);
                        content.set_text_rendering_mode(TextRenderingMode::FillStroke);
                    } else {
                        content.set_text_rendering_mode(TextRenderingMode::Fill);
                    }
                    content.set_font(Name(name.as_bytes()), *size);
                    let skew = if *italic { 0.2 } else { 0.0 };
                    content.set_text_matrix([1.0, 0.0, skew, 1.0, *x, paper.height - y]);
                    content.show(Str(&codes(glyphs)));
                    content.end_text();
                }
                Op::Rect { x, y, w, h, color } => {
                    let (r, g, b) = rgb(*color);
                    content.set_fill_rgb(r, g, b);
                    content.rect(*x, paper.height - y - h, *w, *h);
                    content.fill_nonzero();
                }
                Op::Line { x1, y1, x2, y2, width, color, style } => {
                    let (r, g, b) = rgb(*color);
                    content.save_state();
                    content.set_stroke_rgb(r, g, b);
                    content.set_line_width(*width);
                    match style {
                        LineStyle::Solid => {}
                        LineStyle::Dashed => {
                            content.set_dash_pattern([width * 3.0, width * 2.0], 0.0);
                        }
                        LineStyle::Dotted => {
                            content.set_line_cap(LineCapStyle::RoundCap);
                            content.set_dash_pattern([0.0, width * 2.0], 0.0);
                        }
                    }
                    content.move_to(*x1, paper.height - y1);
                    content.line_to(*x2, paper.height - y2);
                    content.stroke();
                    content.restore_state();
                }
                Op::Image { index, x, y, w, h } => {
                    if pictures.list.get(*index).is_none() {
                        continue;
                    }
                    if !pictures_used.contains(index) {
                        pictures_used.push(*index);
                    }
                    let name = format!("I{index}");
                    content.save_state();
                    content.transform([*w, 0.0, 0.0, *h, *x, paper.height - y - h]);
                    content.x_object(Name(name.as_bytes()));
                    content.restore_state();
                }
                Op::Link { x, y, w, h, url } => {
                    let annotation = ids.next();
                    pdf.annotation(annotation)
                        .subtype(AnnotationType::Link)
                        .rect(Rect::new(*x, paper.height - y - h, x + w, paper.height - y))
                        .border(0.0, 0.0, 0.0, None)
                        .action()
                        .action_type(ActionType::Uri)
                        .uri(Str(url.as_bytes()));
                    annotations.push(annotation);
                }
            }
        }
        let data = deflate(&content.finish());
        pdf.stream(content_ref, &data).filter(Filter::FlateDecode);
        let mut p = pdf.page(page_ref);
        p.parent(tree).media_box(Rect::new(0.0, 0.0, paper.width, paper.height)).contents(content_ref);
        if !annotations.is_empty() {
            p.annotations(annotations);
        }
        let mut resources = p.resources();
        {
            let mut fonts = resources.fonts();
            for face in &faces_used {
                if let Some(Some(r)) = font_refs.get(*face) {
                    fonts.pair(Name(format!("F{face}").as_bytes()), *r);
                }
            }
        }
        {
            let mut objects = resources.x_objects();
            for index in &pictures_used {
                if let Some(r) = picture_refs.get(*index) {
                    objects.pair(Name(format!("I{index}").as_bytes()), *r);
                }
            }
        }
        resources.finish();
        p.finish();
    }

    pdf.pages(tree).kids(page_refs.iter().copied()).count(i32::try_from(page_refs.len()).unwrap_or(i32::MAX));
    pdf.catalog(catalog).pages(tree);
    let mut doc_info = pdf.document_info(info);
    if !options.title.is_empty() {
        doc_info.title(TextStr(&options.title));
    }
    doc_info.producer(TextStr("MailCraft"));
    doc_info.finish();

    for (n, used) in faces.used.iter().enumerate() {
        let Some(Some(type0)) = font_refs.get(n).copied() else { continue };
        let cid = ids.next();
        let descriptor = ids.next();
        let file = ids.next();
        let cmap = ids.next();
        let base = format!("{}MailCraftFont{n}", tag(n));
        let scale = 1000.0 / used.units_per_em.max(1.0);
        let subset = subsetter::subset(used.face.data(), used.face.index(), &used.remapper).unwrap_or_default();

        pdf.type0_font(type0).base_font(Name(base.as_bytes())).encoding_predefined(Name(b"Identity-H")).descendant_font(cid).to_unicode(cmap);

        let system = SystemInfo { registry: Str(b"Adobe"), ordering: Str(b"Identity"), supplement: 0 };
        let mut font = pdf.cid_font(cid);
        font.subtype(if used.cff { CidFontType::Type0 } else { CidFontType::Type2 });
        font.base_font(Name(base.as_bytes())).system_info(system).font_descriptor(descriptor).default_width(0.0);
        if !used.cff {
            font.cid_to_gid_map_predefined(Name(b"Identity"));
        }
        let count = used.remapper.num_gids();
        let widths: Vec<f32> = (0..count).map(|g| used.advances.get(&g).copied().unwrap_or(0.0)).collect();
        if !widths.is_empty() {
            font.widths().consecutive(0, widths);
        }
        font.finish();

        let [x0, y0, x1, y1] = used.bbox;
        let mut d = pdf.font_descriptor(descriptor);
        d.name(Name(base.as_bytes()))
            .flags(FontFlags::SYMBOLIC)
            .bbox(Rect::new(x0 * scale, y0 * scale, x1 * scale, y1 * scale))
            .italic_angle(used.italic_angle)
            .ascent(used.ascent * scale)
            .descent(used.descent * scale)
            .cap_height(used.cap_height * scale)
            .stem_v(80.0);
        if used.cff {
            d.font_file3(file);
        } else {
            d.font_file2(file);
        }
        d.finish();

        let compressed = deflate(&subset);
        let mut stream = pdf.stream(file, &compressed);
        stream.filter(Filter::FlateDecode);
        if used.cff {
            stream.pair(Name(b"Subtype"), Name(b"OpenType"));
        }
        stream.finish();

        let mut unicode = UnicodeCmap::new(Name(b"MailCraft-UCS"), system);
        let mut mapped: Vec<(&u16, &String)> = used.text.iter().collect();
        mapped.sort_by_key(|(g, _)| **g);
        for (g, text) in mapped {
            unicode.pair_with_multiple(*g, text.chars());
        }
        pdf.cmap(cmap, &unicode.finish());
    }

    for (picture, r) in pictures.list.iter().zip(&picture_refs) {
        let (w, h) = (i32::try_from(picture.width).unwrap_or(1), i32::try_from(picture.height).unwrap_or(1));
        match &picture.data {
            Data::Jpeg { bytes, gray } => {
                let mut image = pdf.image_xobject(*r, bytes);
                image.filter(Filter::DctDecode);
                image.width(w).height(h).bits_per_component(8);
                image.color_space_name(Name(if *gray { b"DeviceGray" } else { b"DeviceRGB" }));
                image.finish();
            }
            Data::Raw { rgb, alpha } => {
                let mask = alpha.as_ref().map(|_| ids.next());
                let mut image = pdf.image_xobject(*r, rgb);
                image.filter(Filter::FlateDecode);
                image.width(w).height(h).bits_per_component(8).color_space_name(Name(b"DeviceRGB"));
                if let Some(m) = mask {
                    image.s_mask(m);
                }
                image.finish();
                if let (Some(m), Some(alpha)) = (mask, alpha) {
                    let mut s = pdf.image_xobject(m, alpha);
                    s.filter(Filter::FlateDecode);
                    s.width(w).height(h).bits_per_component(8).color_space_name(Name(b"DeviceGray"));
                    s.finish();
                }
            }
        }
    }
    pdf.finish()
}
