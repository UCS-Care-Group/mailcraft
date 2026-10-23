// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline (#143), in the window: mail older than an account's window goes back to
//! its headers and preview when a mailbox is opened, once a day while it stays open, and when
//! the owner shortens the window (after the question, [`crate::dialogs::confirmation`]). It is
//! done a few hundred messages a frame (`account.thinOffline` with `most`), so that a first
//! thinning of years of mail never holds the window up; its content files go with the next save.

use mailcraft_model::KeepOffline;
use serde_json::{Value, json};

use crate::{MailApp, now_ms};

/// Messages taken back to their headers and preview in one frame.
const BATCH: u64 = 400;
/// How often a mailbox that stays open is thinned again.
const DAY_MS: f64 = 86_400_000.0;

/// When mail was last taken back, and for which opening of a mailbox.
#[derive(Default)]
pub struct Thinning {
    /// [`mailcraft_engine::Session::openings`] when it was last done; `None` before the first.
    opening: Option<u64>,
    at: f64,
    /// More to do at once: messages left past the last batch, or a window just shortened.
    pub(crate) now: bool,
}

/// A step of the slider, in words.
pub(crate) fn label(k: KeepOffline) -> &'static str {
    match k {
        KeepOffline::Days3 => tl!("3 days"),
        KeepOffline::Week1 => tl!("1 week"),
        KeepOffline::Weeks2 => tl!("2 weeks"),
        KeepOffline::Month1 => tl!("1 month"),
        KeepOffline::Months3 => tl!("3 months"),
        KeepOffline::Months6 => tl!("6 months"),
        KeepOffline::Months12 => tl!("12 months"),
        KeepOffline::Years2 => tl!("2 years"),
        KeepOffline::Years5 => tl!("5 years"),
        KeepOffline::Years10 => tl!("10 years"),
        KeepOffline::All => tl!("All"),
    }
}

impl MailApp {
    /// `ui.keepOffline`: the slider was moved (and, when shorter, the owner said yes).
    pub(crate) fn keep_offline(&mut self, p: &Value) -> Result<Value, String> {
        let account = p.get("account").and_then(Value::as_u64).ok_or_else(|| tl!("`account` is required").to_string())?;
        let keep = p
            .get("keepOffline")
            .and_then(Value::as_str)
            .and_then(KeepOffline::parse)
            .ok_or_else(|| tl!("`keepOffline` is 3d, 1w, 2w, 1m, 3m, 6m, 12m, 2y, 5y, 10y or all").to_string())?;
        let before = self.session.mailbox.account(account).map(|a| a.keep_offline).ok_or_else(|| tl!("No such account").to_string())?;
        let said = |changed: bool| json!({"account": account, "keepOffline": keep.code(), "changed": changed});
        if keep == before {
            return Ok(said(false));
        }
        self.execute("account.edit", json!({"id": account, "keepOffline": keep.code()}))?;
        if keep < before {
            // What is outside now goes at once, a batch a frame.
            self.thinning.now = true;
        } else {
            // What it covers now comes in the background, newest first.
            self.fetching.wake();
        }
        Ok(said(true))
    }

    /// Each frame: take mail outside its window back when it is due.
    pub(crate) fn offline_tick(&mut self) {
        let opening = self.session.openings();
        let due = self.thinning.now || self.thinning.opening != Some(opening) || now_ms() - self.thinning.at >= DAY_MS;
        if !due {
            return;
        }
        (self.thinning.opening, self.thinning.at, self.thinning.now) = (Some(opening), now_ms(), false);
        if !self.session.mailbox.accounts.iter().any(|a| a.kind == mailcraft_model::AccountKind::Microsoft) {
            return;
        }
        match self.session.run("account.thinOffline", &json!({"most": BATCH})) {
            Ok(said) => {
                // The rest in the next frames.
                if said["left"].as_u64().unwrap_or(0) > 0 {
                    self.thinning.now = true;
                    if let Some(ctx) = &self.ctx {
                        ctx.request_repaint();
                    }
                }
            }
            Err(e) => log::warn!("taking older mail back to its headers failed: {e}"),
        }
    }
}

