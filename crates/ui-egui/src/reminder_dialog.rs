// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Reminder (#948): Follow Up › Add Reminder… on the selected messages, and Follow Up ›
//! Choose a date… in the message being written. It collects a date and a time (or, for mail the
//! owner sent, a number of days and "only if nobody replies") and then runs `mail.reminder` or
//! `compose.followUp`; it takes no message ids, so that whoever opens it (an agent may, through
//! `ui.dialog`) can't have the owner flag messages they didn't select.

use egui::Ui;
use serde_json::{Value, json};

use crate::MailApp;
use crate::theme::Tokens;

/// The days "if nobody replies" offers; 0 is a date of the owner's choice.
pub(crate) const DAYS: [u16; 5] = [1, 2, 3, 5, 7];
/// Most characters of the date and time boxes.
const FIELD_MAX: usize = 16;

/// What the dialog is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The messages selected when it opened.
    Messages,
    /// The message being written.
    Composing,
}

/// The dialog's state.
pub struct AddReminder {
    pub target: Target,
    /// Mail the owner sent: "only if nobody replies" is offered.
    can_no_reply: bool,
    no_reply: bool,
    /// Days from now for "if nobody replies"; 0: the date and time below.
    days: u16,
    date: String,
    time: String,
    error: String,
}

/// What the dialog did in a frame.
pub enum Outcome {
    Open,
    Close,
}

impl AddReminder {
    /// For the messages selected (`None` when none is), or for the message being written.
    pub fn open(app: &MailApp, target: Target) -> Option<AddReminder> {
        let s = &app.session;
        let can_no_reply = match target {
            Target::Messages => {
                if s.sel.messages.is_empty() {
                    return None;
                }
                s.mailbox.follow_up_refusal(&s.sel.messages).is_none()
            }
            Target::Composing => {
                s.compose.as_ref()?;
                true
            }
        };
        let tomorrow = s.today().add_days(1);
        Some(AddReminder {
            target,
            can_no_reply,
            no_reply: target == Target::Composing,
            days: 3,
            date: tomorrow.iso(),
            time: format!("{:02}:00", s.prefs.work_start.min(23)),
            error: String::new(),
        })
    }

    /// The dialog's name, as `ui.dialog` takes it.
    pub fn name(&self) -> &'static str {
        match self.target {
            Target::Messages => "addReminder",
            Target::Composing => "followUpDate",
        }
    }

    /// The command OK runs, and its parameters.
    pub fn command(&self) -> (&'static str, Value) {
        let when = if self.no_reply && self.days > 0 {
            json!({"days": self.days})
        } else {
            json!({"at": format!("{}T{}", self.date.trim(), self.time.trim())})
        };
        let mut params = when;
        match self.target {
            Target::Composing => ("compose.followUp", params),
            Target::Messages => {
                if let Some(p) = params.as_object_mut() {
                    p.insert("ifNoReply".into(), json!(self.no_reply));
                }
                ("mail.reminder", params)
            }
        }
    }

    fn go(&mut self, app: &mut MailApp) -> Outcome {
        let (id, params) = self.command();
        match app.run(id, params) {
            Ok(_) => Outcome::Close,
            Err(e) => {
                self.error = e;
                Outcome::Open
            }
        }
    }

    /// The dialog's body. `enter` is the frame's Enter.
    pub fn ui(&mut self, ui: &mut Ui, app: &mut MailApp, t: &Tokens, enter: bool) -> Outcome {
        let composing = self.target == Target::Composing;
        ui.label(if composing { tl!("Remind me if nobody replies by:") } else { tl!("Flag the message and remind me:") });
        if self.can_no_reply && !composing {
            ui.radio_value(&mut self.no_reply, false, tl!("At a date and time"));
            ui.radio_value(&mut self.no_reply, true, tl!("Only if nobody replies"));
        }
        if self.no_reply {
            ui.horizontal_wrapped(|ui| {
                for d in DAYS {
                    ui.selectable_value(&mut self.days, d, tln!(d, "{n} day", "{n} days"));
                }
                ui.selectable_value(&mut self.days, 0, tl!("A date"));
            });
            ui.label(
                egui::RichText::new(tl!(
                    "If nobody the message went to has written back in its conversation by then, a reminder is raised and the message is flagged."
                ))
                .color(t.text_dim)
                .size(11.0),
            );
        }
        let mut submitted = false;
        if !self.no_reply || self.days == 0 {
            ui.horizontal(|ui| {
                ui.label(tl!("Date"));
                let date = ui.add(egui::TextEdit::singleline(&mut self.date).hint_text("YYYY-MM-DD").char_limit(FIELD_MAX).desired_width(110.0));
                let date = crate::text_menu::field(date);
                crate::text_menu::hold(&date);
                ui.label(tl!("Time"));
                let time = ui.add(egui::TextEdit::singleline(&mut self.time).hint_text("HH:MM").char_limit(FIELD_MAX).desired_width(70.0));
                let time = crate::text_menu::field(time);
                crate::text_menu::hold(&time);
            });
        }
        ui.label(
            egui::RichText::new(tl!("The time is kept on this computer; a flag goes to the account's server as any flag does."))
                .color(t.text_dim)
                .size(11.0),
        );
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = crate::dialogs::buttons(ui, if composing { tl!("OK") } else { tl!("Add Reminder") }, true);
        submitted |= ok || enter;
        if cancel {
            return Outcome::Close;
        }
        if submitted {
            return self.go(app);
        }
        Outcome::Open
    }
}

#[cfg(test)]
#[path = "reminder_dialog_tests.rs"]
mod tests;
