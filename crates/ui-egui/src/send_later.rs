// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send later and Undo Send in the window (#950, #1363): the dialog that asks for a date and a
//! time (Options › Delay Delivery, the Send button's Send Later…, Change Send Time on a message
//! waiting in the Outbox), the line across the window while Undo Send's wait runs ("Sending in
//! 5 s · Undo"), the line over a message waiting in the Outbox, the status bar's note of the next
//! delayed message, and the split beside Send.
//!
//! The window only collects a time in the owner's zone, shown in the locale's format, and runs the
//! commands (`compose.delay`, `compose.sendLater`, `mail.setSendTime`, `mail.cancelSend`,
//! `compose.undoSend`); what is sent, and when, is the engine's.

use std::sync::Arc;

use egui::Ui;
use mailcraft_model::{Id, Message};
use mailcraft_time::DateTime;
use serde_json::json;

use crate::MailApp;
use crate::forms::kit;
use crate::theme::Tokens;

/// What the dialog does with the time it is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Delay Delivery: the message being written is not sent before it.
    Delay,
    /// Send Later: the message being written is sent, not before it.
    Send,
    /// Change Send Time of a message waiting in the Outbox.
    Change(Id),
}

/// What the dialog did in a frame.
pub enum Outcome {
    Open,
    Close,
}

/// The dialog's state: the date and the time as typed.
pub struct Ask {
    mode: Mode,
    date: String,
    time: String,
    error: String,
    focus: bool,
}

/// How a time is shown: `Sun 10/11/2026 9:30 AM`, in the owner's zone and the locale's format.
pub fn label(app: &MailApp, at: i64) -> String {
    crate::dates::header_label(at, app.session.offset_at(at), app.session.prefs.twelve_hour)
}

impl Ask {
    /// The dialog for `mode`: it starts at the time the message already has, else the next
    /// morning at the start of the working day. `None` when there is nothing to ask about.
    pub fn open(app: &MailApp, mode: Mode) -> Option<Ask> {
        let s = &app.session;
        let current = match mode {
            Mode::Change(id) => s.mailbox.message(id)?.send_at,
            Mode::Delay | Mode::Send => s.compose.as_ref()?.send_at,
        };
        let now = s.now();
        let at = current.filter(|at| *at > now).unwrap_or_else(|| {
            let wall = s.wall_at(now);
            let morning = DateTime { date: wall.date.add_days(1), hour: s.prefs.work_start.min(23), minute: 0, second: 0 };
            s.instant_of(morning)
        });
        let wall = s.wall_at(at);
        Some(Ask {
            mode,
            date: wall.date.iso(),
            time: kit::clock_text(u32::from(wall.hour) * 60 + u32::from(wall.minute), s.prefs.twelve_hour),
            error: String::new(),
            focus: true,
        })
    }

    /// The name `ui.dialog` opens it by.
    pub fn name(&self) -> &'static str {
        match self.mode {
            Mode::Delay => "delay",
            Mode::Send => "sendLater",
            Mode::Change(_) => "sendTime",
        }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Delay => tl!("Delay Delivery"),
            Mode::Send => tl!("Send Later"),
            Mode::Change(_) => tl!("Change Send Time"),
        }
    }

    /// The time typed, as seconds since 1970, in the owner's zone; `None` when it isn't a date and
    /// a time.
    fn at(&self, app: &MailApp) -> Option<i64> {
        let date = mailcraft_time::Date::parse_iso(self.date.trim())?;
        let (hour, minute) = kit::parse_clock(&self.time)?;
        let wall = DateTime::new(date, hour, minute, 0)?;
        Some(app.session.instant_of(wall))
    }

    /// The dialog's body: the date, the time, what happens, and the buttons.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
        let mut pass = kit::Pass::new(ui, &app.session, &mut self.focus);
        pass.submit |= enter;
        ui.label(match self.mode {
            Mode::Delay => tl!("Do not deliver before:"),
            Mode::Send => tl!("Send this message, but not before:"),
            Mode::Change(_) => tl!("Send this message, but not before:"),
        });
        kit::row(ui, tl!("Date"), |ui| {
            pass.date(ui, "send_later_date", &mut self.date, true);
            kit::date_note(ui, &self.date, true);
        });
        kit::row(ui, tl!("Time"), |ui| {
            pass.time(ui, "send_later_time", &mut self.time, true, None);
        });
        if let Some(at) = self.at(app) {
            ui.label(egui::RichText::new(label(app, at)).color(t.text_dim));
        }
        ui.label(
            egui::RichText::new(tl!(
                "The message waits in your Outbox, where you can change or cancel it. MailCraft sends it at that time; if MailCraft isn't running then, it goes the next time it runs."
            ))
            .color(t.text_dim),
        );
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        // Delay Delivery can be taken off again.
        if self.mode == Mode::Delay && app.session.compose.as_ref().is_some_and(|c| c.send_at.is_some()) && ui.link(tl!("Remove the delay")).clicked()
        {
            return match app.execute("compose.delay", json!({"at": null})) {
                Ok(_) => Outcome::Close,
                Err(e) => {
                    self.error = e;
                    Outcome::Open
                }
            };
        }
        let ok_label = if self.mode == Mode::Send { tl!("Send") } else { tl!("OK") };
        let (ok, cancel) = crate::dialogs::buttons(ui, ok_label, true);
        if cancel {
            return Outcome::Close;
        }
        if ok || pass.submit { self.go(app) } else { Outcome::Open }
    }

    /// OK: run the command for the time typed. A refusal is said in the dialog, which stays.
    fn go(&mut self, app: &mut MailApp) -> Outcome {
        let Some(at) = self.at(app) else {
            self.error = tl!("Enter a date and a time.").to_string();
            return Outcome::Open;
        };
        let ran = match self.mode {
            Mode::Delay => app.execute("compose.delay", json!({"at": at})),
            Mode::Send => app.run("compose.sendLater", json!({"at": at})),
            Mode::Change(id) => app.execute("mail.setSendTime", json!({"id": id, "at": at})),
        };
        match ran {
            Ok(_) => Outcome::Close,
            Err(e) => {
                self.error = e;
                Outcome::Open
            }
        }
    }
}

