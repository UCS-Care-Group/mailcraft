// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation ids of messages listed without one (#795): mail listed by an earlier version,
//! and mail of a folder whose delta link was made before (a delta link keeps the `$select` it was
//! made with, so its folder's listings never carry `conversationId`; the link isn't started again
//! for it): `GET
//! /me/messages/{id}?$select=conversationId`, twenty to a `POST /v1.0/$batch`, at the end of a
//! run, for the ids the run's request names ([`GraphMailRequest::threads`], at most
//! [`MAX_THREAD_ASKS`]). Only the account's own server is asked, and it is told nothing but the
//! messages' ids.
//!
//! This is never worth failing a run for: a busy server (said in the run's `busy_for`), an answer
//! that can't be read or the network gone end the look-up where it is, and what wasn't answered is
//! asked in a later run. A message the server no longer has (404) is said with no id, so that it
//! isn't asked again; one it refuses to say for (another 4xx for the message, or a 403 for the
//! whole batch) is said refused, and is asked again only a few runs ([`ThreadOf::refused`]). A
//! batch refused otherwise (a 400 from a server that takes no `$batch`) refuses nothing of the
//! messages: they are asked again in a later run, a batch at a time, as no other part of a run
//! asks such a server either.

use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::{GraphMailRequest, MAX_THREAD_ASKS, ThreadOf, thread_id};
use serde_json::{Value, json};

use super::super::pace::Kind;
use super::super::{ASK_TIMEOUT, BATCH, BATCH_SIZE, Graph, IMMUTABLE_IDS, MAX_BATCH, NetError, text, wait_of};
use super::id_of;
use crate::http;

/// A refusal: a 4xx other than "busy" or "gone".
fn refusal(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 404 | 408 | 429)
}

/// Ask the server for the conversation ids of the messages `request` names; with them, how long
/// the server asked to be left alone when it said it was busy.
pub(super) fn conversations(graph: &Graph<'_>, request: &GraphMailRequest, cancel: &AtomicBool) -> (Vec<ThreadOf>, Option<u32>) {
    let paused = || graph.pace.as_ref().and_then(|p| p.gate.paused_for()).map_or(0, |left| u32::try_from(left.as_secs()).unwrap_or(u32::MAX));
    let mut busy_for: Option<u32> = None;
    let ids: Vec<String> = request.threads.iter().take(MAX_THREAD_ASKS).filter_map(|id| id_of(id)).collect();
    let mut out = Vec::with_capacity(ids.len());
    let select = http::form(&[("$select", "conversationId")]);
    for some in ids.chunks(BATCH_SIZE) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let requests: Vec<Value> = some
            .iter()
            .enumerate()
            .map(|(n, id)| {
                let url = graph.batch_path(&format!("/messages/{}?{select}", http::encode(id)));
                json!({"id": n.to_string(), "method": "GET", "url": url, "headers": {"Prefer": IMMUTABLE_IDS}})
            })
            .collect();
        let body = json!({"requests": requests}).to_string();
        let headers = [("Content-Type", "application/json")];
        let reply = match graph.call(Kind::Read, &graph.url(BATCH), "POST", &headers, Some(&body), ASK_TIMEOUT, MAX_BATCH) {
            Ok(reply) if reply.status == 200 => reply,
            Ok(reply) if matches!(reply.status, 429 | 503 | 504) => {
                busy_for = Some(wait_of(reply.retry_after.as_deref(), mailcraft_time::now()).unwrap_or(0).max(paused()));
                break;
            }
            Ok(reply) => {
                log::info!("graph mail: conversation ids not asked for now (status {}); a later run goes on", reply.status);
                if reply.status == 403 {
                    out.extend(some.iter().map(|id| ThreadOf::refused(id)));
                }
                break;
            }
            Err(NetError::Busy { retry_after, .. }) => {
                busy_for = Some(retry_after.unwrap_or(0).max(paused()));
                break;
            }
            Err(e) => {
                log::info!("graph mail: conversation ids not asked for now ({e}); a later run goes on");
                break;
            }
        };
        let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        let said = json.get("responses").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
        let mut answered = 0usize;
        for one in said.iter().take(BATCH_SIZE * 2) {
            let n = one.get("id").and_then(Value::as_str).and_then(|n| n.parse::<usize>().ok());
            let Some(id) = n.and_then(|n| some.get(n)) else { continue };
            let status = one.get("status").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()).unwrap_or(0);
            let thread = match status {
                200 => thread_id(text(one.get("body").unwrap_or(&Value::Null), "conversationId")),
                // Gone from the server: none, and not asked again (the next listing says it left).
                404 => String::new(),
                429 | 503 | 504 => {
                    busy_for = Some(busy_for.unwrap_or(0).max(paused()));
                    continue;
                }
                // Refused for this message: asked again only a few runs.
                status if refusal(status) => {
                    out.push(ThreadOf::refused(id));
                    continue;
                }
                // Anything else: asked again in a later run.
                _ => continue,
            };
            answered = answered.saturating_add(1);
            out.push(ThreadOf { id: id.clone(), thread, refused: false });
        }
        // Nothing of a batch answered, or the server busy: it isn't answering these now.
        if answered == 0 || busy_for.is_some() {
            break;
        }
    }
    out.dedup_by(|a, b| a.id == b.id);
    log::info!("graph mail: conversation ids of {} of {} messages asked for", out.len(), request.threads.len());
    (out, busy_for)
}
