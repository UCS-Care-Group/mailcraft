// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Switching a Microsoft account kept in step by IMAP over to Microsoft Graph (R3.7): the page
//! that offers it, once for each such account, and what the window does after the owner's answer.
//!
//! The engine does the switch (`account.graphSwitch`); the window only sees to it that it gets
//! its chance:
//!
//! - **The page** opens by itself (where the host wants it offered: `offer_graph_switch`) for each account still on IMAP that its owner wasn't asked
//!   about yet, when nothing else is on screen. Switch and Later each answer it for good; closing
//!   it without either asks again the next time MailCraft starts. It opens any time from command
//!   search ("Switch to Microsoft Graph", `ui.graphSwitch`).
//! - **Switch** (`ui.switchToGraph`): when something waits for the IMAP server, a Send/Receive of
//!   the account goes first, and the switch happens once nothing waits any more (checked again
//!   after every Send/Receive of the account, so a switch that waits while offline goes ahead
//!   by itself once a run gets through). Then the account's first Send/Receive through Graph
//!   starts, and the status bar says where the copy of the mailbox from before is.
//! - **The copy first, off the window's thread.** Where the host can write files in the
//!   background (`Services::write_copy`), the copy of the mailbox is written there while the page
//!   says so with a spinner, and the engine's switch runs once it is on disk; a copy that can't be
//!   written changes nothing, and the account stays on IMAP. Elsewhere `account.graphSwitch`
//!   writes it itself.

use egui::vec2;
use mailcraft_model::{GraphSwitch, Id};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::{Dialog, button_row, window};
use crate::i18n::fmt;
use crate::theme::semibold;
use crate::widgets::primary_button;

/// The copy of the mailbox being written before `account` switches (`answer`), at `path`.
pub(crate) struct Copying {
    account: Id,
    answer: String,
    path: std::path::PathBuf,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
}

/// The account's address, as the page and the status bar name it.
fn address(app: &MailApp, account: Id) -> String {
    app.session.mailbox.account(account).map(|a| if a.email.trim().is_empty() { a.name.clone() } else { a.email.clone() }).unwrap_or_default()
}

/// What the page says, paragraph by paragraph (the list's items begin with a bullet).
pub(crate) fn wording(account: &str) -> Vec<String> {
    let with = |text: &str| fmt(text, &[("account", account)]);
    vec![
        with(tl!(
            "MailCraft can now keep {account} in step with Microsoft through Microsoft Graph, the way Outlook on the web does, instead of IMAP. Your folders, flags, categories and drafts then match what you see in Outlook, and your mail keeps coming even where your organisation has turned IMAP off."
        )),
        tl!("When you switch:").to_string(),
        format!(
            "•  {}",
            tl!(
                "Anything still waiting to go to the server goes first: mail in the Outbox, and flags and moves made while you were offline. If MailCraft can't reach the server, the switch waits until it can."
            )
        ),
        format!("•  {}", tl!("Before anything changes, a copy of your whole mailbox is saved next to it, with today's date in its name.")),
        format!(
            "•  {}",
            tl!(
                "The mail of this account is then downloaded again from Microsoft. The newest messages show within minutes; the rest follows in the background."
            )
        ),
        format!(
            "•  {}",
            with(tl!(
                "Drafts you haven't sent go to your Drafts on the server. Folders and messages that are only on this computer stay on this computer, in a folder called \"Kept from {account}\". Your folders in Favorites stay there."
            ))
        ),
        format!("•  {}", tl!("You may be asked to sign in with Microsoft again, to let MailCraft read and change your mail there.")),
        tl!("Later keeps this account on IMAP for now. You can switch whenever you like: search the commands for \"Switch to Microsoft Graph\".")
            .to_string(),
    ]
}

/// What the page says to an owner whose switch waits because the IMAP server can't be reached
/// with what waits for it: `n` changes would be left behind.
pub(crate) fn waiting_wording(account: &str, n: usize) -> Vec<String> {
    let left = if n == 0 {
        tl!("No changes are left behind: only mail in the Outbox is waiting.").to_string()
    } else {
        tln!(
            n,
            "{n} change waiting for the IMAP server won't reach it; it stays in the backup copy.",
            "{n} changes waiting for the IMAP server won't reach it; they stay in the backup copy."
        )
    };
    vec![
        fmt(tl!("{account} is switching to Microsoft Graph, but MailCraft can't reach its IMAP server to send what is waiting there first."), &[("account", account)]),
        tl!("Keep waiting, and the switch goes ahead by itself once MailCraft can reach the server. Or switch without them, if the server can't be reached any more (for example, your organisation has turned IMAP off):").to_string(),
        left,
        tl!("A copy of your whole mailbox is saved next to it first, as always. Mail in the Outbox isn't left behind: it is sent through Microsoft Graph.").to_string(),
    ]
}

