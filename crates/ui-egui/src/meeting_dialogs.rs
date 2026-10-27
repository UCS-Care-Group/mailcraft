// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The dialogs of a meeting that write to other people, and the one that lists what they
//! proposed: Propose New Time, Forward, and View All Proposals.
//!
//! **Propose New Time** and **Forward** are mail: the first writes to the meeting's organizer, the
//! second to whoever is named. Each says who will be written to, and sends from its own button
//! and from nowhere else: a key doesn't send (Enter is nobody's here, Escape cancels), and what
//! the button runs is the command with `"send": true` (`calendar.propose`, `calendar.forward`),
//! which the engine refuses without it. The mail itself is written by the account's server (or,
//! for an account without one, goes from the Outbox at the next Send/Receive).
//!
//! **View All Proposals** sends nothing: accepting a proposal opens the meeting with the proposed
//! time in its fields, where the owner sends the update as any other, through its question.
//!
//! Everything shown of a meeting that came from outside (its title, the organizer's name, a
//! proposal's sender) is data: cut to size, shown as text, never followed.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{Address, Event, Id, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::MailApp;
use crate::forms::kit::{self, Pass};
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::widgets::primary_button;

/// How many hours of the day the look at the owner's calendar shows at most, and how tall it is.
const STRIP_HOURS: i64 = 6;
const STRIP_H: f32 = 132.0;
/// Most proposals listed, and most people a forward is shown as going to.
const MAX_ROWS: usize = 50;
const MAX_NAMED: usize = 12;

/// One of the three dialogs.
pub enum MeetingDialog {
    Propose(Box<ProposeTime>),
    Forward(Box<ForwardMeeting>),
    Proposals(Box<Proposals>),
}

/// A name from outside, for a label: one line, not too long, never empty.
fn shown_name(name: &str, email: &str) -> String {
    let text = if name.trim().is_empty() { email.trim() } else { name.trim() };
    let text = kit::shown(text, 80);
    if text.is_empty() { tl!("Somebody").to_string() } else { text }
}

/// `Thursday, October 15, 2026 10:00 AM – 11:00 AM`.
fn when_text(start: DateTime, end: DateTime, all_day: bool, twelve: bool) -> String {
    use crate::dates::{clock, long};
    if all_day {
        let last = end.add_minutes(-1).date.max(start.date);
        return if last == start.date { long(start.date) } else { format!("{} – {}", long(start.date), long(last)) };
    }
    let (from, to) = (clock(start, twelve), clock(end, twelve));
    if start.date == end.date {
        format!("{} {from} – {to}", long(start.date))
    } else {
        format!("{} {from} – {} {to}", long(start.date), long(end.date))
    }
}

fn time_of(text: &str) -> Option<DateTime> {
    mailcraft_time::parse_iso(text).map(|(at, _)| at)
}

/// The date and the time of two fields as one moment.
fn moment(date: &str, time: &str) -> Option<DateTime> {
    let day = Date::parse_iso(date.trim())?;
    let (hour, minute) = kit::parse_clock(time)?;
    DateTime::new(day, hour, minute, 0)
}

/// Propose New Time: another start and end for somebody else's meeting, sent to its organizer
/// with a tentative answer or a decline.
pub struct ProposeTime {
    /// What is answered: the item on the calendar, or the invitation a message carries.
    event: Option<Id>,
    message: Option<Id>,
    /// For one occurrence of a series: its day.
    occurrence: Option<Date>,
    title: String,
    /// Who will be written to.
    organizer: String,
    /// When the meeting is now, in words.
    current: String,
    start_date: String,
    start_time: String,
    end_date: String,
    end_time: String,
    /// The length the proposal keeps when its start moves.
    minutes: i64,
    note: String,
    /// Why another time can't be proposed for it; empty when it can.
    blocked: String,
    error: String,
    focus: bool,
}

