// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signatures (R4.5): making them, their pictures, and which one each account starts its
//! messages with. The model and its caps are `mailcraft_model::signature`; what goes into a
//! message is `compose.signature`.
//!
//! A signature is edited as a message is: `signature.open` makes it the text the editing commands
//! (`compose.type`, `format.*`, `edit.*`, `compose.undo`) work on, and `signature.save` puts it
//! back. While it is open, the picture and layout commands for it change the open one.

use mailcraft_model::RichBody;
use mailcraft_model::rich::{Paragraph, RichText, Run, Style};
use mailcraft_model::signature::{Layout, Signature};
use mailcraft_model::{AccountKind, Id, clean_name};
use serde_json::{Value, json};

use super::{bad, done, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, SignatureEdit, p};

/// Most signatures one mailbox keeps.
const MAX_SIGNATURES: usize = 100;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("signature.set", "Signatures", "File › Signatures", set).params(
            r#"{"name": string, "text"?: string (plain text) | "body"?: RichText | "html"?: string (pasted HTML, reduced to the set; pictures in it are not taken), "default"?: bool (use it for this account's new messages, and replies and forwards), "account"?: number} — makes the signature or replaces its text"#,
        ),
        CommandSpec::new("signature.remove", "Delete Signature", "File › Signatures", remove).params(r#"{"name": string}"#),
        CommandSpec::new("signature.rename", "Rename Signature", "File › Signatures", rename).params(r#"{"name": string, "to": string}"#),
        CommandSpec::new("signature.defaults", "Choose Default Signature", "File › Signatures", defaults).params(
            r#"{"account"?: number (default: the default account), "new"?: string ("" for none), "reply"?: string (replies and forwards; "" for none)}"#,
        ),
        CommandSpec::new("signature.picture.add", "Insert Picture", "File › Signatures", picture_add).params(
            r#"{"name": string (the signature), "data"?: base64 | "path"?: string (PNG, JPEG or GIF; never a web address), "file"?: string (its name), "alt"?: string, "at"?: number (a position in the text; default: the start)} — answers the picture's id"#,
        ),
        CommandSpec::new("signature.picture.set", "Picture Size and Description", "File › Signatures", picture_set)
            .params(r#"{"name": string, "id": number, "width"?: number (pixels, at most 600) | null (its own), "alt"?: string}"#),
        CommandSpec::new("signature.picture.remove", "Remove Picture", "File › Signatures", picture_remove)
            .params(r#"{"name": string, "id": number}"#),
        CommandSpec::new("signature.layout", "Picture Layout", "File › Signatures", layout)
            .params(r#"{"name": string, "value": "inline"|"pictureLeft"|"pictureAbove"}"#),
        CommandSpec::new("signature.details", "Create from Details", "File › Signatures", details).params(
            r#"{"name": string, "details": {"name"?, "title"?, "organisation"?, "phone"?, "mobile"?, "email"?, "website"?, "address"?: string}, "restart"?: bool (lay it out again after it was edited by hand)} — lays the signature out from the details (the logo is its first picture); after a hand edit only keeps them, unless restart"#,
        ),
        CommandSpec::new("signature.open", "Edit Signature", "File › Signatures", open)
            .pure()
            .params(r#"{"name": string} — the editing commands work on it until signature.close"#),
        CommandSpec::new("signature.save", "Save Signature", "File › Signatures", save).when(needs_edit),
        CommandSpec::new("signature.close", "Close Signature", "File › Signatures", close)
            .pure()
            .params(r#"{"discard"?: bool} — refused while it has changes, unless discard"#)
            .when(needs_edit),
    ]
}

fn needs_edit(s: &Session) -> Option<&'static str> {
    if s.signature_edit.is_none() { Some("no signature is open") } else { None }
}

/// Whether the params name the open signature (or name none while one is open).
fn names_open(s: &Session, v: &Value) -> bool {
    s.signature_edit.as_ref().is_some_and(|e| p::str(v, "name").is_none_or(|n| n.trim().eq_ignore_ascii_case(&e.signature.name)))
}

/// The signature `name` names (any letter case), to change: the open one when it is that.
fn named<'a>(s: &'a mut Session, v: &Value) -> Result<&'a mut Signature, CmdError> {
    if names_open(s, v)
        && let Some(e) = s.signature_edit.as_mut()
    {
        e.dirty = true;
        return Ok(&mut e.signature);
    }
    let name = p::req_str(v, "name")?.trim().to_string();
    s.mailbox.signatures.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)).ok_or_else(|| failed!("no signature named \"{name}\"", name))
}

