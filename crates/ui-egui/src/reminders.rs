// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reminders that fire: a small window listing the events whose reminder time has come, with
//! Dismiss and Snooze, as the incumbent has. What is due is the engine's answer
//! (`view::due_reminders`); what was dismissed is kept with the window's settings, what was put
//! off only for this run.
//!
//! The window is drawn in a pass of its own (a deferred viewport), not inside the main window's:
//! a compositor gives a minimized window no frames (KDE Plasma and others on Wayland), and the
//! main window's pass, and so a window drawn inside it, waited for the owner to restore it (#771).
//! It shows what [`Shared`] holds and leaves there what was clicked; [`carry_out`] does that from
//! the main window's logic, which runs while it is minimized too.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

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
    /// What the window shows and what was done in it, shared with its own pass.
    shared: Arc<Mutex<Shared>>,
    /// The main window's passes that laid it out, counted.
    passes: u64,
    /// The window was shown in the main window's last pass.
    on_screen: bool,
    /// The window is no longer wanted but still on screen, since the main window's pass
    /// numbered so: no pass of it has come since to take it away.
    ending: Option<u64>,
    /// It was minimized for that, and is made afresh when it is wanted again.
    minimized: bool,
}

/// What the window shows, set from the main window; and what was done in it, for the main
/// window to carry out.
#[derive(Default)]
pub(crate) struct Shared {
    rows: Vec<Row>,
    twelve_hour: bool,
    /// The window's title, and the one it was last given.
    title: String,
    titled: String,
    actions: Vec<Action>,
    /// Where the reminders and their Join buttons were laid out (for [`MailApp::hit`]).
    hits: Vec<(&'static str, u64, egui::Rect)>,
}

/// A reminder as the window shows it, with the link to join its meeting by.
#[derive(Clone, PartialEq)]
struct Row {
    reminder: Reminder,
    join: Option<String>,
}

/// What was done in the window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Action {
    Dismiss(u64, String),
    DismissAll,
    Snooze(u64),
    Open(u64, mailcraft_time::DateTime),
    Join(u64),
    Close,
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
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

fn viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("mailcraft_reminders")
}

/// The window's layer where egui draws it inside the main one (in a browser).
pub(crate) fn embedded_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Middle, egui::Id::unique(viewport()))
}

/// The window, while something is due or the owner asked for it: a window of its own, which
/// can be put wherever the owner likes (where the system has no separate windows, as in a
/// browser, a movable window inside the main one).
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    app.reminders.passes = app.reminders.passes.wrapping_add(1);
    app.reminders.ending = None;
    let wanted = !app.reminders.showing.is_empty() || app.reminders.asked_for;
    // A window minimized because the main window had no passes ([`carry_out`]) is let go in
    // this pass and made afresh in the next, so that it comes back in front.
    if std::mem::take(&mut app.reminders.minimized) {
        app.reminders.on_screen = false;
        if wanted {
            ctx.request_repaint();
        }
        return;
    }
    app.reminders.on_screen = wanted;
    if !wanted {
        return;
    }
    publish(app, ctx);
    let shared = Arc::clone(&app.reminders.shared);
    let (title, rows) = {
        let mut s = lock(&shared);
        s.titled = s.title.clone();
        (s.title.clone(), s.rows.len())
    };
    let height = (rows.clamp(1, 6) as f32) * 46.0 + 96.0;
    let builder = crate::windows::like_main(
        ctx,
        egui::ViewportBuilder::default().with_title(title).with_inner_size([420.0, height]).with_min_inner_size([340.0, 150.0]),
    );
    let inside = Arc::clone(&shared);
    ctx.show_viewport_deferred(viewport(), builder, move |ui, class| paint(ui, class, &inside));
    let hits = lock(&shared).hits.clone();
    for (kind, id, rect) in hits {
        app.hit.add(kind, id, rect);
    }
    // Inside the main window (in a browser) the window's pass was just now.
    apply(app, ctx);
}

