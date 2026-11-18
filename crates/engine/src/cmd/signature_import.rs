// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `signature.import` (R4.5): a signature made from the owner's own HTML and the picture files
//! the document names, both handed over as data.
//!
//! What the document and its pictures make is the importer's (`mailcraft_mime_model::signature_import`):
//! it keeps the HTML byte for byte but each local picture's `src`, makes one payload of a picture
//! that two places name, and checks the model's caps again as it writes. This module is the
//! command's door: it reads the parameters strictly — no field but the three it takes, no field of
//! an asset but its name and its bytes — and bounds every one of them *before* any of it is copied
//! or decoded, so a hostile caller cannot make it allocate what it would then refuse.
//!
//! Nothing here reads a file, asks a server, opens a dialog, opens a signature in the editor,
//! starts a message or changes a default. The signature is made whole and appended once, so a
//! refusal leaves the mailbox, the editor and the account's defaults exactly as they were.

use mailcraft_mime_model::signature_import::{Asset, MAX_ASSET_NAME};
use mailcraft_model::clean_name;
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES, MAX_PICTURES_BYTES};
use serde_json::{Value, json};

use crate::cmd::{bad, fail};
use crate::{CmdError, CmdResult, Session, p};

/// The fields `signature.import` takes, and no others.
const PARAMS: [&str; 3] = ["name", "html", "assets"];
/// The fields one asset of `assets` takes, and no others.
const ASSET_PARAMS: [&str; 2] = ["name", "data"];

/// Most base64 characters one picture's `data` may hold once its white space is gone: what
/// [`MAX_PICTURE_BYTES`] decodes from (`ceil(bytes / 3) * 4`), its padding counted.
const MAX_ENCODED: usize = MAX_PICTURE_BYTES.saturating_add(2) / 3 * 4;

/// Most bytes the `data` string itself may have. Room for the line breaks a MIME encoder puts in
/// it, which are no characters once white space is gone; never more than this is copied, and the
/// encoded characters are counted before the decoder copies them.
const MAX_DATA_BYTES: usize = MAX_ENCODED.saturating_mul(4);

/// A signature of the owner's document and the pictures it names: the ones it names and was given,
/// under a name of its own. Answers that name and nothing else.
pub(super) fn run(s: &mut Session, v: &Value) -> CmdResult {
    only(v, &PARAMS, "`signature.import` takes `name`, `html` and `assets`")?;
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let html = p::req_str(v, "html")?;
    let given = v
        .get("assets")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("`assets` is the list of the picture files the document names (it may be empty)"))?;
    // Every cap before any of it is copied: the document's own bytes, how many files there are,
    // and each file's name and its encoded length (and, once decoded, its own bytes and theirs
    // together). A caller that is over a cap is refused before the memory it asked for is taken.
    if html.len() > MAX_HTML {
        return Err(bad("`html` is longer than a signature's HTML may be (64 KB)"));
    }
    if given.len() > MAX_PICTURES {
        return Err(bad("`assets` holds more picture files than a signature may have pictures (ten)"));
    }
    let mut assets = Vec::with_capacity(given.len());
    let mut total = 0usize;
    for asset in given {
        assets.push(read_asset(asset, &mut total)?);
    }
    // A new name of its own, whatever its case: neither one the mailbox keeps nor the one open in
    // the editor. Nothing is overwritten or replaced here, dirty or clean.
    let taken = s.mailbox.signatures.iter().any(|x| x.name.eq_ignore_ascii_case(&name))
        || s.signature_edit.as_ref().is_some_and(|e| e.signature.name.eq_ignore_ascii_case(&name));
    if taken {
        return Err(failed!("there is already a signature named \"{to}\"", to = name));
    }
    if s.mailbox.signatures.len() >= super::MAX_SIGNATURES {
        return Err(fail("there are too many signatures"));
    }
    // Made whole first, then in the mailbox once: a refused document leaves no signature behind.
    let made = mailcraft_mime_model::signature_import::import(&name, html, &assets).map_err(|e| fail(e.to_string()))?;
    s.mailbox.signatures.push(made);
    Ok(json!({"name": name}))
}

/// One supplied picture file, decoded once its name and its encoded length are within a picture's
/// caps, and its bytes counted against the cap they share. `total` is what the files before it
/// came to, so the first one over the total is where it stops: no later file is decoded.
fn read_asset(asset: &Value, total: &mut usize) -> Result<Asset, CmdError> {
    only(asset, &ASSET_PARAMS, "an asset is a picture file's `name` and its `data`")?;
    let name = p::req_str(asset, "name")?;
    let data = p::req_str(asset, "data")?;
    if name.len() > MAX_ASSET_NAME {
        return Err(bad("a picture's file name is longer than a name may be"));
    }
    if data.len() > MAX_DATA_BYTES || encoded_len_over(data) {
        return Err(bad("a picture's `data` is longer than a picture may be (256 KB decoded)"));
    }
    let bytes = crate::cmd::base64_decode(data)?;
    if bytes.len() > MAX_PICTURE_BYTES {
        return Err(bad("a picture is larger than a signature's picture may be (256 KB)"));
    }
    *total = (*total).saturating_add(bytes.len());
    if *total > MAX_PICTURES_BYTES {
        return Err(bad("the pictures are larger than a signature's pictures may be together (512 KB)"));
    }
    Ok(Asset { name: name.to_string(), data: bytes })
}

/// Whether `data` holds more base64 characters than one picture decodes from, once the white
/// space a MIME encoder's line breaks leave in it is gone. Nothing is allocated for it, and only
/// a `data` already within [`MAX_DATA_BYTES`] is counted.
fn encoded_len_over(data: &str) -> bool {
    data.chars().filter(|c| !c.is_whitespace()).count() > MAX_ENCODED
}

/// That the fields of `v` are only those of `allowed`. A field a caller names and the command does
/// not take is refused rather than ignored — `path`, `default` and `account` would otherwise look
/// as if they did something. Only the field's name is said, bounded and cleaned of control
/// characters; what the field held is never read, never said and never logged.
fn only(v: &Value, allowed: &[&str], takes: &str) -> Result<(), CmdError> {
    let Some(map) = v.as_object() else {
        return Err(bad(format!("the parameters are an object, and {takes}")));
    };
    match map.keys().find(|k| !allowed.contains(&k.as_str())) {
        None => Ok(()),
        Some(key) => {
            let key: String = key.chars().filter(|c| !c.is_control()).take(40).collect();
            Err(bad(format!("`{key}` is no parameter here: {takes}")))
        }
    }
}
