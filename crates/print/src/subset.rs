//! A TrueType font cut down to the glyphs a print uses, for embedding in the PDF.
//!
//! Glyph ids stay as they are (the PDF addresses glyphs by id): the outlines of every glyph that
//! isn't used are emptied, and the tables a PDF reader doesn't need (layout, names, the character
//! map) are left out. The parts of a composite glyph come along with it. The font is our own
//! (`assets/fonts`), but this reads it as carefully as anything else: every offset is checked.

use std::collections::BTreeSet;

/// Tables kept, in the order of their tags.
const KEEP: [&[u8; 4]; 10] = [b"OS/2", b"cvt ", b"fpgm", b"glyf", b"head", b"hhea", b"hmtx", b"loca", b"maxp", b"prep"];

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(at)?, *d.get(at.checked_add(1)?)?]))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    let b = d.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The font's tables: tag and bytes.
fn tables(font: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let count = usize::from(u16_at(font, 4)?);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let rec = 12usize.checked_add(i.checked_mul(16)?)?;
        let tag = font.get(rec..rec + 4)?;
        let offset = u32_at(font, rec + 8)? as usize;
        let length = u32_at(font, rec + 12)? as usize;
        let data = font.get(offset..offset.checked_add(length)?)?;
        out.push(([tag[0], tag[1], tag[2], tag[3]], data));
    }
    Some(out)
}

/// The glyphs a composite glyph is made of.
fn components(glyph: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    // Not a composite: no parts.
    let Some(contours) = u16_at(glyph, 0) else { return out };
    if (contours as i16) >= 0 {
        return out;
    }
    let mut at = 10usize;
    for _ in 0..64 {
        let (Some(flags), Some(id)) = (u16_at(glyph, at), u16_at(glyph, at + 2)) else { break };
        out.push(id);
        at += 4;
        // ARG_1_AND_2_ARE_WORDS
        at += if flags & 0x0001 != 0 { 4 } else { 2 };
        if flags & 0x0008 != 0 {
            at += 2; // WE_HAVE_A_SCALE
        } else if flags & 0x0040 != 0 {
            at += 4; // WE_HAVE_AN_X_AND_Y_SCALE
        } else if flags & 0x0080 != 0 {
            at += 8; // WE_HAVE_A_TWO_BY_TWO
        }
        // MORE_COMPONENTS
        if flags & 0x0020 == 0 {
            break;
        }
    }
    out
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        sum.wrapping_add(u32::from_be_bytes(b))
    })
}

/// The font with only `used` glyphs (and glyph 0, and the parts of composites) keeping their
/// outlines. `None` if the font isn't one this can cut (the whole font is embedded then).
pub fn subset(font: &[u8], used: &BTreeSet<u16>) -> Option<Vec<u8>> {
    let tables = tables(font)?;
    let find = |tag: &[u8; 4]| tables.iter().find(|(t, _)| t == tag).map(|(_, d)| *d);
    let head = find(b"head")?;
    let maxp = find(b"maxp")?;
    let loca = find(b"loca")?;
    let glyf = find(b"glyf")?;
    let long = u16_at(head, 50)? == 1;
    let count = usize::from(u16_at(maxp, 4)?);
    let offset = |g: usize| -> Option<usize> { if long { Some(u32_at(loca, g * 4)? as usize) } else { Some(usize::from(u16_at(loca, g * 2)?) * 2) } };
    let outline = |g: usize| -> Option<&[u8]> {
        let (a, b) = (offset(g)?, offset(g + 1)?);
        if b <= a {
            return Some(&[]);
        }
        glyf.get(a..b)
    };
    // Glyph 0 and what is used, then the parts of composites (a part may itself be one).
    let mut keep: BTreeSet<u16> = used.iter().copied().filter(|g| usize::from(*g) < count).collect();
    keep.insert(0);
    let mut todo: Vec<u16> = keep.iter().copied().collect();
    let mut steps = 0usize;
    while let Some(g) = todo.pop() {
        steps += 1;
        if steps > 100_000 {
            return None;
        }
        for part in components(outline(usize::from(g))?) {
            if usize::from(part) < count && keep.insert(part) {
                todo.push(part);
            }
        }
    }
    // The new outlines and where each starts (the long form of `loca`).
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity((count + 1) * 4);
    for g in 0..count {
        new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
        if keep.contains(&(g as u16)) {
            new_glyf.extend_from_slice(outline(g)?);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.extend_from_slice(&(new_glyf.len() as u32).to_be_bytes());
    let mut new_head = head.to_vec();
    // checkSumAdjustment is worked out at the end; `loca` is now the long form.
    new_head.get_mut(8..12)?.copy_from_slice(&[0, 0, 0, 0]);
    new_head.get_mut(50..52)?.copy_from_slice(&[0, 1]);

    let mut out_tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for tag in KEEP {
        let data = match tag {
            b"glyf" => new_glyf.clone(),
            b"loca" => new_loca.clone(),
            b"head" => new_head.clone(),
            _ => match find(tag) {
                Some(d) => d.to_vec(),
                None => continue,
            },
        };
        out_tables.push((*tag, data));
    }
    let n = out_tables.len() as u16;
    let mut power = 1u16;
    let mut log = 0u16;
    while power.saturating_mul(2) <= n {
        power *= 2;
        log += 1;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&n.to_be_bytes());
    out.extend_from_slice(&(power * 16).to_be_bytes());
    out.extend_from_slice(&log.to_be_bytes());
    out.extend_from_slice(&(n * 16 - power * 16).to_be_bytes());
    let mut at = 12 + usize::from(n) * 16;
    let mut head_at = None;
    for (tag, data) in &out_tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(at as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        if tag == b"head" {
            head_at = Some(at);
        }
        at += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &out_tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }
    if let Some(h) = head_at {
        let adjust = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out.get_mut(h + 8..h + 12)?.copy_from_slice(&adjust.to_be_bytes());
    }
    Some(out)
}
