// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's dialogs that only collect what a command needs, or show what it answered:
//! Email Calendar and Insert › Calendar (which days, and how much of each), Tracking (who
//! answered a meeting how) and Search Calendar (what to look for, and what was found).
//!
//! None of them sends. Email Calendar opens a message for the owner to address and send; Insert
//! › Calendar puts the same text in the message being written. What Tracking shows of the
//! people invited came from outside: cut to size, shown as text.

use egui::{Ui, vec2};
use mailcraft_model::Id;
use mailcraft_time::Date;
use serde_json::{Value, json};

use crate::MailApp;
use crate::forms::kit::{self, Pass};
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// Most people Tracking lists, and most appointments Search Calendar lists.
const MAX_ROWS: usize = 200;

/// Email Calendar, or Insert › Calendar: which days of the owner's calendar, and how much of each.
pub struct ShareCalendar {
    /// Insert › Calendar: into the message being written, rather than a new one.
    insert: bool,
    from: String,
    to: String,
    /// `availability`, `limited` or `full`.
    detail: &'static str,
    /// An iCalendar file of the same goes along (Email Calendar).
    ics: bool,
    error: String,
    focus: bool,
}

impl ShareCalendar {
    pub(super) fn open(app: &MailApp, insert: bool) -> ShareCalendar {
        let today = app.session.today();
        ShareCalendar { insert, from: today.iso(), to: today.add_days(6).iso(), detail: "availability", ics: true, error: String::new(), focus: true }
    }

    /// Insert › Calendar rather than Email Calendar.
    pub(super) fn inserts(&self) -> bool {
        self.insert
    }

    fn command(&self) -> Option<Value> {
        let (from, to) = (Date::parse_iso(self.from.trim())?, Date::parse_iso(self.to.trim())?);
        if to < from || from.days_until(to) >= mailcraft_engine::cmd::calendar::share::MAX_DAYS {
            return None;
        }
        let mut params = json!({"from": from.iso(), "to": to.iso(), "detail": self.detail});
        if !self.insert
            && let Some(fields) = params.as_object_mut()
        {
            fields.insert("ics".into(), json!(self.ics));
        }
        Some(params)
    }

    pub(super) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut pass = Pass::new(ui, &app.session, &mut self.focus);
        let says = if self.insert {
            tl!("Puts the days you choose of your calendar into the message, where the cursor is.")
        } else {
            tl!(
                "Starts a new message with the days you choose of your calendar in it. You say who it goes to, and nothing is sent until you press Send."
            )
        };
        ui.add(egui::Label::new(egui::RichText::new(says).color(t.text_dim)).wrap());
        ui.add_space(2.0);
        kit::row(ui, tl!("From"), |ui| {
            pass.date(ui, "share_from", &mut self.from, true);
        });
        kit::row(ui, tl!("To"), |ui| {
            pass.date(ui, "share_to", &mut self.to, true);
        });
        ui.add_space(2.0);
        ui.label(egui::RichText::new(tl!("Detail")).font(semibold(12.0)));
        for (code, label) in [
            ("availability", tl!("Availability only: when you are free or busy")),
            ("limited", tl!("Limited details: and the titles")),
            ("full", tl!("Full details: and where, and the notes")),
        ] {
            let r = ui.radio_value(&mut self.detail, code, label);
            app.hit.add("shareDetail", crate::ribbon::hit_id(code), r.rect);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(tl!("An appointment marked private shows only that you are busy. Nobody invited to anything is named."))
                    .color(t.text_dim),
            )
            .wrap(),
        );
        if !self.insert {
            let r = ui.checkbox(&mut self.ics, tl!("Attach the same as an iCalendar file"));
            app.hit.add("shareIcs", 0, r.rect);
        }
        let params = self.command();
        if params.is_none() {
            ui.label(egui::RichText::new(tl!("Choose a first and a last day, at most a year apart.")).color(t.red));
        }
        kit::error(ui, &self.error);
        let label = if self.insert { tl!("Insert") } else { tl!("Create Message") };
        let (ok, cancel) = crate::dialogs::buttons(ui, label, params.is_some());
        let mut close = cancel;
        if let Some(params) = params.filter(|_| ok || pass.submit) {
            let id = if self.insert { "insert.calendar" } else { "calendar.email" };
            // (As the owner's: a message being written with changes is asked about first.)
            match app.run(id, params) {
                Ok(_) => close = true,
                Err(e) => self.error = e,
            }
        }
        close
    }
}

