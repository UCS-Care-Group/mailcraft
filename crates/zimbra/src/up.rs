// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What was done here, taken up to the server: folders made, renamed, moved and deleted
//! (`CreateFolderRequest`, `FolderActionRequest`), messages marked, tagged, moved, copied and
//! deleted (`MsgActionRequest`), and what waits in the Outbox, sent (`SendMsgRequest`).
//!
//! **Folders.** A folder deleted here goes to the server's Trash (`trash`), as Zimbra's own web
//! client does; the server's own folders (ids below 256) are never asked to go, and are answered as
//! the server's own.
//!
//! **Messages** keep their ids when they move, so a move is answered with the same id in its new
//! folder. Read is `read`/`!read`, the flag `flag`/`!flag`, the categories the message's tags by name
//! (`update` with `tn`, which makes a tag that isn't there yet), a deletion for good `delete`. A
//! message's importance is part of what it says, not a mark of the server's, and stays as it is
//! there.
//!
//! **Sending.** The message is written as the RFC 5322 text it is here, without its `Bcc` field,
//! uploaded (`/service/upload?fmt=raw`, the token in its cookie) and sent by its upload's id
//! (`SendMsgRequest` with `m@aid`), with a send id of its own (`suid`) that the server knows a second
//! try of the same send by. The server files the copy in Sent. A message with Bcc recipients is held
//! for now, with a line that says so, rather than sent with them in its text or without them. The
//! send itself is asked **once**: an answer that never came holds the message as one that may have
//! gone.

use mailcraft_mime_model::WriteOptions;
use mailcraft_model::sync::{
    ChangeOutcome, FolderChange, FolderChangeDone, GraphFolder, MessageChange, MessageChangeDone, MessageOutcome, SendDone, SendOutcome, SendUp,
};
use mailcraft_model::{FlagStatus, FolderKind, Message};
use serde_json::{Value, json};

use crate::error::{Error, Fault};
use crate::http::{HttpRequest, HttpResponse, Method, Secret};
use crate::{folders, json, soap};

/// Most messages sent in one run.
pub const MAX_SENDS: usize = 50;
/// The largest message uploaded.
pub const MAX_UPLOAD: usize = 32 * 1024 * 1024;
/// Why a message with Bcc recipients is held.
pub const BCC_HELD: &str = "Zimbra accounts don't send to Bcc recipients yet: send it from another account, or without Bcc";
/// Why a message that couldn't be written is held.
pub const NOT_WRITTEN: &str = "the message couldn't be written to be sent";

/// The request that makes folder `name` in `parent` (the top when empty).
pub fn create_folder(base: &str, token: &Secret, name: &str, parent: &str) -> Result<HttpRequest, Error> {
    let parent = if parent.is_empty() { "1" } else { parent };
    let body = json!({"folder": {"name": name, "l": parent, "view": "message", "fie": "1"}});
    Ok(soap::request(base, "CreateFolderRequest", soap::MAIL, body, Some(token))?.once())
}

/// The request that does `op` to folder `id`, with `extra` (`name`, `l`) beside it.
pub fn folder_action(base: &str, token: &Secret, op: &str, id: &str, extra: Value) -> Result<HttpRequest, Error> {
    let mut action = json!({"op": op, "id": id});
    if let (Value::Object(action), Value::Object(extra)) = (&mut action, extra) {
        action.extend(extra);
    }
    soap::request(base, "FolderActionRequest", soap::MAIL, json!({"action": action}), Some(token))
}

/// The request that does `op` to message `id`, with `extra` beside it.
pub fn message_action(base: &str, token: &Secret, op: &str, id: &str, extra: Value) -> Result<HttpRequest, Error> {
    let mut action = json!({"op": op, "id": id});
    if let (Value::Object(action), Value::Object(extra)) = (&mut action, extra) {
        action.extend(extra);
    }
    soap::request(base, "MsgActionRequest", soap::MAIL, json!({"action": action}), Some(token))
}

/// The folder a `CreateFolderResponse` made, as the model lists one.
pub fn created(response: &HttpResponse, name: &str, parent: &str) -> Result<GraphFolder, Error> {
    let answer = soap::answer("CreateFolderRequest", response)?;
    let made = json::list(answer.get("folder")).first().cloned().ok_or(Error::Unreadable)?;
    let id = json::own_id(made.get("id")).ok_or(Error::Unreadable)?;
    let named = json::line(made.get("name"));
    Ok(GraphFolder {
        id,
        parent: parent.to_string(),
        name: if named.is_empty() { name.to_string() } else { named },
        kind: FolderKind::Custom,
        ..GraphFolder::default()
    })
}

