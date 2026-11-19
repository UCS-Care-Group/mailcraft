// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signatures (R4.5): making them, their pictures, and which one each account starts its
//! messages with. The model and its caps are `mailcraft_model::signature`; what goes into a
//! message is `compose.signature`.
//!
//! A signature is edited as a message is: `signature.open` makes it the text the editing commands
//! (`compose.type`, `format.*`, `edit.*`, `compose.undo`) work on, and `signature.save` puts it
//! back. While it is open, the picture and layout commands for it change the open one.

use std::collections::{HashMap, HashSet};

use mailcraft_mime_model::signature_import::{PictureSource, picture_source};
use mailcraft_model::RichBody;
use mailcraft_model::rich::{Paragraph, RichText, Run, Style};
use mailcraft_model::signature::{Layout, Signature, html_fits};
use mailcraft_model::{AccountKind, Id, clean_name};
use serde_json::{Value, json};

use super::{bad, done, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, SignatureEdit, p};

/// Most signatures one mailbox keeps.
const MAX_SIGNATURES: usize = 100;

/// A signature made from the owner's own HTML and the picture files the document names
/// (`signature.import`): its own document and its own door, beside this file's commands.
#[path = "signature_import.rs"]
mod signature_import;

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
        CommandSpec::new("signature.import", "Import HTML Signature", "File › Signatures", signature_import::run).params(
            r#"{"name": string, "html": string (the owner's own HTML, at most 64 KB; its inline styling and its tables are kept as written), "assets": [{"name": string (the file's name, as the document writes it), "data": base64}] (the pictures the document names: at most ten, each at most 256 KB, at most 512 KB together; empty when it names none)} — makes a new signature of it, each picture bound where the document puts it, opens no editor and starts no message; answers its name"#,
        ),
        CommandSpec::new("signature.html", "HTML Signature", "File › Signatures", html).params(
            r#"{"name"?: string (default: the open one), "html": string (the signature's own HTML, at most 64 KB; sent as written once cleaned of what runs or tracks; an HTML signature's own pictures stay bound by the cid: addresses it gives them, and one no longer named is let go when saved; every picture is a web address (http: or https:) or one of its own by its cid:, as signature.import makes them, and any other (a file's name or path, file:, a cid: it doesn't own) is refused where the HTML is kept: at signature.save, or at once when the signature is not open (one that owns pictures refuses a cid: it doesn't own at once too); converting a rich signature that has pictures is refused) | null (rich text again, of what the HTML made)} — makes it an HTML signature, or a rich one again"#,
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

/// That `html` may be kept in the mailbox: the email-compatibility checker read it and found no
/// error in it (its warnings never stop a save). Refused, with a text the window matches on,
/// while an error is left to fix or when the HTML could not be checked at all. The text is the
/// same for any count of errors, since it has no plural forms: the window says how many.
/// `signature.import` keeps such a document anyway, so that it can be fixed in the editor.
fn email_compatible(html: &str) -> Result<(), CmdError> {
    let findings = mailcraft_html::compat::check(html).map_err(|_| fail("this signature's HTML could not be checked for email compatibility"))?;
    if findings.iter().any(|f| f.severity == mailcraft_html::compat::Severity::Error) {
        return Err(fail("this signature has email compatibility errors to fix before it can be saved"));
    }
    Ok(())
}

/// That every picture `html` names is one `sig` can be sent with, each address judged as
/// `signature.import` judges a picture's ([`picture_source`]): a web address stays as it is, and a
/// `data:` one is the email-compatibility checker's (`OL-IMG-004`); a `cid:` must name one of the
/// signature's own pictures; anything else (a file's name or path, a `file:` address, another
/// scheme) is refused, since no file is kept beside a signature and the picture would be sent
/// broken. Checked where HTML is kept, as `email_compatible` is: at the save, and when HTML is
/// written straight to a signature that is not open. The window asks it of the open signature
/// (`SignatureEdit::signature`, whose pictures are those the save keeps and maybe more) before
/// Save and when its page is left, so that it says why the save would be refused. Reads nothing
/// and fetches nothing.
pub fn sendable_pictures(sig: &Signature, html: &str) -> Result<(), CmdError> {
    if pictures_named(&own_addresses(sig), html)?.local {
        return Err(fail(
            "this HTML names a picture that is neither on the web nor one of this signature's own: give it a web address (https://…), or name the picture's file relative to the HTML file and bring both in with Import HTML…",
        ));
    }
    Ok(())
}

/// The internal addresses of `sig`'s own pictures (without `cid:`), with the picture each names.
fn own_addresses(sig: &Signature) -> HashMap<String, u32> {
    sig.pictures().iter().map(|p| (mailcraft_mime_model::rich::internal_cid(p), p.id)).collect()
}

/// What the pictures of some HTML name ([`pictures_named`]).
struct Named {
    /// The signature's own pictures it names by their `cid:` addresses.
    own: HashSet<u32>,
    /// Whether it names a picture neither on the web, nor written into it, nor by `cid:`.
    local: bool,
}

/// The pictures `html` names, each address judged by [`picture_source`]. Refused when it can't be
/// read for its pictures, or names by `cid:` a picture `own` doesn't hold.
fn pictures_named(own: &HashMap<String, u32>, html: &str) -> Result<Named, CmdError> {
    let sources = mailcraft_html::image_sources(html).map_err(unlisted)?;
    let mut named = Named { own: HashSet::new(), local: false };
    for source in &sources {
        match picture_source(&source.src) {
            PictureSource::Web | PictureSource::Data => {}
            PictureSource::Cid(cid) => {
                let id = own.get(cid).ok_or_else(|| fail("this HTML names a picture this signature doesn't have"))?;
                named.own.insert(*id);
            }
            PictureSource::Local => named.local = true,
        }
    }
    Ok(named)
}

/// Why HTML whose pictures [`mailcraft_html::image_sources`] could not list is refused, saying
/// what to change where that is one thing.
fn unlisted(e: mailcraft_html::ImageSourceError) -> CmdError {
    match e {
        mailcraft_html::ImageSourceError::UnsupportedBase => {
            fail("this signature's HTML could not be read for its pictures: remove its <base> element")
        }
        mailcraft_html::ImageSourceError::TooManySources => {
            let n = mailcraft_html::MAX_IMAGE_SOURCES;
            failed!("this signature's HTML could not be read for its pictures: it may name at most {n} pictures", n)
        }
        _ => fail("this signature's HTML could not be read for its pictures"),
    }
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
    if let Some(html) = sig.html() {
        email_compatible(html)?;
        sendable_pictures(&sig, html)?;
    }
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
            mailcraft_mime_model::rich::from_html(html)
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
        // A sender address that had it starts with the account's (#797).
        for x in a.sender_addresses.iter_mut().filter(|x| x.signature.eq_ignore_ascii_case(&name)) {
            x.signature.clear();
        }
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
        for x in a.sender_addresses.iter_mut().filter(|x| x.signature.eq_ignore_ascii_case(&from)) {
            x.signature = to.clone();
        }
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

fn html(s: &mut Session, v: &Value) -> CmdResult {
    let html = match v.get("html") {
        Some(Value::Null) => None,
        Some(Value::String(h)) => Some(h.clone()),
        _ => return Err(bad("`html` is the signature's HTML, or null")),
    };
    let open = names_open(s, v);
    // Changed whole or not at all.
    let sig = named(s, v)?;
    // A signature that is not open is written straight to the mailbox: kept only as a save would be,
    // and first only when it fits, so that the checker never reads what can't be kept.
    if !open && let Some(h) = html.as_deref() {
        html_fits(h).map_err(|e| fail(e.to_string()))?;
        email_compatible(h)?;
        sendable_pictures(sig, h)?;
    }
    let mut changed = sig.clone();
    // An HTML signature that owns pictures (an imported one) is edited with them bound.
    if let Some(h) = html.as_deref()
        && changed.html().is_some()
        && !changed.pictures().is_empty()
    {
        let text = bound_stand_in(&changed, h)?;
        // Open in the editor, a picture the HTML no longer names is kept until the save, so that
        // an undo of the edit brings it back; written straight to the mailbox, it goes now.
        if open {
            changed.set_body_keeping_pictures(text.clone()).map_err(|e| fail(e.to_string()))?;
        } else {
            changed.set_body(text.clone()).map_err(|e| fail(e.to_string()))?;
        }
        changed.set_html(html).map_err(|e| fail(e.to_string()))?;
        *sig = changed;
        if let Some(e) = s.signature_edit.as_mut().filter(|_| open) {
            e.body.replace(text).map_err(|err| fail(err.to_string()))?;
        }
        return done();
    }
    // This legacy conversion builds a stand-in without picture bindings. Refuse rather than
    // silently lose the bytes; accepting HTML with pictures in the model does not make this
    // conversion lossless.
    if html.is_some() && !changed.pictures().is_empty() {
        return Err(fail("this HTML conversion would lose the signature's embedded pictures"));
    }
    changed.set_html(html.clone()).map_err(|e| fail(e.to_string()))?;
    // What a message being written shows of it: its words and look as rich text, or for HTML
    // with no words (a logo alone) its pictures' descriptions.
    let stand_in = html.as_deref().map(|h| {
        let rich = mailcraft_mime_model::rich::from_html(h);
        if rich.to_plain().trim().is_empty() { RichText::from_plain(mailcraft_html::to_text(h).trim()) } else { rich }
    });
    if let Some(text) = &stand_in {
        // New words let the HTML go, so it is set again (checked above, while the pictures were there).
        changed.set_body(text.clone()).map_err(|e| fail(e.to_string()))?;
        changed.set_html(html).map_err(|e| fail(e.to_string()))?;
    }
    *sig = changed;
    if let (Some(text), Some(e)) = (stand_in, s.signature_edit.as_mut().filter(|_| open)) {
        e.body.replace(text).map_err(|err| fail(err.to_string()))?;
    }
    done()
}

/// The text an HTML signature that owns pictures stands in with for `html`, made as
/// `signature.import` makes it: each picture `html` names by the internal address the signature
/// gives it (`cid:`, [`internal_cid`](mailcraft_mime_model::rich::internal_cid)) stands where it
/// is, as that picture. Refused when `html` names by `cid:` a picture the signature doesn't own,
/// or names one of its own where its text can't hold it (hidden, say): sent, either would be a
/// picture without its bytes. Its addresses are judged as [`sendable_pictures`] judges them; one
/// that names a file is taken here, so that the HTML can be typed, and refused by the save.
fn bound_stand_in(sig: &Signature, html: &str) -> Result<RichText, CmdError> {
    let own = own_addresses(sig);
    let named = pictures_named(&own, html)?.own;
    let text = mailcraft_mime_model::rich::from_html_with_pictures(html, &mut |img: &mailcraft_mime_model::rich::ImgRef| own.get(&img.cid).copied());
    let held: std::collections::HashSet<u32> = text.pictures().into_iter().collect();
    if !named.is_subset(&held) {
        return Err(fail("this HTML hides a picture of the signature where its text can't show it"));
    }
    Ok(text)
}

fn layout(s: &mut Session, v: &Value) -> CmdResult {
    let layout = p::req_str(v, "value").ok().and_then(Layout::parse).ok_or_else(|| bad("`value` is inline, pictureLeft or pictureAbove"))?;
    named(s, v)?.layout = layout;
    done()
}