impl ProposeTime {
    fn open(app: &mut MailApp, p: &Value) -> Option<ProposeTime> {
        let twelve = app.session.prefs.twelve_hour;
        let number = |key: &str| p.get(key).and_then(Value::as_u64).filter(|id| *id != 0);
        let mut dialog = ProposeTime { focus: true, ..ProposeTime::empty() };
        let (start, end, all_day);
        if let Some(message) = number("message") {
            // The invitation in the mail, as the engine reads it (a pure command: nothing changes).
            let told = app.session.run("calendar.invitation", &json!({"message": message})).ok()?;
            let text = |key: &str| told.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
            all_day = told.get("allDay").and_then(Value::as_bool).unwrap_or(false);
            (start, end) = if all_day {
                let day = |key: &str| Date::parse_iso(&text(key)).map(DateTime::at_midnight);
                (day("start")?, day("end")?.add_days(1))
            } else {
                (time_of(&text("start"))?, time_of(&text("end"))?)
            };
            let who = told.get("people").and_then(|people| people.get("organizer"));
            let part = |key: &str| who.and_then(|who| who.get(key)).and_then(Value::as_str).unwrap_or_default();
            dialog.message = Some(message);
            dialog.title = kit::shown(&text("title"), 120);
            dialog.organizer = shown_name(part("name"), part("email"));
            if told.get("canPropose").and_then(Value::as_bool) != Some(true) {
                dialog.blocked = if told.get("answerable").and_then(Value::as_bool) != Some(true) {
                    tl!("This invitation can't be answered from here, so no other time can be proposed for it.").to_string()
                } else if told.get("canSend").and_then(Value::as_bool) != Some(true) {
                    tl!("The organizer can't be written to from this account.").to_string()
                } else {
                    tl!("The organizer of this meeting doesn't take proposals of another time.").to_string()
                };
            }
        } else {
            let s = &app.session;
            let event = s.mailbox.event(number("id").or(number("event")).or(s.sel.event)?)?;
            // Of a series: the occurrence that is selected, which is the one the proposal is for.
            let day = s.sel.event_day.filter(|_| s.sel.event == Some(event.id) && event.recurrence.is_some());
            // (When that occurrence is by the clock here: a series may repeat by another zone's.)
            let here = |instant: mailcraft_time::Timestamp| s.offset_at(instant);
            let day = day.filter(|day| event.series_day_on(*day, &here).is_some());
            let times = day.map(|day| event.occurrence_times(event.series_day_on(day, &here).unwrap_or(day), &here));
            let apart = day.map_or(0, |day| event.start.date.days_until(day));
            (start, end) = match times.filter(|_| !event.all_day) {
                Some(times) => times,
                None => (event.start.add_days(apart), event.end.add_days(apart)),
            };
            all_day = event.all_day;
            dialog.event = Some(event.id);
            dialog.occurrence = day;
            dialog.title = kit::shown(&event.title, 120);
            let who = event.organizer.clone().unwrap_or_default();
            dialog.organizer = shown_name(&who.name, &who.email);
            dialog.blocked = Self::why_not(app, event);
        }
        dialog.current = when_text(start, end, all_day, twelve);
        // It opens on the time the meeting has, for the owner to move.
        let (start, end) = if all_day {
            let nine = DateTime { hour: app.session.prefs.work_start.min(23), ..DateTime::at_midnight(start.date) };
            (nine, nine.add_minutes(60))
        } else {
            (start, end.max(start.add_minutes(1)))
        };
        dialog.minutes = start.minutes_until(end).max(1);
        (dialog.start_date, dialog.start_time) = (start.date.iso(), start.time_short(twelve));
        (dialog.end_date, dialog.end_time) = (end.date.iso(), end.time_short(twelve));
        Some(dialog)
    }

    fn empty() -> ProposeTime {
        ProposeTime {
            event: None,
            message: None,
            occurrence: None,
            title: String::new(),
            organizer: String::new(),
            current: String::new(),
            start_date: String::new(),
            start_time: String::new(),
            end_date: String::new(),
            end_time: String::new(),
            minutes: 60,
            note: String::new(),
            blocked: String::new(),
            error: String::new(),
            focus: false,
        }
    }

    /// Why another time can't be proposed for an item of the calendar; empty when it can.
    fn why_not(app: &MailApp, event: &Event) -> String {
        let kept = mailcraft_engine::cmd::remote::keeper(&app.session.mailbox, &event.origin).is_some();
        if !kept {
            tl!("Another time can be proposed for a meeting that the account's server keeps, or from the invitation in the mail.").to_string()
        } else if !event.origin.guest {
            tl!("You organize this meeting, so there is nobody to propose another time to.").to_string()
        } else if !event.response_options.allow_proposals {
            tl!("The organizer of this meeting doesn't take proposals of another time.").to_string()
        } else {
            String::new()
        }
    }

