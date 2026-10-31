// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The automatic replies of the mailbox (File › Info › Automatic Replies): read with
//! `GET /me/mailboxSettings/automaticRepliesSetting`, and changed with
//! `PATCH /me/mailboxSettings`, which is told the automatic replies and nothing else of the
//! settings (the categories and the working hours are never written).
//!
//! Reading takes `MailboxSettings.Read` or `.ReadWrite`; changing takes `.ReadWrite`, which a
//! sign-in from before it was asked for doesn't have: the answer is then
//! [`NetError::Forbidden`], and nothing has changed. Which token the host has is
//! [`crate::oauth::refresh_graph_settings`]'s to say.
//!
//! What comes back is hostile JSON like everything else from a server: the answer is capped, a
//! status or an audience that isn't one is none, and each reply is cut to
//! [`MAX_REPLY_HTML`](mailcraft_model::auto_replies::MAX_REPLY_HTML). The replies are HTML: they
//! are handed on as they are, for the engine to turn into plain text; nothing here shows them.

use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus};
use serde_json::{Value, json};

use super::{ASK_TIMEOUT, Doing, Graph, nonsense};
use crate::NetError;

const SETTING: &str = "/v1.0/me/mailboxSettings/automaticRepliesSetting";
const SETTINGS: &str = "/v1.0/me/mailboxSettings";
/// The largest answer that is read: two replies of the most that is taken, and the rest.
const MAX_ANSWER: usize = 640 * 1024;

/// The automatic replies of the mailbox, for the owner of `token` (an access token for Graph).
pub fn automatic_replies(token: &str) -> Result<AutoReplies, NetError> {
    automatic_replies_at(&super::graph_base(), token)
}

/// Change the automatic replies of the mailbox as `change` says (what it leaves out stays as it
/// is), for the owner of `token`, which must cover `MailboxSettings.ReadWrite`.
pub fn set_automatic_replies(token: &str, change: &AutoReplies) -> Result<(), NetError> {
    set_automatic_replies_at(&super::graph_base(), token, change)
}

/// [`automatic_replies`] from Graph at `base`.
pub(super) fn automatic_replies_at(base: &str, token: &str) -> Result<AutoReplies, NetError> {
    let graph = Graph::at(base, token)?;
    let reply = graph.ask(&graph.url(SETTING), "GET", &[], None, ASK_TIMEOUT, MAX_ANSWER)?;
    if reply.status != 200 {
        return Err(graph.refusal(Doing::Replies, &reply, ""));
    }
    let json = serde_json::from_slice::<Value>(&reply.body).ok().filter(Value::is_object);
    let Some(json) = json else { return Err(nonsense("Microsoft Graph's answer isn't the JSON it should be")) };
    let read = replies_of(&json);
    log::debug!("automatic replies: {}", read.status.map_or("not said", AutoReplyStatus::code));
    Ok(read)
}

/// [`set_automatic_replies`] at Graph at `base`.
pub(super) fn set_automatic_replies_at(base: &str, token: &str, change: &AutoReplies) -> Result<(), NetError> {
    let mut change = change.clone();
    change.tidy();
    if change.is_empty() {
        return Ok(());
    }
    let graph = Graph::at(base, token)?;
    let body = json!({ "automaticRepliesSetting": change }).to_string();
    let reply = graph.ask(&graph.url(SETTINGS), "PATCH", &[("Content-Type", "application/json")], Some(&body), ASK_TIMEOUT, MAX_ANSWER)?;
    match reply.status {
        200 | 204 => {
            log::info!("automatic replies changed ({})", change.status.map_or("status as it was", AutoReplyStatus::code));
            Ok(())
        }
        _ => Err(graph.refusal(Doing::ChangingReplies, &reply, &body)),
    }
}

/// The automatic replies in an answer ([`AutoReplies::from_graph`]).
pub(super) fn replies_of(answer: &Value) -> AutoReplies {
    AutoReplies::from_graph(answer)
}
