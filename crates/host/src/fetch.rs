// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of a Microsoft account fetched whole (R3.2): what the host is asked ([`GraphFetchJob`],
//! plain data that builds for the browser) and the run itself ([`run`], native only), which brings
//! the messages the request names down a few at a time and stops at the next safe place when it
//! is called off. Which messages, in what order and when to look again is the front end's and
//! the engine's ([`mailcraft_engine::sync_driver`]).
//!
//! A host with no window (the phone, MB3.17) has no Graph token of its own at hand: it asks
//! Send/Receive for the account's ([`crate::sync::SendReceive::mail_access`]), which is a token of
//! this session that is still good or else the account's refresh token, and runs its job through
//! [`with_access`], which renews the token the way a Graph mail run does and hands back what the
//! renewal brought for Send/Receive to keep.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::GraphFetchRequest;
#[cfg(not(target_arch = "wasm32"))]
use mailcraft_model::sync::GraphFetchResult;
use mailcraft_model::sync::Tokens;

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

/// How a job of an account's mail gets its Graph token. Never in `Debug` output.
pub enum Access {
    /// A token of this session that is still good.
    Held(String),
    /// None is: one is had from the account's refresh token, as a Graph mail run has it.
    Renew {
        /// The account's sign-in settings, with the application id filled in.
        oauth: mailcraft_model::OAuth,
        refresh: String,
    },
}

impl std::fmt::Debug for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Access::Held(_) => "Access::Held",
            Access::Renew { .. } => "Access::Renew",
        })
    }
}

/// What getting a token for a job came to, beside the job's own answer.
#[derive(Default)]
pub struct Renewal {
    /// The tokens a renewal brought: the access token for the session, and the account's newest
    /// refresh token, for Send/Receive to keep ([`crate::sync::SendReceive::adopt_tokens`]).
    pub tokens: Option<Tokens>,
    /// The sign-in was refused when a token was asked for: the owner has to sign in again.
    pub sign_in_refused: bool,
    /// The sign-in doesn't cover the mail: Microsoft wants the owner's say-so.
    pub consent_needed: bool,
    /// Microsoft couldn't be reached for a token.
    pub offline: bool,
    /// Why no token came, in a few words for the log (never a secret).
    pub error: Option<String>,
}

impl std::fmt::Debug for Renewal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renewal")
            .field("tokens", &self.tokens.is_some())
            .field("sign_in_refused", &self.sign_in_refused)
            .field("consent_needed", &self.consent_needed)
            .field("offline", &self.offline)
            .finish_non_exhaustive()
    }
}

/// Run `work` with a Graph token for the account's mail: the one held, or one renewed from the
/// refresh token with what a Graph mail run asks for (`Mail.ReadWrite` and the rest). `None` as
/// the answer when no token could be had (the renewal says why).
#[cfg(not(target_arch = "wasm32"))]
pub fn with_access<T>(access: Access, work: impl FnOnce(&str) -> T) -> (Option<T>, Renewal) {
    with_access_renewing(access, &mailcraft_net::oauth::refresh_graph_mail, work)
}

/// [`with_access`], with how a token is renewed as `renew` (the tests' own, which asks nobody).
#[cfg(not(target_arch = "wasm32"))]
pub fn with_access_renewing<T>(
    access: Access,
    renew: &dyn Fn(&mailcraft_model::OAuth, &str) -> Result<mailcraft_net::oauth::GraphTokens, mailcraft_net::NetError>,
    work: impl FnOnce(&str) -> T,
) -> (Option<T>, Renewal) {
    let mut renewal = Renewal::default();
    let token = match access {
        Access::Held(token) => token,
        Access::Renew { oauth, refresh } => match renew(&oauth, &refresh) {
            Ok(got) => {
                let token = got.tokens.access.clone();
                renewal.tokens = Some(got.tokens);
                token
            }
            Err(e) if e.is_consent_needed() => {
                log::info!("fetch: the sign-in doesn't cover the mail; the owner's permission is needed");
                renewal.consent_needed = true;
                return (None, renewal);
            }
            Err(e) if e.is_refused_sign_in() => {
                log::info!("fetch: the sign-in was refused when a token was asked for");
                renewal.sign_in_refused = true;
                return (None, renewal);
            }
            Err(e) => {
                log::warn!("fetch: the sign-in couldn't be renewed: {e}");
                renewal.offline = e.is_unreachable();
                renewal.error = Some(format!("Couldn't reach Microsoft for the mail: {e}"));
                return (None, renewal);
            }
        },
    };
    if token.is_empty() {
        renewal.error = Some("Microsoft gave no token for the mail.".to_string());
        return (None, renewal);
    }
    (Some(work(&token)), renewal)
}