    fn span(&self) -> Option<(DateTime, DateTime)> {
        let (start, end) = (moment(&self.start_date, &self.start_time)?, moment(&self.end_date, &self.end_time)?);
        (end > start).then_some((start, end))
    }

    /// The command the button runs: the answer, the other time, and the owner's yes to the mail.
    fn command(&self, response: &str) -> Option<Value> {
        let (start, end) = self.span()?;
        let mut params = json!({"start": start.iso(), "end": end.iso(), "response": response, "send": true});
        let fields = params.as_object_mut()?;
        match (self.message, self.event) {
            (Some(message), _) => fields.insert("message".into(), json!(message)),
            (None, Some(event)) => fields.insert("id".into(), json!(event)),
            (None, None) => return None,
        };
        if let Some(day) = self.occurrence {
            fields.insert("scope".into(), json!("occurrence"));
            fields.insert("occurrence".into(), json!(day.iso()));
        } else if self.event.is_some() {
            fields.insert("scope".into(), json!("series"));
        }
        if !self.note.trim().is_empty() {
            fields.insert("comment".into(), json!(self.note.trim()));
        }
        Some(params)
    }

    /// Lay the dialog out; `true` when it is to close.
    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut pass = Pass::new(ui, &app.session, &mut self.focus);
        let twelve = pass.twelve;
        let about = crate::i18n::fmt(tl!("{name} organizes \"{title}\"."), &[("name", &self.organizer), ("title", &self.title)]);
        ui.add(egui::Label::new(egui::RichText::new(about).font(semibold(12.5))).wrap());
        ui.label(egui::RichText::new(crate::i18n::fmt(tl!("It is now on {when}."), &[("when", &self.current)])).color(t.text_dim));
        if let Some(day) = self.occurrence {
            let one = crate::i18n::fmt(tl!("This is about the occurrence on {day} only."), &[("day", &crate::dates::long(day))]);
            ui.label(egui::RichText::new(one).color(t.text_dim));
        }
        ui.add_space(2.0);
        kit::row(ui, tl!("Proposed start"), |ui| {
            let mut moved = pass.date(ui, "propose_start_date", &mut self.start_date, true);
            moved |= pass.time(ui, "propose_start_time", &mut self.start_time, true, None);
            // The end follows the start, as long as the proposal was.
            if moved && let Some(start) = moment(&self.start_date, &self.start_time) {
                let end = start.add_minutes(self.minutes.max(1));
                (self.end_date, self.end_time) = (end.date.iso(), end.time_short(twelve));
            }
        });
        kit::row(ui, tl!("Proposed end"), |ui| {
            let mut moved = pass.date(ui, "propose_end_date", &mut self.end_date, true);
            let after = moment(&self.start_date, &self.start_time).filter(|start| Date::parse_iso(&self.end_date) == Some(start.date));
            moved |= pass.time(ui, "propose_end_time", &mut self.end_time, true, after.map(|start| start.minutes_of_day()));
            if moved && let Some((start, end)) = self.span() {
                self.minutes = start.minutes_until(end).max(1);
            }
        });
        // The owner's own calendar on that day, with the proposal in it.
        let span = self.span();
        let day = span.map(|(start, _)| start.date).or_else(|| Date::parse_iso(&self.start_date));
        if let Some(day) = day {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), STRIP_H), Sense::hover());
            day_strip(app, ui, rect, day, span, self.event, &t, twelve);
        }
        let note = ui.add(
            egui::TextEdit::multiline(&mut self.note)
                .id(kit::field_id("propose_note"))
                .hint_text(tl!("A note to the organizer (optional)"))
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .char_limit(2_000),
        );
        crate::text_menu::field(note);
        // Who is written to, said before anything is pressed.
        let able = self.blocked.is_empty();
        let says = if able {
            crate::i18n::fmt(tl!("Either button writes to {name}: your answer, and the time you propose."), &[("name", &self.organizer)])
        } else {
            self.blocked.clone()
        };
        ui.add(egui::Label::new(egui::RichText::new(says).color(if able { t.text_dim } else { t.red })).wrap());
        if span.is_none() {
            ui.label(egui::RichText::new(tl!("The proposed end has to be after the proposed start.")).color(t.red));
        }
        kit::error(ui, &self.error);
        let ready = able && span.is_some();
        let (mut close, mut picked) = (false, None);
        crate::dialogs::button_row(ui, |ui| {
            let cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
            app.hit.add("dialogCancel", 0, cancel.rect);
            close = cancel.clicked();
            let decline = ui.add_enabled(ready, egui::Button::new(tl!("Decline and Propose")).min_size(vec2(72.0, 26.0)));
            app.hit.add("proposeDecline", 0, decline.rect);
            if decline.clicked() {
                picked = Some("decline");
            }
            let tentative = primary_button(ui, tl!("Tentative and Propose"), ready);
            app.hit.add("proposeTentative", 0, tentative.rect);
            if tentative.clicked() {
                picked = Some("tentative");
            }
        });
        // The owner pressed one of the two: that is the yes the mail goes on.
        if let Some(params) = picked.and_then(|response| self.command(response)) {
            match app.execute("calendar.propose", params) {
                Ok(_) => {
                    app.status(crate::i18n::fmt(tl!("Another time was proposed to {name}"), &[("name", &self.organizer)]));
                    close = true;
                }
                Err(e) => self.error = e,
            }
        }
        close
    }
}