/// From the main window's logic, which runs while it is minimized too: what was done in the
/// window is carried out, and what it shows brought up to date.
pub fn carry_out(app: &mut MailApp, ctx: &egui::Context) {
    apply(app, ctx);
    let wanted = !app.reminders.showing.is_empty() || app.reminders.asked_for;
    if wanted || !app.reminders.on_screen {
        app.reminders.ending = None;
        return;
    }
    // No longer wanted: the main window's next pass takes it away. A minimized main window has
    // no passes where the compositor gives it no frames (Wayland), and only its logic runs;
    // when the logic comes round again with no pass between, the window is minimized instead,
    // the one thing such a system lets a program do to put it out of sight.
    match app.reminders.ending {
        None => {
            app.reminders.ending = Some(app.reminders.passes);
            ctx.request_repaint();
        }
        Some(pass) if pass == app.reminders.passes && !app.reminders.minimized => {
            ctx.send_viewport_cmd_to(viewport(), egui::ViewportCommand::Minimized(true));
            app.reminders.minimized = true;
        }
        Some(_) => {}
    }
}

/// Carry out what was done in the window, and give it what it is to show.
fn apply(app: &mut MailApp, ctx: &egui::Context) {
    let actions = std::mem::take(&mut lock(&app.reminders.shared).actions);
    for action in actions {
        match action {
            Action::Dismiss(event, occurrence) => {
                let _ = app.execute("ui.dismissReminder", json!({"event": event, "occurrence": occurrence}));
            }
            Action::DismissAll => {
                let _ = app.execute("ui.dismissReminder", json!({"all": true}));
            }
            Action::Snooze(minutes) => {
                let _ = app.execute("ui.snoozeReminders", json!({"minutes": minutes}));
            }
            Action::Open(event, start) => open(app, event, start),
            Action::Join(event) => {
                if let Err(e) = app.execute("ui.joinMeeting", json!({"event": event})) {
                    app.status(e);
                }
            }
            // Closing the window puts what is showing off for a while; it comes back, as a
            // reminder should.
            Action::Close => {
                if app.reminders.showing.is_empty() {
                    app.reminders.asked_for = false;
                } else {
                    let _ = app.execute("ui.snoozeReminders", json!({"minutes": 5}));
                }
            }
        }
    }
    publish(app, ctx);
}

/// What the window shows, from what is due; its pass is asked for when that changed.
fn publish(app: &MailApp, ctx: &egui::Context) {
    let rows: Vec<Row> = app
        .reminders
        .showing
        .iter()
        .map(|r| Row {
            reminder: r.clone(),
            // An online meeting that has a link to join by gets a Join button beside Dismiss.
            join: app.session.mailbox.event(r.event).and_then(|e| e.join_link()).and_then(mailcraft_model::OnlineMeeting::clean_link),
        })
        .collect();
    let twelve_hour = app.session.prefs.twelve_hour;
    let title = tln!(rows.len(), "{n} Reminder", "{n} Reminders");
    let mut s = lock(&app.reminders.shared);
    if s.rows != rows || s.twelve_hour != twelve_hour || s.title != title {
        (s.rows, s.twelve_hour, s.title) = (rows, twelve_hour, title);
        drop(s);
        ctx.request_repaint_of(viewport());
    }
}