/// The open signature's text is what `set` gave the signature of that name.
fn reopen(s: &mut Session, name: &str) {
    if let Some(e) = s.signature_edit.as_mut().filter(|e| e.signature.name.eq_ignore_ascii_case(name))
        && let Some(sig) = s.mailbox.signature(name)
    {
        *e = SignatureEdit { signature: sig.clone(), body: RichBody::new(sig.body().clone()), dirty: false };
    }
}

fn details(s: &mut Session, v: &Value) -> CmdResult {
    let given = v.get("details").filter(|d| d.is_object()).ok_or_else(|| bad("`details` is an object of the details"))?;
    let mut d: mailcraft_model::signature::Details =
        serde_json::from_value(given.clone()).map_err(|e| bad(format!("`details` is not the details: {e}")))?;
    let restart = p::bool(v, "restart") == Some(true);
    let open = names_open(s, v);
    let mut sig = match s.signature_edit.as_ref().filter(|_| open) {
        Some(e) => edited(e)?,
        None => named(s, v)?.clone(),
    };
    let laid = restart || !sig.hand_edited();
    if laid {
        sig.lay_out(d).map_err(|e| fail(e.to_string()))?;
    } else {
        // Kept for when the owner starts again from them; the text is the owner's.
        d.made = sig.details.as_ref().map_or(0, |old| old.made);
        sig.details = Some(d.clean());
    }
    let answer = json!({"laidOut": laid, "handEdited": sig.hand_edited()});
    match s.signature_edit.as_mut().filter(|_| open) {
        Some(e) => {
            if laid {
                e.body.replace(sig.body().clone()).map_err(|err| fail(err.to_string()))?;
            }
            e.signature = sig;
            e.dirty = true;
        }
        None => *named(s, v)? = sig,
    }
    Ok(answer)
}

fn open(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?.trim().to_string();
    if s.signature_edit.as_ref().is_some_and(|e| e.dirty && !e.signature.name.eq_ignore_ascii_case(&name)) {
        return Err(fail("save or close the signature being edited first"));
    }
    let sig = s.mailbox.signature(&name).cloned().ok_or_else(|| failed!("no signature named \"{name}\"", name))?;
    if s.signature_edit.as_ref().is_some_and(|e| e.signature.name == sig.name) {
        return done();
    }
    s.signature_edit = Some(SignatureEdit { body: RichBody::new(sig.body().clone()), signature: sig, dirty: false });
    done()
}

/// The open signature with the text it has now (pictures it no longer holds let go).
fn edited(e: &SignatureEdit) -> Result<Signature, CmdError> {
    let mut sig = e.signature.clone();
    sig.set_body(e.body.text().clone()).map_err(|err| fail(err.to_string()))?;
    Ok(sig)
}

fn save(s: &mut Session, _: &Value) -> CmdResult {
    let e = s.signature_edit.as_ref().ok_or_else(|| fail("no signature is open"))?;
    let sig = edited(e)?;
    let name = sig.name.clone();
    // (Removing or renaming the open signature closes or follows it, so it is there.)
    let kept =
        s.mailbox.signatures.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)).ok_or_else(|| fail("the signature is no longer there"))?;
    *kept = sig.clone();
    if let Some(e) = s.signature_edit.as_mut() {
        e.signature = sig;
        e.dirty = false;
    }
    Ok(json!({"name": name}))
}

fn close(s: &mut Session, v: &Value) -> CmdResult {
    if s.signature_edit.as_ref().is_some_and(|e| e.dirty) && p::bool(v, "discard") != Some(true) {
        return Err(fail("the signature has changes: save it, or close it with discard"));
    }
    s.signature_edit = None;
    done()
}