/// Whether `account`'s switch is waiting for its IMAP server.
fn switch_waits(app: &MailApp, account: Id) -> bool {
    app.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft() && a.graph_switch == GraphSwitch::Switching)
}

/// Draw the page for `account`. `true` when it is done with (answered, or closed).
pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context, account: Id) -> bool {
    if app.graph_switch_copy.as_ref().is_some_and(|c| c.account == account) {
        return show_copying(app, ctx, account);
    }
    if switch_waits(app, account) {
        return show_waiting(app, ctx, account);
    }
    let whose = address(app, account);
    let mut answer: Option<&str> = None;
    let open = window(ctx, tl!("Switch to Microsoft Graph"), 520.0, |ui| {
        let lines = wording(&whose);
        for line in &lines {
            if line == tl!("When you switch:") {
                ui.label(egui::RichText::new(line.as_str()).font(semibold(13.0)));
            } else if let Some(item) = line.strip_prefix("•  ") {
                // An item of the list, its lines under its first word.
                ui.horizontal_top(|ui| {
                    ui.label("•");
                    ui.add(egui::Label::new(item).wrap());
                });
            } else {
                ui.add(egui::Label::new(line.as_str()).wrap());
            }
        }
        ui.add_space(6.0);
        button_row(ui, |ui| {
            // Neither Enter nor Escape answers it: each answer takes a deliberate click.
            let switch = primary_button(ui, tl!("Switch"), true);
            // (Named for screen readers: the button paints its own words.)
            switch.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Switch")));
            if switch.clicked() {
                answer = Some("switch");
            }
            if ui.add(egui::Button::new(tl!("Later")).min_size(vec2(72.0, 26.0))).clicked() {
                answer = Some("later");
            }
        });
    });
    if let Some(answer) = answer {
        if let Err(e) = app.switch_to_graph(account, answer) {
            app.status(e);
        }
        // While the copy is written, the page stays to say so.
        return !copying(app, account);
    }
    !open
}

fn copying(app: &MailApp, account: Id) -> bool {
    app.graph_switch_copy.as_ref().is_some_and(|c| c.account == account)
}

/// The page while the copy of the mailbox is written: a spinner, and what is happening. Closing
/// it doesn't stop anything; the status bar says when the switch is made.
fn show_copying(app: &mut MailApp, ctx: &egui::Context, account: Id) -> bool {
    let whose = address(app, account);
    let open = window(ctx, tl!("Switch to Microsoft Graph"), 520.0, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(18.0));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(tl!("Saving a copy of your mailbox before the switch…")).font(semibold(13.0)));
        });
        ui.add_space(4.0);
        ui.add(
            egui::Label::new(fmt(
                tl!("A large mailbox takes a while. {account} switches to Microsoft Graph as soon as the copy is saved; nothing has changed yet."),
                &[("account", &whose)],
            ))
            .wrap(),
        );
    });
    // The spinner turns while the copy is written.
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    !open || !copying(app, account)
}

/// The page for a switch that waits: Keep Waiting, or Switch without Them.
fn show_waiting(app: &mut MailApp, ctx: &egui::Context, account: Id) -> bool {
    let whose = address(app, account);
    let n = mailcraft_engine::cmd::graph_switch::left_behind(&app.session, account);
    let mut answer: Option<&str> = None;
    let open = window(ctx, tl!("Switch to Microsoft Graph"), 520.0, |ui| {
        for line in waiting_wording(&whose, n) {
            ui.add(egui::Label::new(line.as_str()).wrap());
        }
        ui.add_space(6.0);
        button_row(ui, |ui| {
            // Waiting is what loses nothing: it is the button that stands out.
            let wait = primary_button(ui, tl!("Keep Waiting"), true);
            wait.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Keep Waiting")));
            if wait.clicked() {
                answer = Some("wait");
            }
            if ui.add(egui::Button::new(tl!("Switch without Them")).min_size(vec2(72.0, 26.0))).clicked() {
                answer = Some("switchWithout");
            }
        });
    });
    match answer {
        Some("switchWithout") => {
            if let Err(e) = app.switch_to_graph(account, "switchWithout") {
                app.status(e);
            }
            !copying(app, account)
        }
        Some(_) => true,
        None => !open,
    }
}

