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
        let mut done =
            SendDone { message: send.message, outcome: SendOutcome::Waits, draft: None, draft_left: String::new(), blind_sent: Vec::new() };
        let result = one(graph, send, &mut done);
        // (A message cut off after some of its Bcc recipients had theirs is told, so that a retry
        // sends only the rest.)
        let tried = done.draft.is_some() || done.outcome != SendOutcome::Waits || !done.blind_sent.is_empty();
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
        // A message to be Digitally signed goes only as its own bytes (#30 R19): sending the
        // server's draft would have Microsoft build it again, unsigned. Never sent that way.
        (Some(_), Some(message)) if message.sign_with.is_some() => {
            done.outcome = SendOutcome::Held { lost: false, why: mailcraft_mime::NOT_SIGNED.to_string() };
            return Ok(());
        }
        // The same for one to be Encrypted (#30 part 7): never sent as the server's draft, which is clear.
        (Some(_), Some(message)) if message.encrypt_with.is_some() || message.encrypt == Some(true) => {
            done.outcome = SendOutcome::Held { lost: false, why: mailcraft_mime::NOT_ENCRYPTED.to_string() };
            return Ok(());
        }
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
            // Each of an Encrypted message's Bcc recipients is sent a message of their own first, to
            // them alone (#30 part 7: one message for everybody would name their keys); then the
            // message goes without them (or not at all when it has nobody but them). Whatever can't
            // go is found before any goes.
            let mut parts: Vec<(Option<String>, String)> = Vec::new();
            for (address, copy) in message.blind_messages(true) {
                if mailcraft_mime::body_not_encrypted(&copy) {
                    done.outcome = SendOutcome::Held { lost: false, why: mailcraft_mime::NOT_ENCRYPTED.to_string() };
                    return Ok(());
                }
                match mime_body(&copy) {
                    Ok(body) => parts.push((Some(address), body)),
                    Err(e) => {
                        done.outcome = SendOutcome::Held { lost: false, why: e.to_string() };
                        return Ok(());
                    }
                }
            }
            if message.has_main_recipients() {
                match mime_body(&message.for_main()) {
                    Ok(body) => parts.push((None, body)),
                    Err(e) => {
                        done.outcome = SendOutcome::Held { lost: false, why: e.to_string() };
                        return Ok(());
                    }
                }
            } else if parts.is_empty() && message.blind_sent.is_empty() {
                done.outcome = SendOutcome::Held { lost: false, why: "the message has no recipients".to_string() };
                return Ok(());
            }
            // Everybody who was to get a message of their own has had it: nothing is left to send.
            let Some(last) = parts.len().checked_sub(1) else {
                done.outcome = SendOutcome::Sent;
                return Ok(());
            };
            for (at, (address, body)) in parts.iter().enumerate() {
                // The message itself files the one copy in Sent Items (a message to Bcc recipients
                // alone, the last of theirs does); the copies of the others are not filed.
                let filed = at == last;
                done.outcome = SendOutcome::Waits;
                if filed {
                    let asked = graph.call(
                        Kind::Once,
                        &graph.url(&graph.mail_path("/sendMail")),
                        "POST",
                        &[("Content-Type", "text/plain")],
                        Some(body),
                        SEND_TIMEOUT,
                        MAX_PAGE,
                    );
                    settle(graph, asked, done)?;
                } else {
                    send_unfiled(graph, body, done)?;
                }
                if done.outcome != SendOutcome::Sent {
                    // (Held, or told to wait: what went already is remembered, and a send again
                    // gives them nothing more. Only the message itself, last, makes it Sent.)
                    return Ok(());
                }
                if let Some(address) = address {
                    done.blind_sent.push(address.clone());
                }
                if !filed {
                    done.outcome = SendOutcome::Waits;
                }
            }
            // A Digitally signed message saved as a draft went as its own bytes (#30 R19): the
            // draft the server has goes now. One that couldn't be taken away is named, and goes
            // with the next run as any other delete; the send is never made again.
            if done.outcome == SendOutcome::Sent && !send.discard.is_empty() {
                match discard(graph, &send.discard) {
                    Ok(true) => {}
                    Ok(false) => done.draft_left = send.discard.clone(),
                    // It went: the rest of the run's sends go on, and the draft goes next run.
                    Err(e) => {
                        log::info!("graph sending: the draft of a signed message sent couldn't be taken away ({e}); it goes next run");
                        done.draft_left = send.discard.clone();
                    }
                }
            }
            return Ok(());
        }
        (None, None) => return Ok(()),
    };
    let asked = graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some("{}"), SEND_TIMEOUT, MAX_PAGE);
    settle(graph, asked, done)
}

