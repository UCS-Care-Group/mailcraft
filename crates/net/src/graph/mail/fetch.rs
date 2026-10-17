//! Messages of a Microsoft account fetched whole (R3.2; R3 spec S12, S14): `GET
//! /me/messages/{id}/$value`, each message's MIME as the server has it, attachments and all.
//!
//! A host asks for a few at a time: in the background, newest first, with the share of requests
//! background work has (at most 2 of the mailbox's 4); at once, ahead of that, for the message
//! the owner opened or is replying to. Cancel is looked at before every message. What comes back
//! is the message's own bytes, which the engine reads as any message is read; nothing of it goes
//! into a log line or an error.
//!
//! A message the server won't give (a `400`, an answer too large to take) is passed over and
//! named in [`GraphFetchResult::failed`], logged by its status and Graph's code alone; the
//! busy server, the network gone, the sign-in or the permission refused, or three refused one
//! after another stop the batch, and the rest wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_model::sync::{GraphFetchRequest, GraphFetchResult, GraphFetched};

use super::super::pace::{Gate, Kind, Priority, Retry};
use super::super::{Doing, Graph, graph_base};
use super::id_of;
use crate::NetError;
use crate::http;

/// Largest message taken (Microsoft's own limit is 150 MB).
const MAX_MIME: usize = 160 * 1024 * 1024;
/// How long one message may take to come.
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);
/// Messages refused one after another before the batch stops: by then it is likely the request,
/// not the messages.
const MAX_FAILED_IN_A_ROW: usize = 3;

/// Fetch the messages of `request` whole, as the owner of `token`.
pub fn fetch_messages(token: &str, request: &GraphFetchRequest, cancel: &AtomicBool) -> GraphFetchResult {
    fetch_messages_with(&graph_base(), token, request, Gate::for_mailbox(&request.email), Retry::STANDARD, cancel)
}

/// [`fetch_messages`] at `base`, through `gate`, asking again as `retry` says.
pub(crate) fn fetch_messages_with(
    base: &str,
    token: &str,
    request: &GraphFetchRequest,
    gate: Arc<Gate>,
    retry: Retry,
    cancel: &AtomicBool,
) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account, email: request.email.clone(), ..Default::default() };
    let priority = if request.background { Priority::Background } else { Priority::Owner };
    let shared = request.shared.then_some(request.email.as_str());
    let graph = match Graph::for_mail(base, token, gate, priority, retry).and_then(|graph| graph.of_shared(shared)) {
        Ok(graph) => graph,
        Err(e) => {
            out.errors.push(format!("Couldn't reach Microsoft for the messages: {e}"));
            return out;
        }
    };
    let mut in_a_row = 0usize;
    for id in request.ids.iter().take(100) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(id) = id_of(id) else { continue };
        let url = graph.url(&graph.mail_path(&format!("/messages/{}/$value", http::encode(&id))));
        let reply = match graph.call(Kind::Read, &url, "GET", &[], None, FETCH_TIMEOUT, MAX_MIME) {
            Ok(reply) => reply,
            // Busy, or no way through: the rest wait.
            Err(e) if e.is_unreachable() || matches!(e, NetError::Busy { .. }) => {
                stop(&mut out, &e);
                break;
            }
            // This message's answer couldn't be taken (too large, garbled): passed over.
            Err(e) => {
                log::warn!("graph mail: a message couldn't be fetched whole ({e}); it is passed over");
                out.failed.push(id);
                in_a_row += 1;
                if in_a_row >= MAX_FAILED_IN_A_ROW {
                    stop(&mut out, &e);
                    break;
                }
                continue;
            }
        };
        match reply.status {
            200 => {
                in_a_row = 0;
                out.fetched.push(GraphFetched { id, mime: reply.body });
            }
            404 => {
                in_a_row = 0;
                out.missing.push(id);
            }
            // The sign-in, the mailbox's permission or the server's patience: nothing else would
            // go better now.
            401 | 403 | 429 | 503 | 504 => {
                let e = graph.refusal(Doing::Mail, &reply, "");
                stop(&mut out, &e);
                break;
            }
            // This message, as far as can be told (`refusal` logs the status and Graph's code):
            // passed over, so that it can't hold the rest up.
            _ => {
                let e = graph.refusal(Doing::Mail, &reply, "");
                // A shared mailbox that won't be opened is the mailbox, not the message (#70).
                if matches!(e, NetError::SharedRefused(_)) {
                    stop(&mut out, &e);
                    break;
                }
                log::warn!("graph mail: a message couldn't be fetched whole (status {}); it is passed over", reply.status);
                out.failed.push(id);
                in_a_row += 1;
                // Every one refused: it is likely the request, not the messages. The rest wait.
                if in_a_row >= MAX_FAILED_IN_A_ROW {
                    stop(&mut out, &e);
                    break;
                }
            }
        }
    }
    let level = if out.errors.is_empty() && out.failed.is_empty() { log::Level::Info } else { log::Level::Warn };
    log::log!(
        level,
        "graph mail: {} of {} messages fetched whole, {} no longer there, {} passed over, {} problems",
        out.fetched.len(),
        request.ids.len(),
        out.missing.len(),
        out.failed.len(),
        out.errors.len()
    );
    out
}

fn stop(out: &mut GraphFetchResult, e: &NetError) {
    if let NetError::Busy { retry_after, .. } = e {
        out.busy_for = Some(retry_after.unwrap_or(0));
    }
    out.offline = e.is_unreachable();
    out.errors.push(format!("Couldn't download the messages from Microsoft: {e}"));
}
