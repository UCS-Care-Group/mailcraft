// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of an IMAP account's Automatic Replies and rules kept in a Sieve script, and of
//! the Sieve rule editor's Check (#2057): what
//! the window asks ([`SieveJob`]) done with `mailcraft-net`'s ManageSieve client on a background
//! thread, and what the server has handed back as `sync.applySieve` takes it. Nothing of a
//! script, a name or a secret is logged.

use mailcraft_engine::sieve::SCRIPT_NAME;
use mailcraft_net::sieve::{self, Checked, OwnChange, OwnWritten, ScriptChange, SieveError, SieveState, Written};
use mailcraft_ui_egui::sieve_replies::{ServerCheck, SieveJob, SieveResult};
use serde_json::{Value, json};

/// Write what the job says (if anything), then read what the server has.
pub(crate) fn sieve_job(job: SieveJob) -> SieveResult {
    let mut out = SieveResult { account: job.account.id, ..Default::default() };
    let server = match sieve::server_for(&job.account) {
        Ok(server) => server,
        Err(e) => {
            out.error = e.to_string();
            return out;
        }
    };
    // A check alone: nothing is written, nor read.
    if let Some(script) = &job.check {
        match sieve::check(&job.account, &server, &job.secret, script) {
            Ok(checked) => {
                out.checked = Some(match checked {
                    Checked::Fine => ServerCheck::Fine,
                    Checked::Warnings(text) => ServerCheck::Warnings(text),
                    Checked::Errors(text) => ServerCheck::Errors(text),
                    Checked::NotOffered => ServerCheck::NotOffered,
                });
            }
            Err(e) => said(&mut out, &e),
        }
        return out;
    }
    // The owner's own script, written back as typed (#2057): never MailCraft's own this way.
    if let Some(write) = job.write.as_ref().filter(|w| w.own) {
        if write.name == SCRIPT_NAME {
            out.error = "MailCraft's own script isn't written this way".into();
            return out;
        }
        let Some(read) = write.last_written.as_deref() else {
            out.error = "the script's text as it was read isn't known".into();
            return out;
        };
        let change = OwnChange { name: &write.name, text: &write.text, read };
        match sieve::write_own(&job.account, &server, &job.secret, &change) {
            Ok(OwnWritten::Done { .. }) => out.written = Some("done".into()),
            Ok(OwnWritten::Changed { .. }) => out.written = Some("changedElsewhere".into()),
            Ok(OwnWritten::NoSpace) => out.written = Some("noSpace".into()),
            Ok(OwnWritten::TooLarge) => out.written = Some("tooLarge".into()),
            Err(e) => {
                said(&mut out, &e);
                read_after_failed_write(&job, &server, &mut out);
                return out;
            }
        }
    } else if let Some(write) = &job.write {
        let change = ScriptChange { name: &write.name, text: &write.text, last_written: write.last_written.as_deref(), activate: write.activate };
        match sieve::write(&job.account, &server, &job.secret, &change) {
            Ok(Written::Done { .. }) => out.written = Some("done".into()),
            Ok(Written::AnotherActive { .. }) => out.written = Some("anotherActive".into()),
            Ok(Written::ChangedElsewhere { .. }) => out.written = Some("changedElsewhere".into()),
            Err(e) => {
                said(&mut out, &e);
                read_after_failed_write(&job, &server, &mut out);
                return out;
            }
        }
    }
    match sieve::read(&job.account, &server, &job.secret, SCRIPT_NAME) {
        Ok(state) => out.state = Some(state_json(&state)),
        Err(e) => said(&mut out, &e),
    }
    out
}

/// After a write that failed once the server was reached and signed in to (a SETACTIVE refused
/// after the PUTSCRIPT went through, say): what the server has now is read all the same, so that
/// the window shows it; the write's error stays the one said (#2058).
fn read_after_failed_write(job: &SieveJob, server: &mailcraft_model::Server, out: &mut SieveResult) {
    if out.unavailable || out.sign_in_refused {
        return;
    }
    if let Ok(state) = sieve::read(&job.account, server, &job.secret, SCRIPT_NAME) {
        out.state = Some(state_json(&state));
    }
}

/// What the server has, as `sync.applySieve` takes it.
fn state_json(state: &SieveState) -> Value {
    let scripts: Vec<Value> = state.scripts.iter().map(|s| json!({"name": s.name, "active": s.active})).collect();
    let active = state.active_script.as_ref().map(|(name, text)| json!({"name": name, "text": text}));
    json!({"extensions": state.extensions, "scripts": scripts, "script": state.script, "activeScript": active})
}

/// What went wrong, as the window says it: no ManageSieve session could be had (what was found
/// where the server was looked for, naming the host and port, and what to do about it), a
/// sign-in it refused, or anything else in a few words.
fn said(out: &mut SieveResult, e: &SieveError) {
    match e {
        SieveError::Reach(reach) => {
            out.unavailable = true;
            out.why = reach.said().to_string();
            out.advice = reach.advice().to_string();
        }
        _ if e.is_refused_sign_in() => out.sign_in_refused = true,
        SieveError::Net(e) => out.error = e.to_string(),
    }
}