/// The property that has the server take a message away once it is sent, so that no copy is filed in
/// Sent Items (`PidTagDeleteAfterSubmit`, a Boolean).
const DELETE_AFTER_SUBMIT: &str = "Boolean 0xe01";

/// Send the message `body` (MIME in base64) to whom it names without a copy of it staying in Sent
/// Items: `sendMail` with MIME files one, and has no way not to, so it is made a draft, marked to be
/// taken away once it is sent, and sent as a draft (#30 part 7: each Bcc recipient of an Encrypted
/// message has a message of their own, and the owner's Sent Items has the one). A server that won't
/// mark it sends it all the same, and its copy stays: logged. Whatever became of it is `done.outcome`,
/// as for any send.
fn send_unfiled(graph: &Graph<'_>, body: &str, done: &mut SendDone) -> Result<(), NetError> {
    let made = graph.call(
        Kind::Once,
        &graph.url(&graph.mail_path("/messages")),
        "POST",
        &[("Content-Type", "text/plain")],
        Some(body),
        SEND_TIMEOUT,
        MAX_PAGE,
    );
    let reply = match made {
        Ok(reply) => reply,
        // (Not known whether it was made; it was not sent, as nothing asked for that.)
        Err(e) if e.is_unreachable() => {
            done.outcome = SendOutcome::Held { lost: false, why: "Microsoft didn't answer".to_string() };
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    super::refusing(graph, &reply)?;
    let id = match reply.status {
        200 | 201 => {
            let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
            id_of(text(&json, "id")).map(|id| http::encode(&id))
        }
        _ => None,
    };
    let Some(id) = id else {
        // Refused or not understood: what a send says of it.
        return settle(graph, Ok(reply), done);
    };
    let mark = serde_json::json!({"singleValueExtendedProperties": [{"id": DELETE_AFTER_SUBMIT, "value": "true"}]}).to_string();
    let url = graph.url(&graph.mail_path(&format!("/messages/{id}")));
    match graph.call(Kind::Same, &url, "PATCH", &[("Content-Type", "application/json")], Some(&mark), SEND_TIMEOUT, MAX_PAGE) {
        Ok(reply) if matches!(reply.status, 200..=299) => {}
        _ => log::info!("graph sending: a Bcc recipient's message couldn't be marked not to be filed; it is sent, and its copy stays in Sent Items"),
    }
    let url = graph.url(&graph.mail_path(&format!("/messages/{id}/send")));
    let asked = graph.call(Kind::Once, &url, "POST", &[("Content-Type", "application/json")], Some("{}"), SEND_TIMEOUT, MAX_PAGE);
    settle(graph, asked, done)?;
    // Not sent and not lost (the server refused it): the draft made for it goes again, best effort.
    if matches!(done.outcome, SendOutcome::Held { lost: false, .. }) {
        let _ = graph.call(Kind::Same, &graph.url(&graph.mail_path(&format!("/messages/{id}"))), "DELETE", &[], None, SEND_TIMEOUT, MAX_PAGE);
    }
    Ok(())
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

/// Take away the server's draft of a message that went another way: whether it is gone (or never
/// was there). Graph's delete keeps it recoverable for the retention period.
fn discard(graph: &Graph<'_>, id: &str) -> Result<bool, NetError> {
    let Some(id) = id_of(id).map(|id| http::encode(&id)) else { return Ok(true) };
    let url = graph.url(&graph.mail_path(&format!("/messages/{id}")));
    let reply = graph.call(Kind::Same, &url, "DELETE", &[], None, SEND_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200..=299 | 404 | 410 => Ok(true),
        status => {
            log::info!("graph sending: the draft of a signed message sent couldn't be taken away (status {status}); it goes next run");
            Ok(false)
        }
    }
}