/// The dialog's frame, for [`crate::dialogs`].
pub fn show(ctx: &egui::Context, d: &mut Ask, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
    let mut outcome = Outcome::Open;
    let open = crate::dialogs::window(ctx, d.title(), 420.0, |ui| outcome = d.ui(ui, app, t, enter));
    if open { outcome } else { Outcome::Close }
}

/// The line across the window while Undo Send's wait runs: "Sending in 5 s", and Undo.
pub(crate) fn undo_bar(app: &mut MailApp, ui: &mut Ui) {
    let Some((_, at)) = app.session.undo_send_wait() else { return };
    let left = at.saturating_sub(app.session.now()).max(0);
    // (The count goes down a second at a time.)
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    let said = crate::i18n::fmt(tl!("Sending in {n} s"), &[("n", &left.to_string())]);
    crate::updates::bar(app, ui, "undo_send", &said, &[("undoSend", tl!("Undo"), true)], |app, _| {
        let _ = app.run("compose.undoSend", json!({}));
    });
}

/// What the status bar says of delayed mail: the next time, or that some went late.
///
/// Worked out at most once a second: it looks through the Outboxes, which a window of many messages
/// shouldn't do at every frame.
pub(crate) fn status_text(app: &MailApp, ctx: &egui::Context) -> Option<String> {
    let key = egui::Id::unique("send_later_status");
    let now = crate::now_ms();
    if let Some((at, text)) = ctx.data(|d| d.get_temp::<(f64, Option<String>)>(key))
        && (0.0..1000.0).contains(&(now - at))
    {
        return text;
    }
    let text = note_text(app);
    ctx.data_mut(|d| d.insert_temp(key, (now, text.clone())));
    text
}

fn note_text(app: &MailApp) -> Option<String> {
    let note = app.session.send_later_note()?;
    if note.late > 0 {
        let text = tln!(
            note.late,
            "{n} delayed message is late: MailCraft wasn't running at its time",
            "{n} delayed messages are late: MailCraft wasn't running at their time"
        );
        return Some(text);
    }
    let next = label(app, note.next?);
    let text = tln!(note.waiting, "Delayed message goes {when}", "{n} delayed messages: the next goes {when}");
    Some(crate::i18n::fmt(&text, &[("when", &next), ("n", &note.waiting.to_string())]))
}

/// The line over a message waiting in the Outbox for its Send time: when it goes, and the ways to
/// change that.
pub(crate) fn queued_bar(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>) {
    let Some(at) = m.send_at.filter(|_| crate::send_later::waits(app, m)) else { return };
    let t = Tokens::get(ui.ctx());
    let id = m.id;
    let due = at <= app.session.now();
    let said = if due {
        tl!("This message is due and goes with the next Send/Receive.").to_string()
    } else {
        crate::i18n::fmt(tl!("This message is delayed. It goes {when}."), &[("when", &label(app, at))])
    };
    let r = egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin { left: 20, right: 20, top: 6, bottom: 6 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(egui::RichText::new(said).font(crate::theme::regular(12.0)).color(t.text));
            if ui.button(tl!("Change Send Time")).clicked() {
                let _ = app.run("mail.setSendTime", json!({"id": id}));
            }
            if ui.button(tl!("Cancel Send")).clicked() {
                let _ = app.run("mail.cancelSend", json!({"id": id}));
            }
        });
    });
    app.hit.add("queuedBar", 0, r.response.rect);
}

/// Whether `m` waits in an Outbox for its Send time or the next run.
pub(crate) fn waits(app: &MailApp, m: &Message) -> bool {
    mailcraft_engine::cmd::send_later::waits_in_outbox(&app.session, m)
}

/// The small arrow beside Send: a menu with Send Later….
pub(crate) fn arrow(ui: &mut Ui, app: &mut MailApp) {
    let label = tl!("Send options");
    let enabled = app.enabled("compose.sendLater");
    let response = crate::widgets::icon_button(ui, "dropdown", label, 10.0, enabled);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    app.hit.add("sendOptions", 0, response.rect);
    if enabled {
        egui::Popup::menu(&response).show(|ui| {
            ui.set_min_width(150.0);
            if ui.button(tl!("Send Later…")).clicked() {
                let _ = app.run("compose.sendLater", json!({}));
                ui.close();
            }
        });
    }
}

#[cfg(test)]
#[path = "send_later_tests.rs"]
mod send_later_tests;