/// Whether an action's answer says it was done.
pub fn acted(name: &str, response: &HttpResponse) -> Result<(), Error> {
    soap::answer(name, response).map(|_| ())
}

/// The error of an answer that ends the run, not only the change it was for: a sign-in the server
/// no longer takes, or a server that is busy.
pub fn stops(name: &str, response: &HttpResponse) -> Option<Error> {
    match soap::answer(name, response) {
        Err(e @ (Error::AuthRefused | Error::Busy(_))) => Some(e),
        _ => None,
    }
}

/// A server's own folder: one Zimbra makes in every mailbox.
fn own_folder(id: &str) -> bool {
    id.parse::<u64>().is_ok_and(|n| n < 256)
}

/// What a refused folder change is said as.
fn refused(e: Error) -> ChangeOutcome {
    match e {
        Error::NoSuchItem => ChangeOutcome::Gone,
        e => ChangeOutcome::Refused { why: e.to_string() },
    }
}

/// The request for a folder change, and how its answer is read; `None` for a change that needs no
/// request (the server's own folder asked to go), with its outcome.
pub fn folder_change(base: &str, token: &Secret, change: &FolderChange) -> Result<HttpRequest, ChangeOutcome> {
    let asked = match change {
        FolderChange::Create { name, parent, .. } => create_folder(base, token, name, parent),
        FolderChange::Rename { id, name, .. } => folder_action(base, token, "rename", id, json!({"name": name})),
        FolderChange::Move { id, parent, .. } => folder_action(base, token, "move", id, json!({"l": if parent.is_empty() { "1" } else { parent }})),
        FolderChange::Delete { id, .. } if own_folder(id) => {
            return Err(ChangeOutcome::OwnFolder { why: "the Zimbra server keeps its own folders".to_string() });
        }
        FolderChange::Delete { id, .. } => folder_action(base, token, "trash", id, json!({})),
    };
    asked.map_err(refused)
}

/// What became of a folder change, from its answer.
pub fn folder_done(change: &FolderChange, response: &HttpResponse) -> ChangeOutcome {
    let outcome = match change {
        FolderChange::Create { name, parent, .. } => created(response, name, parent).map(|folder| ChangeOutcome::Done { folder }),
        FolderChange::Rename { id, name, .. } => acted("FolderActionRequest", response).map(|()| ChangeOutcome::Done {
            folder: GraphFolder { id: id.clone(), name: name.clone(), kind: folders::kind_of(id), ..GraphFolder::default() },
        }),
        FolderChange::Move { id, parent, .. } => acted("FolderActionRequest", response).map(|()| ChangeOutcome::Done {
            folder: GraphFolder { id: id.clone(), parent: parent.clone(), kind: folders::kind_of(id), ..GraphFolder::default() },
        }),
        FolderChange::Delete { .. } => acted("FolderActionRequest", response).map(|()| ChangeOutcome::Gone),
    };
    outcome.unwrap_or_else(refused)
}

/// The requests a message change takes, in order (an update may take several: read, flag, tags).
pub fn message_requests(base: &str, token: &Secret, change: &MessageChange) -> Result<Vec<HttpRequest>, Error> {
    let mut out = Vec::new();
    match change {
        MessageChange::Update { id, read, flag, categories, .. } => {
            if let Some(read) = read {
                out.push(message_action(base, token, if *read { "read" } else { "!read" }, id, json!({}))?);
            }
            if let Some(flag) = flag {
                let on = flag.status != FlagStatus::None;
                out.push(message_action(base, token, if on { "flag" } else { "!flag" }, id, json!({}))?);
            }
            if let Some(categories) = categories {
                let names: Vec<String> =
                    categories.here.iter().map(|c| c.replace(',', " ").trim().to_string()).filter(|c| !c.is_empty()).take(50).collect();
                out.push(message_action(base, token, "update", id, json!({"tn": names.join(",")}))?);
            }
        }
        MessageChange::Move { id, to, .. } => out.push(message_action(base, token, "move", id, json!({"l": to}))?),
        MessageChange::Copy { id, to, .. } => out.push(message_action(base, token, "copy", id, json!({"l": to}))?.once()),
        MessageChange::Delete { id } | MessageChange::MovedOut { id, .. } => out.push(message_action(base, token, "delete", id, json!({}))?),
    }
    Ok(out)
}