/// A look at the owner's own calendar on `day`: the hours round `span`, what is on in the
/// calendars that are shown, and the proposed time. `skip` is the meeting itself, which would
/// otherwise stand in its own way.
#[allow(clippy::too_many_arguments)]
fn day_strip(app: &MailApp, ui: &Ui, rect: Rect, day: Date, span: Option<(DateTime, DateTime)>, skip: Option<Id>, t: &Tokens, twelve: bool) {
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect(rect, 4.0, t.surface, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    let head = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 20.0));
    let label = crate::i18n::fmt(tl!("Your calendar: {day}"), &[("day", &crate::dates::long(day))]);
    paint_clipped(&p, pos2(head.min.x + 8.0, head.center().y), head.width() - 16.0, &label, semibold(11.0), t.text_dim);
    p.line_segment([pos2(rect.min.x, head.max.y), pos2(rect.max.x, head.max.y)], Stroke::new(1.0, t.border));
    let grid = Rect::from_min_max(pos2(rect.min.x, head.max.y), rect.max);
    let minutes = |at: DateTime| day.days_until(at.date).clamp(-1, 2) * 24 * 60 + i64::from(at.minutes_of_day());
    let (starts, ends) = match span {
        Some((start, end)) => (minutes(start), minutes(end).max(minutes(start) + 1)),
        None => (9 * 60, 10 * 60),
    };
    let first = (starts.div_euclid(60) - 1).clamp(0, 24 - STRIP_HOURS);
    let last = ((ends + 59).div_euclid(60) + 1).clamp(first + 3, first + STRIP_HOURS).min(24);
    let per_minute = grid.height() / ((last - first) * 60) as f32;
    let y_of = |minute: i64| grid.min.y + (minute - first * 60).clamp(0, (last - first) * 60) as f32 * per_minute;
    const HOURS_W: f32 = 52.0;
    for hour in first..last {
        let y = y_of(hour * 60);
        if hour > first {
            p.line_segment([pos2(grid.min.x, y), pos2(grid.max.x, y)], Stroke::new(1.0, t.border));
        }
        let label = crate::dates::time(u8::try_from(hour).unwrap_or(0), 0, twelve);
        p.text(pos2(grid.min.x + HOURS_W - 6.0, y + 2.0), Align2::RIGHT_TOP, label, regular(10.0), t.text_dim);
    }
    p.line_segment([pos2(grid.min.x + HOURS_W, grid.min.y), pos2(grid.min.x + HOURS_W, grid.max.y)], Stroke::new(1.0, t.border));
    let lane = Rect::from_min_max(pos2(grid.min.x + HOURS_W + 2.0, grid.min.y), pos2(grid.max.x - 3.0, grid.max.y));
    let half = lane.center().x;
    let block = |from: i64, to: i64, left: f32, right: f32| {
        let (top, bottom) = (y_of(from), y_of(to));
        Rect::from_min_max(pos2(left, top + 1.0), pos2(right, bottom.max(top + 12.0) - 1.0))
    };
    // What else is on, to the right; the proposal to the left.
    let others = mailcraft_engine::cmd::calendar::occurrences(&app.session, day, day);
    for occurrence in others.iter().filter(|o| Some(o.event) != skip).take(40) {
        let Some(event) = app.session.mailbox.event(occurrence.event).filter(|event| !event.all_day) else { continue };
        let (from, to) = (minutes(occurrence.start), minutes(occurrence.end));
        if to <= first * 60 || from >= last * 60 || to <= from {
            continue;
        }
        let r = block(from, to, half + 1.0, lane.max.x);
        let clash = span.is_some() && from < ends && to > starts && event.show_as != ShowAs::Free;
        p.rect(r, 2.0, if event.show_as == ShowAs::Free { t.surface } else { t.hover }, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_min_max(r.min, pos2(r.min.x + 3.0, r.max.y)), 1.0, if clash { t.orange } else { t.border_strong });
        if r.height() >= 13.0 {
            paint_clipped(&p, pos2(r.min.x + 7.0, r.min.y + 7.0), r.width() - 10.0, &kit::shown(&event.title, 80), regular(10.5), t.text);
        }
    }
    if span.is_some() {
        let r = block(starts, ends, lane.min.x, half - 1.0);
        p.rect(r, 2.0, t.accent.linear_multiply(if t.dark { 0.35 } else { 0.16 }), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        if r.height() >= 13.0 {
            paint_clipped(&p, pos2(r.min.x + 6.0, r.min.y + 7.0), r.width() - 9.0, tl!("Proposed time"), semibold(10.5), t.text);
        }
    }
}

