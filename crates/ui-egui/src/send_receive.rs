// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the window does for Send / Receive › Cancel All and Work Offline, for Help's pages and
//! for File › Account; the commands themselves are the engine's (`sync.cancel`, `sync.offline`,
//! `help.*`, `file.account`).
//!
//! Work Offline is the engine's preference: while it is on, the window starts nothing that goes
//! to a server (a Send/Receive, by hand or by itself, a sign-in, a message fetched whole, a
//! picture, a sender's image, a look-up of a domain's mail servers, a calendar or tasks job, a
//! search on the server), and mail that is sent waits in the Outbox. Each place that would ask
//! a host for something looks at [`MailApp::offline`] first.

use serde_json::{Value, json};

use crate::MailApp;

impl MailApp {
    /// Work Offline is on: nothing goes to a server.
    pub fn offline(&self) -> bool {
        self.session.prefs.offline
    }
}

/// What is said of something that would go to a server while working offline (the engine's
/// refusal, `mailcraft_engine::cmd::send_receive::OFFLINE`, in the same words).
pub(crate) fn said() -> String {
    tl!("You're working offline. Turn off Work Offline (Send / Receive) to connect.").to_string()
}

/// Before the engine runs `id`: Cancel All calls off the run the window has under way, as Stop
/// does (what came in stays). `None`: the engine's command answers.
pub(crate) fn before(app: &mut MailApp, id: &str) -> Option<Result<Value, String>> {
    if id != "sync.cancel" || !app.syncing() {
        return None;
    }
    app.ui_command("ui.cancelSync", &json!({}))
}

/// After the engine ran `id` for anybody (the owner, a script, an agent): what the window does
/// with its answer.
pub(crate) fn after(app: &mut MailApp, id: &str, r: &Result<Value, String>) {
    let Ok(v) = r else { return };
    match id {
        "sync.offline" if v.get("offline").and_then(Value::as_bool) == Some(true) => went_offline(app),
        "sync.offline" => went_online(app),
        // File › Account: the page that says what this copy is, and its updates.
        "file.account" => {
            app.ui.backstage = true;
            app.ui.backstage_page = "about".to_string();
        }
        _ => {}
    }
}

/// After the owner ran `id` in the window: Help's pages open in the system's browser (through
/// the host's one way of opening pages), and Feedback opens Report a Problem.
pub(crate) fn after_owner(app: &mut MailApp, id: &str, r: &Result<Value, String>) {
    let Ok(v) = r else { return };
    match id {
        "help.feedback" => {
            let _ = app.run("ui.reportProblem", json!({}));
        }
        "help.help" | "help.support" | "help.whatsNew" => {
            // Only the project's own pages, which the engine names.
            let Some(url) = v.get("url").and_then(Value::as_str).filter(|u| u.starts_with(mailcraft_engine::cmd::help::REPOSITORY)) else {
                return;
            };
            if let Some(ctx) = app.ctx.clone() {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }
        _ => {}
    }
}

/// Work Offline turned on: what is under way is called off (what came in stays), nothing waits
/// to start, and no look-up is left in flight.
fn went_offline(app: &mut MailApp) {
    if app.syncing() {
        let _ = app.ui_command("ui.cancelSync", &json!({}));
    }
    app.sync_queue.clear();
    app.inbox_queue.clear();
    app.pause_fetch();
    // (A sender's image being looked up stops before its next request; none is kept.)
    app.sender_images = crate::sender_images::SenderImages::default();
    app.status(tl!("Working offline: mail you send waits in the Outbox"));
}

/// Work Offline turned off: what waits in the Outbox goes with a Send/Receive at once.
fn went_online(app: &mut MailApp) {
    app.status(tl!("Back online"));
    if app.auto_sync && app.services.can_sync() && !app.server_accounts().is_empty() && !app.syncing() {
        let _ = app.start_sync(&json!({"quiet": true}));
    }
}

#[cfg(test)]
#[path = "send_receive_tests.rs"]
mod tests;