/// What became of a message change whose requests were all answered `Ok`.
pub fn message_done(change: &MessageChange) -> MessageOutcome {
    match change {
        MessageChange::Update { flag, categories, .. } => MessageOutcome::Done {
            categories: categories.as_ref().map(|c| c.here.clone()),
            flag: flag.clone(),
            folder: String::new(),
            id: String::new(),
        },
        MessageChange::Move { id, to, .. } => MessageOutcome::Done { categories: None, flag: None, folder: to.clone(), id: id.clone() },
        MessageChange::Copy { to, .. } => MessageOutcome::Done { categories: None, flag: None, folder: to.clone(), id: String::new() },
        MessageChange::Delete { .. } | MessageChange::MovedOut { .. } => MessageOutcome::Gone,
    }
}

/// What a refused message change is said as.
pub fn message_refused(e: Error) -> MessageOutcome {
    match e {
        Error::NoSuchItem => MessageOutcome::Gone,
        Error::Fault(Fault::PermissionDenied) => MessageOutcome::Refused { status: 403 },
        _ => MessageOutcome::Refused { status: 400 },
    }
}

/// A done change of a folder, as a piece carries it.
pub fn folder_change_done(change: &FolderChange, outcome: ChangeOutcome) -> FolderChangeDone {
    FolderChangeDone { change: change.clone(), outcome }
}

/// A done change of a message, as a piece carries it.
pub fn message_change_done(change: &MessageChange, outcome: MessageOutcome) -> MessageChangeDone {
    MessageChangeDone { change: change.clone(), outcome, there: None }
}

/// The text `message` is sent as: without its Bcc field, and without the marks that are the
/// mailbox's alone, so that it is the same octets at every run.
pub fn written(message: &Message) -> Vec<u8> {
    let message = Message { read: false, answered: false, flag: FlagStatus::None, ..message.clone() };
    mailcraft_mime_model::write(&message, &WriteOptions { include_bcc: false, offset_minutes: 0 })
}

/// The upload of a message's text.
pub fn upload(base: &str, token: &Secret, mime: Vec<u8>) -> Result<HttpRequest, Error> {
    let url = format!("{}/service/upload?fmt=raw", base.trim_end_matches('/'));
    let cookie = Secret::new(&format!("ZM_AUTH_TOKEN={}", token.expose()));
    let fields = [("Cookie", cookie.expose()), ("Content-Type", "message/rfc822"), ("Content-Disposition", "attachment; filename=\"message.eml\"")];
    Ok(HttpRequest::new(Method::Post, &url, &fields, mime)?.with_response_limit(64 * 1024))
}

/// The upload's id in what the upload servlet answers with `fmt=raw`: `200,'null','<id>'`.
pub fn uploaded(response: &HttpResponse) -> Result<String, Error> {
    if matches!(response.status(), 401 | 403) {
        return Err(Error::AuthRefused);
    }
    if response.status() != 200 {
        return Err(Error::Status(response.status()));
    }
    let text = String::from_utf8_lossy(response.body());
    let mut parts = text.trim().splitn(3, ',');
    let status = parts.next().unwrap_or("").trim();
    let _request = parts.next();
    let id = parts.next().unwrap_or("").trim().trim_matches(['\'', '"']);
    let fit = status == "200" && !id.is_empty() && id.len() <= 256 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b));
    if fit { Ok(id.to_string()) } else { Err(Error::Unreadable) }
}

/// The send of an uploaded message, with its send id; never repeated by the program.
pub fn send(base: &str, token: &Secret, upload: &str, suid: &str) -> Result<HttpRequest, Error> {
    Ok(soap::request(base, "SendMsgRequest", soap::MAIL, json!({"suid": suid, "m": {"aid": upload}}), Some(token))?.once())
}

/// The send id of a message: the same at every run for the same message, so that the server knows a
/// second try.
pub fn send_id(account: u64, send: &SendUp, message: &Message) -> String {
    format!("mailcraft-{account}-{}-{}", send.message, message.date)
}

/// Why a message can't go, before anything is asked: no text to send, Bcc recipients.
pub fn held_before(send: &SendUp) -> Option<SendDone> {
    let held =
        |why: &str| SendDone { message: send.message, outcome: SendOutcome::Held { lost: false, why: why.to_string() }, ..SendDone::default() };
    let Some(message) = send.mime.as_ref() else { return Some(held(NOT_WRITTEN)) };
    if !message.bcc.is_empty() {
        return Some(held(BCC_HELD));
    }
    None
}
