// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reminders that fire: a small window listing the events whose reminder time has come, with
//! Dismiss and Snooze, as the incumbent has. What is due is the engine's answer
//! (`view::due_reminders`); what was dismissed is kept with the window's settings, what was put
//! off only for this run.

use std::collections::{HashMap, HashSet};

use egui::{Align2, RichText, Sense, vec2};
use mailcraft_engine::cmd::view::{Reminder, due_reminders};
use serde_json::json;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons, now_ms};

/// How often what is due is worked out again when nothing else changed (milliseconds).
const LOOK_EVERY: f64 = 15_000.0;
/// How many dismissals are remembered; older ones are for events long past.
const REMEMBERED: usize = 400;

#[derive(Default)]
pub struct State {
    /// What is due and neither dismissed nor put off, soonest first.
    pub(crate) showing: Vec<Reminder>,
    pub(crate) looked: f64,
    looked_rev: u64,
    /// Reminders put off, and until when (milliseconds).
    pub(crate) snoozed: HashMap<String, f64>,
    /// Reminders the owner has been told about (the taskbar is only asked for attention once).
    announced: HashSet<String>,
    /// The owner asked for the window (View › Reminders Window): shown even with nothing due.
    pub(crate) asked_for: bool,
}

/// What a reminder is known by: its event and the occurrence it is for.
pub(crate) fn key(event: u64, occurrence: &str) -> String {
    format!("{event}@{occurrence}")
}

fn key_of(r: &Reminder) -> String {
    key(r.event, &r.start.iso())
}

/// `in 10 minutes`, `now`, `25 minutes ago`, `in 2 hours`, `3 days ago`.
pub(crate) fn when(minutes: i64) -> String {
    let unit = match minutes.unsigned_abs() {
        0 => return tl!("now").to_string(),
        m @ 1..=119 => tln!(m, "{n} minute", "{n} minutes"),
        m @ 120..=2879 => tln!(m / 60, "{n} hour", "{n} hours"),
        m => tln!(m / 1440, "{n} day", "{n} days"),
    };
    if minutes > 0 { crate::i18n::fmt(tl!("in {time}"), &[("time", &unit)]) } else { crate::i18n::fmt(tl!("{time} ago"), &[("time", &unit)]) }
}

/// Work out what is due, every few seconds and whenever the mailbox changed; ask for attention
/// when something new is.
pub fn tick(app: &mut MailApp, ctx: &egui::Context) {
    let now = now_ms();
    let rev = app.session.rev();
    if now - app.reminders.looked < LOOK_EVERY && rev == app.reminders.looked_rev && app.reminders.looked != 0.0 {
        return;
    }
    app.reminders.looked = now;
    app.reminders.looked_rev = rev;
    refresh(app);
    let mut fresh = Vec::new();
    for r in &app.reminders.showing {
        if app.reminders.announced.insert(key_of(r)) {
            fresh.push(r.clone());
        }
    }
    if !fresh.is_empty() && !ctx.input(|i| i.focused) {
        ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
    }
    // And a desktop alert for each, in front or not ([`crate::alerts`]).
    crate::alerts::reminders_due(app, &fresh);
    if !app.session.mailbox.events.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(LOOK_EVERY as u64));
    }
}

/// What is due, less what was dismissed or is put off.
fn refresh(app: &mut MailApp) {
    let now = now_ms();
    app.reminders.snoozed.retain(|_, until| *until > now);
    let due = due_reminders(&app.session, app.session.local_now());
    app.reminders.showing =
        due.into_iter().filter(|r| !app.ui.dismissed_reminders.contains(&key_of(r)) && !app.reminders.snoozed.contains_key(&key_of(r))).collect();
}

/// Dismiss one reminder (`event` and `occurrence`), or all that are showing.
pub fn dismiss(app: &mut MailApp, one: Option<(u64, &str)>) -> usize {
    let keys: Vec<String> = match one {
        Some((event, occurrence)) => vec![key(event, occurrence)],
        None => app.reminders.showing.iter().map(key_of).collect(),
    };
    let n = keys.len();
    for k in keys {
        if !app.ui.dismissed_reminders.contains(&k) {
            app.ui.dismissed_reminders.push(k);
        }
    }
    let extra = app.ui.dismissed_reminders.len().saturating_sub(REMEMBERED);
    app.ui.dismissed_reminders.drain(..extra);
    refresh(app);
    if app.reminders.showing.is_empty() {
        app.reminders.asked_for = false;
    }
    n
}