/// One person invited, as Tracking lists them.
struct Person {
    who: String,
    optional: bool,
    response: &'static str,
}

/// Tracking: who is invited to a meeting, and how each has answered, as far as is known here.
pub struct Tracking {
    title: String,
    organizer: String,
    organizes: bool,
    people: Vec<Person>,
    counts: [u64; 4],
}

impl Tracking {
    pub(super) fn open(app: &mut MailApp, p: &Value) -> Option<Tracking> {
        let id = p.get("id").and_then(Value::as_u64).filter(|id| *id != 0).or(app.session.sel.event)?;
        // (A pure command: reading the answers changes nothing.)
        let told = app.session.run("calendar.tracking", &json!({"id": id})).ok()?;
        let text = |of: &Value, key: &str| of.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        let people = told
            .get("attendees")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .take(MAX_ROWS)
            .map(|one| Person {
                who: super::shown_name(&text(one, "name"), &text(one, "email")),
                optional: one.get("optional").and_then(Value::as_bool) == Some(true),
                response: match one.get("response").and_then(Value::as_str) {
                    Some("accepted") => tl!("Accepted"),
                    Some("tentative") => tl!("Tentative"),
                    Some("declined") => tl!("Declined"),
                    _ => tl!("No response"),
                },
            })
            .collect();
        let count = |key: &str| told.get("counts").and_then(|c| c.get(key)).and_then(Value::as_u64).unwrap_or(0);
        let organizer = told.get("organizer").filter(|o| !o.is_null()).map(|o| super::shown_name(&text(o, "name"), &text(o, "email")));
        Some(Tracking {
            title: kit::shown(&text(&told, "title"), 120),
            organizer: organizer.unwrap_or_default(),
            organizes: told.get("organizes").and_then(Value::as_bool) == Some(true),
            people,
            counts: [count("accepted"), count("tentative"), count("declined"), count("none")],
        })
    }

    pub(super) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        ui.add(egui::Label::new(egui::RichText::new(self.title.as_str()).font(semibold(12.5))).wrap());
        if self.organizes {
            ui.label(egui::RichText::new(tl!("You organize this meeting.")).color(t.text_dim));
        } else if !self.organizer.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Organizer: {name}"), &[("name", &self.organizer)])).color(t.text_dim));
        }
        let [accepted, tentative, declined, none] = self.counts;
        let tally = format!(
            "{} · {} · {} · {}",
            tln!(accepted, "{n} accepted", "{n} accepted"),
            tln!(tentative, "{n} tentative", "{n} tentative"),
            tln!(declined, "{n} declined", "{n} declined"),
            tln!(none, "{n} not answered", "{n} not answered"),
        );
        ui.label(tally);
        if self.people.is_empty() {
            ui.label(egui::RichText::new(tl!("Nobody is invited to this appointment.")).color(t.text_dim));
        }
        egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("tracking").num_columns(3).spacing(vec2(16.0, 4.0)).striped(true).show(ui, |ui| {
                ui.label(egui::RichText::new(tl!("Name")).font(semibold(11.5)));
                ui.label(egui::RichText::new(tl!("Attendance")).font(semibold(11.5)));
                ui.label(egui::RichText::new(tl!("Response")).font(semibold(11.5)));
                ui.end_row();
                for person in &self.people {
                    // (Cut to size when it was read: a label that truncates has no width in a grid.)
                    ui.label(person.who.as_str());
                    ui.label(if person.optional { tl!("Optional") } else { tl!("Required") });
                    ui.label(person.response);
                    ui.end_row();
                }
            });
        });
        ui.add(
            egui::Label::new(egui::RichText::new(tl!("These are the answers this computer has been told of.")).color(t.text_dim).size(11.0)).wrap(),
        );
        let mut close = false;
        crate::dialogs::button_row(ui, |ui| {
            let button = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0)));
            app.hit.add("dialogCancel", 0, button.rect);
            close = button.clicked();
        });
        close
    }
}

