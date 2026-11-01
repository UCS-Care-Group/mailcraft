// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! JMAP accounts in the browser (M6): the host that runs their Send/Receive and fetches their
//! messages whole ([`Services::jmap`], [`Services::jmap_fetch`]), over the browser's `fetch`
//! ([`crate::fetch`]), each job as a task of its own on the page's thread. A run's pieces go to the
//! window as they come, and wake it.
//!
//! The secret of a job is the one the owner typed, which the window hands over with it. A page that
//! serves MailCraft and has signed the owner in itself may instead hold it: it says so before
//! MailCraft starts, as
//!
//! ```js
//! window.mailcraftJmap = {
//!   accounts: [{ email: "ada@example.org", session: "https://mail.example.org/jmap/session", bearer: true, name: "Work" }],
//!   secret(email, session) { return token; },   // or a promise of one
//! };
//! ```
//!
//! `session` is the address of the session resource itself: a page follows no redirect, so a
//! server's name alone, or a `/.well-known/jmap` that sends the client on (RFC 8620 §2.2), comes to
//! nothing here, and the account's Send/Receive says which address to give instead.
//!
//! Its accounts are added to the mailbox when they aren't in it yet. With `secret`, the window
//! never asks for the credential of one of them (at the server the page named): each job asks the
//! page for it at its start, and a run the server turns it down for asks the page once more, so
//! that the page can hand over a renewed one, before the run gives up. A page that hasn't answered
//! in thirty seconds has none to give. MailCraft keeps none of it beyond the job. Without
//! `secret`, the owner types it, as for any other JMAP account.
//!
//! Stop, and another mailbox opened, call off the exchange under way at once ([`Fetch`]). What is
//! decided here without the browser is in `jmap_rules.rs`, tested on the host.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use mailcraft_jmap::fetch::{failed_fetch, fetch_messages};
use mailcraft_jmap::{Connection, Credentials, Error, TransportError, failed_run, sync_mail};
use mailcraft_model::sync::{GraphFetchResult, JmapMailRequest, JmapMailResult};
use mailcraft_model::{Account, AccountKind};
use mailcraft_ui_egui::jmap_mail::{JmapFetchJob, JmapJob};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;

use crate::fetch::Fetch;
use crate::jmap_rules::{Next, PageAccount, again, named, redirect_line, stopped};

/// The most accounts a page may name.
const MAX_ACCOUNTS: usize = 50;
/// How long the page has to hand over a credential, in milliseconds.
const PAGE_ANSWERS_MS: i32 = 30_000;

/// What the page that serves MailCraft says of its JMAP accounts: `window.mailcraftJmap`.
pub(crate) struct Hook {
    /// The page's object itself, `this` of `secret`.
    hook: JsValue,
    accounts: Vec<PageAccount>,
    secret: Option<js_sys::Function>,
}

impl Hook {
    /// What the page says, when it says anything that can be used.
    pub(crate) fn read() -> Option<Hook> {
        let window: JsValue = web_sys::window()?.into();
        let hook = js_sys::Reflect::get(&window, &JsValue::from_str("mailcraftJmap")).ok().filter(JsValue::is_object)?;
        let field = |of: &JsValue, name: &str| js_sys::Reflect::get(of, &JsValue::from_str(name)).ok();
        let list = field(&hook, "accounts").filter(js_sys::Array::is_array).map(|list| list.unchecked_into::<js_sys::Array>());
        let accounts = list
            .iter()
            .flat_map(|list| list.iter().take(MAX_ACCOUNTS))
            .filter_map(|entry| {
                let text = |name: &str| field(&entry, name).and_then(|value| value.as_string()).map(|text| text.trim().to_string());
                Some(PageAccount {
                    email: text("email").filter(|email| !email.is_empty())?,
                    session: text("session").filter(|session| !session.is_empty())?,
                    bearer: field(&entry, "bearer").and_then(|value| value.as_bool()).unwrap_or(false),
                    name: text("name").unwrap_or_default(),
                })
            })
            .collect();
        let secret = field(&hook, "secret").and_then(|value| value.dyn_into::<js_sys::Function>().ok());
        Some(Hook { hook, accounts, secret })
    }