/// Forward: a meeting or an appointment sent on to more people, with a note.
pub struct ForwardMeeting {
    event: Id,
    /// For one occurrence of a series: its day.
    occurrence: Option<Date>,
    title: String,
    when: String,
    /// An account's server keeps it, and forwards it itself; otherwise it goes as mail.
    on_server: bool,
    /// The owner is invited to it: its organizer is then told of the forward by the server.
    guest: bool,
    to: String,
    note: String,
    error: String,
    focus: bool,
}

impl ForwardMeeting {
    fn open(app: &MailApp, p: &Value) -> Option<ForwardMeeting> {
        let s = &app.session;
        let number = |key: &str| p.get(key).and_then(Value::as_u64).filter(|id| *id != 0);
        let event = s.mailbox.event(number("id").or(number("event")).or(s.sel.event)?)?;
        // Which of a series: as the command was told, or the occurrence that is selected when
        // it was told "this one"; the whole series when nothing says.
        let said = p.get("occurrence").and_then(Value::as_str).and_then(Date::parse_iso);
        let one = p.get("scope").and_then(Value::as_str) == Some("occurrence");
        let day = said.or(s.sel.event_day.filter(|_| one && s.sel.event == Some(event.id))).filter(|_| event.recurrence.is_some());
        // (When that occurrence is by the clock here: a series may repeat by another zone's.)
        let here = |instant: mailcraft_time::Timestamp| s.offset_at(instant);
        let day = day.filter(|day| event.series_day_on(*day, &here).is_some());
        let apart = day.map_or(0, |day| event.start.date.days_until(day));
        let (starts, ends) = match day.filter(|_| !event.all_day) {
            Some(day) => event.occurrence_times(event.series_day_on(day, &here).unwrap_or(day), &here),
            None => (event.start.add_days(apart), event.end.add_days(apart)),
        };
        let mut when = when_text(starts, ends, event.all_day, s.prefs.twelve_hour);
        if let (Some(rule), None) = (&event.recurrence, day) {
            when = format!("{when} ({})", kit::shown(&rule.describe(event.start.date), 120));
        }
        Some(ForwardMeeting {
            event: event.id,
            occurrence: day,
            title: kit::shown(&event.title, 120),
            when,
            on_server: mailcraft_engine::cmd::remote::keeper(&s.mailbox, &event.origin).is_some(),
            guest: event.origin.guest,
            to: p.get("to").and_then(Value::as_str).unwrap_or_default().chars().take(4_000).collect(),
            note: String::new(),
            error: String::new(),
            focus: true,
        })
    }