/// One appointment Search Calendar found.
struct Found {
    id: Id,
    day: Date,
    title: String,
    when: String,
}

/// Search Calendar: what to look for, in the search box's language, and what was found.
pub struct SearchCalendar {
    query: String,
    found: Vec<Found>,
    /// How many more were found than are listed.
    more: u64,
    searched: bool,
    error: String,
    focus: bool,
}

impl SearchCalendar {
    pub(super) fn open(p: &Value) -> SearchCalendar {
        let query = p.get("query").and_then(Value::as_str).unwrap_or_default().chars().take(500).collect();
        SearchCalendar { query, found: Vec::new(), more: 0, searched: false, error: String::new(), focus: true }
    }

    fn search(&mut self, app: &mut MailApp) {
        self.error.clear();
        // (A pure command: looking changes nothing.)
        match app.session.run("calendar.search", &json!({"query": self.query.trim()})) {
            Ok(told) => {
                let twelve = app.session.prefs.twelve_hour;
                self.found = told
                    .get("events")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .take(MAX_ROWS)
                    .filter_map(|e| {
                        let text = |key: &str| e.get(key).and_then(Value::as_str).unwrap_or_default();
                        let (start, end) = (super::time_of(text("start"))?, super::time_of(text("end"))?);
                        let all_day = e.get("allDay").and_then(Value::as_bool) == Some(true);
                        let mut when = super::when_text(start, end, all_day, twelve);
                        if e.get("repeats").and_then(Value::as_bool) == Some(true) {
                            when = format!("{when} · {}", tl!("Repeats"));
                        }
                        Some(Found { id: e.get("id").and_then(Value::as_u64)?, day: start.date, title: kit::shown(text("title"), 120), when })
                    })
                    .collect();
                self.more = told
                    .get("more")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    .saturating_add(told.get("events").and_then(Value::as_array).map_or(0, |a| a.len().saturating_sub(MAX_ROWS) as u64));
                self.searched = true;
            }
            Err(e) => {
                self.found.clear();
                self.searched = false;
                self.error = e.to_string();
            }
        }
    }

    pub(super) fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        let t = Tokens::get(ui.ctx());
        let mut pass = Pass::new(ui, &app.session, &mut self.focus);
        let mut go = false;
        ui.horizontal(|ui| {
            let width = ui.available_width() - 90.0;
            pass.text(ui, "calendar_search", &mut self.query, width, tl!("Words, from:, to:, category:, on:…"));
            let button = primary_button(ui, tl!("Search"), !self.query.trim().is_empty());
            app.hit.add("calendarSearchGo", 0, button.rect);
            go = button.clicked();
        });
        if (go || pass.submit) && !self.query.trim().is_empty() {
            self.search(app);
        }
        kit::error(ui, &self.error);
        if self.searched && self.found.is_empty() {
            ui.label(egui::RichText::new(tl!("No appointment matches.")).color(t.text_dim));
        }
        let mut picked: Option<(Id, Date)> = None;
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for (n, found) in self.found.iter().enumerate() {
                let r = ui.add(egui::Button::new(egui::RichText::new(found.title.as_str()).font(semibold(12.0))).frame(false));
                app.hit.add("calendarSearchResult", n as u64, r.rect);
                if r.clicked() {
                    picked = Some((found.id, found.day));
                }
                ui.label(egui::RichText::new(found.when.as_str()).color(t.text_dim).size(11.0));
                ui.add_space(2.0);
            }
        });
        if self.more > 0 {
            ui.label(egui::RichText::new(tln!(self.more, "{n} more not listed", "{n} more not listed")).color(t.text_dim));
        }
        let mut close = false;
        crate::dialogs::button_row(ui, |ui| {
            let button = ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0)));
            app.hit.add("dialogCancel", 0, button.rect);
            close = button.clicked();
        });
        // A result picked: the calendar goes to its day, with it selected.
        if let Some((id, day)) = picked {
            let _ = app.run("view.goToDate", json!({"date": day.iso()}));
            if let Err(e) = app.run("calendar.select", json!({"id": id, "day": day.iso()})) {
                self.error = e;
            } else {
                close = true;
            }
        }
        close
    }
}