fn set(s: &mut Session, v: &Value) -> CmdResult {
    let name = clean_name(p::req_str(v, "name")?).ok_or_else(|| bad("`name` is empty"))?;
    let body = match (v.get("body"), p::str(v, "html"), p::str(v, "text")) {
        (Some(body), _, _) => serde_json::from_value::<RichText>(body.clone()).map_err(|e| bad(format!("`body` is not rich text: {e}")))?,
        (None, Some(html), _) => {
            if html.len() > super::format::MAX_PASTE {
                return Err(bad("`html` is too long"));
            }
            mailcraft_mime::rich::from_html(html)
        }
        (None, None, Some(text)) => Signature::plain_text("", text).body().clone(),
        (None, None, None) => return Err(bad("`text`, `body` or `html` is required")),
    };
    match s.mailbox.signatures.iter().position(|x| x.name.eq_ignore_ascii_case(&name)) {
        Some(i) => {
            if let Some(sig) = s.mailbox.signatures.get_mut(i) {
                sig.set_body(body).map_err(|e| fail(e.to_string()))?;
            }
        }
        None if s.mailbox.signatures.len() >= MAX_SIGNATURES => return Err(fail("there are too many signatures")),
        None => {
            let mut sig = Signature::plain_text(name.clone(), "");
            sig.set_body(body).map_err(|e| fail(e.to_string()))?;
            s.mailbox.signatures.push(sig);
        }
    }
    reopen(s, &name);
    if p::bool(v, "default") == Some(true) {
        let account = account_of(s, v)?;
        let a = s.mailbox.account_mut(account)?;
        a.signature = name.clone();
        a.reply_signature = Some(name);
    }
    done()
}

/// The account a command names, or the default one: one whose signatures can be chosen here.
fn account_of(s: &Session, v: &Value) -> Result<Id, CmdError> {
    let id = match p::id(v, "account") {
        Some(id) if s.mailbox.account(id).is_some() => id,
        Some(id) => return Err(failed!("no account with id {id}", id)),
        None => s.mailbox.default_account().map(|a| a.id).ok_or_else(|| fail("add an account first"))?,
    };
    // (Its settings are written back as its file had them: a choice made here would be lost.)
    match s.mailbox.account(id) {
        Some(a) if a.kind == AccountKind::Unknown => Err(super::account::added_by_newer(a, "which keeps its signatures: choose them there")),
        _ => Ok(id),
    }
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let name = p::req_str(v, "name")?.trim().to_string();
    let before = s.mailbox.signatures.len();
    s.mailbox.signatures.retain(|x| !x.name.eq_ignore_ascii_case(&name));
    if s.mailbox.signatures.len() == before {
        return Err(failed!("no signature named \"{name}\"", name));
    }
    if s.signature_edit.as_ref().is_some_and(|e| e.signature.name.eq_ignore_ascii_case(&name)) {
        s.signature_edit = None;
    }
    // (An account a newer MailCraft added keeps its own, as its file has them.)
    for a in s.mailbox.accounts.iter_mut().filter(|a| a.kind != AccountKind::Unknown) {
        // An account that had it for one kind of message has none there now; the other stays.
        let reply = a.reply_signature().to_string();
        if a.signature.eq_ignore_ascii_case(&name) {
            a.signature.clear();
        }
        a.reply_signature = Some(if reply.eq_ignore_ascii_case(&name) { String::new() } else { reply });
    }
    done()
}

fn rename(s: &mut Session, v: &Value) -> CmdResult {
    let to = clean_name(p::req_str(v, "to")?).ok_or_else(|| bad("`to` is empty"))?;
    let name = p::req_str(v, "name")?.trim().to_string();
    let from = s.mailbox.signature(&name).map(|x| x.name.clone()).ok_or_else(|| failed!("no signature named \"{name}\"", name))?;
    if !to.eq_ignore_ascii_case(&from) && s.mailbox.signature(&to).is_some() {
        return Err(failed!("there is already a signature named \"{to}\"", to));
    }
    let found =
        s.mailbox.signatures.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&from)).ok_or_else(|| failed!("no signature named \"{from}\"", from))?;
    found.name = to.clone();
    if let Some(e) = s.signature_edit.as_mut().filter(|e| e.signature.name.eq_ignore_ascii_case(&from)) {
        e.signature.name = to.clone();
    }
    for a in s.mailbox.accounts.iter_mut().filter(|a| a.kind != AccountKind::Unknown) {
        let reply = a.reply_signature().to_string();
        if a.signature.eq_ignore_ascii_case(&from) {
            a.signature = to.clone();
        }
        a.reply_signature = Some(if reply.eq_ignore_ascii_case(&from) { to.clone() } else { reply });
    }
    Ok(json!({"name": to}))
}

fn defaults(s: &mut Session, v: &Value) -> CmdResult {
    let account = account_of(s, v)?;
    // A name, or "" for none; a name has to be a signature's (and is kept as that one spells it).
    let pick = |key: &str| -> Result<Option<String>, CmdError> {
        match p::str(v, key).map(str::trim) {
            None => Ok(None),
            Some("") => Ok(Some(String::new())),
            Some(name) => s.mailbox.signature(name).map(|sig| Some(sig.name.clone())).ok_or_else(|| failed!("no signature named \"{name}\"", name)),
        }
    };
    let (new, reply) = (pick("new")?, pick("reply")?);
    if new.is_none() && reply.is_none() {
        return Err(bad("`new` or `reply` is required"));
    }
    let a = s.mailbox.account_mut(account)?;
    // Pinned before `signature` changes, so a reply that followed it keeps what it had.
    a.reply_signature = Some(reply.unwrap_or_else(|| a.reply_signature().to_string()));
    if let Some(new) = new {
        a.signature = new;
    }
    Ok(json!({"account": account, "new": a.signature, "reply": a.reply_signature()}))
}