impl MailApp {
    fn graph_switch_busy(&self) -> bool {
        self.syncing() || self.graph_running() || self.signing_in.is_some() || self.approving() || self.graph_switch_copy.is_some()
    }

    /// Each frame: the copy of the mailbox written before a switch is done (the switch is made)
    /// or couldn't be written (nothing changes, and the status bar says why).
    pub(crate) fn poll_graph_switch_copy(&mut self) {
        let Some(c) = &self.graph_switch_copy else { return };
        let done = match c.rx.try_recv() {
            Ok(done) => done,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(tl!("it stopped unexpectedly").to_string()),
        };
        let Some(c) = self.graph_switch_copy.take() else { return };
        // The page that said the copy was being saved has said all it had to.
        if matches!(self.dialog, Some(Dialog::GraphSwitch { account }) if account == c.account) {
            self.dialog = None;
        }
        let whose = address(self, c.account);
        match done {
            Ok(()) => {
                self.session.graph_switch_backup = Some((c.account, c.path));
                if let Err(e) = self.switch_now(c.account, &c.answer) {
                    self.status(e);
                }
                self.session.graph_switch_backup = None;
            }
            Err(e) => {
                log::warn!("the copy of the mailbox before the switch to Microsoft Graph couldn't be written: {e}");
                let why = mailcraft_engine::cmd::graph_switch::copy_failed(&e);
                self.status(fmt(tl!("{account} couldn't switch to Microsoft Graph: {why}"), &[("account", &whose), ("why", &why)]));
            }
        }
    }

    /// Each frame, when nothing else is on screen: a switch that waits goes ahead once nothing
    /// waits for the IMAP server any more, and an account its owner wasn't asked about gets the
    /// page.
    pub(crate) fn tick_graph_switch(&mut self) {
        if self.dialog.is_some() || self.needs_setup() || self.graph_switch_busy() {
            return;
        }
        let accounts: Vec<(Id, GraphSwitch)> =
            self.session.mailbox.accounts.iter().filter(|a| a.on_imap_with_microsoft()).map(|a| (a.id, a.graph_switch)).collect();
        for (account, state) in &accounts {
            if *state == GraphSwitch::Switching
                && !self.graph_switch_tried.contains(account)
                && mailcraft_engine::cmd::graph_switch::waiting(&self.session, *account) == 0
            {
                // Once until the next Send/Receive of the account: a switch that can't be made
                // (the mailbox isn't in a file) isn't tried every frame.
                self.graph_switch_tried.insert(*account);
                if let Err(e) = self.switch_to_graph(*account, "switch") {
                    self.status(e);
                }
                return;
            }
        }
        if !self.offer_graph_switch {
            return;
        }
        let asked = accounts.iter().find(|(account, state)| *state == GraphSwitch::NotAsked && !self.graph_switch_shown.contains(account));
        if let Some((account, _)) = asked {
            self.graph_switch_shown.insert(*account);
            self.dialog = Some(Dialog::GraphSwitch { account: *account });
        }
    }

