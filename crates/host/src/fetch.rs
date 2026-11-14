// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of a Microsoft account fetched whole (R3.2): what the host is asked ([`GraphFetchJob`],
//! plain data that builds for the browser) and the run itself ([`run`], native only), which brings
//! the messages the request names down a few at a time and stops at the next safe place when it
//! is called off. Which messages, in what order and when to look again is the front end's and
//! the engine's ([`mailcraft_engine::sync_driver`]).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::GraphFetchRequest;
#[cfg(not(target_arch = "wasm32"))]
use mailcraft_model::sync::GraphFetchResult;

/// What the host is asked to fetch.
pub struct GraphFetchJob {
    pub request: GraphFetchRequest,
    /// A Graph token of this session that covers the mail.
    pub access: String,
    pub cancel: Arc<AtomicBool>,
}

/// Fetch the messages the job names.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(job: GraphFetchJob) -> GraphFetchResult {
    mailcraft_net::graph::fetch_messages(&job.access, &job.request, &job.cancel)
}