/// Put one reminder, or all that are showing, off for `minutes`.
pub fn snooze(app: &mut MailApp, one: Option<(u64, &str)>, minutes: u64) -> usize {
    let until = now_ms() + minutes.clamp(1, 10_080) as f64 * 60_000.0;
    let keys: Vec<String> = match one {
        Some((event, occurrence)) => vec![key(event, occurrence)],
        None => app.reminders.showing.iter().map(key_of).collect(),
    };
    let n = keys.len();
    for k in keys {
        // Due again once the time is up: told of again then.
        app.reminders.announced.remove(&k);
        app.reminders.snoozed.insert(k, until);
    }
    refresh(app);
    app.reminders.asked_for = false;
    n
}

/// The window, while something is due or the owner asked for it: a window of its own, which
/// can be put wherever the owner likes (where the system has no separate windows, as in a
/// browser, a movable window inside the main one).
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    if app.reminders.showing.is_empty() && !app.reminders.asked_for {
        return;
    }
    let t = Tokens::get(ctx);
    let twelve = app.session.prefs.twelve_hour;
    let rows = app.reminders.showing.clone();
    let (mut dismissed, mut snoozed, mut opened) = (None, None, None);
    let mut joined: Option<u64> = None;
    let (mut dismiss_all, mut closed) = (false, false);
    let title = tln!(rows.len(), "{n} Reminder", "{n} Reminders");
    let height = (rows.len().clamp(1, 6) as f32) * 46.0 + 96.0;
    let builder = crate::windows::like_main(
        ctx,
        egui::ViewportBuilder::default().with_title(title).with_inner_size([420.0, height]).with_min_inner_size([340.0, 150.0]),
    );
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("mailcraft_reminders"), builder, |ui, class| {
        let own_window = class != egui::ViewportClass::EmbeddedWindow;
        if own_window {
            closed = ui.ctx().input(|i| i.viewport().close_requested());
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.menu);
        }
        egui::Frame::NONE.inner_margin(egui::Margin::same(if own_window { 10 } else { 2 })).show(ui, |ui| {
            if !own_window {
                ui.set_width(380.0);
            }
            if rows.is_empty() {
                ui.add_space(10.0);
                ui.label(RichText::new(tl!("No reminders are due.")).font(regular(12.5)).color(t.text_dim));
                ui.add_space(10.0);
                if ui.button(tl!("Close")).clicked() {
                    closed = true;
                }
                return;
            }
            let list = if own_window { (ui.available_height() - 44.0).max(60.0) } else { 300.0 };
            egui::ScrollArea::vertical().max_height(list).auto_shrink([false, !own_window]).show(ui, |ui| {
                for r in &rows {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 4.0, t.hover);
                    }
                    icons::paint(
                        ui.painter(),
                        egui::Rect::from_center_size(egui::pos2(rect.min.x + 16.0, rect.center().y), vec2(16.0, 16.0)),
                        "bell",
                        t.icon,
                        t.accent,
                    );
                    let name = if r.title.trim().is_empty() { tl!("(no title)") } else { r.title.as_str() };
                    // An online meeting that has a link to join by gets a Join button beside Dismiss.
                    let join = app.session.mailbox.event(r.event).and_then(|e| e.join_link()).and_then(mailcraft_model::OnlineMeeting::clean_link);
                    let aside = if join.is_some() { 56.0 } else { 0.0 };
                    // Dismiss is as wide as its word needs in the interface language (64 at least).
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    let wide = ui.fonts_mut(|f| f.layout_no_wrap(tl!("Dismiss").to_string(), font, t.text).size().x + 16.0).max(64.0);
                    crate::theme::paint_clipped(
                        ui.painter(),
                        egui::pos2(rect.min.x + 34.0, rect.min.y + 15.0),
                        rect.width() - wide - 66.0 - aside,
                        name,
                        semibold(12.5),
                        t.text,
                    );
                    let time = if r.all_day { crate::dates::medium(r.start.date) } else { crate::dates::medium_time(r.start, twelve) };
                    let detail = if r.location.trim().is_empty() { time } else { format!("{time} · {}", r.location) };
                    crate::theme::paint_clipped(
                        ui.painter(),
                        egui::pos2(rect.min.x + 34.0, rect.min.y + 32.0),
                        rect.width() - wide - 66.0 - aside,
                        &detail,
                        regular(11.5),
                        t.text_dim,
                    );
                    let color = if r.minutes <= 0 { t.red } else { t.text_dim };
                    let said = egui::pos2(rect.max.x - wide - 14.0 - aside, rect.min.y + 15.0);
                    ui.painter().text(said, Align2::RIGHT_CENTER, when(r.minutes), regular(11.5), color);
                    let button = egui::Rect::from_min_size(egui::pos2(rect.max.x - wide - 6.0, rect.center().y - 12.0), vec2(wide, 24.0));
                    // The link is shown where it leads, and followed only on the click.
                    let mut pressed = false;
                    if let Some(link) = &join {
                        let place = egui::Rect::from_min_size(egui::pos2(button.min.x - aside, button.min.y), vec2(aside - 6.0, 24.0));
                        let joins = ui.put(place, egui::Button::new(tl!("Join"))).on_hover_text(link.as_str());
                        app.hit.add("reminderJoin", r.event, joins.rect);
                        if joins.clicked() {
                            joined = Some(r.event);
                            pressed = true;
                        }
                    }
                    if ui.put(button, egui::Button::new(tl!("Dismiss"))).clicked() {
                        dismissed = Some((r.event, r.start.iso()));
                    } else if resp.double_clicked() && !pressed {
                        opened = Some((r.event, r.start));
                    }
                    app.hit.add("reminder", r.event, rect);
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let mut minutes = ui.data(|d| d.get_temp::<u64>(egui::Id::unique("reminder_snooze"))).unwrap_or(5);
                if ui.button(tl!("Snooze")).on_hover_text(tl!("Remind me again later")).clicked() {
                    snoozed = Some(minutes);
                }
                egui::ComboBox::from_id_salt("reminder_snooze_for").selected_text(snooze_label(minutes)).show_ui(ui, |ui| {
                    for m in [5u64, 10, 15, 30, 60, 120, 1440] {
                        ui.selectable_value(&mut minutes, m, snooze_label(m));
                    }
                });
                ui.data_mut(|d| d.insert_temp(egui::Id::unique("reminder_snooze"), minutes));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(tl!("Dismiss All")).clicked() {
                        dismiss_all = true;
                    }
                });
            });
        });
    });
    if let Some((event, occurrence)) = dismissed {
        let _ = app.execute("ui.dismissReminder", json!({"event": event, "occurrence": occurrence}));
    }
    if dismiss_all {
        let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    }
    if let Some(minutes) = snoozed {
        let _ = app.execute("ui.snoozeReminders", json!({"minutes": minutes}));
    }
    if let Some((event, start)) = opened {
        open(app, event, start);
    }
    if let Some(event) = joined
        && let Err(e) = app.execute("ui.joinMeeting", json!({"event": event}))
    {
        app.status(e);
    }
    // Closing the window puts what is showing off for a while; it comes back, as a reminder should.
    if closed {
        if app.reminders.showing.is_empty() {
            app.reminders.asked_for = false;
        } else {
            let _ = app.execute("ui.snoozeReminders", json!({"minutes": 5}));
        }
    }
}