/// A slider over `count` steps, Outlook's kind: a rail with a tick at each step, the part up to
/// the step chosen in the accent colour, and a knob. Clicked or dragged, it goes to the nearest
/// step; with the keyboard, the arrow keys move it a step. `changed` once the step differs.
pub(crate) fn step_slider(ui: &mut egui::Ui, index: &mut usize, count: usize) -> egui::Response {
    let t = crate::theme::Tokens::get(ui.ctx());
    let last = count.saturating_sub(1).max(1);
    let (rect, mut response) = ui.allocate_exact_size(egui::vec2(200.0, 22.0), egui::Sense::click_and_drag());
    let (left, right) = (rect.left() + 7.0, rect.right() - 7.0);
    let x_of = |i: usize| left + (right - left) * (i.min(last) as f32 / last as f32);
    let before = *index;
    if let Some(p) = response.interact_pointer_pos() {
        let f = ((p.x - left) / (right - left).max(1.0)).clamp(0.0, 1.0);
        *index = (f * last as f32).round() as usize;
        response.request_focus();
    }
    if response.has_focus() {
        let (less, more) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowLeft) || i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowRight) || i.key_pressed(egui::Key::ArrowUp),
            )
        });
        if less {
            *index = index.saturating_sub(1);
        }
        if more {
            *index = (*index + 1).min(last);
        }
    }
    *index = (*index).min(last);
    if *index != before {
        response.mark_changed();
    }
    let painter = ui.painter_at(rect.expand(2.0));
    let y = rect.center().y;
    painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], egui::Stroke::new(2.0, t.border_strong));
    painter.line_segment([egui::pos2(left, y), egui::pos2(x_of(*index), y)], egui::Stroke::new(2.0, t.accent));
    for i in 0..=last {
        let x = x_of(i);
        painter.line_segment([egui::pos2(x, y + 4.0), egui::pos2(x, y + 7.0)], egui::Stroke::new(1.0, t.text_dim));
    }
    let knob = egui::pos2(x_of(*index), y);
    let lit = response.hovered() || response.dragged() || response.has_focus();
    painter.circle(knob, if lit { 7.0 } else { 6.0 }, t.accent, egui::Stroke::new(2.0, t.surface));
    response
}

/// "Mail to keep offline" for `account` (a Microsoft account of its own: a shared mailbox
/// follows it), as a slider over Outlook's steps from 3 days to All, with the step in words.
/// Moving it runs `ui.keepOffline` once it is let go (a shorter window asks first); while it is
/// dragged it only shows where it is. Answers the step it shows.
pub(crate) fn slider(app: &mut MailApp, ui: &mut egui::Ui, account: mailcraft_model::Id, now: KeepOffline) -> KeepOffline {
    let steps = KeepOffline::STEPS;
    let last = steps.len().saturating_sub(1);
    let at = steps.iter().position(|k| *k == now).unwrap_or(last);
    let held = egui::Id::unique(("keep-offline", account));
    let mut index = ui.data(|d| d.get_temp::<usize>(held)).unwrap_or(at);
    let response = step_slider(ui, &mut index, last + 1).on_hover_text(tl!(
            "How far back mail is kept in full on this computer. Older mail keeps its headers and a preview, and downloads when you open it. It stays on the server."
        ));
    app.hit.add("keepOffline", account, response.rect);
    let shown = steps.get(index).copied().unwrap_or(now);
    ui.label(label(shown));
    if response.dragged() {
        ui.data_mut(|d| d.insert_temp(held, index));
        return shown;
    }
    ui.data_mut(|d| d.remove::<usize>(held));
    if (response.changed() || response.drag_stopped()) && shown != now {
        let _ = app.run("ui.keepOffline", json!({"account": account, "keepOffline": shown.code()}));
    }
    shown
}

#[cfg(test)]
mod tests {
    use mailcraft_engine::Session;
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{FolderKind, Message, ServerFolder};

