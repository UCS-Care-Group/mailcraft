// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an attachment in the window (#126, #1281) with the computer's own
//! program for that kind of file, as Outlook's double-click and Open do.
//!
//! What the sender chose is never trusted: the kind of file decides first
//! ([`mailcraft_engine::opening::verdict`]): programs, scripts, shortcuts, installers, disk
//! images and files of no kind are not opened (the owner can save them); Office files with macros
//! and web pages are asked about first ([`question`]). The host writes the file into a folder of
//! MailCraft's own, under a name made safe, and asks the system to open it
//! ([`Services::open_file`](crate::Services::open_file)). The browser has no such thing: there the
//! file is downloaded instead. Agents can't ask for any of it (`agent::access`).

use mailcraft_engine::opening::{self, Verdict};
use mailcraft_model::{Attachment, Id};
use serde_json::{Value, json};

use crate::{MailApp, dialogs::Question};

pub(crate) const OPEN: &str = "mail.openAttachment";
pub(crate) const OPEN_COMPOSE: &str = "ui.openComposeAttachment";

fn index(p: &Value) -> Result<usize, String> {
    p.get("index")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| "bad parameters: `index` (number) is required".to_string())
}

fn compose_target<'a>(app: &'a MailApp, p: &Value) -> Result<&'a Attachment, String> {
    let compose = mailcraft_engine::p::id_strict(p, "compose")
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "bad parameters: `compose` (number) is required".to_string())?;
    let index = index(p)?;
    let part = app
        .session
        .composing_by_id(compose)
        .and_then(|c| c.attachments.get(index))
        .ok_or_else(|| tl!("The message has no such attachment").to_string())?;
    if p.get("attachmentName").is_some_and(|asked| asked.as_str() != Some(&part.name)) {
        return Err(tl!("The message has no such attachment").to_string());
    }
    Ok(part)
}

/// The attachment the params name: its message, its index and its name.
fn target(app: &MailApp, p: &Value) -> Result<(Id, usize, String), String> {
    // Read as the engine reads it (#553): named wrongly, it is refused, never taken for the selection.
    let id = mailcraft_engine::p::message_strict(p)
        .map_err(|e| e.to_string())?
        .or(app.session.sel.focus())
        .ok_or_else(|| tl!("Select a message first").to_string())?;
    let index = index(p)?;
    let m = app.session.mailbox.message(id).ok_or_else(|| tl!("The message isn't there any more").to_string())?;
    let a = m.attachments.get(index).ok_or_else(|| tl!("The message has no such attachment").to_string())?;
    Ok((id, index, a.name.clone()))
}

/// What the owner is asked before an Office file with macros or a web page is opened; `None`
/// for everything else (it opens, or it is refused, without a question).
pub(crate) fn question(app: &MailApp, id: &str, p: &Value) -> Option<Question> {
    let name = match id {
        OPEN => target(app, p).ok()?.2,
        OPEN_COMPOSE => compose_target(app, p).ok()?.name.clone(),
        _ => return None,
    };
    let shown = opening::file_name(&name);
    let why = match opening::verdict(&name) {
        Verdict::Macros => tl!("Files like this can carry macros, which run as programs do. Open it only if you know who sent it and expect it."),
        Verdict::Scripts => {
            tl!("Pages like this open in the browser, where scripts in them run. Open it only if you know who sent it and expect it.")
        }
        Verdict::Open | Verdict::Blocked => return None,
    };
    let text = crate::i18n::fmt(tl!("Open {name}? {why}"), &[("name", &shown), ("why", why)]);
    let with = if id == OPEN_COMPOSE { json!({"confirmed": true, "attachmentName": name}) } else { Value::Null };
    Some(Question::new(tl!("Opening Mail Attachment"), text, tl!("Open"), with))
}

/// Open the attachment named by a message or a draft.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    match id {
        OPEN => Some(open(app, p)),
        OPEN_COMPOSE => Some(open_compose(app, p)),
        _ => None,
    }
}

fn open(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let (message, index, name) = target(app, p)?;
    let file = safe_name(&name)?;
    let bytes = app.session.content(message, mailcraft_engine::Part::Attachment(index)).map_err(|e| e.to_string())?;
    launch(app, file, bytes.as_slice())
}

fn open_compose(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let part = compose_target(app, p)?;
    let file = safe_name(&part.name)?;
    let read = app.session.attachment_reader(part).map_err(|e| e.to_string())?;
    let bytes = read().map_err(|e| e.to_string())?;
    launch(app, file, bytes.as_slice())
}

fn safe_name(name: &str) -> Result<String, String> {
    if opening::verdict(name) == Verdict::Blocked {
        return Err(opening::blocked_message(name));
    }
    Ok(opening::file_name(name))
}

fn launch(app: &mut MailApp, file: String, bytes: &[u8]) -> Result<Value, String> {
    if let Some(open_file) = &app.services.open_file {
        open_file(&file, bytes)?;
        app.status(crate::i18n::fmt(tl!("Opening {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "opened": true}));
    }
    // The browser opens nothing of the computer's: the file is downloaded instead.
    if let Some(download) = &app.services.download {
        download(&file, bytes)?;
        app.status(crate::i18n::fmt(tl!("Downloaded {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "downloaded": true}));
    }
    Err(tl!("Attachments can't be opened here: save it with Save As instead.").to_string())
}

#[cfg(test)]
mod tests;