    /// Who the line names: those with an address that is one, and what isn't one.
    fn named(&self) -> (Vec<Address>, Vec<String>) {
        let (mut good, mut bad): (Vec<Address>, Vec<String>) = (Vec::new(), Vec::new());
        for who in Address::parse_list(&self.to).into_iter().take(200) {
            if !who.is_valid() {
                bad.push(kit::shown(if who.email.trim().is_empty() { &who.name } else { &who.email }, 60));
            } else if !good.iter().any(|have| have.same_mailbox(&who)) {
                good.push(who);
            }
        }
        (good, bad)
    }

    fn command(&self) -> Value {
        let mut params = json!({"id": self.event, "to": self.to, "send": true});
        if let Some(fields) = params.as_object_mut() {
            match self.occurrence {
                Some(day) => {
                    fields.insert("scope".into(), json!("occurrence"));
                    fields.insert("occurrence".into(), json!(day.iso()));
                }
                None => {
                    fields.insert("scope".into(), json!("series"));
                }
            }
            if !self.note.trim().is_empty() {
                fields.insert("comment".into(), json!(self.note.trim()));
            }
        }
        params
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut pass = Pass::new(ui, &app.session, &mut self.focus);
        ui.add(egui::Label::new(egui::RichText::new(self.title.as_str()).font(semibold(12.5))).wrap());
        ui.add(egui::Label::new(egui::RichText::new(self.when.as_str()).color(t.text_dim)).wrap());
        ui.add_space(2.0);
        kit::row(ui, tl!("To"), |ui| {
            pass.text(ui, "forward_to", &mut self.to, f32::INFINITY, tl!("Names or addresses, separated by ;"));
        });
        let note = ui.add(
            egui::TextEdit::multiline(&mut self.note)
                .id(kit::field_id("forward_note"))
                .hint_text(tl!("A note to go with it (optional)"))
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .char_limit(2_000),
        );
        crate::text_menu::field(note);
        // Who is written to, said before Send is pressed.
        let (good, bad) = self.named();
        let names: Vec<String> = good.iter().take(MAX_NAMED).map(|who| kit::shown(&who.email, 80)).collect();
        let more = good.len().saturating_sub(names.len());
        let says = if good.is_empty() {
            tl!("Name who it is to go to. Nobody is written to until you press Send.").to_string()
        } else {
            let list = if more > 0 {
                crate::i18n::fmt(tl!("{names} and {n} more"), &[("names", &names.join(", ")), ("n", &more.to_string())])
            } else {
                names.join(", ")
            };
            crate::i18n::fmt(tl!("Send writes to: {people}."), &[("people", &list)])
        };
        ui.add(egui::Label::new(egui::RichText::new(says).color(t.text_dim)).wrap());
        if !bad.is_empty() {
            let words = crate::i18n::fmt(tl!("Not an e-mail address: {names}"), &[("names", &bad.join("; "))]);
            ui.add(egui::Label::new(egui::RichText::new(words).color(t.red)).wrap());
        }
        let how = match (self.on_server, self.guest) {
            (true, true) => tl!("The account's server forwards it, and tells the meeting's organizer that you did."),
            (true, false) => tl!("The account's server forwards it."),
            (false, _) => tl!("It goes as mail from the Outbox at the next Send/Receive, with the item attached as an iCalendar file."),
        };
        ui.add(egui::Label::new(egui::RichText::new(how).color(t.text_dim)).wrap());
        kit::error(ui, &self.error);
        let ready = !good.is_empty() && bad.is_empty();
        let (mut close, mut send) = (false, false);
        crate::dialogs::button_row(ui, |ui| {
            let cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0)));
            app.hit.add("dialogCancel", 0, cancel.rect);
            close = cancel.clicked();
            let button = primary_button(ui, tl!("Send"), ready);
            app.hit.add("forwardSend", 0, button.rect);
            send = button.clicked();
        });
        // The owner pressed Send: that is the yes the mail goes on.
        if send && ready {
            match app.execute("calendar.forward", self.command()) {
                Ok(answer) => {
                    let n = answer.get("to").and_then(Value::as_u64).unwrap_or(0);
                    app.status(tln!(n, "Forwarded to {n} person", "Forwarded to {n} people"));
                    close = true;
                }
                Err(e) => self.error = e,
            }
        }
        close
    }
}