/// The window's own pass: what [`Shared`] holds, and what is clicked left there for the main
/// window, which is woken for it.
fn paint(ui: &mut egui::Ui, class: egui::ViewportClass, shared: &Mutex<Shared>) {
    let ctx = ui.ctx().clone();
    let t = Tokens::get(&ctx);
    let mut s = lock(shared);
    let (rows, twelve) = (s.rows.clone(), s.twelve_hour);
    let mut done = Vec::new();
    let mut hits = Vec::new();
    let own_window = class != egui::ViewportClass::EmbeddedWindow;
    if own_window {
        if ctx.input(|i| i.viewport().close_requested()) {
            done.push(Action::Close);
        }
        // The title is set in the main window's pass, which a minimized one may not have.
        if s.titled != s.title {
            s.titled = s.title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(s.title.clone()));
        }
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
                done.push(Action::Close);
            }
            return;
        }
        let list = if own_window { (ui.available_height() - 44.0).max(60.0) } else { 300.0 };
        egui::ScrollArea::vertical().max_height(list).auto_shrink([false, !own_window]).show(ui, |ui| {
            for Row { reminder: r, join } in &rows {
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
                if let Some(link) = join {
                    let place = egui::Rect::from_min_size(egui::pos2(button.min.x - aside, button.min.y), vec2(aside - 6.0, 24.0));
                    let joins = ui.put(place, egui::Button::new(tl!("Join"))).on_hover_text(link.as_str());
                    hits.push(("reminderJoin", r.event, joins.rect));
                    if joins.clicked() {
                        done.push(Action::Join(r.event));
                        pressed = true;
                    }
                }
                if ui.put(button, egui::Button::new(tl!("Dismiss"))).clicked() {
                    done.push(Action::Dismiss(r.event, r.start.iso()));
                } else if resp.double_clicked() && !pressed {
                    done.push(Action::Open(r.event, r.start));
                }
                hits.push(("reminder", r.event, rect));
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            let mut minutes = ui.data(|d| d.get_temp::<u64>(egui::Id::unique("reminder_snooze"))).unwrap_or(5);
            if ui.button(tl!("Snooze")).on_hover_text(tl!("Remind me again later")).clicked() {
                done.push(Action::Snooze(minutes));
            }
            egui::ComboBox::from_id_salt("reminder_snooze_for").selected_text(snooze_label(minutes)).show_ui(ui, |ui| {
                for m in [5u64, 10, 15, 30, 60, 120, 1440] {
                    ui.selectable_value(&mut minutes, m, snooze_label(m));
                }
            });
            ui.data_mut(|d| d.insert_temp(egui::Id::unique("reminder_snooze"), minutes));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tl!("Dismiss All")).clicked() {
                    done.push(Action::DismissAll);
                }
            });
        });
    });
    s.hits = hits;
    if !done.is_empty() {
        s.actions.extend(done);
        drop(s);
        // In a window of its own, the main window's logic carries it out.
        ctx.request_repaint_of(egui::ViewportId::ROOT);
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
#[path = "reminders_tests.rs"]
mod reminders_tests;

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
        assert_eq!(app.session.driver.calendar.opened, Some((daily, Some(day(13)))));
        assert_eq!((app.session.sel.event, app.session.sel.event_day), (Some(daily), Some(day(13))));
        // What its form saves is saved for that day alone, as if the question had been answered.
        let saved = crate::dialogs::as_opened(&mut app, "calendar.edit", json!({"id": daily, "title": "Daily sync, moved"}));
        assert_eq!((saved["scope"].as_str(), saved["occurrence"].as_str()), (Some("occurrence"), Some("2026-10-13")));
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;

        // Today's, by its start; and an appointment that doesn't repeat simply opens.
        open(&mut app, daily, at(day(14), 16, 10));
        assert_eq!(app.session.driver.calendar.opened, Some((daily, Some(day(14)))));
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;
        open(&mut app, single, at(day(14), 16, 20));
        assert_eq!((app.session.driver.calendar.opened, app.session.sel.event), (None, Some(single)));
        assert!(app.windows.of_event(single).is_some() || app.dialog.is_some());
        // An item that is gone opens nothing, and a start nobody could mean is still a day.
        let _ = app.execute("ui.closeWindow", json!({"all": true}));
        app.dialog = None;
        open(&mut app, 999_999, at(day(14), 16, 20));
        assert!(app.dialog.is_none() && app.windows.of_event(999_999).is_none());
        open(&mut app, daily, at(Date::new(9999, 12, 31).unwrap(), 23, 59));
    }
}
