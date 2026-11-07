// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `mail.openAttachment` in the window (#126): an attachment opened with the computer's own
//! program for that kind of file, as Outlook's double-click and Open do.
//!
//! What the sender chose is never trusted: the kind of file decides first
//! ([`mailcraft_engine::opening::verdict`]): programs, scripts, shortcuts, installers, disk
//! images and files of no kind are not opened (the owner can save them); Office files with macros
//! and web pages are asked about first ([`question`]). The host writes the file into a folder of
//! MailCraft's own, under a name made safe, and asks the system to open it
//! ([`Services::open_file`](crate::Services::open_file)). The browser has no such thing: there the
//! file is downloaded instead. Agents can't ask for any of it (`agent::access`).
//!
//! `mail.copyAttachment` (#807) is here too: the same attachment, put on the clipboard as a file.

use mailcraft_engine::opening::{self, Verdict};
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::MailApp;

pub(crate) const OPEN: &str = "mail.openAttachment";
pub(crate) const COPY: &str = "mail.copyAttachment";

/// The attachment the params name: its message, its index and its name.
fn target(app: &MailApp, p: &Value) -> Result<(Id, usize, String), String> {
    // Read as the engine reads it (#553): named wrongly, it is refused, never taken for the selection.
    let id = mailcraft_engine::p::message_strict(p)
        .map_err(|e| e.to_string())?
        .or(app.session.sel.focus())
        .ok_or_else(|| tl!("Select a message first").to_string())?;
    let index = p.get("index").and_then(Value::as_u64).ok_or_else(|| "bad parameters: `index` (number) is required".to_string())?;
    let index = usize::try_from(index).map_err(|_| tl!("The message has no such attachment").to_string())?;
    let m = app.session.mailbox.message(id).ok_or_else(|| tl!("The message isn't there any more").to_string())?;
    let a = m.attachments.get(index).ok_or_else(|| tl!("The message has no such attachment").to_string())?;
    Ok((id, index, a.name.clone()))
}

/// What the owner is asked before an Office file with macros or a web page is opened; `None`
/// for everything else (it opens, or it is refused, without a question).
pub(crate) fn question(app: &MailApp, p: &Value) -> Option<(String, String, String)> {
    let (_, _, name) = target(app, p).ok()?;
    let shown = opening::file_name(&name);
    let why = match opening::verdict(&name) {
        Verdict::Macros => tl!("Files like this can carry macros, which run as programs do. Open it only if you know who sent it and expect it."),
        Verdict::Scripts => {
            tl!("Pages like this open in the browser, where scripts in them run. Open it only if you know who sent it and expect it.")
        }
        Verdict::Open | Verdict::Blocked => return None,
    };
    let text = crate::i18n::fmt(tl!("Open {name}? {why}"), &[("name", &shown), ("why", why)]);
    Some((tl!("Opening Mail Attachment").to_string(), text, tl!("Open").to_string()))
}

/// `mail.openAttachment` and `mail.copyAttachment` from the window, the control channel or a test.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    match id {
        OPEN => Some(open(app, p)),
        COPY => Some(copy(app, p)),
        _ => None,
    }
}

/// Copy (#807): the file put on the system's clipboard as a file manager puts the files it
/// copies, for one to paste, or a message being written. As Save As, any kind of file and under
/// the name Save As gives it (nothing is opened); the host writes it ([`Services::copy_file`],
/// [`crate::RichClipboard::put_files`]). The browser has no such clipboard: Copy says so.
///
/// [`Services::copy_file`]: crate::Services::copy_file
fn copy(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let (message, index, name) = target(app, p)?;
    // The control channel's clicks are no Copy of the owner's: the clipboard is never an agent's.
    if app.driven {
        return Err(tl!("Only you can copy an attachment, with its menu's Copy or Ctrl+C.").to_string());
    }
    let (Some(write), Some(board)) = (&app.services.copy_file, &app.services.clipboard) else {
        return Err(tl!("An attachment can't be copied here: save it with Save As instead.").to_string());
    };
    let bytes = app.session.content(message, mailcraft_engine::Part::Attachment(index)).map_err(|e| e.to_string())?;
    // The host names it, as Save As does ([`opening::save_name`]); said as it named it.
    let path = write(&name, bytes.as_slice())?;
    let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| opening::save_name(&name));
    board.put_files(std::slice::from_ref(&path))?;
    app.status(crate::i18n::fmt(tl!("Copied {name}"), &[("name", &file)]));
    Ok(json!({"name": file, "copied": true}))
}

fn open(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let (message, index, name) = target(app, p)?;
    if opening::verdict(&name) == Verdict::Blocked {
        return Err(opening::blocked_message(&name));
    }
    let file = opening::file_name(&name);
    let bytes = app.session.content(message, mailcraft_engine::Part::Attachment(index)).map_err(|e| e.to_string())?;
    if let Some(open_file) = &app.services.open_file {
        open_file(&file, bytes.as_slice())?;
        app.status(crate::i18n::fmt(tl!("Opening {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "opened": true}));
    }
    // The browser opens nothing of the computer's: the file is downloaded instead.
    if let Some(download) = &app.services.download {
        download(&file, bytes.as_slice())?;
        app.status(crate::i18n::fmt(tl!("Downloaded {name}"), &[("name", &file)]));
        return Ok(json!({"name": file, "downloaded": true}));
    }
    Err(tl!("Attachments can't be opened here: save it with Save As instead.").to_string())
}

#[cfg(test)]
mod tests;
