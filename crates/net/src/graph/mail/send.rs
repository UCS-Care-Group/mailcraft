// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's Outbox through Graph (R3.4.2; R3 spec S27, S28, "never twice").
//!
//! After the drafts, each message waiting in the Outbox is sent:
//!
//! - **A draft the server has** (it was saved as one) is brought up to date first, as a draft is
//!   ([`super::drafts`]), and then sent with `POST /me/messages/{id}/send`. One that isn't whole
//!   on the server yet (an attachment that didn't go) waits for the next run.
//! - **A message never saved as a draft** goes with `POST /me/sendMail`, as MIME, with its Bcc.
//!
//! A shared mailbox's (#70) goes the same way at its own address (`/users/{address}/sendMail`,
//! `/users/{address}/messages/{id}/send`): the copy is in **its** Sent Items, and Exchange says
//! whether it goes as the mailbox or on its behalf. One Exchange won't let the owner send at
//! all (`ErrorSendAsDenied`) is held with that reason ([`SEND_AS_DENIED`]); it is never sent
//! from the owner's own address instead, and nothing of it ever goes to `/me`.
//!
//! The server files the copy in Sent Items, which comes down by delta: nothing is uploaded. There
//! is no SMTP and no other way. Neither request is ever asked twice by itself: when its answer
//! is lost (or is one that doesn't say whether it went: 500, 502, 504) the message is **held**,
//! "may have been sent"; when the server refuses it, held with why. A server that says it is busy
//! (429, 503) hasn't taken it: it waits for the next run. No subject, address or id goes into a
//! log line.

use mailcraft_model::sync::{DraftDone, MAX_SENDS, SEND_AS_DENIED, SendDone, SendOutcome, SendUp};
use serde_json::Value;

use super::super::pace::Kind;
use super::super::{Doing, Graph, MAX_PAGE, SEND_TIMEOUT, code_of, mime_body, text};
use super::id_of;
use crate::NetError;
use crate::http::{self, Reply};

/// Send what waits in the Outbox, putting what became of each into `out`. One that wasn't tried
/// isn't there. An error is what stops the run's sending (busy, no sign-in, no answer before
/// anything was asked to go).
pub(super) fn sends(graph: &Graph<'_>, sends: &[SendUp], out: &mut Vec<SendDone>) -> Result<(), NetError> {
    for send in sends.iter().take(MAX_SENDS) {
        let mut done = SendDone { message: send.message, outcome: SendOutcome::Waits, draft: None, draft_left: String::new() };
        let result = one(graph, send, &mut done);
        let tried = done.draft.is_some() || done.outcome != SendOutcome::Waits;
        if tried {
            out.push(done);
        }
        result?;
    }
    if !sends.is_empty() {
        let sent = out.iter().filter(|d| d.outcome == SendOutcome::Sent).count();
        log::info!("graph sending: {sent} of {} messages sent", sends.len().min(MAX_SENDS));
    }
    Ok(())
}

fn one(graph: &Graph<'_>, send: &SendUp, done: &mut SendDone) -> Result<(), NetError> {
    let url = match (&send.draft, &send.mime) {
        (Some(draft), _) => {
            let mut up = DraftDone { change: draft.without_data(), id: draft.id.clone(), ..Default::default() };
            let brought = super::drafts::one(graph, draft, &mut up);
            let whole = !up.id.is_empty() && up.fields && (draft.attachments.is_none() || up.attachments.is_some()) && !up.conflict;
            let (refused, id) = (up.refused, up.id.clone());
            if !up.id.is_empty() || up.refused.is_some() {
                done.draft = Some(up);
            }
            brought?;
            if let Some(status) = refused {
                done.outcome = held_refused(status, "", graph.shared());
                return Ok(());
            }
            if !whole {
                log::info!("graph sending: a message isn't whole on the server yet; it waits for the next run");
                return Ok(());
            }
            let Some(id) = id_of(&id).map(|id| http::encode(&id)) else { return Ok(()) };
            graph.url(&graph.mail_path(&format!("/messages/{id}/send")))
        }
        (None, Some(message)) => {
            let body = match mime_body(message) {
                Ok(body) => body,
                // Not a message that can go (no recipients, an address that isn't one): said so.
                Err(e) => {
                    done.outcome = SendOutcome::Held { lost: false, why: e.to_string() };
                    return Ok(());
                }
            };
            let asked = graph.call(
                Kind::Once,
                &graph.url(&graph.mail_path("/sendMail")),
                "POST",
                &[("Content-Type", "text/plain")],
                Some(&body),
                SEND_TIMEOUT,
                MAX_PAGE,
            );
            return settle(graph, asked, done);
        }
        (None, None) => return Ok(()),
    };
    let asked = graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some("{}"), SEND_TIMEOUT, MAX_PAGE);
    settle(graph, asked, done)
}

/// What the answer to a send means: sent, held, or the error that stops the run (it wasn't taken).
fn settle(graph: &Graph<'_>, asked: Result<Reply, NetError>, done: &mut SendDone) -> Result<(), NetError> {
    let reply = match asked {
        Ok(reply) => reply,
        Err(e) if e.is_unreachable() => {
            log::info!("graph sending: the answer to a send was lost; the message is held, not sent again by itself");
            done.outcome = SendOutcome::Held { lost: true, why: String::new() };
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    // A shared mailbox that won't let the owner in at all, by Graph's code (#70): not taken, so it
    // waits here, and the mailbox shows No access.
    super::refusing(graph, &reply)?;
    match reply.status {
        200..=299 => done.outcome = SendOutcome::Sent,
        // Not taken: it goes next run.
        401 | 429 | 503 => return Err(graph.refusal(Doing::Sending, &reply, "")),
        400 | 403 | 404 | 413 | 415 => {
            let code = code_of(&reply.body, &[graph.token]);
            log::info!("graph sending: a message was refused (status {}{code})", reply.status);
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            done.outcome = held_refused(reply.status, text(json.get("error").unwrap_or(&Value::Null), "code"), graph.shared());
        }
        // An answer that doesn't say whether it went.
        status => {
            log::info!("graph sending: a send was answered with status {status}; the message is held");
            done.outcome = SendOutcome::Held { lost: true, why: String::new() };
        }
    }
    Ok(())
}

/// Held for a refusal: why, in plain words. `shared`: it was to go from a shared mailbox (#70).
fn held_refused(status: u16, code: &str, shared: bool) -> SendOutcome {
    let why = match (status, code) {
        // Exchange's own permission, not the program's: nothing goes from another address.
        (_, "ErrorSendAsDenied") if shared => SEND_AS_DENIED.to_string(),
        (_, "ErrorSendAsDenied") => "Microsoft says this account may not send as that address".to_string(),
        (413, _) => "the message is too large for Microsoft to send".to_string(),
        (_, "ErrorInvalidRecipients") => "Microsoft says a recipient isn't valid".to_string(),
        (403, _) => "Microsoft says this account may not send it".to_string(),
        (404, _) => "Microsoft no longer has the draft it was saved as".to_string(),
        _ => "Microsoft wouldn't take the message".to_string(),
    };
    SendOutcome::Held { lost: false, why }
}
