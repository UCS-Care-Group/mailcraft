// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To Do job (#248): keep an account's tasks in step with Microsoft To Do. A token of its own
//! for the lists (a sign-in from before they were asked for gets none: the answer says the owner
//! is to be asked), what waits to go up told to the server, then the lists and tasks read whole.
//! Nothing of a task is logged.
//!
//! What to run and when is the engine's ([`mailcraft_engine::sync_driver`]); what the answer
//! means for the mailbox is [`mailcraft_engine::cmd::tasks_remote`], applied by the front end.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mailcraft_model::remote_tasks::{TasksDownload, TasksUpload, TasksUploaded};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Account, AccountKind, Id, OAuth};
#[cfg(not(target_arch = "wasm32"))]
use mailcraft_net::oauth::{GraphTokens, Resource};
#[cfg(not(target_arch = "wasm32"))]
use mailcraft_net::{NetError, graph};

/// What the host is asked to do for one account's to-dos.
pub struct TasksJob {
    pub account: Account,
    /// The account's sign-in settings, with the application id filled in.
    pub oauth: OAuth,
    /// The account's refresh token: the job gets its own token for the to-dos with it.
    pub refresh: String,
    /// What the server is to be told first.
    pub upload: TasksUpload,
    /// The zone the calendar is shown in ([`mailcraft_engine::Session::zone`]): a to-do's start
    /// and due dates are days there, read from and sent to the server as its midnights (#809).
    pub zone: mailcraft_time::Zone,
    pub cancel: Arc<AtomicBool>,
}

/// What a [`TasksJob`] came to. Never an error as a whole: every trouble is a line in the answer.
#[derive(Default)]
pub struct TasksResult {
    pub account: Id,
    /// The refresh token is the account's newest.
    pub tokens: Option<Tokens>,
    /// The sign-in doesn't cover `Tasks.ReadWrite`: the owner is to be asked.
    pub consent_needed: bool,
    /// The sign-in itself is no longer taken.
    pub sign_in_refused: bool,
    /// The server said this account may not use its to-dos.
    pub denied: bool,
    pub uploaded: Option<TasksUploaded>,
    pub download: Option<TasksDownload>,
    pub errors: Vec<String>,
}

// The browser can't open sockets: `mailcraft_net` is empty there, so only the job's data (above) builds.

/// How a to-do job's account has its mail: what a no for the to-dos is checked against (#520).
#[cfg(not(target_arch = "wasm32"))]
pub fn mail_resource(account: &Account) -> Resource {
    if account.kind == AccountKind::Microsoft { Resource::GraphMail } else { Resource::Mail }
}

/// Run the job: a token of its own for the lists, what waits to go told to the server, then the
/// lists and tasks read whole.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(job: TasksJob) -> TasksResult {
    let mail = mail_resource(&job.account);
    run_with(job, &move |oauth, refresh| mailcraft_net::oauth::refresh_graph_tasks(oauth, refresh, mail))
}

/// [`run`], with how the token is had as `renew` (the tests' own, which asks nobody).
#[cfg(not(target_arch = "wasm32"))]
pub fn run_with(job: TasksJob, renew: &dyn Fn(&OAuth, &str) -> Result<GraphTokens, NetError>) -> TasksResult {
    use mailcraft_model::remote_tasks::TaskResult;
    let mut out = TasksResult { account: job.account.id, ..Default::default() };
    let access = match renew(&job.oauth, &job.refresh) {
        Ok(got) => {
            let access = got.tokens.access.clone();
            out.tokens = Some(got.tokens);
            access
        }
        Err(e) => {
            out.consent_needed = e.is_consent_needed();
            out.sign_in_refused = !out.consent_needed && e.is_refused_sign_in();
            if !out.consent_needed && !out.sign_in_refused {
                out.errors.push(format!("Couldn't sign in to Microsoft for the tasks: {e}"));
            }
            return out;
        }
    };
    if !job.upload.ops.is_empty() {
        let uploaded = graph::push_tasks(&access, &job.upload, job.zone).for_account(&job.account);
        out.denied = uploaded.outcomes.iter().any(|o| o.result == TaskResult::Denied);
        let refused = uploaded.token_refused;
        out.uploaded = Some(uploaded);
        if refused {
            out.sign_in_refused = true;
            return out;
        }
    }
    if out.denied {
        return out;
    }
    match graph::tasks(&access, job.zone) {
        Ok(download) => out.download = Some(download.for_account(&job.account)),
        // The token covers the permission and Graph still says no: the organisation keeps it.
        Err(NetError::Forbidden(_)) => out.denied = true,
        Err(e) if e.is_refused_sign_in() => out.sign_in_refused = true,
        Err(e) => out.errors.push(format!("Couldn't bring the tasks down: {e}")),
    }
    out
}