fn picture_add(s: &mut Session, v: &Value) -> CmdResult {
    let path = p::str(v, "path").map(str::trim).unwrap_or_default();
    if p::str(v, "data").is_none() && ["http:", "https:", "ftp:", "//"].iter().any(|scheme| path.to_ascii_lowercase().starts_with(scheme)) {
        return Err(fail("pictures from the web are not downloaded: save the picture and add it from the file"));
    }
    let (_, bytes) = super::source(&json!({"data": v.get("data"), "path": v.get("path")}))?;
    let file = p::str(v, "file").or_else(|| (!path.is_empty()).then(|| path.rsplit(['/', '\\']).next().unwrap_or(path))).unwrap_or("picture");
    let at = p::u64(v, "at").and_then(|n| usize::try_from(n).ok());
    let alt = p::str(v, "alt");
    if names_open(s, v)
        && let Some(e) = s.signature_edit.as_mut()
    {
        // ponytail: the open one's pictures are first made what its text holds now, so a picture
        // deleted and then brought back by undo is gone; add it again.
        let mut sig = edited(e)?;
        let id = sig.add_picture(file, bytes, alt, None).map_err(|err| fail(err.to_string()))?;
        if let Some(at) = at {
            e.body.select(at, at).map_err(|err| fail(err.to_string()))?;
        }
        let run = RichText::from_paragraphs(vec![Paragraph::new(vec![Run::picture(id, Style::default())], Default::default())]);
        e.body.paste(&run).map_err(|err| fail(err.to_string()))?;
        let bytes = sig.picture_bytes();
        e.signature = sig;
        e.dirty = true;
        return Ok(json!({"id": id, "bytes": bytes, "heavy": bytes > mailcraft_model::signature::HEAVY_BYTES}));
    }
    let at = at.unwrap_or(0);
    let sig = named(s, v)?;
    let id = sig.add_picture(file, bytes, alt, Some(at)).map_err(|e| fail(e.to_string()))?;
    Ok(json!({"id": id, "bytes": sig.picture_bytes(), "heavy": sig.picture_bytes() > mailcraft_model::signature::HEAVY_BYTES}))
}

fn picture_set(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::u64(v, "id").and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("`id` is a picture's id"))?;
    let width = match v.get("width") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(_) => Some(Some(
            p::u64(v, "width").and_then(|n| u16::try_from(n).ok()).filter(|n| *n > 0).ok_or_else(|| bad("`width` is a number of pixels, or null"))?,
        )),
    };
    let alt = p::str(v, "alt");
    named(s, v)?.set_picture(id, width, alt).map_err(|e| fail(e.to_string()))?;
    done()
}

fn picture_remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::u64(v, "id").and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad("`id` is a picture's id"))?;
    if names_open(s, v)
        && let Some(e) = s.signature_edit.as_mut()
    {
        let places = picture_places(e.body.text(), id);
        if places.is_empty() {
            return Err(failed!("no picture {id} in this signature", id));
        }
        for at in places.into_iter().rev() {
            e.body.delete(at, at + 1).map_err(|err| fail(err.to_string()))?;
        }
        e.dirty = true;
        return done();
    }
    named(s, v)?.remove_picture(id).map_err(|e| fail(e.to_string()))?;
    done()
}

/// Where a picture stands in a text (each place it does).
pub(crate) fn picture_places(text: &RichText, id: u32) -> Vec<usize> {
    let mut at = 0;
    let mut places = Vec::new();
    for p in text.paragraphs() {
        for run in &p.runs {
            let n = run.text.chars().count();
            if run.picture == Some(id) {
                places.push(at);
            }
            at += n;
        }
        at += 1;
    }
    places
}

fn layout(s: &mut Session, v: &Value) -> CmdResult {
    let layout = p::req_str(v, "value").ok().and_then(Layout::parse).ok_or_else(|| bad("`value` is inline, pictureLeft or pictureAbove"))?;
    named(s, v)?.layout = layout;
    done()
}