    /// Whether the page names accounts of its own.
    pub(crate) fn names_accounts(&self) -> bool {
        !self.accounts.is_empty()
    }

    /// Whether the page holds the credential of the account at `email` on the server whose session
    /// address is `session`: one of its accounts, at the server it named.
    fn holds_at(&self, email: &str, session: &str) -> bool {
        self.secret.is_some() && named(&self.accounts, email, session)
    }

    /// Whether the page holds the credential of `a`.
    fn holds(&self, a: &Account) -> bool {
        a.kind == AccountKind::Jmap && a.jmap.as_ref().is_some_and(|server| self.holds_at(&a.email, &server.session))
    }

    /// The credential the page has now for the account at `email` on `session`'s server, when the
    /// page holds it and hands one over in time.
    async fn secret(&self, email: &str, session: &str) -> Option<String> {
        if !self.holds_at(email, session) {
            return None;
        }
        let asked = self.secret.as_ref()?.call2(&self.hook, &JsValue::from_str(email), &JsValue::from_str(session.trim())).ok()?;
        // (A value, a promise or anything with a `then`; and not for ever.)
        let window = web_sys::window()?;
        let late = js_sys::Promise::new(&mut |resolve, _| {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, PAGE_ANSWERS_MS);
        });
        let answer = js_sys::Promise::race(&js_sys::Array::of2(&js_sys::Promise::resolve(&asked), &late));
        JsFuture::from(answer).await.ok()?.as_string().filter(|secret| !secret.is_empty())
    }

    /// Add the page's accounts that aren't in the mailbox yet. One that can't be added is said in
    /// the status bar (the log gets no address).
    pub(crate) fn add_accounts(&self, app: &mut MailApp) {
        for p in &self.accounts {
            if app.session.mailbox.accounts.iter().any(|a| a.email.trim().eq_ignore_ascii_case(&p.email)) {
                continue;
            }
            let mut params = json!({"email": p.email, "kind": "jmap", "jmap": {"session": p.session, "bearer": p.bearer}});
            if !p.name.is_empty() {
                params["name"] = json!(p.name);
            }
            if let Err(e) = app.execute("account.add", params) {
                log::warn!("mailcraft-web: an account the page named couldn't be added");
                app.status(e);
            }
        }
    }
}

/// The JMAP host of the window: its runs, its fetches, and the accounts whose credential the page
/// holds.
pub(crate) fn install(services: &mut Services, ctx: &egui::Context, hook: Option<Rc<Hook>>) {
    let (ctx_run, hook_run) = (ctx.clone(), hook.clone());
    services.jmap = Some(Box::new(move |job: JmapJob| {
        let (tx, rx) = std::sync::mpsc::channel();
        wasm_bindgen_futures::spawn_local(run(job, hook_run.clone(), ctx_run.clone(), tx));
        rx
    }));
    let (ctx_fetch, hook_fetch) = (ctx.clone(), hook.clone());
    services.jmap_fetch = Some(Box::new(move |job: JmapFetchJob| {
        let (tx, rx) = std::sync::mpsc::channel();
        wasm_bindgen_futures::spawn_local(fetch_whole(job, hook_fetch.clone(), ctx_fetch.clone(), tx));
        rx
    }));
    if let Some(hook) = hook.filter(|hook| hook.secret.is_some()) {
        services.jmap_host_credentials = Some(Box::new(move |a: &Account| hook.holds(a)));
    }
}

/// The credentials a job goes with: the secret as a bearer token, or as the password of the
/// account's address. `None` when it can't be sent (empty, or more than a header field takes).
fn credentials(email: &str, bearer: bool, secret: &str) -> Option<Credentials> {
    if secret.is_empty() {
        return None;
    }
    if bearer { Credentials::bearer(secret).ok() } else { Credentials::basic(email, secret).ok() }
}

/// The page's credential for the account at `email` on `session`'s server, when there is a page
/// and it holds one.
async fn asked(hook: Option<&Hook>, email: &str, session: &str) -> Option<String> {
    hook?.secret(email, session).await
}