/// Open the item a reminder is for, as a double click on the reminder does: in a window of its
/// own, like a double click in the calendar; where that can't be (too many are open), as the
/// form in the main window. The reminder of an item that repeats is for one occurrence of it
/// (the one that starts at `start`), and that occurrence is what opens: nothing is asked, and
/// what its form saves is saved for that day alone.
pub(crate) fn open(app: &mut MailApp, event: u64, start: mailcraft_time::DateTime) {
    let Some(repeats) = app.session.mailbox.event(event).map(|e| e.recurrence.is_some()) else { return };
    let day = start.date.iso();
    // The occurrence is the one that is selected, as the calendar selects what it opens.
    let _ = app.execute("calendar.select", json!({"id": event, "day": day}));
    // Which of a series is meant, in the words the question about it would have been answered in.
    let which = if repeats { json!({"event": event, "scope": "occurrence", "occurrence": day}) } else { json!({"event": event}) };
    crate::dialogs::go_ahead(app, "ui.openWindow", which);
    if app.windows.of_event(event).is_none() && app.dialog.is_none() {
        let _ = app.run("ui.dialog", json!({"name": "editEvent", "id": event}));
    }
}

fn snooze_label(minutes: u64) -> String {
    match minutes {
        m if m < 60 => format!("{m} {}", tl!("minutes")),
        60 => tl!("1 hour").to_string(),
        m if m < 1440 => format!("{} {}", m / 60, tl!("hours")),
        _ => tl!("1 day").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_in_words() {
        assert_eq!(when(0), "now");
        assert_eq!(when(1), "in 1 minute");
        assert_eq!(when(15), "in 15 minutes");
        assert_eq!(when(-25), "25 minutes ago");
        assert_eq!(when(180), "in 3 hours");
        assert_eq!(when(-4000), "2 days ago");
        assert_eq!(when(i64::MIN), format!("{} days ago", i64::MIN.unsigned_abs() / 1440));
        assert_eq!(snooze_label(5), "5 minutes");
        assert_eq!(snooze_label(60), "1 hour");
        assert_eq!(snooze_label(120), "2 hours");
        assert_eq!(snooze_label(1440), "1 day");
    }

    /// A double click on a reminder opens the item it is for; of an item that repeats, the
    /// occurrence the reminder is for, without asking which is meant.
    #[test]
    fn a_reminder_opens_the_occurrence_it_is_for() {
        use mailcraft_engine::Session;
        use mailcraft_time::{Date, DateTime};
        // Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
        app.autosave = false;
        // (The sample has its own events; this test wants only the ones it makes.)
        let ids: Vec<u64> = app.session.mailbox.events.keys().copied().collect();
        for id in ids {
            app.execute("calendar.delete", json!({"id": id})).unwrap();
        }
        // Every day at ten past four since Monday, with a reminder; and one appointment today.
        let daily =
            json!({"title": "Daily sync", "start": "2026-10-12T16:10", "end": "2026-10-12T16:30", "recurrence": {"freq": "daily"}, "reminder": 15});
        let daily = app.execute("calendar.new", daily).unwrap()["id"].as_u64().unwrap();
        let single = app.execute("calendar.new", json!({"title": "Dentist", "start": "2026-10-14T16:20", "reminder": 30})).unwrap();
        let single = single["id"].as_u64().unwrap();
        app.execute("calendar.select", json!({"id": null})).unwrap();
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut frame = |app: &mut MailApp, events: Vec<egui::Event>| {
            time += 1.0 / 60.0;
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(1280.0, 800.0));
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(time), events, ..Default::default() };
            let output = ctx.run_ui(input, |ui| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            });
            output.drop_without_applying_deltas();
        };
        for _ in 0..6 {
            frame(&mut app, Vec::new());
        }
        // Yesterday's occurrence (over for less than a day), today's, and the appointment.
        let day = |n: u8| Date::new(2026, 10, n).unwrap();
        let at = |date: Date, hour: u8, minute: u8| DateTime::new(date, hour, minute, 0).unwrap();
        let showing: Vec<(u64, DateTime)> = app.reminders.showing.iter().map(|r| (r.event, r.start)).collect();
        assert_eq!(showing, [(daily, at(day(13), 16, 10)), (daily, at(day(14), 16, 10)), (single, at(day(14), 16, 20))]);

        // A double click on the first of them, which is yesterday's.
        let row = app.hit.rect("reminder", daily).expect("the reminders are on screen");
        let pos = egui::pos2(row.min.x + 60.0, row.center().y);
        frame(&mut app, vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false, true, false] {
            let button = egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            frame(&mut app, vec![egui::Event::PointerMoved(pos), button]);
        }
        for _ in 0..3 {
            frame(&mut app, Vec::new());
        }
        // It opened, nothing was asked, and it is that occurrence that is meant.
        assert!(!matches!(app.dialog, Some(crate::dialogs::Dialog::Confirm { .. })), "no question about the series");
        assert!(app.windows.of_event(daily).is_some() || app.dialog.as_ref().is_some_and(|d| d.name() == "editEvent"));
        assert_eq!(app.calendar.opened, Some((daily, Some(day(13)))));
        assert_eq!((app.session.sel.event, app.session.sel.event_day), (Some(daily), Some(day(13))));
        // What its form saves is saved for that day alone, as if the question had been answered.
        let saved = crate::dialogs::as_opened(&mut app, "calendar.edit", json!({"id": daily, "title": "Daily sync, moved"}));
        assert_eq!((saved["scope"].as_str(), saved["occurrence"].as_str()), (Some("occurrence"), Some("2026-10-13")));
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;

        // Today's, by its start; and an appointment that doesn't repeat simply opens.
        open(&mut app, daily, at(day(14), 16, 10));
        assert_eq!(app.calendar.opened, Some((daily, Some(day(14)))));
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;
        open(&mut app, single, at(day(14), 16, 20));
        assert_eq!((app.calendar.opened, app.session.sel.event), (None, Some(single)));
        assert!(app.windows.of_event(single).is_some() || app.dialog.is_some());
        // An item that is gone opens nothing, and a start nobody could mean is still a day.
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;
        open(&mut app, 999_999, at(day(14), 16, 20));
        assert!(app.dialog.is_none() && app.windows.of_event(999_999).is_none());
        open(&mut app, daily, at(Date::new(9999, 12, 31).unwrap(), 23, 59));
    }
}
