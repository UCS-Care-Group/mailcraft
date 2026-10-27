// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pictures in the text of a message being written (#547): Insert › Pictures, and the Ctrl+V of a
//! picture on the clipboard (the window passes it here as `data`).
//!
//! A picture is one character of the text (a picture run), so typing over it, Backspace and
//! Delete, cut and undo treat it as they do a letter. Its bytes stay in `Compose::pictures`; a
//! message in HTML sends them as inline parts the HTML refers to by `cid:` (`compose.rs`).

use std::collections::HashSet;

use mailcraft_model::pictures as model;
use mailcraft_model::pictures::{HEAVY_BYTES, MAX_PICTURE_BYTES};
use mailcraft_model::rich::{Paragraph, RichText, Run, Style};
use serde_json::{Value, json};

use super::format::position;
use super::{bad, fail};
use crate::{BodyFormat, CmdResult, CommandSpec, Session, p};

/// Most bytes of a picture read to be made smaller (more is refused before it is decoded).
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("insert.picture", "Pictures", "Insert › Illustrations", insert)
            .pure()
            .params(
                r#"{"path": string (a PNG, JPEG, GIF or WebP file on this computer; never a web address) | "data": base64 (the picture's bytes), "file"?: string (its name, for `data`), "alt"?: string (what it shows, in words), "width"?: number (pixels; shown at most this wide), "at"?: number (a position in the text; default: the caret, over the selection)} — puts the picture in the text; a message in HTML sends it as part of the message, a plain-text one says `[picture]`. A picture wider than 1920 pixels, or over 5 MB, is made smaller to fit (an animated one is not: it is refused when over the cap); at most 5 MB each, 20 MB and 30 pictures together. Answers {"id", "bytes" (all the message's pictures), "heavy"}"#,
            )
            .when(needs_picture),
    ]
}

/// Enabled while a message in HTML is being written (a signature has its own pictures).
fn needs_picture(s: &Session) -> Option<&'static str> {
    match &s.compose {
        None => Some("no message is being written"),
        Some(_) if s.signature_edit.is_some() => Some("a signature is open: add its pictures with signature.picture.add"),
        Some(c) if c.format == BodyFormat::Plain => Some("the message is plain text; switch it to HTML first"),
        Some(_) => None,
    }
}

fn insert(s: &mut Session, v: &Value) -> CmdResult {
    if let Some(reason) = needs_picture(s) {
        return Err(fail(reason));
    }
    let path = p::str(v, "path").map(str::trim).unwrap_or_default();
    let data = p::str(v, "data");
    if data.is_none() && ["http:", "https:", "ftp:", "//"].iter().any(|scheme| path.to_ascii_lowercase().starts_with(scheme)) {
        return Err(fail("pictures from the web are not downloaded: save the picture and add it from the file"));
    }
    // Refused before it is decoded, so that nothing large is made for nothing.
    if data.is_some_and(|d| d.len() > MAX_SOURCE_BYTES / 3 * 4 + 1024) {
        return Err(failed!("a picture in a message is at most {mb} MB", mb = MAX_PICTURE_BYTES >> 20));
    }
    let width = match v.get("width") {
        None | Some(Value::Null) => None,
        Some(_) => {
            Some(p::u64(v, "width").and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| bad("`width` is a number of pixels"))?)
        }
    };
    let at = position(v, "at")?;
    let (_, bytes) = super::source(&json!({"data": v.get("data"), "path": v.get("path")}))?;
    let file = p::str(v, "file").or_else(|| (!path.is_empty()).then_some(path)).unwrap_or("picture");
    // Made smaller when it needs to be (and can be); what it is called then has the extension of
    // what it is now.
    let (bytes, ext) = crate::picture_fit::fit(bytes);
    let renamed = ext.map(|ext| {
        let name = model::clean_name(file);
        format!("{}.{ext}", name.rsplit_once('.').map_or(name.as_str(), |(stem, _)| stem))
    });
    let file = renamed.as_deref().unwrap_or(file);
    let alt = p::str(v, "alt");
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    let (kept, seq) = (c.pictures.clone(), c.picture_seq);
    if let Some(at) = at {
        c.body.select(at, at).map_err(|e| fail(e.to_string()))?;
    }
    let held: HashSet<u32> = c.body.text().pictures().into_iter().collect();
    let id = model::add(&mut c.pictures, &mut c.picture_seq, &held, file, bytes, alt, width).map_err(|e| fail(e.to_string()))?;
    let run = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(id, Style::default())], Default::default())]);
    if let Err(e) = c.body.paste(&run) {
        (c.pictures, c.picture_seq) = (kept, seq);
        return Err(fail(e.to_string()));
    }
    c.dirty = true;
    let held: HashSet<u32> = c.body.text().pictures().into_iter().collect();
    let bytes: usize = c.pictures.iter().filter(|p| held.contains(&p.id)).map(|p| p.data.len()).sum();
    Ok(json!({"id": id, "bytes": bytes, "heavy": bytes > HEAVY_BYTES}))
}