    use super::*;
    use crate::Services;

    /// 2027-01-15.
    const NOW: i64 = 1_800_000_000;
    const DAY: i64 = 86_400;

    #[test]
    fn mail_ages_out_at_opening_and_once_a_day_and_what_was_opened_stays_until_the_next_opening() {
        let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        let account = app.execute("account.add", json!({"email": "offline@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        app.execute("account.edit", json!({"id": account, "keepOffline": "1w"})).unwrap();
        let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
        app.session.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), ..Default::default() });
        let mut ids = Vec::new();
        for (n, days) in [(1, 2), (2, 6), (3, 30)] {
            let server = ServerMessage { id: format!("M{n}"), folder: "in".into(), ..Default::default() };
            let body = mailcraft_model::Body { text: format!("All of message {n}. ").repeat(100), html: None };
            let m = Message { account, folder: inbox, received: NOW - days * DAY, server: Some(server), body, ..Default::default() };
            ids.push(app.session.mailbox.add_message(m).unwrap());
        }
        let whole = |app: &MailApp| ids.iter().map(|id| !app.session.mailbox.message(*id).unwrap().unfetched).collect::<Vec<_>>();
        // At the first frame: the month-old one goes.
        app.offline_tick();
        assert_eq!(whole(&app), [true, true, false]);
        assert!(app.session.mailbox.message(ids[2]).unwrap().body.text.starts_with("All of message 3."), "its preview");
        // Two days on, within the same day of running nothing happens; a day after the last time, the six-day-old one goes.
        app.session.clock.now = Some(NOW + 2 * DAY);
        app.offline_tick();
        assert_eq!(whole(&app), [true, true, false], "not due yet");
        app.thinning.at -= DAY_MS;
        app.offline_tick();
        assert_eq!(whole(&app), [true, false, false]);
        // Opened (fetched by the owner) outside the window: it stays whole while the mailbox
        // stays open, and goes once it is opened again.
        let fetched = mailcraft_model::sync::GraphFetchResult {
            account,
            email: "offline@example.org".into(),
            fetched: vec![mailcraft_model::sync::GraphFetched {
                id: "M3".into(),
                mime: b"From: kit@example.org\r\nSubject: Three\r\n\r\nThe whole of three.\r\n".to_vec(),
            }],
            ..Default::default()
        };
        mailcraft_engine::cmd::graph_mail::apply_graph_fetch(&mut app.session, fetched).unwrap();
        assert_eq!(whole(&app), [true, false, true]);
        app.thinning.at -= DAY_MS;
        app.offline_tick();
        assert_eq!(whole(&app), [true, false, true], "kept until the next start");
        let mailbox = app.session.mailbox.clone();
        app.session.replace_mailbox(mailbox);
        app.offline_tick();
        assert_eq!(whole(&app), [true, false, false]);

        // The slider shortened: asked first, saying the space it frees; Cancel keeps the window.
        assert_eq!(app.run("ui.keepOffline", json!({"account": account, "keepOffline": "3d"})).unwrap()["pending"], "confirm");
        match &app.dialog {
            Some(crate::dialogs::Dialog::Confirm { ask, .. }) => {
                assert!(ask.text.starts_with("About ") && ask.text.contains("older than 3 days") && ask.text.contains("stays on the server"))
            }
            _ => panic!("no question"),
        }
        app.dialog = None;
        assert_eq!(app.session.mailbox.keep_offline_of(account), KeepOffline::Week1, "Cancel");
        // Said yes: the window is shorter, and what is outside goes at the next frame.
        app.run("ui.keepOffline", json!({"account": account, "keepOffline": "3d", "confirmed": true})).unwrap();
        assert_eq!(app.session.mailbox.keep_offline_of(account), KeepOffline::Days3);
        app.offline_tick();
        assert_eq!(whole(&app), [false, false, false]);
        // Longer asks nothing.
        assert_eq!(app.run("ui.keepOffline", json!({"account": account, "keepOffline": "all"})).unwrap()["changed"], true);
    }
}