/// A Send/Receive of one account. With the page's credential, a run the server turns it down for is
/// tried once more if the page then has another one; the pieces of both go to the window as one
/// run, and only the last piece of the second ends it.
async fn run(job: JmapJob, hook: Option<Rc<Hook>>, ctx: egui::Context, tx: Sender<JmapMailResult>) {
    let send = |piece: JmapMailResult| {
        let _ = tx.send(piece);
        ctx.request_repaint();
    };
    let JmapJob { request, secret: typed, cancel } = job;
    let held = typed.is_empty();
    let (mut turned_down, mut tried_again): (Option<(String, JmapMailResult)>, bool) = (None, false);
    loop {
        let fresh = if held { asked(hook.as_deref(), &request.email, &request.session).await } else { Some(typed.clone()) };
        let secret = match crate::jmap_rules::next(fresh, turned_down.take()) {
            Next::Go(secret) => secret,
            Next::End(last) => return send(*last),
            Next::NoSecret => return send(failed_run(&request, &Error::CredentialsRefused)),
        };
        // Stopped while the page was asked: nothing is read.
        if cancel.load(Ordering::Relaxed) {
            return send(stopped(&request));
        }
        let last = one_run(&request, &secret, &cancel, &send).await;
        if again(&last, held, tried_again) {
            (turned_down, tried_again) = (Some((secret, last)), true);
            continue;
        }
        return send(last);
    }
}

/// One run with `secret`: every piece but the last goes to the window; the last is answered.
async fn one_run(request: &JmapMailRequest, secret: &str, cancel: &Arc<AtomicBool>, send: &dyn Fn(JmapMailResult)) -> JmapMailResult {
    let Some(credentials) = credentials(&request.email, request.bearer, secret) else {
        return failed_run(request, &Error::CredentialsRefused);
    };
    match Connection::connect(Fetch::new(cancel.clone()), &request.session, credentials).await {
        Ok(connection) => {
            let mut last = None;
            sync_mail(&connection, request, cancel, &mut |piece| {
                if piece.partial {
                    send(piece);
                } else {
                    last = Some(piece);
                }
            })
            .await;
            last.unwrap_or_else(|| failed_run(request, &Error::Transport(TransportError::Other)))
        }
        // Stopped while signing in (the exchange called off): as before anything was read.
        Err(Error::Transport(TransportError::Aborted)) if cancel.load(Ordering::Relaxed) => stopped(request),
        Err(error) => {
            let mut failed = failed_run(request, &error);
            said_in_a_browser(&mut failed.errors, &error, false);
            failed
        }
    }
}

/// The line of a failure that is the browser's doing, in place of the client's.
fn said_in_a_browser(errors: &mut Vec<String>, error: &Error, downloading: bool) {
    if let Some(line) = redirect_line(error, downloading) {
        *errors = vec![line.to_string()];
    }
}

/// Messages of one account fetched whole, with the typed secret or the page's.
async fn fetch_whole(job: JmapFetchJob, hook: Option<Rc<Hook>>, ctx: egui::Context, tx: Sender<GraphFetchResult>) {
    let JmapFetchJob { request, secret, cancel } = job;
    let secret = if secret.is_empty() { asked(hook.as_deref(), &request.email, &request.session).await } else { Some(secret) };
    let fetched = match secret.and_then(|secret| credentials(&request.email, request.bearer, &secret)) {
        // Called off while the page was asked: nothing is read.
        Some(_) if cancel.load(Ordering::Relaxed) => {
            GraphFetchResult { account: request.account, email: request.email.clone(), session: request.session.clone(), ..Default::default() }
        }
        Some(credentials) => match Connection::connect(Fetch::new(cancel.clone()), &request.session, credentials).await {
            Ok(connection) => fetch_messages(&connection, &request, &cancel).await,
            Err(error) => {
                let mut failed = failed_fetch(&request, &error);
                said_in_a_browser(&mut failed.errors, &error, true);
                failed
            }
        },
        None => failed_fetch(&request, &Error::CredentialsRefused),
    };
    let _ = tx.send(fetched);
    ctx.request_repaint();
}