    /// A Send/Receive of `account` by IMAP is over (`ok`: it got through): a switch that waits
    /// for it looks again.
    pub(crate) fn graph_switch_after_sync(&mut self, account: Id, ok: bool) {
        let switching = self.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft() && a.graph_switch == GraphSwitch::Switching);
        if !switching {
            return;
        }
        self.graph_switch_tried.remove(&account);
        if !ok {
            let whose = address(self, account);
            self.status(fmt(tl!("The switch of {account} to Microsoft Graph waits until MailCraft can reach the server."), &[("account", &whose)]));
            // Once a run of the program, the owner is offered to switch without what waits.
            if self.offer_graph_switch && self.dialog.is_none() && self.graph_switch_waiting_shown.insert(account) {
                self.dialog = Some(Dialog::GraphSwitch { account });
            }
        }
    }

    /// The owner's answer for `account`: `switch`, `switchWithout` or `later`. Where the switch
    /// would be made now and the host can write files in the background, the copy of the mailbox
    /// is written there first ([`MailApp::poll_graph_switch_copy`] makes the switch after it).
    pub(crate) fn switch_to_graph(&mut self, account: Id, answer: &str) -> Result<Value, String> {
        #[cfg(not(target_arch = "wasm32"))]
        if answer != "later" {
            if self.graph_switch_copy.is_some() {
                return Err(tl!("A copy of the mailbox is being saved for a switch already: wait for it to finish.").to_string());
            }
            let now = answer == "switchWithout" || mailcraft_engine::cmd::graph_switch::waiting(&self.session, account) == 0;
            let on_imap = self.session.mailbox.account(account).is_some_and(|a| a.on_imap_with_microsoft());
            if now
                && on_imap
                && let Some(write) = &self.services.write_copy
            {
                let whose = address(self, account);
                let (path, copy) = mailcraft_engine::cmd::graph_switch::prepare_backup(&self.session, account)
                    .map_err(|e| fmt(tl!("{account} couldn't switch to Microsoft Graph: {why}"), &[("account", &whose), ("why", &e.to_string())]))?;
                let rx = write(path.clone(), copy);
                self.graph_switch_copy = Some(Copying { account, answer: answer.to_string(), path, rx });
                if self.dialog.is_none() {
                    self.dialog = Some(Dialog::GraphSwitch { account });
                }
                self.status(tl!("Saving a copy of your mailbox before the switch…"));
                return Ok(json!({"account": account, "saving": true}));
            }
        }
        self.switch_now(account, answer)
    }

    /// The engine's switch for `account`, as the owner answered.
    fn switch_now(&mut self, account: Id, answer: &str) -> Result<Value, String> {
        let whose = address(self, account);
        let said = self.execute("account.graphSwitch", json!({"account": account, "answer": answer}));
        let said = said.map_err(|e| fmt(tl!("{account} couldn't switch to Microsoft Graph: {why}"), &[("account", &whose), ("why", &e)]))?;
        if said.get("later").and_then(Value::as_bool) == Some(true) {
            self.status(fmt(
                tl!("{account} stays on IMAP for now. To switch later, search the commands for \"Switch to Microsoft Graph\"."),
                &[("account", &whose)],
            ));
            return Ok(said);
        }
        if said.get("switched").and_then(Value::as_bool) != Some(true) {
            // Something waits for the IMAP server: a Send/Receive first, and the switch after it.
            if !self.graph_switch_busy() {
                self.start_sync(&json!({"account": account}))?;
            }
            self.status(fmt(
                tl!("Sending what is waiting for the server first; then {account} switches to Microsoft Graph."),
                &[("account", &whose)],
            ));
            return Ok(said);
        }
        let backup = said.get("backup").and_then(Value::as_str).unwrap_or_default().to_string();
        let left = said.get("leftBehind").and_then(Value::as_u64).unwrap_or(0);
        // The account's first run through Graph: its folders, then its mail, afresh.
        let started = self.start_sync(&json!({"account": account}));
        self.status(fmt(
            tl!(
                "{account} now uses Microsoft Graph, and its mail is downloading again. A copy of your mailbox from before the switch is in {backup}."
            ),
            &[("account", &whose), ("backup", &backup)],
        ));
        if answer == "switchWithout" && left > 0 {
            let said = tln!(
                left,
                "{account} now uses Microsoft Graph, and its mail is downloading again. {n} change that didn't reach the IMAP server is in the copy of your mailbox from before the switch: {backup}.",
                "{account} now uses Microsoft Graph, and its mail is downloading again. {n} changes that didn't reach the IMAP server are in the copy of your mailbox from before the switch: {backup}."
            );
            self.status(fmt(&said, &[("account", &whose), ("backup", &backup)]));
        }
        if let Err(e) = started {
            log::warn!("the first Send/Receive after the switch didn't start: {e}");
        }
        Ok(said)
    }
}

/// The window's commands of the switch.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if !matches!(id, "ui.graphSwitch" | "ui.switchToGraph") {
        return None;
    }
    let mb = &app.session.mailbox;
    let account = p
        .get("account")
        .and_then(Value::as_u64)
        .or_else(|| mb.accounts.iter().find(|a| a.on_imap_with_microsoft()).map(|a| a.id))
        .filter(|id| mb.account(*id).is_some_and(|a| a.on_imap_with_microsoft()))
        .ok_or_else(|| tl!("No Microsoft account is kept in step by IMAP").to_string());
    Some(match id {
        "ui.graphSwitch" => account.map(|account| {
            app.dialog = Some(Dialog::GraphSwitch { account });
            json!({"account": account})
        }),
        "ui.switchToGraph" => match (account, p.get("answer").and_then(Value::as_str)) {
            (Ok(account), Some(answer @ ("switch" | "switchWithout" | "later"))) => app.switch_to_graph(account, answer),
            (Ok(_), _) => Err("`answer` is switch, switchWithout or later".to_string()),
            (Err(e), _) => Err(e),
        },
        _ => return None,
    })
}

#[cfg(test)]
#[path = "graph_switch_tests.rs"]
pub(crate) mod tests;