/// One proposal of another time.
struct Row {
    who: String,
    start: DateTime,
    end: DateTime,
}

/// View All Proposals: the other times people proposed for a meeting the owner organizes, each
/// with a way to open the meeting on it.
pub struct Proposals {
    event: Option<Id>,
    /// For a proposal about one occurrence of a series: its day, as the engine says it.
    occurrence: Option<String>,
    title: String,
    current: String,
    organizes: bool,
    rows: Vec<Row>,
}

impl Proposals {
    fn open(app: &mut MailApp, p: &Value) -> Option<Proposals> {
        let mut ask = serde_json::Map::new();
        for key in ["id", "message"] {
            if let Some(said) = p.get(key).filter(|said| said.as_u64().is_some_and(|id| id != 0)) {
                ask.insert(key.to_string(), said.clone());
            }
        }
        // (A pure command: reading what was proposed changes nothing.)
        let told = app.session.run("calendar.proposals", &Value::Object(ask)).ok()?;
        let s = &app.session;
        let text = |of: &Value, key: &str| of.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let rows: Vec<Row> = told
            .get("proposals")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .take(MAX_ROWS)
            .filter_map(|one| {
                Some(Row {
                    who: shown_name(&text(one, "name"), &text(one, "email")),
                    start: time_of(&text(one, "start"))?,
                    end: time_of(&text(one, "end"))?,
                })
            })
            .collect();
        let event = told.get("event").and_then(Value::as_u64);
        let on_calendar = event.and_then(|id| s.mailbox.event(id));
        Some(Proposals {
            event,
            occurrence: told.get("occurrence").and_then(Value::as_str).map(str::to_string),
            title: on_calendar.map(|e| kit::shown(&e.title, 120)).unwrap_or_default(),
            current: on_calendar.map(|e| when_text(e.start, e.end, e.all_day, s.prefs.twelve_hour)).unwrap_or_default(),
            organizes: told.get("organizes").and_then(Value::as_bool) == Some(true),
            rows,
        })
    }

    fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let twelve = app.session.prefs.twelve_hour;
        if self.event.is_none() {
            ui.add(egui::Label::new(tl!("The meeting these proposals are for isn't on your calendar.")).wrap());
        } else {
            ui.add(egui::Label::new(egui::RichText::new(self.title.as_str()).font(semibold(12.5))).wrap());
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("It is now on {when}."), &[("when", &self.current)])).color(t.text_dim));
        }
        if self.rows.is_empty() {
            ui.label(egui::RichText::new(tl!("Nobody has proposed another time.")).color(t.text_dim));
        }
        let mut accept: Option<usize> = None;
        egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
            for (n, row) in self.rows.iter().enumerate() {
                ui.horizontal(|ui| {
                    let able = self.event.is_some() && self.organizes;
                    let button = ui.add_enabled(able, egui::Button::new(tl!("Accept Proposal")).min_size(vec2(72.0, 24.0)));
                    app.hit.add("proposalAccept", n as u64, button.rect);
                    if button.clicked() {
                        accept = Some(n);
                    }
                    let words = format!("{}: {}", row.who, when_text(row.start, row.end, false, twelve));
                    ui.add(egui::Label::new(words).truncate());
                });
            }
        });
        if self.event.is_some() && !self.organizes && !self.rows.is_empty() {
            ui.label(egui::RichText::new(tl!("Only the meeting's organizer can change its time.")).color(t.text_dim));
        } else if !self.rows.is_empty() {
            let words = tl!("Accepting a proposal opens the meeting with that time filled in. Nothing is sent until you send the update there.");
            ui.add(egui::Label::new(egui::RichText::new(words).color(t.text_dim)).wrap());
        }
        let mut close = false;
        crate::dialogs::button_row(ui, |ui| {
            let button = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0)));
            app.hit.add("dialogCancel", 0, button.rect);
            close = button.clicked();
        });
        if let (Some(row), Some(event)) = (accept.and_then(|n| self.rows.get(n)), self.event) {
            accept_proposal(app, event, self.occurrence.as_deref(), row.start, row.end);
            close = true;
        }
        close
    }
}

