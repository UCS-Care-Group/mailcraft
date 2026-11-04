// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation ids of messages listed before they were asked for (#795): `GET
//! /me/messages/{id}?$select=conversationId`, twenty to a `POST /v1.0/$batch`, at the end of a
//! run, for the ids the run's request names ([`GraphMailRequest::threads`], at most
//! [`MAX_THREAD_ASKS`]). Only the account's own server is asked, and it is told nothing but the
//! messages' ids.
//!
//! This is never worth failing a run for: a busy server, an answer that can't be read or the
//! network gone end the look-up where it is, and what wasn't answered is asked in a later run. A
//! message the server no longer has (404) is said with no id, so that it isn't asked again.

use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::{GraphMailRequest, MAX_THREAD_ASKS, ThreadOf, thread_id};
use serde_json::{Value, json};

use super::super::pace::Kind;
use super::super::{ASK_TIMEOUT, BATCH, BATCH_SIZE, Graph, IMMUTABLE_IDS, MAX_BATCH, text};
use super::id_of;
use crate::http;

/// Ask the server for the conversation ids of the messages `request` names.
pub(super) fn conversations(graph: &Graph<'_>, request: &GraphMailRequest, cancel: &AtomicBool) -> Vec<ThreadOf> {
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
            Ok(reply) => {
                log::info!("graph mail: conversation ids not asked for now (status {}); a later run goes on", reply.status);
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
            let thread = match one.get("status").and_then(Value::as_u64) {
                Some(200) => thread_id(text(one.get("body").unwrap_or(&Value::Null), "conversationId")),
                // Gone from the server: none, and not asked again (the next listing says it left).
                Some(404) => String::new(),
                // Busy, or anything else: asked again in a later run.
                _ => continue,
            };
            answered = answered.saturating_add(1);
            out.push(ThreadOf { id: id.clone(), thread });
        }
        // Nothing of a batch answered: the server isn't answering these now.
        if answered == 0 {
            break;
        }
    }
    out.dedup_by(|a, b| a.id == b.id);
    log::info!("graph mail: conversation ids of {} of {} messages asked for", out.len(), request.threads.len());
    out
}