/// Open the meeting `event` in its window with another time in its fields: the one somebody
/// proposed. Nothing is changed and nothing is sent: the owner sends the update from there (or
/// closes the window, and the meeting is as it was).
pub(crate) fn accept_proposal(app: &mut MailApp, event: Id, occurrence: Option<&str>, start: DateTime, end: DateTime) {
    let mut params = json!({"event": event, "proposed": {"start": start.iso(), "end": end.iso()}});
    if let (Some(day), Some(fields)) = (occurrence, params.as_object_mut()) {
        fields.insert("scope".into(), json!("occurrence"));
        fields.insert("occurrence".into(), json!(day));
    } else if let Some(fields) = params.as_object_mut() {
        // (A proposal is for the meeting as a whole unless it says which day.)
        fields.insert("scope".into(), json!("series"));
    }
    if let Err(e) = app.execute("ui.openWindow", params) {
        app.status(e);
    }
}

impl MeetingDialog {
    /// The dialog `name` (`proposeTime`, `forwardMeeting`, `proposals`), set up from the app's
    /// state and `params`; `None` for another name, or when there is nothing to open it on.
    pub fn open(name: &str, app: &mut MailApp, params: &Value) -> Option<MeetingDialog> {
        Some(match name {
            "proposeTime" => MeetingDialog::Propose(Box::new(ProposeTime::open(app, params)?)),
            "forwardMeeting" => MeetingDialog::Forward(Box::new(ForwardMeeting::open(app, params)?)),
            "proposals" => MeetingDialog::Proposals(Box::new(Proposals::open(app, params)?)),
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            MeetingDialog::Propose(_) => "proposeTime",
            MeetingDialog::Forward(_) => "forwardMeeting",
            MeetingDialog::Proposals(_) => "proposals",
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            MeetingDialog::Propose(_) => tl!("Propose New Time"),
            MeetingDialog::Forward(_) => tl!("Forward"),
            MeetingDialog::Proposals(_) => tl!("Proposed Times"),
        }
    }

    pub fn width(&self) -> f32 {
        match self {
            MeetingDialog::Propose(_) => 500.0,
            MeetingDialog::Forward(_) => 480.0,
            MeetingDialog::Proposals(_) => 520.0,
        }
    }

    /// Lay the dialog out; `true` when it is to close.
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        match self {
            MeetingDialog::Propose(dialog) => dialog.show(app, ui),
            MeetingDialog::Forward(dialog) => dialog.show(app, ui),
            MeetingDialog::Proposals(dialog) => dialog.show(app, ui),
        }
    }
}

/// `ui.joinMeeting`: open the link to join an online meeting by in the system's browser. The
/// link came from the meeting's server (or a message): it is followed only because the owner
/// asked, only when it is an `https` address, and by the rules a link in a message is followed by.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != "ui.joinMeeting" {
        return None;
    }
    let event = p.get("event").and_then(Value::as_u64).or(app.session.sel.event);
    let Some(event) = event else { return Some(Err(tl!("Select a meeting first").to_string())) };
    let told = match app.session.run("calendar.join", &json!({"id": event})) {
        Ok(told) => told,
        Err(e) => return Some(Err(e.to_string())),
    };
    let url = told.get("url").and_then(Value::as_str).unwrap_or_default();
    // Twice over: an https address as the model has it, and one a message's link would be followed to.
    let safe = mailcraft_model::OnlineMeeting::clean_link(url).filter(|url| mailcraft_html::is_safe_url(url) && !mailcraft_html::is_embedded(url));
    let Some(url) = safe else { return Some(Err(tl!("This meeting's link isn't one that can be opened.").to_string())) };
    let Some(ctx) = app.ctx.clone() else { return Some(Err(tl!("The window isn't up yet.").to_string())) };
    ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
    Some(Ok(json!({"event": event, "url": url})))
}

#[cfg(test)]
mod tests;
