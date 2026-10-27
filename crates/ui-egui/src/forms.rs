// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The forms the dialog window hosts: appointment and meeting, contact, task, account, categories,
//! rules.
//!
//! A form collects what the user types and, on OK, runs one command with it; the command's error,
//! if any, is shown in the form, which stays open.

use egui::Ui;
use serde_json::Value;

use crate::MailApp;

pub(crate) use event::Page;

/// A form the dialog window hosts, with what was typed into it so far.
pub enum Form {
    /// An appointment or a meeting, new or being edited.
    Event(Box<event::EventForm>),
    /// A contact, new or being edited.
    Contact(Box<contact::ContactForm>),
    /// A task, new or being edited.
    Task(Box<task::TaskForm>),
    /// Add Account, and the settings of an account that exists.
    Account(Box<account::AccountForm>),
    /// The colour categories: recolour, rename, delete, add.
    Categories(Box<categories::CategoriesForm>),
    /// The mail rules, and a builder for a new one.
    Rules(Box<rules::RulesForm>),
    /// Junk E-mail Options: the Safe and Blocked Senders lists (#504).
    Junk(Box<junk::JunkForm>),
}

impl Form {
    /// The form called `name`: `event` / `meeting` / `contact` / `task` start something new (`params` may
    /// prefill it), `editEvent` / `editContact` / `editTask` edit the selected one (or `params.id`),
    /// `account`, `categories` and `rules` manage those. `None` for an unknown name or nothing to edit.
    pub fn open(name: &str, app: &MailApp, params: &Value) -> Option<Form> {
        let s = &app.session;
        Some(match name {
            "event" => Form::Event(Box::new(event::EventForm::new(s, params, false))),
            "meeting" => Form::Event(Box::new(event::EventForm::new(s, params, true))),
            "editEvent" => Form::Event(Box::new(event::EventForm::edit(s, params)?)),
            "contact" => Form::Contact(Box::new(contact::ContactForm::new(params))),
            "editContact" => Form::Contact(Box::new(contact::ContactForm::edit(s, params)?)),
            "task" => Form::Task(Box::new(task::TaskForm::new(s, params))),
            "editTask" => Form::Task(Box::new(task::TaskForm::edit(s, params)?)),
            "account" => Form::Account(Box::new(account::AccountForm::open(app, params)?)),
            "categories" => Form::Categories(Box::new(categories::CategoriesForm::new(s))),
            "rules" => Form::Rules(Box::new(rules::RulesForm::new(s))),
            "junkOptions" => Form::Junk(Box::new(junk::JunkForm::new(params))),
            _ => return None,
        })
    }

    /// The name it was opened by (`ui.inspect` reports it).
    pub fn name(&self) -> &'static str {
        match self {
            Form::Event(f) => f.name(),
            Form::Contact(f) => f.name(),
            Form::Task(f) => f.name(),
            Form::Account(_) => "account",
            Form::Categories(_) => "categories",
            Form::Rules(_) => "rules",
            Form::Junk(_) => "junkOptions",
        }
    }

    /// The dialog's title, in the interface language.
    pub fn title(&self) -> String {
        match self {
            Form::Event(f) => f.title(),
            Form::Contact(f) => f.title(),
            Form::Task(f) => f.title(),
            Form::Account(f) => f.title(),
            Form::Categories(_) => tl!("Categories").to_string(),
            Form::Rules(_) => tl!("Rules").to_string(),
            Form::Junk(_) => tl!("Junk E-mail Options").to_string(),
        }
    }

    /// The dialog's width in points.
    pub fn width(&self) -> f32 {
        match self {
            Form::Event(_) | Form::Task(_) => 580.0,
            Form::Contact(_) => 540.0,
            Form::Account(_) => crate::account_setup::WIDTH,
            Form::Categories(_) => 420.0,
            Form::Rules(_) => 620.0,
            Form::Junk(_) => junk::WIDTH,
        }
    }

    /// Lay out the form. Returns `true` when the dialog should close (OK succeeded, or Cancel).
    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
        match self {
            Form::Event(f) => f.show(app, ui),
            Form::Contact(f) => f.show(app, ui),
            Form::Task(f) => f.show(app, ui),
            Form::Account(f) => f.show(app, ui),
            Form::Categories(f) => f.show(app, ui),
            Form::Rules(f) => f.show(app, ui),
            Form::Junk(f) => f.show(app, ui),
        }
    }

    /// The dialog was closed with the window's own close button: Add Account gives up what it
    /// has under way, as on Escape.
    pub fn closed(&mut self, app: &mut MailApp) {
        if let Form::Account(f) = self {
            f.give_up(app);
        }
    }

    /// Lay out the form in a window of its own ([`crate::windows`]): an appointment or a meeting
    /// under that window's bar of commands. `ask` is what the window's keys asked for.
    pub(crate) fn show_in_window(&mut self, app: &mut MailApp, ui: &mut Ui, ask: Option<Ask>) -> Verdict {
        match self {
            Form::Event(f) => f.show_in_window(app, ui, ask),
            other => {
                if other.show(app, ui) {
                    Verdict::Close
                } else {
                    Verdict::Stay
                }
            }
        }
    }

    /// The width of a window of its own that shows the form, in points.
    pub(crate) fn window_width(&self) -> f32 {
        match self {
            Form::Event(_) => event::WINDOW_WIDTH,
            other => other.width() + 32.0,
        }
    }

    /// The appointment or meeting the form is about, when it is about one.
    pub(crate) fn event(&self) -> Option<&event::EventForm> {
        match self {
            Form::Event(f) => Some(f),
            _ => None,
        }
    }

    pub(crate) fn event_mut(&mut self) -> Option<&mut event::EventForm> {
        match self {
            Form::Event(f) => Some(f),
            _ => None,
        }
    }

    /// Whether something was changed in the form that isn't saved: its window asks before it
    /// closes.
    pub(crate) fn dirty(&self) -> bool {
        self.event().is_some_and(event::EventForm::dirty)
    }

    /// Whether the keys are for something of the form's own that is open over it (a dialog), or
    /// the form waits for a question to be answered: its window leaves the keys alone.
    pub(crate) fn engaged(&self) -> bool {
        self.event().is_some_and(|f| f.modal() || f.waiting())
    }

    /// Whether a command of the form waits for the answer to a question the main window asks.
    pub(crate) fn waiting(&self) -> bool {
        self.event().is_some_and(event::EventForm::waiting)
    }

    /// Whether the form has a dialog of its own open over it (the recurrence editor).
    pub(crate) fn modal(&self) -> bool {
        self.event().is_some_and(event::EventForm::modal)
    }
}

/// What a window asks of the form it hosts beyond laying it out: what its keys stand for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ask {
    /// Save, and stay open.
    Save,
    /// Save and close (the answer to "save the changes?").
    SaveAndClose,
    /// Send a meeting.
    Send,
}

/// What came of laying out a form in a window of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The window stays as it is.
    Stay,
    /// The window closes: what it was for is done, or given up.
    Close,
    /// The window stays, and is about this event from now on (it was saved).
    Now(mailcraft_model::Id),
}

/// What the forms share: labelled rows, text, date and time fields, chips, the buttons at the
/// bottom, and the list of fields that changed. Other dialogs of the window may build on it.
pub(crate) mod kit {
    use std::fmt::Debug;
    use std::hash::Hash;

    use egui::{Align, Align2, Color32, CornerRadius, Response, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
    use mailcraft_engine::Session;
    use mailcraft_model::Category;
    use mailcraft_time::{Date, parse_time, time_short};
    use serde_json::{Map, Value, json};

    use crate::theme::{Tokens, c32, paint_clipped, regular};
    use crate::widgets::{icon_button, primary_button};

    /// Width of the label column.
    pub const LABEL_W: f32 = 110.0;
    /// Height of a one-line row.
    pub const ROW_H: f32 = 24.0;

    /// Where a form is laid out when that isn't the dialog: a window of its own
    /// ([`crate::windows`]). Forms in different windows don't share their fields, and a form that
    /// has a window of the system to itself fills it. (Which of them an Enter is for is
    /// [`crate::keys`]'s to say.)
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Host {
        /// Tells this form's fields from those of the same form in another window; 0 for the dialog.
        pub id: u64,
        /// The form has the height of its window, and fills it.
        pub fill: bool,
    }

    thread_local! {
        /// The host of the form that is being laid out.
        static HOST: std::cell::Cell<Host> = const { std::cell::Cell::new(Host { id: 0, fill: false }) };
        /// Where the fields of the form being laid out end when it fills its window (a height
        /// on the page of its fields); not a number otherwise.
        static BODY_END: std::cell::Cell<f32> = const { std::cell::Cell::new(f32::NAN) };
    }

    fn host() -> Host {
        HOST.with(std::cell::Cell::get)
    }

    /// The number of the window the form being laid out is in; 0 for the dialog.
    pub fn window() -> u64 {
        host().id
    }

    /// Whether the form being laid out is in a window of its own rather than in the dialog.
    pub fn windowed() -> bool {
        host().id != 0
    }

    /// Whether the form being laid out has the height of its window to fill.
    pub fn filling() -> bool {
        host().fill
    }

    /// Lay a form out as `host`'s: what `add` lays out has its fields to itself.
    pub fn hosted<R>(host: Host, add: impl FnOnce() -> R) -> R {
        /// Puts the host back that was there before, whatever happens in between.
        struct Back(Host);
        impl Drop for Back {
            fn drop(&mut self) {
                HOST.with(|cell| cell.set(self.0));
                BODY_END.with(|cell| cell.set(f32::NAN));
            }
        }
        let _back = Back(HOST.with(|cell| cell.replace(host)));
        add()
    }

    /// The id of a form's field. Fields keep their ids when rows above them come and go, and
    /// the same field of the same form in another window ([`hosted`]) has another.
    pub fn field_id(key: impl Hash + Debug) -> egui::Id {
        match host().id {
            0 => egui::Id::unique(("form", key)),
            window => egui::Id::unique(("form", window, key)),
        }
    }

    /// A labelled row: the label on the left, the control after it.
    pub fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
        ui.horizontal(|ui| {
            label_cell(ui, label);
            add(ui);
        });
    }

    /// A labelled row for a control taller than one line: the label stays at the top.
    pub fn tall_row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
        ui.horizontal_top(|ui| {
            label_cell(ui, label);
            ui.vertical(add);
        });
    }

    fn label_cell(ui: &mut Ui, label: &str) {
        let t = Tokens::get(ui.ctx());
        let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, ROW_H), Sense::hover());
        paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), LABEL_W - 6.0, label, regular(12.5), t.text);
    }

    /// What the buttons at the bottom take of a form's height, with the gap over them.
    const FOOTER_ROOM: f32 = 38.0;

    /// The body of a form. It scrolls when the window is short, so the buttons stay in view.
    pub fn body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
        body_over(ui, 0.0, add);
    }

    /// [`body`], for a form that lays `below` points out between its fields and its buttons
    /// (what a command refused, [`error_room`]).
    pub fn body_over(ui: &mut Ui, below: f32, add: impl FnOnce(&mut Ui)) {
        body_sized(ui, below, true, add);
    }

    /// [`body_over`], for a form that may have no buttons under its fields (`footer`): one whose
    /// commands are in a bar of its own over them ([`bar`]).
    pub fn body_sized(ui: &mut Ui, below: f32, footer: bool, add: impl FnOnce(&mut Ui)) {
        let fill = host().fill;
        let below = if below.is_finite() { below.max(0.0) } else { 0.0 };
        let buttons = if footer { FOOTER_ROOM } else { 0.0 };
        // In a dialog: the height of the main window less the dialog's own frame and the room
        // round it (and, for a window inside the main one, less its bar). In a window of its
        // own the form has all of that window: its fields take what the buttons leave, down to
        // the window's foot.
        let around = if footer { 220.0 } else { 290.0 };
        let room = if fill { ui.available_height() - buttons - below } else { ui.ctx().content_rect().height() - around };
        let room = if room.is_finite() { room.max(120.0) } else { 120.0 };
        // The room is offered whatever height the window had last frame, so a form opens at its
        // full height rather than growing into it.
        ui.allocate_ui(vec2(ui.available_width().max(0.0), room), |ui| {
            egui::ScrollArea::vertical().id_salt("form_body").max_height(room).auto_shrink([false, !fill]).show(ui, |ui| {
                // Room for the scroll bar beside the fields.
                let margin = egui::Margin { left: 0, right: 12, top: 1, bottom: 2 };
                BODY_END.with(|cell| cell.set(if fill { ui.cursor().top() + room - 3.0 } else { f32::NAN }));
                egui::Frame::NONE.inner_margin(margin).show(ui, add);
                BODY_END.with(|cell| cell.set(f32::NAN));
            });
        });
    }

    /// The height [`error`] takes for `message`, with the gap under it: nothing when there is
    /// nothing to say.
    pub fn error_room(ui: &Ui, message: &str) -> f32 {
        if message.is_empty() {
            return 0.0;
        }
        let font = egui::TextStyle::Body.resolve(ui.style());
        let lines = ui.painter().layout(shown(message, 400), font, Color32::WHITE, ui.available_width().max(40.0));
        lines.size().y + ui.spacing().item_spacing.y
    }

    /// One frame of a form: the colours, the clock, and what the keyboard asked for.
    pub struct Pass {
        pub t: Tokens,
        pub today: Date,
        /// Times read `3:05 PM` rather than `15:05`.
        pub twelve: bool,
        enter: bool,
        /// Enter asked for OK: it was pressed in a one-line field, or with nothing focused.
        pub submit: bool,
        /// The next field laid out takes the keyboard (the first one, when the form opens).
        focus: bool,
    }

    impl Pass {
        /// Start a frame. `focus` is taken: the form's first field gets the keyboard once.
        pub fn new(ui: &Ui, s: &Session, focus: &mut bool) -> Pass {
            // Enter is the form's when its window (or dialog) is where the key went (#406): not
            // under a dialog or a question, not in a window behind the active one, not while a
            // list is open, and never a held key's repeats.
            let layer = ui.layer_id();
            Pass {
                t: Tokens::get(ui.ctx()),
                today: s.today(),
                twelve: s.prefs.twelve_hour,
                enter: crate::keys::enter(ui.ctx(), layer),
                submit: crate::keys::enter_alone(ui.ctx(), layer),
                focus: std::mem::take(focus),
            }
        }

        /// Note a one-line field: the first one takes the keyboard, and Enter in it asks for OK.
        pub fn field(&mut self, resp: &Response) {
            if self.focus {
                resp.request_focus();
                self.focus = false;
            }
            if self.enter && resp.lost_focus() {
                self.submit = true;
            }
        }

        /// A one-line text field `width` points wide (infinity: the rest of the row).
        pub fn text(&mut self, ui: &mut Ui, key: impl Hash + Debug, value: &mut String, width: f32, hint: &str) -> Response {
            let resp = ui.add(egui::TextEdit::singleline(value).id(field_id(key)).desired_width(width.max(16.0)).hint_text(hint.to_owned()));
            self.field(&resp);
            resp
        }

        /// A date typed as `YYYY-MM-DD`, outlined in red while the text isn't a date. An empty
        /// field is wrong only when `required`.
        pub fn date_text(&mut self, ui: &mut Ui, key: impl Hash + Debug, text: &mut String, required: bool) -> Response {
            let resp = ui.add(
                egui::TextEdit::singleline(text)
                    .id(field_id(key))
                    .desired_width(84.0)
                    .char_limit(10)
                    .hint_text(egui::RichText::new(tl!("YYYY-MM-DD")).size(10.5)),
            );
            self.field(&resp);
            if !date_ok(text, required) {
                mark_wrong(ui, &resp, &self.t);
            }
            resp
        }

        /// A date field with buttons that step a day back and forward. Returns `true` when the
        /// text changed.
        pub fn date(&mut self, ui: &mut Ui, key: impl Hash + Debug, text: &mut String, required: bool) -> bool {
            let before = text.clone();
            let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 2.0);
            self.date_text(ui, key, text, required);
            for (icon, step, tip) in [("previous", -1, tl!("The day before")), ("next", 1, tl!("The day after"))] {
                if icon_button(ui, icon, tip, 12.0, true).clicked() {
                    *text = Date::parse_iso(text).map_or(self.today, |d| d.add_days(step)).iso();
                }
            }
            ui.spacing_mut().item_spacing.x = gap;
            *text != before
        }

        /// A time of day as text (`9`, `9:30`, `9am`, `14:15`), rewritten the way times are shown
        /// when the field is left, with a menu of half-hour slots. After a start time (`after`,
        /// in minutes since midnight) the menu lists later times with their lengths. An empty
        /// field is wrong only when `required`. Returns `true` when the text changed.
        pub fn time(&mut self, ui: &mut Ui, key: impl Hash + Debug, text: &mut String, required: bool, after: Option<u32>) -> bool {
            let before = text.clone();
            let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 2.0);
            let resp = ui.add(egui::TextEdit::singleline(text).id(field_id(key)).desired_width(64.0).char_limit(11).hint_text(time_short(
                9,
                0,
                self.twelve,
            )));
            self.field(&resp);
            if resp.lost_focus()
                && let Some((h, m)) = parse_clock(text)
            {
                *text = time_short(h, m, self.twelve);
            }
            if !time_ok(text, required) {
                mark_wrong(ui, &resp, &self.t);
            }
            let button = icon_button(ui, "dropdown", tl!("Pick a time"), 10.0, true);
            ui.spacing_mut().item_spacing.x = gap;
            let opening = !egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&button));
            let current = parse_clock(text).map(|(h, m)| u32::from(h) * 60 + u32::from(m));
            let mut picked = None;
            egui::Popup::menu(&button).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                egui::ScrollArea::vertical().max_height(232.0).show(ui, |ui| {
                    ui.set_min_width(if after.is_some() { 168.0 } else { 92.0 });
                    for (minutes, label) in slots(after, self.twelve) {
                        let item = ui.selectable_label(current == Some(minutes), label);
                        // The menu opens at the time the field shows.
                        if opening && current.is_some_and(|c| c / 30 * 30 == minutes) {
                            item.scroll_to_me_animation(Some(Align::Center), egui::style::ScrollAnimation::none());
                        }
                        if item.clicked() {
                            picked = Some(minutes);
                            ui.close();
                        }
                    }
                });
            });
            if let Some(m) = picked {
                *text = clock_text(m, self.twelve);
            }
            *text != before
        }
    }

    /// A red outline around a field whose text can't be used.
    fn mark_wrong(ui: &Ui, resp: &Response, t: &Tokens) {
        ui.painter().rect_stroke(resp.rect, 4.0, Stroke::new(1.0, t.red), StrokeKind::Inside);
    }

    /// What a date field says after itself: the weekday, or how to write a date when the text
    /// isn't one.
    pub fn date_note(ui: &mut Ui, text: &str, required: bool) {
        let t = Tokens::get(ui.ctx());
        match Date::parse_iso(text) {
            Some(d) => {
                ui.label(egui::RichText::new(tl!(d.weekday().short())).color(t.text_dim));
            }
            None => date_hint(ui, text, required),
        }
    }

    /// How to write a date, in red, after a date field whose text isn't one.
    pub fn date_hint(ui: &mut Ui, text: &str, required: bool) {
        if !date_ok(text, required) {
            let t = Tokens::get(ui.ctx());
            ui.label(egui::RichText::new(tl!("Use YYYY-MM-DD")).color(t.red));
        }
    }

    /// Whether a date field's text can be used: a date, or nothing where the field may stay empty.
    pub fn date_ok(text: &str, required: bool) -> bool {
        if text.trim().is_empty() { !required } else { Date::parse_iso(text).is_some() }
    }

    /// Whether a time field's text can be used: a time, or nothing where the field may stay empty.
    pub fn time_ok(text: &str, required: bool) -> bool {
        if text.trim().is_empty() { !required } else { parse_clock(text).is_some() }
    }

    /// The hour and minute a time field holds.
    pub fn parse_clock(text: &str) -> Option<(u8, u8)> {
        parse_time(text).map(|(h, m, _)| (h, m))
    }

    /// A time of day, given in minutes since midnight, as times are shown.
    pub fn clock_text(minutes: u32, twelve: bool) -> String {
        time_short(u8::try_from(minutes / 60 % 24).unwrap_or(0), u8::try_from(minutes % 60).unwrap_or(0), twelve)
    }

    /// The half-hour slots of a time menu, as minutes since midnight with their labels. After a
    /// start time they run from half an hour later to the end of that day, each with its length.
    pub fn slots(after: Option<u32>, twelve: bool) -> Vec<(u32, String)> {
        let later: Vec<(u32, String)> = match after {
            Some(start) => (1..48u32)
                .map(|k| start.saturating_add(30 * k))
                .take_while(|m| *m < 24 * 60)
                .map(|m| (m, format!("{} ({})", clock_text(m, twelve), crate::dates::duration(i64::from(m - start)))))
                .collect(),
            None => Vec::new(),
        };
        if later.is_empty() { (0..48u32).map(|k| (k * 30, clock_text(k * 30, twelve))).collect() } else { later }
    }

    /// What a command gets for a date field: `2026-10-16`, `null` for an empty field, else the
    /// text as typed (the command says what is wrong with it).
    pub fn date_value(text: &str) -> Value {
        match Date::parse_iso(text) {
            Some(d) => json!(d.iso()),
            None if text.trim().is_empty() => Value::Null,
            None => json!(text.trim()),
        }
    }

    /// What a command gets for a date field and a time field: `2026-10-16T15:00`, else both as
    /// typed.
    pub fn stamp(date: &str, time: &str) -> String {
        match (Date::parse_iso(date), parse_clock(time)) {
            (Some(d), Some((h, m))) => format!("{}T{h:02}:{m:02}", d.iso()),
            _ => format!("{}T{}", date.trim(), time.trim()),
        }
    }

    /// A multi-line text field of `rows` lines; a longer text scrolls inside it.
    pub fn multiline(ui: &mut Ui, key: impl Hash + Debug + Copy, value: &mut String, rows: usize) -> Response {
        let room = rows as f32 * 36.0 + 12.0;
        let edit = egui::TextEdit::multiline(value).id(field_id(key)).desired_rows(rows).desired_width(f32::INFINITY);
        // Its own room, also when the field is below what the form shows without scrolling.
        ui.allocate_ui(vec2(ui.available_width().max(0.0), room), |ui| {
            egui::ScrollArea::vertical().id_salt(("form_lines", key)).max_height(room).show(ui, |ui| ui.add(edit)).inner
        })
        .inner
    }

    /// A multi-line text field of at least `rows` lines for the last of a form's fields: where
    /// the form fills a window of its own ([`Host::fill`]) it takes what is left of the height.
    pub fn multiline_rest(ui: &mut Ui, key: impl Hash + Debug + Copy, value: &mut String, rows: usize) -> Response {
        let left = BODY_END.with(std::cell::Cell::get) - ui.cursor().top();
        let line = ui.text_style_height(&egui::TextStyle::Body).max(8.0);
        // (Not a number where the form doesn't fill a window: then it is `rows` lines.)
        if left.is_nan() || left < rows as f32 * line + 24.0 {
            return multiline(ui, key, value, rows);
        }
        let left = left.min(4000.0);
        // Down to the last point of it: whole lines first, then the rest of the height.
        let lines = (((left - 12.0) / line).floor() as usize).clamp(rows, 400);
        let edit =
            egui::TextEdit::multiline(value).id(field_id(key)).desired_rows(lines).desired_width(f32::INFINITY).min_size(vec2(0.0, left - 8.0));
        ui.allocate_ui(vec2(ui.available_width().max(0.0), left), |ui| {
            egui::ScrollArea::vertical().id_salt(("form_lines", key)).max_height(left).show(ui, |ui| ui.add(edit)).inner
        })
        .inner
    }

    /// A chip that is on or off, with its colour. `true` when it was clicked.
    pub fn chip(ui: &mut Ui, label: &str, on: bool, color: Color32) -> bool {
        let t = Tokens::get(ui.ctx());
        let text_w = ui.painter().layout_no_wrap(label.to_string(), regular(11.5), t.text).size().x.min(180.0);
        let (r, resp) = ui.allocate_exact_size(vec2(text_w + 32.0, 22.0), Sense::click());
        let fill = if on {
            color.linear_multiply(if t.dark { 0.45 } else { 0.22 })
        } else if resp.hovered() {
            t.hover
        } else {
            t.input
        };
        ui.painter().rect(r, 11.0, fill, Stroke::new(1.0, if on { color.linear_multiply(0.8) } else { t.input_border }), StrokeKind::Inside);
        let mark = pos2(r.min.x + 12.0, r.center().y);
        if on {
            ui.painter().text(mark, Align2::CENTER_CENTER, "✓", regular(11.0), t.text);
        } else {
            ui.painter().circle_filled(mark, 4.0, color);
        }
        paint_clipped(ui.painter(), pos2(r.min.x + 22.0, r.center().y), text_w + 2.0, label, regular(11.5), if on { t.text } else { t.text_dim });
        resp.clicked()
    }

    /// A small square toggle (a day of the week). `true` when it was clicked.
    pub fn day_chip(ui: &mut Ui, label: &str, on: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let (r, resp) = ui.allocate_exact_size(vec2(42.0, ROW_H), Sense::click());
        let fill = if on {
            t.checked
        } else if resp.hovered() {
            t.hover
        } else {
            t.input
        };
        ui.painter().rect(r, 4.0, fill, Stroke::new(1.0, if on { t.accent } else { t.input_border }), StrokeKind::Inside);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, regular(12.0), if on { t.text } else { t.text_dim });
        resp.clicked()
    }

    /// A choice of a few values as buttons joined in a row.
    pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) {
        let t = Tokens::get(ui.ctx());
        let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 0.0);
        for (i, (option, label)) in options.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(vec2(78.0, ROW_H), Sense::click());
            let on = *value == *option;
            let (left, right) = (if i == 0 { 4 } else { 0 }, if i + 1 == options.len() { 4 } else { 0 });
            let corners = CornerRadius { nw: left, sw: left, ne: right, se: right };
            let fill = if on {
                t.checked
            } else if resp.hovered() {
                t.hover
            } else {
                t.input
            };
            ui.painter().rect(r, corners, fill, Stroke::new(1.0, if on { t.accent } else { t.input_border }), StrokeKind::Inside);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, *label, regular(12.0), if on { t.text } else { t.text_dim });
            if resp.clicked() {
                *value = *option;
            }
        }
        ui.spacing_mut().item_spacing.x = gap;
    }

    /// Toggle chips for the mailbox's categories, and for names the item carries that the
    /// mailbox doesn't list.
    pub fn category_chips(ui: &mut Ui, all: &[Category], chosen: &mut Vec<String>) {
        let t = Tokens::get(ui.ctx());
        let mut chips: Vec<(String, Color32)> = all.iter().map(|c| (c.name.clone(), c32(c.color))).collect();
        for name in chosen.iter() {
            if !chips.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                chips.push((name.clone(), t.text_disabled));
            }
        }
        if chips.is_empty() {
            ui.label(egui::RichText::new(tl!("No categories yet")).color(t.text_dim));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (name, color) in chips {
                let on = chosen.iter().any(|c| c.eq_ignore_ascii_case(&name));
                if chip(ui, &name, on, color) {
                    chosen.retain(|c| !c.eq_ignore_ascii_case(&name));
                    if !on {
                        chosen.push(name);
                    }
                }
            }
        });
    }

    /// A command's error, in red under the fields. Nothing when there is none.
    pub fn error(ui: &mut Ui, message: &str) {
        if message.is_empty() {
            return;
        }
        let t = Tokens::get(ui.ctx());
        ui.label(egui::RichText::new(shown(message, 400)).color(t.red));
    }

    /// Text from elsewhere cut to `max` characters on one line, for a label.
    pub fn shown(text: &str, max: usize) -> String {
        let mut out: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect();
        if text.chars().nth(max).is_some() {
            out.push('…');
        }
        out
    }

    /// Which of the buttons at the bottom was clicked.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Footer {
        pub ok: bool,
        pub cancel: bool,
        pub delete: bool,
    }

    /// The buttons at the bottom: OK and Cancel on the right, and Delete on the left when
    /// something existing is being edited.
    pub fn footer(ui: &mut Ui, ok_label: &str, ok_enabled: bool, delete: bool) -> Footer {
        let t = Tokens::get(ui.ctx());
        let mut out = Footer::default();
        strip(ui, |ui| {
            out.cancel = ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
            out.ok = primary_button(ui, ok_label, ok_enabled).clicked();
            if delete {
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    out.delete = ui.add(egui::Button::new(egui::RichText::new(tl!("Delete")).color(t.red)).min_size(vec2(72.0, 26.0))).clicked();
                });
            }
        });
        out
    }

    /// The one button of a dialog that only manages things. `true` when it was clicked.
    pub fn close_button(ui: &mut Ui) -> bool {
        let mut close = false;
        strip(ui, |ui| close = primary_button(ui, tl!("Close"), true).clicked());
        close
    }

    /// The strip the bottom buttons sit in, laid out from the right. It has a height of its own,
    /// so the buttons stay right under the fields however much room the window has left.
    fn strip(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
        ui.add_space(4.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width().max(0.0), 26.0), egui::Layout::right_to_left(egui::Align::Center), add);
    }

    /// The fields of a JSON object; none for anything else.
    pub fn object(v: Value) -> Map<String, Value> {
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }

    /// The fields of `now` that differ from `before`. A field in one of the `groups` brings the
    /// rest of its group along (a new start is sent with the end it goes with).
    pub fn changes(before: &Map<String, Value>, now: &Map<String, Value>, groups: &[&[&str]]) -> Map<String, Value> {
        let differs = |k: &str| before.get(k) != now.get(k);
        now.iter()
            .filter(|(k, _)| match groups.iter().find(|g| g.contains(&k.as_str())) {
                Some(group) => group.iter().any(|k| differs(k)),
                None => differs(k),
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// A window's own row of commands, in the look of the main window's simplified ribbon: one
    /// row of buttons on a sheet, in which what doesn't fit first loses its label and then goes
    /// behind `…`.
    pub mod bar {
        use egui::{Align2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

        use crate::control::HitMap;
        use crate::icons;
        use crate::theme::{Tokens, medium, regular};
        use crate::widgets::bg;

        /// The measures of the simplified ribbon's row (which keeps its own to itself): the
        /// sheet's height and a button's; in a button, the space at its ends, its icon, the gap
        /// before its label and the label's size, the part that holds its `▾` and the gap before
        /// that part with a label and without; the gap between two buttons, and between two
        /// that a rule parts; and the `…` button.
        const BAND_H: f32 = 48.0;
        const BUTTON_H: f32 = 32.0;
        const PAD: f32 = 8.0;
        const ICON: f32 = 20.0;
        const LABEL_GAP: f32 = 8.0;
        const LABEL_SIZE: f32 = 13.333;
        const DROP_W: f32 = 22.0;
        const DROP_GAP_LABEL: f32 = 2.0;
        const DROP_GAP_ICON: f32 = 5.0;
        const BUTTON_GAP: f32 = 2.0;
        const RULE_GAP: f32 = 9.0;
        const MORE_W: f32 = 36.0;
        /// The room over the sheet, under it and beside it.
        const OVER: f32 = 6.0;
        const UNDER: f32 = 7.0;
        const SIDE: f32 = 8.0;
        /// What the bar takes of a window's height.
        pub const HEIGHT: f32 = OVER + BAND_H + UNDER;

        /// One button of the bar.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Item {
            /// What the hit map and the form call it.
            pub key: &'static str,
            pub icon: &'static str,
            /// What it says, in the interface language (for a choice: what is chosen).
            pub label: String,
            /// What its tooltip calls it, and the menu behind `…`.
            pub tip: String,
            /// Which stretch of the row it is in: a rule parts two stretches.
            pub section: u8,
            /// How much it matters, 1 the most: what matters least gives way first.
            pub rank: u8,
            /// Its icon says what it is, so its label is among the first to go.
            pub plain: bool,
            /// It opens a menu rather than doing something.
            pub menu: bool,
            /// It is something that stays on, and it is on.
            pub checked: bool,
            /// The one that finishes the window (Save & Close, Send): it stands out.
            pub primary: bool,
            pub on: bool,
        }

        /// The id a button of the bar of the window numbered `window` goes by in the hit map
        /// (kind `windowBar`): `…` is `more`.
        pub fn hit_id(window: u64, key: &str) -> u64 {
            crate::ribbon::hit_id(&format!("{window}:{key}"))
        }

        fn button_width(item: &Item, labelled: bool, text: f32) -> f32 {
            let label = if labelled { LABEL_GAP + text } else { 0.0 };
            let end = match (item.menu, labelled) {
                (true, true) => DROP_GAP_LABEL + DROP_W,
                (true, false) => DROP_GAP_ICON + DROP_W,
                (false, _) => PAD,
            };
            PAD + ICON + label + end
        }

        fn row_width(items: &[Item], state: &[(bool, bool)], width: &dyn Fn(usize, bool) -> f32) -> f32 {
            let mut total = 0.0;
            let mut before: Option<u8> = None;
            for (i, (item, (shown, labelled))) in items.iter().zip(state).enumerate() {
                if !shown {
                    continue;
                }
                if let Some(section) = before {
                    total += if section == item.section { BUTTON_GAP } else { RULE_GAP };
                }
                total += width(i, *labelled);
                before = Some(item.section);
            }
            if state.iter().any(|(shown, _)| !shown) {
                total += BUTTON_GAP + MORE_W;
            }
            total
        }

        /// Fit `items` into `available`, as the simplified ribbon fits its row: the labels of
        /// the buttons whose icon says what they are go first, then the other labels (all but
        /// the first-rank buttons'), then buttons go behind `…` (all but the first rank), then
        /// the first-rank labels and last those buttons. In each step what matters least goes
        /// first, and of two the one further along. Answers the buttons in the row, each with
        /// whether its label shows, and the ones behind `…`.
        pub fn fit(items: &[Item], width: &dyn Fn(usize, bool) -> f32, available: f32) -> (Vec<(usize, bool)>, Vec<usize>) {
            let mut state = vec![(true, true); items.len()];
            let mut steps: Vec<(u8, u8, usize, bool)> = Vec::with_capacity(items.len() * 2);
            for (i, item) in items.iter().enumerate() {
                let first = item.rank <= 1;
                let label = if item.plain {
                    0
                } else if first {
                    3
                } else {
                    1
                };
                steps.push((label, item.rank, i, false));
                steps.push((if first { 4 } else { 2 }, item.rank, i, true));
            }
            steps.sort_by_key(|(step, rank, place, _)| (*step, std::cmp::Reverse(*rank), std::cmp::Reverse(*place)));
            for (_, _, i, out) in steps {
                if row_width(items, &state, width) <= available {
                    break;
                }
                if let Some(s) = state.get_mut(i) {
                    *s = if out { (false, false) } else { (s.0, false) };
                }
            }
            (
                state.iter().enumerate().filter(|(_, (shown, _))| *shown).map(|(i, (_, labelled))| (i, *labelled)).collect(),
                state.iter().enumerate().filter(|(_, (shown, _))| !shown).map(|(i, _)| i).collect(),
            )
        }

        /// A small `▾`, centred on `c`.
        fn drop_mark(ui: &Ui, c: egui::Pos2, color: egui::Color32) {
            ui.painter()
                .add(Shape::line(vec![pos2(c.x - 2.75, c.y - 1.0), pos2(c.x, c.y + 1.25), pos2(c.x + 2.75, c.y - 1.0)], Stroke::new(1.1, color)));
        }

        /// Lay the bar out across `ui`, for the window numbered `window`. `menu` lists what is
        /// under the `▾` of the button with the key it is given. Answers the key of the button
        /// that was clicked.
        pub fn show(ui: &mut Ui, hit: &mut HitMap, window: u64, items: &[Item], menu: &mut dyn FnMut(&mut Ui, &'static str)) -> Option<&'static str> {
            let t = Tokens::get(ui.ctx());
            let (strip, _) = ui.allocate_exact_size(vec2(ui.available_width().max(0.0), HEIGHT), Sense::hover());
            ui.painter().rect_filled(strip, 0.0, t.tab_strip);
            let sheet = Rect::from_min_max(pos2(strip.min.x + SIDE, strip.min.y + OVER), pos2(strip.max.x - SIDE, strip.min.y + OVER + BAND_H));
            // In the dark the sheet stands out by itself; on a light window it needs its edge.
            let line = if t.dark { Stroke::NONE } else { Stroke::new(1.0, t.border) };
            ui.painter().rect(sheet, 8.0, t.ribbon, line, egui::StrokeKind::Inside);
            let edge = (BAND_H - BUTTON_H) / 2.0;
            let area = Rect::from_min_max(pos2(sheet.min.x + SIDE, sheet.min.y + edge), pos2(sheet.max.x - SIDE, sheet.max.y - edge));
            let names: Vec<f32> = items
                .iter()
                .map(|item| {
                    let font = if item.primary { medium(LABEL_SIZE) } else { regular(LABEL_SIZE) };
                    ui.ctx().fonts_mut(|f| f.layout_no_wrap(item.label.clone(), font, t.text).size().x)
                })
                .collect();
            let width = |i: usize, labelled: bool| match (items.get(i), names.get(i)) {
                (Some(item), Some(text)) => button_width(item, labelled, *text),
                _ => 0.0,
            };
            let (shown, more) = fit(items, &width, area.width());
            let mut clicked = None;
            let mut x = area.min.x;
            let mut before: Option<u8> = None;
            for (i, labelled) in &shown {
                let Some(item) = items.get(*i) else { continue };
                if let Some(section) = before {
                    if section == item.section {
                        x += BUTTON_GAP;
                    } else {
                        // A rule between two stretches of the row.
                        let rule = (x + RULE_GAP / 2.0).floor() + 0.5;
                        ui.painter().line_segment([pos2(rule, area.min.y), pos2(rule, area.max.y)], Stroke::new(1.0, t.border_strong));
                        x += RULE_GAP;
                    }
                }
                let w = width(*i, *labelled);
                let rect = Rect::from_min_size(pos2(x, area.min.y), vec2(w, area.height()));
                let resp = ui.interact(rect, ui.scope_id().with(("window_bar", window, item.key)), Sense::click());
                hit.add("windowBar", hit_id(window, item.key), rect);
                let (ink, accent, text) = if !item.on {
                    (t.text_disabled, t.text_disabled, t.text_disabled)
                } else if item.primary {
                    let fill = if resp.is_pointer_button_down_on() || resp.hovered() { t.accent_text } else { t.accent };
                    ui.painter().rect_filled(rect, 4.0, fill);
                    (t.on_accent, t.on_accent, t.on_accent)
                } else {
                    bg(ui, rect, &resp, item.checked, &t);
                    (t.icon, t.accent, t.text)
                };
                let icon = Rect::from_center_size(pos2(rect.min.x + PAD + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
                icons::paint(ui.painter(), icon, item.icon, ink, accent);
                if *labelled {
                    let font = if item.primary { medium(LABEL_SIZE) } else { regular(LABEL_SIZE) };
                    ui.painter().text(pos2(icon.max.x + LABEL_GAP, rect.center().y), Align2::LEFT_CENTER, item.label.as_str(), font, text);
                }
                let tip = if *labelled && item.tip == item.label { None } else { Some(item.tip.as_str()) };
                let resp = match tip {
                    Some(tip) => resp.on_hover_text(tip),
                    None => resp,
                };
                if item.menu {
                    drop_mark(ui, pos2(rect.max.x - DROP_W / 2.0, rect.center().y), ink);
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(200.0);
                        menu(ui, item.key);
                    });
                } else if resp.clicked() && item.on {
                    clicked = Some(item.key);
                }
                x += w;
                before = Some(item.section);
            }
            if !more.is_empty() {
                // What didn't fit, behind `…` at the end of the row.
                let rect = Rect::from_min_size(pos2(area.max.x - MORE_W, area.min.y), vec2(MORE_W, area.height()));
                let resp = ui.interact(rect, ui.scope_id().with(("window_bar_more", window)), Sense::click());
                hit.add("windowBar", hit_id(window, "more"), rect);
                bg(ui, rect, &resp, false, &t);
                icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(ICON, ICON)), "more", t.icon, t.accent);
                let resp = resp.on_hover_text(tl!("More commands"));
                egui::Popup::menu(&resp).show(|ui| {
                    ui.set_min_width(200.0);
                    let mut before: Option<u8> = None;
                    for item in more.iter().filter_map(|i| items.get(*i)) {
                        if before.is_some_and(|section| section != item.section) {
                            ui.separator();
                        }
                        before = Some(item.section);
                        if item.menu {
                            ui.menu_button(item.tip.as_str(), |ui| menu(ui, item.key));
                        } else if ui
                            .add_enabled(item.on, egui::Button::new(item.tip.as_str()).selected(item.checked).min_size(vec2(190.0, 0.0)))
                            .clicked()
                        {
                            clicked = Some(item.key);
                            ui.close();
                        }
                    }
                });
            }
            clicked
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            fn item(key: &'static str, section: u8, rank: u8, plain: bool) -> Item {
                Item {
                    key,
                    icon: "save",
                    label: key.to_string(),
                    tip: key.to_string(),
                    section,
                    rank,
                    plain,
                    menu: false,
                    checked: false,
                    primary: false,
                    on: true,
                }
            }

            #[test]
            fn labels_give_way_before_buttons_and_what_matters_most_last() {
                let items = [item("save", 0, 1, false), item("delete", 0, 3, true), item("showAs", 1, 5, false), item("private", 1, 6, true)];
                // Each button is 40 wide, and 100 with its label.
                let width = |_: usize, labelled: bool| if labelled { 100.0 } else { 40.0 };
                let all = |available: f32| fit(&items, &width, available);
                // Everything fits: 4 × 100, two gaps of 2 and a rule's 9.
                assert_eq!(all(413.0), (vec![(0, true), (1, true), (2, true), (3, true)], vec![]));
                // A little less: the labels of what its icon explains go, the last first.
                assert_eq!(all(412.0).0, [(0, true), (1, true), (2, true), (3, false)]);
                assert_eq!(all(300.0).0, [(0, true), (1, false), (2, true), (3, false)]);
                // Then the other labels, all but the first rank's.
                assert_eq!(all(240.0), (vec![(0, true), (1, false), (2, false), (3, false)], vec![]));
                // Then buttons go behind `…`, which takes room of its own; the first rank stays.
                assert_eq!(all(230.0), (vec![(0, true), (1, false), (2, false)], vec![3]));
                assert_eq!(all(150.0), (vec![(0, true)], vec![1, 2, 3]));
                // Last its label, and then the button itself.
                assert_eq!(all(100.0), (vec![(0, false)], vec![1, 2, 3]));
                assert_eq!(all(0.0), (vec![], vec![0, 1, 2, 3]));
                // Nothing to fit, and nonsense for a width.
                assert_eq!(fit(&[], &width, 100.0), (vec![], vec![]));
                assert_eq!(all(f32::NAN).1.len(), 4);
                assert_ne!(hit_id(1, "save"), hit_id(2, "save"));
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn date_and_time_fields_read_what_people_type() {
            assert!(date_ok("2026-10-16", true) && date_ok(" 20261016 ", true));
            assert!(!date_ok("2026-02-30", true) && !date_ok("16/10/2026", false) && !date_ok("tomorrow", false));
            // An empty field is fine only where a date may be left out.
            assert!(date_ok("  ", false) && !date_ok("", true));
            assert_eq!(parse_clock("9"), Some((9, 0)));
            assert_eq!(parse_clock("9:30"), Some((9, 30)));
            assert_eq!(parse_clock("9am"), Some((9, 0)));
            assert_eq!(parse_clock("14:15"), Some((14, 15)));
            assert_eq!(parse_clock(" 9:30 PM "), Some((21, 30)));
            assert_eq!(parse_clock("12am"), Some((0, 0)));
            for wrong in ["", "25:00", "9:75", "13pm", "noon", "9.5", "-1"] {
                assert_eq!(parse_clock(wrong), None, "{wrong}");
            }
            assert!(time_ok("", false) && !time_ok("", true) && time_ok("7pm", true) && !time_ok("7 o'clock", false));
            assert_eq!((clock_text(870, false).as_str(), clock_text(870, true).as_str()), ("14:30", "2:30 PM"));
            let _ = clock_text(u32::MAX, true);
        }

        #[test]
        fn fields_become_command_values() {
            assert_eq!(stamp("2026-10-16", "3pm"), "2026-10-16T15:00");
            assert_eq!(stamp(" 2026-10-16 ", "09:05"), "2026-10-16T09:05");
            // Text that isn't a date or a time goes as typed, for the command to refuse.
            assert_eq!(stamp(" soon ", "late "), "soonTlate");
            assert_eq!(date_value("2026-10-16"), json!("2026-10-16"));
            assert_eq!(date_value("20261016"), json!("2026-10-16"));
            assert_eq!(date_value("   "), Value::Null);
            assert_eq!(date_value(" 2026-1-1 "), json!("2026-1-1"));
        }

        #[test]
        fn time_menus_list_half_hours() {
            let all = slots(None, true);
            assert_eq!(all.len(), 48);
            assert_eq!(all.first(), Some(&(0, "12:00 AM".to_string())));
            assert_eq!(all.last(), Some(&(1410, "11:30 PM".to_string())));
            assert_eq!(slots(None, false).get(29), Some(&(870, "14:30".to_string())));
            // After a start, later times that day with how long the event would be.
            let after = slots(Some(15 * 60), true);
            assert_eq!(after.first(), Some(&(930, "3:30 PM (30 minutes)".to_string())));
            assert_eq!(after.get(1), Some(&(960, "4:00 PM (1 hour)".to_string())));
            assert_eq!(after.get(2).map(|s| s.1.as_str()), Some("4:30 PM (1.5 hours)"));
            assert_eq!(after.last().map(|s| s.0), Some(1410));
            // Nothing is left of the day after 11:30 PM: the whole list again.
            assert_eq!(slots(Some(23 * 60 + 45), true).len(), 48);
            assert_eq!(slots(Some(u32::MAX), true).len(), 48);
        }

        #[test]
        fn only_what_changed_is_sent() {
            let before = object(json!({"title": "A", "start": "s", "end": "e", "allDay": false, "private": false}));
            let same = changes(&before, &before, &[&["start", "end", "allDay"]]);
            assert!(same.is_empty());
            let now = object(json!({"title": "B", "start": "s2", "end": "e", "allDay": false, "private": false, "calendar": 3}));
            let got = changes(&before, &now, &[&["start", "end", "allDay"]]);
            // A new start brings the end and All day along; a new field counts as changed.
            assert_eq!(Value::Object(got), json!({"title": "B", "start": "s2", "end": "e", "allDay": false, "calendar": 3}));
            assert_eq!(object(json!([1, 2])), Map::new());
        }

        #[test]
        fn hostile_text_is_cut_for_labels() {
            assert_eq!(shown("a\tb\nc\u{7}", 10), "a b c ");
            assert_eq!(shown("abcdef", 3), "abc…");
            // Cut by characters, never inside one.
            let long = "é\u{202e}\u{0}".repeat(10_000);
            assert_eq!(shown(&long, 400).chars().count(), 401);
            assert_eq!(shown("", 5), "");
        }
    }
}

/// The appointment and meeting form.
mod event {
    use egui::{Ui, vec2};
    use mailcraft_engine::Session;
    use mailcraft_model::{Address, Freq, Id, Importance, Recurrence, ResponseOptions, ShowAs, format_list};
    use mailcraft_time::{Date, DateTime, Weekday, Zone, month_name, time_short};
    use serde_json::{Map, Value, json};

    use super::kit::{
        self, Pass, bar, body_sized, category_chips, changes, chip, date_note, date_ok, day_chip, error, error_room, footer, multiline_rest, object,
        parse_clock, row, segmented, stamp, tall_row, time_ok,
    };
    use super::{Ask, Verdict};
    use crate::MailApp;
    use crate::theme::{Tokens, c32, semibold};

    /// The reminders offered, in minutes before the start (`None`: no reminder).
    const REMINDERS: [Option<u32>; 9] = [None, Some(0), Some(5), Some(10), Some(15), Some(30), Some(60), Some(120), Some(1440)];

    /// The room round the fields of a form that is in a window of its own.
    const MARGIN: i8 = 16;
    /// How wide a window of its own opens that shows an appointment or a meeting.
    pub const WINDOW_WIDTH: f32 = 760.0;

    /// How a series ends.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Ends {
        #[default]
        Never,
        /// After a number of times.
        After,
        /// On a date.
        Until,
    }

    /// How an item repeats, as the form keeps it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Repeat {
        /// `None`: the event happens once.
        pub freq: Option<Freq>,
        pub interval: u32,
        /// Weekly: the days it happens on.
        pub days: Vec<Weekday>,
        /// Monthly and yearly: count the start's weekday in its month (`Some(2)`: the second one,
        /// `Some(-1)`: the last) instead of using the day's number.
        pub nth: Option<i8>,
        pub ends: Ends,
        pub count: u32,
        pub until: String,
    }

    impl Default for Repeat {
        fn default() -> Self {
            Repeat { freq: None, interval: 1, days: Vec::new(), nth: None, ends: Ends::Never, count: 10, until: String::new() }
        }
    }

    /// Which of its weekday a date is in its month: `(2, Wed)` for a second Wednesday, `-1` for
    /// the last one of the month (also when it is the fourth).
    pub fn nth_weekday(date: Date) -> (i8, Weekday) {
        let last = u16::from(date.day) + 7 > u16::from(date.end_of_month().day);
        (if last { -1 } else { ordinal_in_month(date) }, date.weekday())
    }

    /// 1 for the first seven days of a month, 2 for the next seven, up to 5.
    fn ordinal_in_month(date: Date) -> i8 {
        i8::try_from(date.day.saturating_sub(1) / 7 + 1).unwrap_or(1)
    }

    /// The ways to count a date's weekday in its month: `[2]`, `[-1]`, or `[4, -1]` for a fourth
    /// one that is also the last.
    pub fn nth_choices(date: Date) -> Vec<i8> {
        match (nth_weekday(date).0, ordinal_in_month(date)) {
            (-1, 4) => vec![4, -1],
            (n, _) => vec![n],
        }
    }

    /// `nth` when the date can be counted that way, else the way [`nth_weekday`] counts it.
    fn fit_nth(date: Date, nth: i8) -> i8 {
        if nth_choices(date).contains(&nth) { nth } else { nth_weekday(date).0 }
    }

    fn ordinal(nth: i8) -> &'static str {
        match nth {
            1 => "first",
            2 => "second",
            3 => "third",
            4 => "fourth",
            5 => "fifth",
            _ => "last",
        }
    }

    /// The `nth` (1 to 5, or -1 for the last) `weekday` of the month that starts on `first`;
    /// `None` when the month has no such day (a fifth Friday).
    fn nth_day(first: Date, weekday: Weekday, nth: i8) -> Option<Date> {
        if nth > 0 {
            let offset = (i64::from(weekday.index()) - i64::from(first.weekday().index())).rem_euclid(7);
            let day = first.add_days(offset + 7 * (i64::from(nth.min(5)) - 1));
            (day.month == first.month && day.year == first.year).then_some(day)
        } else {
            let last = first.end_of_month();
            let back = (i64::from(last.weekday().index()) - i64::from(weekday.index())).rem_euclid(7);
            Some(last.add_days(-back))
        }
    }

    /// Monday to Friday.
    const WEEKDAYS: [Weekday; 5] = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri];

    impl Repeat {
        /// The fields for an event's rule; the series starts on `start`.
        pub fn of(rule: Option<&Recurrence>, start: Date) -> Repeat {
            let Some(r) = rule else { return Repeat::default() };
            Repeat {
                freq: Some(r.freq),
                interval: r.interval.clamp(1, 999),
                days: match r.freq {
                    Freq::Weekly if r.by_day.is_empty() => vec![start.weekday()],
                    Freq::Daily | Freq::Weekly => r.by_day.clone(),
                    Freq::Monthly | Freq::Yearly => Vec::new(),
                },
                nth: match r.freq {
                    Freq::Monthly | Freq::Yearly => r.nth.filter(|n| *n != 0).map(|n| fit_nth(start, n)),
                    Freq::Daily | Freq::Weekly => None,
                },
                ends: if r.count.is_some() {
                    Ends::After
                } else if r.until.is_some() {
                    Ends::Until
                } else {
                    Ends::Never
                },
                count: r.count.unwrap_or(10).clamp(1, 9999),
                until: r.until.map(Date::iso).unwrap_or_default(),
            }
        }

        /// The rule the fields describe, for a series that starts on `start`; `None` when the
        /// event happens once.
        pub fn rule(&self, start: Date) -> Option<Recurrence> {
            let freq = self.freq?;
            let mut by_day = match freq {
                Freq::Daily => self.days.clone(),
                Freq::Weekly if self.days.is_empty() => vec![start.weekday()],
                Freq::Weekly => self.days.clone(),
                Freq::Monthly | Freq::Yearly if self.nth.is_some() => vec![start.weekday()],
                Freq::Monthly | Freq::Yearly => Vec::new(),
            };
            by_day.sort();
            by_day.dedup();
            Some(Recurrence {
                freq,
                interval: self.interval.clamp(1, 999),
                by_day,
                nth: match freq {
                    Freq::Monthly | Freq::Yearly => self.nth.map(|n| fit_nth(start, n)),
                    Freq::Daily | Freq::Weekly => None,
                },
                count: (self.ends == Ends::After).then_some(self.count.clamp(1, 9999)),
                until: if self.ends == Ends::Until { Date::parse_iso(&self.until) } else { None },
            })
        }

        /// Repeat another way. A weekly series starts on the start's own day.
        fn set_freq(&mut self, freq: Option<Freq>, start: Option<Date>) {
            if freq == self.freq {
                return;
            }
            self.freq = freq;
            self.days = match (freq, start) {
                (Some(Freq::Weekly), Some(d)) => vec![d.weekday()],
                _ => Vec::new(),
            };
            self.nth = None;
        }

        /// The start moved from one day to another: a weekly rule on the old day alone, and a
        /// rule that counts weekdays of the month, follow it.
        fn follow(&mut self, before: Option<Date>, now: Date) {
            if self.freq == Some(Freq::Weekly)
                && let Some(b) = before
                && self.days == [b.weekday()]
            {
                self.days = vec![now.weekday()];
            }
            self.nth = self.nth.map(|n| fit_nth(now, n));
        }
    }

    /// A rule as `calendar.new` takes it.
    fn rule_json(r: &Recurrence) -> Value {
        let mut m = Map::new();
        m.insert("freq".into(), json!(r.freq.code()));
        m.insert("interval".into(), json!(r.interval));
        if !r.by_day.is_empty() {
            m.insert("byDay".into(), json!(r.by_day.iter().map(|d| d.short().to_lowercase()).collect::<Vec<_>>()));
        }
        if let Some(n) = r.nth {
            m.insert("nth".into(), json!(n));
        }
        if let Some(c) = r.count {
            m.insert("count".into(), json!(c));
        }
        if let Some(u) = r.until {
            m.insert("until".into(), json!(u.iso()));
        }
        Value::Object(m)
    }

    fn reminder_label(minutes: Option<u32>) -> String {
        match minutes {
            None => tl!("None").to_string(),
            Some(0) => tl!("At start time").to_string(),
            Some(m) => crate::dates::duration(i64::from(m)),
        }
    }

    fn freq_label(freq: Freq) -> &'static str {
        match freq {
            Freq::Daily => "Daily",
            Freq::Weekly => "Weekly",
            Freq::Monthly => "Monthly",
            Freq::Yearly => "Yearly",
        }
    }

    /// `day` or `days` after "every N".
    fn unit(freq: Freq, interval: u32) -> &'static str {
        match (freq, interval == 1) {
            (Freq::Daily, true) => "day",
            (Freq::Daily, false) => "days",
            (Freq::Weekly, true) => "week",
            (Freq::Weekly, false) => "weeks",
            (Freq::Monthly, true) => "month",
            (Freq::Monthly, false) => "months",
            (Freq::Yearly, true) => "year",
            (Freq::Yearly, false) => "years",
        }
    }

    /// When a new event starts and ends, as an event keeps it (whole days end at the midnight
    /// after the last one): what `start` and `end` say (ISO; a date alone is fine, and names the
    /// last day of a whole-day event), else what `calendar.new` would pick: the next half hour
    /// today, the start of the working day on another day, half an hour long. The third time is
    /// a start with a time of day, for when the event stops being all day.
    fn new_span(s: &Session, start: Option<&str>, end: Option<&str>, all_day: bool) -> (DateTime, DateTime, DateTime) {
        let now = s.local_now();
        let clock = |day: Date| {
            if day == now.date {
                DateTime::at_midnight(day).add_minutes((i64::from(now.minutes_of_day()) / 30 + 1) * 30)
            } else {
                DateTime { hour: s.prefs.work_start.min(23), ..DateTime::at_midnight(day) }
            }
        };
        let read = |text: Option<&str>| text.and_then(|t| mailcraft_time::parse_iso(t).map(|(at, _)| (at, t.trim().contains(['T', 't', ' ']))));
        let timed = match read(start) {
            Some((at, true)) => at,
            Some((at, false)) => clock(at.date),
            None => clock(s.view.date),
        };
        if all_day {
            let first = read(start).map_or(timed.date, |(at, _)| at.date);
            let last = read(end).map_or(first, |(at, _)| at.date).max(first);
            return (DateTime::at_midnight(first), DateTime::at_midnight(last.add_days(1)), timed);
        }
        let end = match read(end) {
            Some((at, true)) => at,
            Some((at, false)) => DateTime { date: at.date, ..timed }.add_minutes(30),
            None => timed.add_minutes(30),
        };
        (timed, if end < timed { timed.add_minutes(30) } else { end }, timed)
    }

    /// An attendee line with the names on it that stand for exactly one contact replaced by
    /// that contact (as the composer's Check Names does it), and what on it is neither an
    /// address nor anybody's name. An address is left as it was typed.
    pub fn check_names(s: &Session, line: &str) -> (String, Vec<String>) {
        let mut unresolved = Vec::new();
        let list: Vec<Address> = Address::parse_list(line)
            .into_iter()
            .take(500)
            .map(|a| {
                if a.is_valid() {
                    return a;
                }
                let typed = a.email.clone();
                let mut hits = s.mailbox.contacts.values().filter(|k| k.matches(&typed)).filter_map(|k| k.address()).filter(Address::is_valid);
                match (hits.next(), hits.next()) {
                    (Some(one), None) => one,
                    _ => {
                        unresolved.push(typed.chars().filter(|c| !c.is_control()).take(80).collect());
                        a
                    }
                }
            })
            .collect();
        (format_list(&list), unresolved)
    }

    /// The people an attendee line names, as it reads while nobody is typing in it: each by
    /// name (the one on the line, or the one a contact with that address has), underlined when
    /// it is somebody, and in red when it is neither an address nor a name that was found.
    fn names_job(s: &Session, line: &str, font: egui::FontId, t: &Tokens) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        job.break_on_newline = false;
        let plain = egui::TextFormat { font_id: font.clone(), color: t.text, ..Default::default() };
        // (A line with very many people on it isn't looked up one by one.)
        let people = Address::parse_list(line);
        let look_up = people.len() <= 60;
        for (i, who) in people.iter().take(500).enumerate() {
            if i > 0 {
                job.append("; ", 0.0, plain.clone());
            }
            let known = who.is_valid();
            let name: String = if !who.name.is_empty() || !known || !look_up {
                who.display().to_string()
            } else {
                let contact = s.mailbox.contacts.values().find(|c| c.has_email(&who.email)).map(|c| c.name()).filter(|name| !name.trim().is_empty());
                contact.unwrap_or_else(|| who.email.clone())
            };
            let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).take(120).collect();
            let format = if known {
                egui::TextFormat { underline: egui::Stroke::new(1.0, t.text_dim), ..plain.clone() }
            } else {
                egui::TextFormat { color: t.red, ..plain.clone() }
            };
            job.append(&name, 0.0, format);
        }
        job
    }

    /// An attendee line. As it is typed in, recipients are suggested underneath, as the composer
    /// suggests them ([`crate::compose::recipient_line`]); a line that is only passed through
    /// (with Tab, say) is suggested nothing and stays exactly as it is. When it is left,
    /// the names on it that stand for one contact become that contact, and from then on it reads
    /// as the people it names.
    fn attendee_line(ui: &mut Ui, pass: &mut Pass, s: &Session, key: &'static str, text: &mut String, hint: &str) {
        let t = pass.t;
        let id = kit::field_id(key);
        let focused = ui.memory(|m| m.has_focus(id));
        // Something was typed into the line since the keyboard came to it.
        let typing = id.with("typed");
        let typed = focused && ui.data(|d| d.get_temp::<bool>(typing)).unwrap_or(false);
        // While nobody types in it, it reads as the people it names. (Laid out once for a text,
        // not every frame: finding a contact's name looks through the contacts.)
        let reading = !focused && !text.trim().is_empty();
        let font = egui::TextStyle::Body.resolve(ui.style());
        let names = reading.then(|| {
            use std::hash::{Hash as _, Hasher as _};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            (text.as_str(), s.rev(), t.dark, font.size.to_bits()).hash(&mut hasher);
            let stamp = hasher.finish();
            let kept = id.with("names");
            match ui.data(|d| d.get_temp::<(u64, egui::text::LayoutJob)>(kept)).filter(|(at, _)| *at == stamp) {
                Some((_, job)) => job,
                None => {
                    let job = names_job(s, text, font.clone(), &t);
                    ui.data_mut(|d| d.insert_temp(kept, (stamp, job.clone())));
                    job
                }
            }
        });
        let mut as_names = |ui: &Ui, _: &dyn egui::TextBuffer, _: f32| ui.painter().layout_job(names.clone().unwrap_or_default());
        let mut edit = egui::TextEdit::singleline(text).id(id).desired_width(f32::INFINITY).hint_text(hint.to_owned());
        if reading {
            edit = edit.layouter(&mut as_names);
        }
        let resp = crate::compose::recipient_line(ui, s, id, edit, typed).text_edit.response;
        pass.field(&resp);
        let typed = typed || resp.changed();
        if resp.lost_focus() && typed {
            // Left, after something was typed: what stands for one contact becomes that contact.
            let (checked, _) = check_names(s, text);
            if Address::parse_list(text) != Address::parse_list(&checked) {
                *text = checked;
            }
        }
        let typed = typed && ui.memory(|m| m.has_focus(id));
        ui.data_mut(|d| d.insert_temp(typing, typed));
    }

    /// A menu entry with a mark when it is the current choice. `true` when it was chosen.
    fn check_item(ui: &mut Ui, label: &str, checked: bool) -> bool {
        // An invisible mark keeps the unchecked labels aligned with the checked one.
        let mark = egui::RichText::new("✓");
        let mark = if checked { mark } else { mark.color(egui::Color32::TRANSPARENT) };
        let chosen = ui.add(egui::Button::new((mark, label)).selected(checked).min_size(vec2(190.0, 0.0))).clicked();
        if chosen {
            ui.close();
        }
        chosen
    }

    /// The pages of a meeting's window.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Page {
        /// The fields.
        #[default]
        Appointment,
        /// When everybody is free and busy.
        Scheduling,
    }

    impl Page {
        pub fn code(self) -> &'static str {
            match self {
                Page::Appointment => "appointment",
                Page::Scheduling => "scheduling",
            }
        }
    }

    /// Which of an item that repeats the form is about, when the owner said so on opening it:
    /// what the form saves, or deletes, is saved or deleted for that.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Scope {
        /// All of it.
        Series,
        /// The occurrence of this day, which the form shows on its own day.
        One(Date),
    }

    /// What is to happen to the window once a command of the form has gone through.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum After {
        Close,
        /// It stays open, on the item as it is saved now.
        Stay,
    }

    /// A command of the form that the owner is being asked about first (which of a series, mail
    /// to the people invited): the form waits for what comes of the question.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Waiting {
        /// The number of the last calendar command that had run when the question came up.
        seq: u64,
        command: &'static str,
        after: After,
    }

    /// What the owner asked of the form in a frame.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Act {
        Save {
            close: bool,
        },
        /// `pressed`: with the Send button itself, rather than with a key.
        Send {
            pressed: bool,
        },
        Delete,
    }

    /// How the Appointment Recurrence dialog was left.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Left {
        Ok,
        Cancel,
        Remove,
    }

    /// The Appointment Recurrence dialog: the time of day, how the item repeats (daily, weekly
    /// on chosen days, monthly or yearly by a date or by "the second Tuesday"), and the range
    /// (the day the series starts on; no end, after a number of times, by a date).
    ///
    /// The mailbox keeps a series by its first day: a pattern that names another day than the
    /// one the range starts on (the 15th, a Thursday) starts the series on the first such day on
    /// or after it, as Outlook does.
    #[derive(Clone, Debug, PartialEq)]
    pub struct RecurrenceDialog {
        /// How it repeats. While the dialog is open it repeats some way.
        pub repeat: Repeat,
        /// The day the range starts on.
        pub start: String,
        pub start_time: String,
        pub end_time: String,
        /// The length the item keeps when its start time changes.
        minutes: i64,
        all_day: bool,
        /// By a date: the day of the month.
        pub day: u32,
        /// By a weekday: which one. Yearly: the month.
        pub weekday: Weekday,
        pub month: u8,
        /// The item repeated when the dialog opened: there is a rule to remove.
        had_rule: bool,
        /// What it opened with: leaving it with OK untouched changes nothing.
        opened: Option<Box<RecurrenceDialog>>,
        focus: bool,
    }

    impl RecurrenceDialog {
        /// The dialog for an item that starts on `start` (its times as its fields have them) and
        /// repeats as `repeat` says; one that doesn't repeat yet is offered every week on its
        /// own day.
        pub fn open(repeat: &Repeat, start: Date, start_time: &str, end_time: &str, minutes: i64, all_day: bool) -> RecurrenceDialog {
            let had_rule = repeat.freq.is_some();
            let mut repeat = repeat.clone();
            if !had_rule {
                repeat = Repeat { freq: Some(Freq::Weekly), days: vec![start.weekday()], ..Repeat::default() };
            }
            let mut dialog = RecurrenceDialog {
                repeat,
                start: start.iso(),
                start_time: start_time.to_string(),
                end_time: end_time.to_string(),
                minutes: minutes.clamp(0, 366 * 24 * 60),
                all_day,
                day: u32::from(start.day),
                weekday: start.weekday(),
                month: start.month,
                had_rule,
                opened: None,
                focus: true,
            };
            dialog.opened = Some(Box::new(dialog.clone()));
            dialog
        }

        fn freq(&self) -> Freq {
            self.repeat.freq.unwrap_or(Freq::Weekly)
        }

        /// Whether anything was changed since it opened.
        fn touched(&self) -> bool {
            let now = RecurrenceDialog { opened: None, focus: true, ..self.clone() };
            self.opened.as_deref().is_none_or(|opened| *opened != now)
        }

        /// The first day on or after `from` that the pattern falls on: the day the series
        /// starts. `None` when there is no such day within reach (February 30th).
        pub fn first_day(&self, from: Date) -> Option<Date> {
            let on_days = |days: &[Weekday]| (0..7).map(|k| from.add_days(k)).find(|day| days.is_empty() || days.contains(&day.weekday()));
            let day = u8::try_from(self.day.clamp(1, 31)).unwrap_or(1);
            let month = self.month.clamp(1, 12);
            match (self.freq(), self.repeat.nth) {
                (Freq::Daily | Freq::Weekly, _) => on_days(&self.repeat.days),
                (Freq::Monthly, None) => (0..48).find_map(|k| {
                    let first = from.start_of_month().add_months(k);
                    Date::new(first.year, first.month, day).filter(|found| *found >= from)
                }),
                (Freq::Monthly, Some(nth)) => {
                    (0..48).find_map(|k| nth_day(from.start_of_month().add_months(k), self.weekday, nth).filter(|found| *found >= from))
                }
                (Freq::Yearly, None) => (0..12).find_map(|k| Date::new(from.year.saturating_add(k), month, day).filter(|found| *found >= from)),
                (Freq::Yearly, Some(nth)) => (0..12).find_map(|k| {
                    let first = Date::new(from.year.saturating_add(k), month, 1)?;
                    nth_day(first, self.weekday, nth).filter(|found| *found >= from)
                }),
            }
        }

        /// The rule as the form keeps it.
        fn as_repeat(&self) -> Repeat {
            let freq = self.freq();
            let mut days = match freq {
                Freq::Daily | Freq::Weekly => self.repeat.days.clone(),
                Freq::Monthly | Freq::Yearly => Vec::new(),
            };
            days.sort();
            days.dedup();
            Repeat {
                freq: Some(freq),
                interval: self.repeat.interval.clamp(1, 999),
                days,
                nth: match freq {
                    Freq::Monthly | Freq::Yearly => self.repeat.nth,
                    Freq::Daily | Freq::Weekly => None,
                },
                ends: self.repeat.ends,
                count: self.repeat.count.clamp(1, 9999),
                until: self.repeat.until.clone(),
            }
        }

        /// Repeat another way: a weekly pattern starts on the start's own day, a monthly and a
        /// yearly one by its date.
        fn set_freq(&mut self, freq: Freq) {
            if self.repeat.freq == Some(freq) {
                return;
            }
            let start = Date::parse_iso(&self.start);
            self.repeat.set_freq(Some(freq), start);
            if let Some(day) = start {
                (self.day, self.weekday, self.month) = (u32::from(day.day), day.weekday(), day.month);
            }
        }

        /// What keeps OK from being pressed, in words; `None` when all is well.
        fn wrong(&self) -> Option<&'static str> {
            let from = Date::parse_iso(&self.start);
            if !self.all_day && !(time_ok(&self.start_time, true) && time_ok(&self.end_time, true)) {
                Some("The start and the end need a time")
            } else if from.is_none() {
                Some("The range needs a day to start on")
            } else if self.repeat.ends == Ends::Until && !date_ok(&self.repeat.until, true) {
                Some("The range needs a day to end by")
            } else if from.and_then(|from| self.first_day(from)).is_none() {
                Some("No day fits this pattern")
            } else {
                None
            }
        }

        /// The start time changed: the end follows, keeping the length.
        fn start_time_moved(&mut self, twelve: bool) {
            if let Some((h, m)) = parse_clock(&self.start_time) {
                let end = (i64::from(h) * 60 + i64::from(m) + self.minutes).rem_euclid(24 * 60);
                self.end_time = kit::clock_text(u32::try_from(end).unwrap_or(0), twelve);
            }
        }

        /// The end time changed: the length is what it is now (an end before the start is on
        /// the day after).
        fn end_time_moved(&mut self) {
            if let (Some((h, m)), Some((eh, em))) = (parse_clock(&self.start_time), parse_clock(&self.end_time)) {
                let (start, end) = (i64::from(h) * 60 + i64::from(m), i64::from(eh) * 60 + i64::from(em));
                // Whole days it already lasted are kept.
                self.minutes = self.minutes / (24 * 60) * 24 * 60 + (end - start).rem_euclid(24 * 60);
            }
        }

        /// Lay the dialog out over the window that is being laid out. How it was left, once it was.
        fn show(&mut self, ctx: &egui::Context, s: &Session) -> Option<Left> {
            let t = Tokens::get(ctx);
            let mut left = None;
            let frame = egui::Frame::window(&ctx.global_style()).fill(t.surface).inner_margin(egui::Margin::same(16));
            let width = (ctx.content_rect().width() - 48.0).clamp(160.0, 540.0);
            let week_start = s.prefs.week_start;
            let mut enter = false;
            egui::Modal::new(kit::field_id("recurrence_dialog"))
                .frame(frame)
                .backdrop_color(egui::Color32::from_black_alpha(if t.dark { 110 } else { 60 }))
                .show(ctx, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                    ui.label(egui::RichText::new(tl!("Appointment Recurrence")).font(semibold(14.0)));
                    let mut pass = Pass::new(ui, s, &mut self.focus);
                    let twelve = pass.twelve;
                    let heading = |ui: &mut Ui, text: &str| {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(text).font(semibold(12.0)).color(t.text_dim));
                    };
                    egui::ScrollArea::vertical().max_height((ui.ctx().content_rect().height() - 160.0).max(120.0)).show(ui, |ui| {
                        heading(ui, tl!("Time"));
                        if self.all_day {
                            row(ui, tl!("Start"), |ui| {
                                ui.label(tl!("All day"));
                            });
                        } else {
                            row(ui, tl!("Start"), |ui| {
                                if pass.time(ui, "recurrence_start_time", &mut self.start_time, true, None) {
                                    self.start_time_moved(twelve);
                                }
                            });
                            row(ui, tl!("End"), |ui| {
                                let after = parse_clock(&self.start_time).map(|(h, m)| u32::from(h) * 60 + u32::from(m));
                                if pass.time(ui, "recurrence_end_time", &mut self.end_time, true, after) {
                                    self.end_time_moved();
                                }
                                ui.label(egui::RichText::new(format!("· {}", crate::dates::duration(self.minutes))).color(t.text_dim));
                            });
                        }
                        heading(ui, tl!("Pattern"));
                        row(ui, tl!("Repeats"), |ui| {
                            let mut freq = self.freq();
                            let options = [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly].map(|f| (f, tl!(freq_label(f))));
                            segmented(ui, &mut freq, &options);
                            self.set_freq(freq);
                        });
                        self.pattern(ui, week_start);
                        heading(ui, tl!("Range"));
                        row(ui, tl!("Starts"), |ui| {
                            pass.date(ui, "recurrence_start", &mut self.start, true);
                            date_note(ui, &self.start, true);
                        });
                        tall_row(ui, tl!("Ends"), |ui| {
                            if ui.radio(self.repeat.ends == Ends::Never, tl!("No end date")).clicked() {
                                self.repeat.ends = Ends::Never;
                            }
                            ui.horizontal(|ui| {
                                if ui.radio(self.repeat.ends == Ends::After, tl!("After")).clicked() {
                                    self.repeat.ends = Ends::After;
                                }
                                if ui.add(egui::DragValue::new(&mut self.repeat.count).range(1..=9999).speed(0.05)).changed() {
                                    self.repeat.ends = Ends::After;
                                }
                                // (The number is in the box before it: only the word follows it.)
                                ui.label(tln!(self.repeat.count, "time", "times"));
                            });
                            ui.horizontal(|ui| {
                                if ui.radio(self.repeat.ends == Ends::Until, tl!("By")).clicked() {
                                    self.repeat.ends = Ends::Until;
                                }
                                // A series that ends on a date starts with three months to run.
                                if self.repeat.ends == Ends::Until && self.repeat.until.trim().is_empty() {
                                    self.repeat.until = Date::parse_iso(&self.start).unwrap_or(pass.today).add_months(3).iso();
                                }
                                if self.repeat.ends == Ends::Until {
                                    pass.date(ui, "recurrence_until", &mut self.repeat.until, true);
                                    date_note(ui, &self.repeat.until, true);
                                }
                            });
                        });
                        // The rule in words, and the day the series starts on when that is not
                        // the day the range does.
                        let from = Date::parse_iso(&self.start);
                        if let Some((from, first)) = from.and_then(|from| Some((from, self.first_day(from)?)))
                            && let Some(rule) = self.as_repeat().rule(first)
                        {
                            let words = rule.describe(first);
                            let words = if first == from {
                                words
                            } else {
                                crate::i18n::fmt(tl!("{rule}, from {day}"), &[("rule", &words), ("day", &crate::dates::medium(first))])
                            };
                            ui.label(egui::RichText::new(words).color(t.text_dim));
                        }
                    });
                    let wrong = self.wrong();
                    if let Some(wrong) = wrong {
                        ui.label(egui::RichText::new(tl!(wrong)).color(t.red));
                    }
                    enter = pass.submit;
                    ui.add_space(4.0);
                    ui.allocate_ui_with_layout(vec2(ui.available_width().max(0.0), 26.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked() {
                            left = Some(Left::Cancel);
                        }
                        if crate::widgets::primary_button(ui, tl!("OK"), wrong.is_none()).clicked() {
                            left = Some(Left::Ok);
                        }
                        if self.had_rule {
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(tl!("Remove Recurrence")).min_size(vec2(72.0, 26.0))).clicked() {
                                    left = Some(Left::Remove);
                                }
                            });
                        }
                    });
                });
            // Escape leaves it as it was (when it is on top: a list open in it closes first, #406);
            // Enter in one of its fields, or with none, is OK.
            let escape = crate::keys::escape(ctx, crate::keys::modal_layer(kit::field_id("recurrence_dialog")));
            left.or(if escape {
                Some(Left::Cancel)
            } else if enter && self.wrong().is_none() {
                Some(Left::Ok)
            } else {
                None
            })
        }

        /// The part of the dialog that says on which days: it differs with how often.
        fn pattern(&mut self, ui: &mut Ui, week_start: Weekday) {
            let freq = self.freq();
            let every = |ui: &mut Ui, interval: &mut u32| {
                ui.add(egui::DragValue::new(interval).range(1..=999).speed(0.05));
                ui.label(tl!(unit(freq, *interval)));
            };
            let pick_nth = |ui: &mut Ui, salt: &str, nth: &mut i8| {
                egui::ComboBox::from_id_salt(kit::field_id(("recurrence_nth", salt))).width(84.0).selected_text(tl!(ordinal(*nth))).show_ui(
                    ui,
                    |ui| {
                        for n in [1, 2, 3, 4, -1] {
                            if ui.selectable_label(*nth == n, tl!(ordinal(n))).clicked() {
                                *nth = n;
                            }
                        }
                    },
                );
            };
            let pick_weekday = |ui: &mut Ui, salt: &str, weekday: &mut Weekday| {
                egui::ComboBox::from_id_salt(kit::field_id(("recurrence_weekday", salt))).width(104.0).selected_text(tl!(weekday.name())).show_ui(
                    ui,
                    |ui| {
                        for i in 0..7 {
                            let day = Weekday::from_index(i64::from(week_start.index()) + i);
                            if ui.selectable_label(*weekday == day, tl!(day.name())).clicked() {
                                *weekday = day;
                            }
                        }
                    },
                );
            };
            let pick_month = |ui: &mut Ui, salt: &str, month: &mut u8| {
                egui::ComboBox::from_id_salt(kit::field_id(("recurrence_month", salt))).width(104.0).selected_text(tl!(month_name(*month))).show_ui(
                    ui,
                    |ui| {
                        for m in 1..=12u8 {
                            if ui.selectable_label(*month == m, tl!(month_name(m))).clicked() {
                                *month = m;
                            }
                        }
                    },
                );
            };
            match freq {
                Freq::Daily => tall_row(ui, "", |ui| {
                    let weekdays = self.repeat.days.len() == WEEKDAYS.len() && WEEKDAYS.iter().all(|day| self.repeat.days.contains(day));
                    let chosen = !self.repeat.days.is_empty() && !weekdays;
                    ui.horizontal(|ui| {
                        if ui.radio(self.repeat.days.is_empty(), tl!("Every")).clicked() {
                            self.repeat.days.clear();
                        }
                        every(ui, &mut self.repeat.interval);
                    });
                    if ui.radio(weekdays, tl!("Every weekday")).clicked() {
                        self.repeat.days = WEEKDAYS.to_vec();
                        self.repeat.interval = 1;
                    }
                    // A rule from elsewhere that keeps to other days stays what it is.
                    if chosen {
                        ui.horizontal(|ui| {
                            let _ = ui.radio(true, tl!("Only on"));
                            ui.spacing_mut().item_spacing.x = 4.0;
                            self.day_chips(ui, week_start);
                        });
                    }
                }),
                Freq::Weekly => {
                    row(ui, "", |ui| {
                        ui.label(tl!("Every"));
                        every(ui, &mut self.repeat.interval);
                        ui.label(tl!("on"));
                    });
                    row(ui, "", |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        self.day_chips(ui, week_start);
                    });
                }
                Freq::Monthly => tall_row(ui, "", |ui| {
                    ui.horizontal(|ui| {
                        if ui.radio(self.repeat.nth.is_none(), tl!("Day")).clicked() {
                            self.repeat.nth = None;
                        }
                        if ui.add(egui::DragValue::new(&mut self.day).range(1..=31).speed(0.05)).changed() {
                            self.repeat.nth = None;
                        }
                        ui.label(tl!("of every"));
                        every(ui, &mut self.repeat.interval);
                    });
                    ui.horizontal(|ui| {
                        let mut nth = self.repeat.nth.unwrap_or_else(|| Date::parse_iso(&self.start).map_or(1, |day| nth_weekday(day).0));
                        let (before, weekday) = (nth, self.weekday);
                        if ui.radio(self.repeat.nth.is_some(), tl!("The")).clicked() {
                            self.repeat.nth = Some(nth);
                        }
                        pick_nth(ui, "monthly", &mut nth);
                        pick_weekday(ui, "monthly", &mut self.weekday);
                        if nth != before || self.weekday != weekday || self.repeat.nth.is_some() {
                            self.repeat.nth = Some(nth);
                        }
                        ui.label(tl!("of every"));
                        ui.label(format!("{} {}", self.repeat.interval, tl!(unit(freq, self.repeat.interval))));
                    });
                }),
                Freq::Yearly => {
                    row(ui, "", |ui| {
                        ui.label(tl!("Every"));
                        every(ui, &mut self.repeat.interval);
                    });
                    tall_row(ui, "", |ui| {
                        ui.horizontal(|ui| {
                            if ui.radio(self.repeat.nth.is_none(), tl!("On")).clicked() {
                                self.repeat.nth = None;
                            }
                            pick_month(ui, "by_date", &mut self.month);
                            if ui.add(egui::DragValue::new(&mut self.day).range(1..=31).speed(0.05)).changed() {
                                self.repeat.nth = None;
                            }
                        });
                        ui.horizontal(|ui| {
                            let mut nth = self.repeat.nth.unwrap_or_else(|| Date::parse_iso(&self.start).map_or(1, |day| nth_weekday(day).0));
                            let (before, weekday) = (nth, self.weekday);
                            if ui.radio(self.repeat.nth.is_some(), tl!("On the")).clicked() {
                                self.repeat.nth = Some(nth);
                            }
                            pick_nth(ui, "yearly", &mut nth);
                            pick_weekday(ui, "yearly", &mut self.weekday);
                            if nth != before || self.weekday != weekday || self.repeat.nth.is_some() {
                                self.repeat.nth = Some(nth);
                            }
                            ui.label(tl!("of"));
                            ui.label(tl!(month_name(self.month)));
                        });
                    });
                }
            }
        }

        /// The days of the week as toggles, from the first day of the week on.
        fn day_chips(&mut self, ui: &mut Ui, week_start: Weekday) {
            for i in 0..7 {
                let day = Weekday::from_index(i64::from(week_start.index()) + i);
                let on = self.repeat.days.contains(&day);
                if day_chip(ui, tl!(day.short()), on) {
                    self.repeat.days.retain(|d| *d != day);
                    if !on {
                        self.repeat.days.push(day);
                    }
                }
            }
        }
    }

    /// The appointment and meeting form: a new one, or an existing event being edited.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct EventForm {
        /// The name the form was opened by.
        opened: &'static str,
        /// The event being edited; `None` for a new one.
        id: Option<Id>,
        /// The attendee lines are shown: OK makes (or keeps) a meeting.
        meeting: bool,
        /// The event had attendees when it was opened.
        was_meeting: bool,
        /// Somebody else organizes it and an account's server keeps it: who is invited, and how
        /// it repeats, are not the owner's to say.
        guest: bool,
        title: String,
        location: String,
        start_date: String,
        start_time: String,
        end_date: String,
        end_time: String,
        all_day: bool,
        /// The length the event keeps when its start moves, for a timed event.
        minutes: i64,
        /// The same for whole days: the days after the first one.
        days: i64,
        /// The last start date that read as a date.
        last_start: Option<Date>,
        required: String,
        optional: String,
        repeat: Repeat,
        reminder: Option<u32>,
        /// The reminder was chosen here, so ticking All day leaves it alone.
        reminder_set: bool,
        show_as: ShowAs,
        calendar: Id,
        categories: Vec<String>,
        private: bool,
        description: String,
        error: String,
        focus: bool,
        /// The fields as the form opened; for an existing event, OK sends what differs from
        /// them, and a window asks before it closes on fields that differ from them.
        baseline: Map<String, Value>,
        /// The page of a meeting's window that is in view.
        page: Page,
        /// The Appointment Recurrence dialog, while it is open.
        recurrence: Option<RecurrenceDialog>,
        /// A command that waits for the owner's answer to a question.
        waiting: Option<Waiting>,
        /// Find a Time's own state.
        assistant: crate::scheduling::View,
        /// Which of a series it is about, when that was said on opening it.
        scope: Option<Scope>,
        /// For one occurrence: how many days after the series' first day it is. The fields show
        /// the occurrence's own day, and the commands are told the series' (which is how
        /// `calendar.edit` takes a change to one occurrence).
        apart: i64,
        /// How much it matters: the High and Low Importance buttons.
        importance: Importance,
        /// What the people invited are asked for (Response Options).
        options: ResponseOptions,
        /// It is (to be) a Teams meeting.
        online: bool,
        /// The account's server has made it one already, which it then stays.
        online_made: bool,
        /// The link to join it by, when its server has given one.
        join_url: String,
        /// How many of the people invited have proposed another time (a meeting the owner
        /// organizes): the form says so, with a way to look at them.
        proposals: usize,
        /// Time Zones is on: the start and the end each say which zone their time is by, and
        /// the date and time fields are by the clocks of those zones.
        zones: bool,
        /// The zones of the start and of the end, by name; empty for the zone the calendar is
        /// shown in.
        start_zone: String,
        end_zone: String,
        /// The event had zones of its own when the form opened (taking them off is then a
        /// change to send).
        had_zones: bool,
        /// What is typed into a zone's list to find a zone.
        zone_query: String,
        /// The zones picked in this form, for the list of the ones chosen lately.
        picked_zones: Vec<String>,
        /// The zone the calendar is shown in, by name: what an empty zone stands for.
        shown_zone: String,
    }

    impl EventForm {
        /// A new appointment (a meeting when `meeting`), prefilled from `p` where it says
        /// something and otherwise as `calendar.new` would make it.
        pub fn new(s: &Session, p: &Value, meeting: bool) -> EventForm {
            let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            let all_day = p.get("allDay").and_then(Value::as_bool).unwrap_or(false);
            let (start, end, clock) = new_span(s, p.get("start").and_then(Value::as_str), p.get("end").and_then(Value::as_str), all_day);
            let mut f = EventForm {
                opened: if meeting { "meeting" } else { "event" },
                meeting: meeting || !text("attendees").trim().is_empty() || !text("optional").trim().is_empty(),
                title: text("title"),
                location: text("location"),
                required: text("attendees"),
                optional: text("optional"),
                reminder: match p.get("reminder") {
                    Some(Value::Null) => None,
                    Some(v) => v.as_u64().map(|m| u32::try_from(m.min(40_320)).unwrap_or(0)).or(s.prefs.default_reminder),
                    // A whole day reminds the afternoon before, as `calendar.new` has it.
                    None if all_day => s.prefs.default_reminder.map(|_| mailcraft_engine::cmd::calendar::ALL_DAY_REMINDER),
                    None => s.prefs.default_reminder,
                },
                reminder_set: p.get("reminder").is_some(),
                // And it doesn't take the day up.
                show_as: p.get("showAs").and_then(Value::as_str).and_then(ShowAs::parse).unwrap_or(if all_day { ShowAs::Free } else { ShowAs::Busy }),
                // The calendar being worked in, unless another is named.
                calendar: p
                    .get("calendar")
                    .and_then(Value::as_u64)
                    .filter(|c| s.mailbox.calendar(*c).is_some())
                    .unwrap_or_else(|| mailcraft_engine::cmd::calendar::current_calendar(s)),
                categories: p
                    .get("categories")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(100).collect())
                    .unwrap_or_default(),
                private: p.get("private").and_then(Value::as_bool).unwrap_or(false),
                description: text("description"),
                focus: true,
                importance: p.get("importance").and_then(Value::as_str).and_then(Importance::parse).unwrap_or_default(),
                options: ResponseOptions {
                    request_responses: p.get("requestResponses").and_then(Value::as_bool).unwrap_or(true),
                    allow_proposals: p.get("allowProposals").and_then(Value::as_bool).unwrap_or(true),
                },
                online: p.get("online").and_then(Value::as_bool).unwrap_or(false),
                shown_zone: s.zone().id(),
                ..Default::default()
            };
            // Zones it is to have of its own: its times are then by their clocks.
            let zone = |k: &str| p.get(k).and_then(Value::as_str).and_then(Zone::named).map(Zone::id).unwrap_or_default();
            (f.start_zone, f.end_zone) = (zone("startZone"), zone("endZone"));
            if f.end_zone.is_empty() {
                f.end_zone = f.start_zone.clone();
            }
            f.zones = !all_day && !f.start_zone.is_empty();
            f.set_span(start, end, all_day, clock, s.prefs.twelve_hour);
            // What it opened with is nothing to ask about when its window closes.
            f.baseline = f.fields();
            f
        }

        /// The form for an existing event: the one `p.id` names, else the selected one.
        pub fn edit(s: &Session, p: &Value) -> Option<EventForm> {
            let id = p.get("id").and_then(Value::as_u64).filter(|i| *i != 0).or(s.sel.event)?;
            let e = s.mailbox.event(id)?;
            let line =
                |optional: bool| format_list(&e.attendees.iter().filter(|a| a.optional == optional).map(|a| a.address.clone()).collect::<Vec<_>>());
            let mut f = EventForm {
                opened: "editEvent",
                id: Some(id),
                meeting: e.is_meeting(),
                was_meeting: e.is_meeting(),
                guest: e.origin.guest && mailcraft_engine::cmd::remote::keeper(&s.mailbox, &e.origin).is_some(),
                title: e.title.clone(),
                location: e.location.clone(),
                required: line(false),
                optional: line(true),
                repeat: Repeat::of(e.recurrence.as_ref(), e.start.date),
                reminder: e.reminder_minutes,
                reminder_set: true,
                show_as: e.show_as,
                calendar: e.calendar,
                categories: e.categories.clone(),
                private: e.private,
                description: e.description.clone(),
                focus: true,
                importance: e.importance,
                options: e.response_options,
                online: e.online.is_some(),
                // (Made by its server: one that is still on its way there can be taken back.)
                online_made: e.online.is_some() && !e.origin.id.is_empty() && mailcraft_engine::cmd::remote::keeper(&s.mailbox, &e.origin).is_some(),
                join_url: e.join_link().unwrap_or_default().to_string(),
                proposals: e.proposals.len(),
                shown_zone: s.zone().id(),
                ..Default::default()
            };
            let here = |instant: mailcraft_time::Timestamp| s.offset_at(instant);
            // Which of a series is meant, when that is said: one occurrence shows on its own day.
            // (The day that is said is the day the occurrence starts on here; the series names
            // it by a day of its own, which is another on some nights for a series that repeats
            // by another zone's clock.)
            f.scope = match (p.get("scope").and_then(Value::as_str), e.recurrence.is_some()) {
                (Some("series"), true) => Some(Scope::Series),
                (Some("occurrence"), true) => {
                    let said = p.get("occurrence").and_then(Value::as_str).and_then(Date::parse_iso);
                    let day = said.or(s.sel.event_day.filter(|_| s.sel.event == Some(id)));
                    day.filter(|day| e.series_day_on(*day, &here).is_some()).map(Scope::One)
                }
                _ => None,
            };
            f.apart = match f.scope {
                Some(Scope::One(day)) => e.start.date.days_until(e.series_day_on(day, &here).unwrap_or(day)),
                _ => 0,
            };
            let (mut start, mut end) = (e.start.add_days(f.apart), e.end.add_days(f.apart));
            // An event with zones of its own shows its times by their clocks, with the zones
            // beside them, as Outlook does; and a series counts its days there.
            if let (false, Some(zone)) = (e.all_day, e.zone()) {
                let ends_in = e.zone_of_end().unwrap_or(zone);
                let (begins, ends) = e.instants(&here);
                let first = zone.wall(begins);
                (start, end) = (first.add_days(f.apart), ends_in.wall(ends).add_days(f.apart));
                (f.zones, f.had_zones, f.start_zone, f.end_zone) = (true, true, zone.id(), ends_in.id());
                f.repeat = Repeat::of(e.recurrence.as_ref(), first.date);
            }
            let clock = DateTime { hour: s.prefs.work_start.min(23), ..DateTime::at_midnight(start.date) };
            f.set_span(start, end, e.all_day, clock, s.prefs.twelve_hour);
            f.baseline = f.fields();
            // Opened to take up another time somebody proposed: that time is in its fields, for
            // the owner to send as an update (or not). Nothing is changed by opening it.
            if let Some(proposed) = p.get("proposed") {
                let time = |key: &str| proposed.get(key).and_then(Value::as_str).and_then(|text| mailcraft_time::parse_iso(text).map(|(at, _)| at));
                if let (Some(mut start), Some(mut end)) = (time("start"), time("end")) {
                    // (It is said by the clock here. A form that shows its times by the clocks of
                    // the event's own zones shows that moment by those.)
                    if let (false, Some(zone)) = (e.all_day, e.zone()) {
                        let ends_in = e.zone_of_end().unwrap_or(zone);
                        let moment = |wall: DateTime| mailcraft_model::zoned::instant_at(wall, &here);
                        (start, end) = (zone.wall(moment(start)), ends_in.wall(moment(end)));
                    }
                    f.take_proposal(start, end, s.prefs.twelve_hour);
                }
            }
            Some(f)
        }

        /// Put another time into the fields: the one somebody proposed for the meeting. The
        /// form is then one with a change that isn't sent yet.
        pub(crate) fn take_proposal(&mut self, start: DateTime, end: DateTime, twelve: bool) {
            if end <= start || self.guest {
                return;
            }
            let clock = DateTime::at_midnight(start.date);
            self.set_span(start, end, false, clock, twelve);
            self.page = Page::Appointment;
        }

        /// How much it matters, whether it is (to be) a Teams meeting, and the link to join by.
        pub(crate) fn meeting_state(&self) -> (Importance, bool, &str) {
            (self.importance, self.online, self.join_url.as_str())
        }

        /// Fill the date and time fields from a start and an end as an event keeps them. `clock`
        /// is the start a whole-day event gets when it stops being all day.
        fn set_span(&mut self, start: DateTime, end: DateTime, all_day: bool, clock: DateTime, twelve: bool) {
            self.all_day = all_day;
            self.start_date = start.date.iso();
            self.last_start = Some(start.date);
            if all_day {
                let last = end.add_minutes(-1).date.max(start.date);
                self.end_date = last.iso();
                self.days = start.date.days_until(last);
                self.minutes = 30;
                self.start_time = clock.time_short(twelve);
                self.end_time = clock.add_minutes(30).time_short(twelve);
            } else {
                self.end_date = end.date.iso();
                self.start_time = start.time_short(twelve);
                self.end_time = end.time_short(twelve);
                self.minutes = start.minutes_until(end).max(0);
                self.days = 0;
            }
        }

        /// The name the form was opened by.
        pub fn name(&self) -> &'static str {
            self.opened
        }

        /// The dialog's title, in the interface language.
        pub fn title(&self) -> String {
            match (self.id.is_some(), self.meeting) {
                (false, false) => tl!("New Appointment"),
                (false, true) => tl!("New Meeting"),
                (true, false) => tl!("Appointment"),
                (true, true) => tl!("Meeting"),
            }
            .to_string()
        }

        /// Whether it is about a meeting (people are invited, or are about to be).
        pub fn is_meeting(&self) -> bool {
            self.meeting
        }

        /// What the item is called, as its title field has it.
        pub fn subject(&self) -> &str {
            self.title.trim()
        }

        /// The page of its window that is in view.
        pub fn page(&self) -> Page {
            self.page
        }

        /// Show a page of its window. Only a meeting the owner organizes has another than the
        /// first. Whether the page is in view now.
        pub fn set_page(&mut self, page: Page) -> bool {
            self.page = if self.meeting && !self.guest { page } else { Page::Appointment };
            self.page == page
        }

        /// The meeting as Find a Time needs to know it.
        pub(crate) fn meeting(&self) -> crate::scheduling::Meeting {
            let people = |line: &str| Address::parse_list(line).into_iter().take(500).collect();
            // (Everybody's time is drawn by the calendar's clock: so is the meeting's, whatever
            // zones its own fields are by.)
            let span = match (self.zones && !self.all_day, self.instants(), Zone::named(&self.shown_zone)) {
                (true, Some((begins, ends)), Some(here)) => (ends >= begins).then(|| (here.wall(begins), here.wall(ends))),
                _ => self.span(),
            };
            crate::scheduling::Meeting {
                calendar: self.calendar,
                event: self.id,
                required: people(&self.required),
                optional: people(&self.optional),
                day: span.map_or_else(|| self.shown_day(), |(start, _)| start.date),
                span,
                all_day: self.all_day,
            }
        }

        /// What Find a Time drew the last time it was in view.
        pub(crate) fn scheduling(&self) -> &crate::scheduling::Drawn {
            &self.assistant.drawn
        }

        /// From when to when the fields say it is, as an event keeps it (whole days end at the
        /// midnight after the last one); `None` while they don't say, or say an end before the
        /// start.
        fn span(&self) -> Option<(DateTime, DateTime)> {
            let (start, end) = (self.start()?, self.end()?);
            let end = if self.all_day { end.add_days(1) } else { end };
            (end >= start).then_some((start, end))
        }

        /// The meeting was moved or stretched in Find a Time: the fields say so.
        fn set_span_of_day(&mut self, start: DateTime, end: DateTime, twelve: bool) {
            if self.all_day || end < start {
                return;
            }
            let minutes = start.minutes_until(end).max(0);
            // (It was moved by the calendar's clock; the fields of an event with zones of its
            // own are by theirs.)
            let (start, end) = match (self.zones, Zone::named(&self.shown_zone), self.zone_of(&self.start_zone), self.zone_of(&self.end_zone)) {
                (true, Some(here), Some(starts_in), Some(ends_in)) => (here.wall_in(start, starts_in), here.wall_in(end, ends_in)),
                _ => (start, end),
            };
            (self.start_date, self.start_time) = (start.date.iso(), start.time_short(twelve));
            (self.end_date, self.end_time) = (end.date.iso(), end.time_short(twelve));
            self.minutes = minutes;
            if self.apart == 0 {
                self.repeat.follow(self.last_start, start.date);
            }
            self.last_start = Some(start.date);
        }

        /// The Find a Time page: when the meeting is, over when everybody is free.
        fn scheduling_page(&mut self, app: &mut MailApp, ui: &mut Ui, pass: &mut Pass, below: f32) {
            let twelve = pass.twelve;
            self.time_rows(ui, pass, &app.session);
            // In a window of the system the grid has what is left of it; inside the main
            // window it has a height of its own.
            let height =
                if kit::filling() { ui.available_height() - below } else { (ui.ctx().content_rect().height() - 430.0).min(400.0) }.max(190.0);
            let meeting = self.meeting();
            let moved = crate::scheduling::show(app, ui, &mut self.assistant, &meeting, kit::window(), height);
            if let Some((start, end)) = moved {
                self.set_span_of_day(start, end, twelve);
            }
        }

        /// Whether the fields differ from what the form opened with: there is something to save.
        pub fn dirty(&self) -> bool {
            self.fields() != self.baseline
        }

        /// Whether a dialog of the form's own is open over it (Appointment Recurrence): the keys
        /// are that dialog's.
        pub fn modal(&self) -> bool {
            self.recurrence.is_some()
        }

        /// Whether a command of the form waits for the owner's answer to a question.
        pub fn waiting(&self) -> bool {
            self.waiting.is_some()
        }

        /// The start the fields give; midnight for whole days.
        fn start(&self) -> Option<DateTime> {
            let date = Date::parse_iso(&self.start_date)?;
            if self.all_day {
                return Some(DateTime::at_midnight(date));
            }
            let (h, m) = parse_clock(&self.start_time)?;
            DateTime::new(date, h, m, 0)
        }

        /// The end the fields give; for whole days, the last day.
        fn end(&self) -> Option<DateTime> {
            let date = Date::parse_iso(&self.end_date)?;
            if self.all_day {
                return Some(DateTime::at_midnight(date));
            }
            let (h, m) = parse_clock(&self.end_time)?;
            DateTime::new(date, h, m, 0)
        }

        /// The start changed: the end follows, keeping the length, and so does a rule tied to
        /// the start's day.
        fn start_moved(&mut self, twelve: bool) {
            let Some(start) = self.start() else { return };
            if self.all_day {
                self.end_date = start.date.add_days(self.days).iso();
            } else {
                // (By two clocks, the end is so much later, as its own clock shows that.)
                let zones = self.zone_of(&self.start_zone).zip(self.zone_of(&self.end_zone)).filter(|_| self.two_clocks());
                let end = match zones {
                    Some((starts_in, ends_in)) => ends_in.wall(starts_in.instant(start).saturating_add(self.minutes.saturating_mul(60))),
                    None => start.add_minutes(self.minutes),
                };
                self.end_date = end.date.iso();
                self.end_time = end.time_short(twelve);
            }
            // (One occurrence that moves to another day leaves the rule of its series alone.)
            if self.apart == 0 {
                self.repeat.follow(self.last_start, start.date);
            }
            self.last_start = Some(start.date);
        }

        /// The end changed: an end after the start is the length to keep from now on.
        fn end_moved(&mut self) {
            let (Some(start), Some(end)) = (self.start(), self.end()) else { return };
            if self.all_day {
                self.days = start.date.days_until(end.date).max(0);
            } else if self.two_clocks() {
                if let Some((begins, ends)) = self.instants().filter(|(begins, ends)| ends >= begins) {
                    self.minutes = (ends - begins) / 60;
                }
            } else if start.minutes_until(end) >= 0 {
                self.minutes = start.minutes_until(end);
            }
        }

        /// Tick or clear All day. Whole days cover the days the times touched; times come back
        /// with the length they had. A new event's untouched reminder goes with it, as
        /// `calendar.new` does it. `default_reminder` is the preference.
        fn set_all_day(&mut self, on: bool, default_reminder: Option<u32>, twelve: bool) {
            if on == self.all_day {
                return;
            }
            if on {
                let span = self.start().zip(self.end());
                self.all_day = true;
                if let Some((start, end)) = span {
                    let last = if end > start { end.add_minutes(-1).date } else { start.date };
                    self.end_date = last.iso();
                    self.days = start.date.days_until(last).max(0);
                } else if let Some(first) = Date::parse_iso(&self.start_date) {
                    self.end_date = first.iso();
                    self.days = 0;
                }
            } else {
                self.all_day = false;
                if let Some(start) = self.start() {
                    let end = start.add_minutes(self.minutes);
                    self.end_date = end.date.iso();
                    self.end_time = end.time_short(twelve);
                }
            }
            if self.id.is_none() && !self.reminder_set {
                self.reminder = if on { default_reminder.map(|_| mailcraft_engine::cmd::calendar::ALL_DAY_REMINDER) } else { default_reminder };
            }
            // A new whole-day event shows as Free, a new one with times as Busy, until the
            // owner says otherwise.
            if self.id.is_none() {
                match (on, self.show_as) {
                    (true, ShowAs::Busy) => self.show_as = ShowAs::Free,
                    (false, ShowAs::Free) => self.show_as = ShowAs::Busy,
                    _ => {}
                }
            }
        }

        /// How long the event is, in words; `None` when the fields don't say.
        fn length(&self) -> Option<String> {
            let (start, end) = (self.start()?, self.end()?);
            if self.all_day {
                let days = start.date.days_until(end.date);
                (days >= 0).then(|| crate::dates::duration((days + 1) * 24 * 60))
            } else if self.two_clocks() {
                let (begins, ends) = self.instants()?;
                (ends >= begins).then(|| crate::dates::duration((ends - begins) / 60))
            } else {
                let minutes = start.minutes_until(end);
                (minutes >= 0).then(|| crate::dates::duration(minutes))
            }
        }

        /// The day the start field says (the last one that read as a day).
        fn shown_day(&self) -> Date {
            Date::parse_iso(&self.start_date).or(self.last_start).unwrap_or_default()
        }

        /// The day the series starts on, for its rule.
        fn series_start(&self) -> Date {
            let shown = self.shown_day();
            if self.apart == 0 { shown } else { shown.add_days(self.apart.saturating_neg()) }
        }

        /// A date field as the commands are told it: for one occurrence of a series, the day of
        /// the series' first occurrence that goes with it.
        fn told(&self, text: &str) -> String {
            match Date::parse_iso(text) {
                Some(day) if self.apart != 0 => day.add_days(self.apart.saturating_neg()).iso(),
                _ => text.to_string(),
            }
        }

        /// A command's parameters with which of a series is meant, when that was said on opening.
        fn scoped(&self, mut params: Value) -> Value {
            if let Some(fields) = params.as_object_mut() {
                match self.scope {
                    Some(Scope::One(day)) => {
                        fields.insert("scope".into(), json!("occurrence"));
                        fields.insert("occurrence".into(), json!(day.iso()));
                    }
                    Some(Scope::Series) => {
                        fields.insert("scope".into(), json!("series"));
                    }
                    None => {}
                }
            }
            params
        }

        /// Every field as `calendar.new` takes it. Attendee lines are there for a meeting, and
        /// empty ones for an event that stops being one.
        fn fields(&self) -> Map<String, Value> {
            let (from, to) = (self.told(&self.start_date), self.told(&self.end_date));
            let (start, end) = if self.all_day {
                let day = |text: &str| Date::parse_iso(text).map_or_else(|| text.trim().to_string(), Date::iso);
                (day(&from), day(&to))
            } else {
                (stamp(&from, &self.start_time), stamp(&to, &self.end_time))
            };
            let mut m = object(json!({
                "title": self.title,
                "location": self.location,
                "description": self.description,
                "start": start,
                "end": end,
                "allDay": self.all_day,
                "recurrence": self.repeat.rule(self.series_start()).as_ref().map(rule_json),
                "reminder": self.reminder,
                "showAs": self.show_as.code(),
                "categories": self.categories,
                "private": self.private,
                "importance": self.importance.code(),
            }));
            if self.calendar != 0 {
                m.insert("calendar".into(), json!(self.calendar));
            }
            if self.meeting {
                m.insert("attendees".into(), json!(self.required));
                m.insert("optional".into(), json!(self.optional));
                // What a meeting has beyond an appointment: what the people invited are asked
                // for, and whether it is held online.
                m.insert("requestResponses".into(), json!(self.options.request_responses));
                m.insert("allowProposals".into(), json!(self.options.allow_proposals));
                m.insert("online".into(), json!(self.online));
            } else if self.was_meeting {
                m.insert("attendees".into(), json!(""));
                m.insert("optional".into(), json!(""));
            }
            // The zones the times above are by, for an event that has (or had) zones of its
            // own: none for whole days, and none again when Time Zones was turned off.
            let zoned = self.zones && !self.all_day;
            if zoned || self.had_zones {
                let name = |zone: &str| if zoned && !zone.is_empty() { json!(zone) } else { Value::Null };
                m.insert("startZone".into(), name(&self.start_zone));
                m.insert("endZone".into(), if self.end_zone == self.start_zone { Value::Null } else { name(&self.end_zone) });
            }
            m
        }

        /// The zone a name of this form stands for: the one it names, or for none the zone the
        /// calendar is shown in.
        fn zone_of(&self, name: &str) -> Option<Zone> {
            Zone::named(if self.zones && !name.is_empty() { name } else { &self.shown_zone })
        }

        /// When the fields say the event starts and ends, as instants: each time by the clock
        /// of its own zone. `None` when they don't read, or the zones are not known.
        fn instants(&self) -> Option<(mailcraft_time::Timestamp, mailcraft_time::Timestamp)> {
            let (start, end) = (self.start()?, self.end()?);
            Some((self.zone_of(&self.start_zone)?.instant(start), self.zone_of(&self.end_zone)?.instant(end)))
        }

        /// The start and the end are by the clocks of two zones (a flight): how long it is, and
        /// where the end is when the start moves, is then a matter of instants.
        fn two_clocks(&self) -> bool {
            self.zones && !self.all_day && self.zone_of(&self.start_zone) != self.zone_of(&self.end_zone)
        }

        /// Turn Time Zones on or off. On, the start and the end say the zone the calendar is
        /// shown in, to be changed; off, the event has no zones of its own. The times stay as
        /// they are on the clock either way, as in Outlook.
        fn set_zones(&mut self, on: bool) {
            self.zones = on && !self.all_day;
            let zone = if self.zones { self.shown_zone.clone() } else { String::new() };
            (self.start_zone, self.end_zone) = (zone.clone(), zone);
            self.zone_query.clear();
        }

        /// Another zone was picked for the start (or, with `end`, for the end). The time stays
        /// as it is on the clock, which is another instant in another zone, as in Outlook; the
        /// end's zone follows the start's while the two are the same.
        fn pick_zone(&mut self, zone: Zone, end: bool) {
            let together = self.end_zone == self.start_zone;
            if end {
                self.end_zone = zone.id();
            } else {
                self.start_zone = zone.id();
                if together {
                    self.end_zone = zone.id();
                }
            }
            self.picked_zones.push(zone.id());
            self.picked_zones.truncate(16);
            self.end_moved();
        }

        /// The zone the calendar is shown in is another one now (the computer was carried
        /// there while the form stood open). An event without zones of its own had its times
        /// by the old clock: they are the same instants by the new one, so that saving it
        /// moves nothing. One with zones of its own reads as it did.
        pub(crate) fn rezone(&mut self, was: Zone, now: Zone, twelve: bool) {
            let own = self.zones && !self.all_day;
            if !own && !self.all_day {
                let turn = |date: &str, time: &str| {
                    let day = Date::parse_iso(date)?;
                    let (h, m) = parse_clock(time)?;
                    Some(was.wall_in(DateTime::new(day, h, m, 0)?, now))
                };
                let fresh = !self.dirty();
                if let (Some(start), Some(end)) = (turn(&self.start_date, &self.start_time), turn(&self.end_date, &self.end_time)) {
                    (self.start_date, self.start_time) = (start.date.iso(), start.time_short(twelve));
                    (self.end_date, self.end_time) = (end.date.iso(), end.time_short(twelve));
                    self.last_start = Some(start.date);
                }
                // What it opened with reads by the new clock too: it is the same event.
                let stamp_of = |text: &str| {
                    mailcraft_time::parse_iso(text).map(|(at, _)| stamp(&was.wall_in(at, now).date.iso(), &was.wall_in(at, now).time_short(twelve)))
                };
                if fresh {
                    self.shown_zone = now.id();
                    self.baseline = self.fields();
                } else {
                    for key in ["start", "end"] {
                        if let Some(turned) = self.baseline.get(key).and_then(Value::as_str).and_then(stamp_of) {
                            self.baseline.insert(key.into(), json!(turned));
                        }
                    }
                }
            }
            self.shown_zone = now.id();
        }

        /// Whether anybody is named on the attendee lines.
        fn invites(&self) -> bool {
            self.meeting && (!self.required.trim().is_empty() || !self.optional.trim().is_empty())
        }

        /// The command OK runs, with its params: a new appointment or meeting with every field,
        /// or an edit with the fields that changed.
        pub fn command(&self) -> (&'static str, Value) {
            let mut now = self.fields();
            match self.id {
                Some(id) => {
                    // (A time goes with the zone it is by.)
                    let mut m = changes(&self.baseline, &now, &[&["start", "end", "allDay", "startZone", "endZone"], &["attendees", "optional"]]);
                    m.insert("id".into(), json!(id));
                    ("calendar.edit", self.scoped(Value::Object(m)))
                }
                None => {
                    if now.get("recurrence").is_some_and(Value::is_null) {
                        now.remove("recurrence");
                    }
                    (if self.meeting { "calendar.newMeeting" } else { "calendar.new" }, Value::Object(now))
                }
            }
        }

        /// [`command`](Self::command), for saving what there is rather than sending it: a
        /// meeting nobody is named for yet is saved as the appointment it still is.
        fn save_command(&self) -> (&'static str, Value) {
            match self.command() {
                ("calendar.newMeeting", params) if !self.invites() => ("calendar.new", params),
                other => other,
            }
        }

        /// What is missing before the item can be saved, in words; `None` when nothing is.
        fn missing(&self) -> Option<&'static str> {
            let times = self.all_day || (time_ok(&self.start_time, true) && time_ok(&self.end_time, true));
            let until = self.repeat.freq.is_none() || self.repeat.ends != Ends::Until || date_ok(&self.repeat.until, true);
            if self.title.trim().is_empty() {
                Some("Give it a title first")
            } else if !(date_ok(&self.start_date, true) && date_ok(&self.end_date, true) && times) {
                Some("The start and the end need a date and a time")
            } else if !until {
                Some("The series needs a day to end by")
            } else {
                None
            }
        }

        /// Whether OK can run: the required fields are filled in and the dates and times read
        /// as such.
        fn ready(&self) -> bool {
            let invited = self.id.is_some() || !self.meeting || !self.required.trim().is_empty() || !self.optional.trim().is_empty();
            self.missing().is_none() && invited
        }

        /// Open the Appointment Recurrence dialog on the item as its fields have it.
        pub fn open_recurrence(&mut self) {
            // (How somebody else's meeting repeats is the organizer's to say; and one occurrence
            // has no rule of its own.)
            if self.guest || self.apart != 0 || matches!(self.scope, Some(Scope::One(_))) || self.recurrence.is_some() {
                return;
            }
            let dialog = RecurrenceDialog::open(&self.repeat, self.series_start(), &self.start_time, &self.end_time, self.minutes, self.all_day);
            self.recurrence = Some(dialog);
        }

        /// Close the Appointment Recurrence dialog without taking anything from it.
        pub fn close_recurrence(&mut self) {
            self.recurrence = None;
        }

        /// Take what the Appointment Recurrence dialog says: the time of day, the day the
        /// series starts on (the first one on or after the range's start that fits the pattern)
        /// and the rule. A dialog that was left as it opened changes nothing.
        fn apply_recurrence(&mut self, dialog: &RecurrenceDialog, twelve: bool) {
            if !dialog.touched() && dialog.had_rule {
                return;
            }
            let Some(first) = Date::parse_iso(&dialog.start).and_then(|from| dialog.first_day(from)) else { return };
            if !self.all_day {
                if let Some((h, m)) = parse_clock(&dialog.start_time) {
                    self.start_time = time_short(h, m, twelve);
                }
                self.minutes = dialog.minutes.max(0);
            }
            self.start_date = first.iso();
            self.start_moved(twelve);
            self.repeat = dialog.as_repeat();
            self.repeat.nth = self.repeat.nth.map(|n| fit_nth(first, n));
        }

        /// The rule in a line: how it repeats, from when, and at what time of day.
        fn recurrence_line(&self, twelve: bool) -> Option<String> {
            let start = self.series_start();
            let rule = self.repeat.rule(start)?;
            let words = crate::i18n::fmt(tl!("{rule}, from {day}"), &[("rule", &rule.describe(start)), ("day", &crate::dates::medium(start))]);
            match (self.all_day, parse_clock(&self.start_time), parse_clock(&self.end_time)) {
                (false, Some((h, m)), Some((eh, em))) => {
                    Some(format!("{words}, {} – {}", crate::dates::time(h, m, twelve), crate::dates::time(eh, em, twelve)))
                }
                _ => Some(words),
            }
        }

        /// Replace the names on the attendee lines that stand for one contact with that contact.
        /// What stands for nobody, or for several, is answered.
        pub fn check_names(&mut self, s: &Session) -> Vec<String> {
            let mut unresolved = Vec::new();
            for line in [&mut self.required, &mut self.optional] {
                let (checked, mut left) = check_names(s, line);
                if Address::parse_list(line) != Address::parse_list(&checked) {
                    *line = checked;
                }
                unresolved.append(&mut left);
            }
            unresolved
        }

        /// The buttons of the window's own bar, as the item stands.
        fn bar_items(&self) -> Vec<bar::Item> {
            let item = |key: &'static str, icon: &'static str, label: &str, section: u8, rank: u8| bar::Item {
                key,
                icon,
                label: label.to_string(),
                tip: label.to_string(),
                section,
                rank,
                plain: false,
                menu: false,
                checked: false,
                primary: false,
                on: true,
            };
            // A meeting the owner organizes is sent; anything else is saved.
            let organizes = self.meeting && !self.guest;
            let sent = organizes && self.id.is_some() && self.was_meeting;
            let mut items = Vec::new();
            if organizes {
                let label = if sent { tl!("Send Update") } else { tl!("Send") };
                items.push(bar::Item { primary: true, ..item("send", "send", label, 0, 1) });
            } else {
                items.push(bar::Item { primary: true, ..item("save", "save", tl!("Save & Close"), 0, 1) });
            }
            items.push(bar::Item { plain: true, ..item("delete", "delete", if sent { tl!("Cancel Meeting") } else { tl!("Delete") }, 0, 3) });
            // (Sending it on is for an item that is saved: what goes is what the calendar has.)
            if self.id.is_some() {
                items.push(bar::Item { plain: true, ..item("forward", "forward", tl!("Forward"), 0, 7) });
            }
            if organizes {
                items.push(bar::Item { checked: self.page == Page::Appointment, ..item("appointment", "appointment", tl!("Appointment"), 1, 5) });
                items.push(bar::Item { checked: self.page == Page::Scheduling, ..item("scheduling", "scheduling", tl!("Find a Time"), 1, 2) });
            }
            items.push(bar::Item { menu: true, tip: tl!("Show As").to_string(), ..item("showAs", "showAs", tl!(self.show_as.label()), 2, 5) });
            items.push(bar::Item {
                menu: true,
                tip: tl!("Reminder").to_string(),
                ..item("reminder", "reminder", &reminder_label(self.reminder), 2, 5)
            });
            let one = matches!(self.scope, Some(Scope::One(_)));
            if !self.guest && !one {
                items.push(bar::Item { checked: self.repeat.freq.is_some(), ..item("recurrence", "recurrence", tl!("Recurrence"), 2, 3) });
            }
            // (Whole days have no zones.)
            items.push(bar::Item {
                checked: self.zones && !self.all_day,
                on: !self.all_day,
                ..item("timeZones", "timeZones", tl!("Time Zones"), 2, 4)
            });
            items.push(bar::Item { menu: true, plain: true, ..item("categorize", "categorize", tl!("Categorize"), 3, 6) });
            items.push(bar::Item { plain: true, checked: self.private, ..item("private", "private", tl!("Private"), 3, 6) });
            // How much it matters: each of the two is on or off, and at most one of them is on.
            items.push(bar::Item {
                plain: true,
                checked: self.importance == Importance::High,
                ..item("importanceHigh", "importanceHigh", tl!("High Importance"), 3, 6)
            });
            // (The one that is wanted least of all the row: the first to go behind `…`.)
            items.push(bar::Item {
                plain: true,
                checked: self.importance == Importance::Low,
                ..item("importanceLow", "importanceLow", tl!("Low Importance"), 3, 8)
            });
            if organizes {
                // A Teams meeting is made by the calendar's server: where there is none to make
                // it, and once one is made (which it then stays), the button is there and says why
                // it can't be pressed.
                items.push(bar::Item {
                    checked: self.online,
                    on: !self.online_made,
                    tip: if self.online_made {
                        tl!("The account's server has made this a Teams meeting, and it stays one").to_string()
                    } else {
                        tl!("Teams Meeting").to_string()
                    },
                    ..item("teams", "onlineMeeting", tl!("Teams Meeting"), 4, 3)
                });
                // (Set once for most meetings, if at all: it gives way before what was in the row
                // before it.)
                items.push(bar::Item { menu: true, plain: true, ..item("responseOptions", "responseOptions", tl!("Response Options"), 4, 7) });
                items.push(bar::Item { plain: true, ..item("checkNames", "checkNames", tl!("Check Names"), 4, 6) });
                items.push(item("invite", "cancel", tl!("Cancel Invitation"), 4, 4));
            } else if !self.guest {
                items.push(item("invite", "meeting", tl!("Invite Attendees"), 4, 2));
            }
            items
        }

        /// The window's own bar. The key of the button that was clicked.
        fn bar(&mut self, app: &mut MailApp, ui: &mut Ui) -> Option<&'static str> {
            let items = self.bar_items();
            let categories: Vec<(String, egui::Color32)> =
                app.session.mailbox.categories.iter().map(|c| (c.name.clone(), c32(c.color))).take(200).collect();
            let mut menu = |ui: &mut Ui, key: &'static str| match key {
                "showAs" => {
                    for x in ShowAs::ALL {
                        if check_item(ui, tl!(x.label()), x == self.show_as) {
                            self.show_as = x;
                        }
                    }
                }
                "reminder" => {
                    // A reminder set elsewhere to another length stays a choice.
                    let current = self.reminder;
                    for m in REMINDERS.into_iter().chain((!REMINDERS.contains(&current)).then_some(current)) {
                        if check_item(ui, &reminder_label(m), m == current) {
                            self.reminder = m;
                            self.reminder_set = true;
                        }
                    }
                }
                "categorize" => {
                    if categories.is_empty() {
                        ui.label(tl!("No categories yet"));
                    }
                    for (name, color) in &categories {
                        let on = self.categories.iter().any(|c| c.eq_ignore_ascii_case(name));
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                            ui.painter().rect_filled(r.shrink(1.0), 3.0, *color);
                            if on {
                                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "✓", crate::theme::medium(10.0), egui::Color32::WHITE);
                            }
                            if ui.add(egui::Button::new(kit::shown(name, 60)).selected(on).min_size(vec2(170.0, 0.0))).clicked() {
                                self.categories.retain(|c| !c.eq_ignore_ascii_case(name));
                                if !on {
                                    self.categories.push(name.clone());
                                }
                                ui.close();
                            }
                        });
                    }
                    if !self.categories.is_empty() {
                        ui.separator();
                        if ui.add(egui::Button::new(tl!("Clear All Categories")).min_size(vec2(190.0, 0.0))).clicked() {
                            self.categories.clear();
                            ui.close();
                        }
                    }
                }
                "responseOptions" => {
                    if check_item(ui, tl!("Request Responses"), self.options.request_responses) {
                        self.options.request_responses = !self.options.request_responses;
                    }
                    if check_item(ui, tl!("Allow New Time Proposals"), self.options.allow_proposals) {
                        self.options.allow_proposals = !self.options.allow_proposals;
                    }
                }
                _ => {}
            };
            bar::show(ui, &mut app.hit, kit::window(), &items, &mut menu)
        }

        /// The fields of the Appointment page. What was asked for beyond them: `recurrence`
        /// (the dialog), `invite` (attendees, or none after all).
        fn page_fields(&mut self, ui: &mut Ui, pass: &mut Pass, s: &Session) -> Option<&'static str> {
            let windowed = kit::windowed();
            let (t, twelve) = (pass.t, pass.twelve);
            let mut asked = None;
            row(ui, tl!("Title"), |ui| {
                pass.text(ui, "title", &mut self.title, f32::INFINITY, "");
            });
            if self.meeting {
                // Who is invited to somebody else's meeting is the organizer's to say.
                let hint = tl!("Names or addresses, separated by ;");
                ui.add_enabled_ui(!self.guest, |ui| {
                    row(ui, tl!("Required"), |ui| attendee_line(ui, pass, s, "required", &mut self.required, hint));
                    row(ui, tl!("Optional"), |ui| attendee_line(ui, pass, s, "optional", &mut self.optional, hint));
                });
            }
            self.time_rows(ui, pass, s);
            row(ui, tl!("Location"), |ui| {
                pass.text(ui, "location", &mut self.location, f32::INFINITY, "");
            });
            // A Teams meeting: the link to join it by, as its server gave it. It is shown as it
            // is (where it really leads), copied on request, and followed only on a click.
            if self.online {
                row(ui, tl!("Teams meeting"), |ui| match mailcraft_model::OnlineMeeting::clean_link(&self.join_url) {
                    Some(link) => {
                        if ui.button(tl!("Join")).on_hover_text(link.as_str()).clicked() {
                            asked = Some("join");
                        }
                        if ui.button(tl!("Copy Link")).on_hover_text(link.as_str()).clicked() {
                            ui.ctx().copy_text(link.clone());
                        }
                        ui.add(egui::Label::new(egui::RichText::new(kit::shown(&link, 300)).color(t.text_dim)).truncate())
                            .on_hover_text(link.as_str());
                    }
                    None if self.online_made => {
                        ui.label(egui::RichText::new(tl!("The account's server hasn't given it a link to join by yet.")).color(t.text_dim));
                    }
                    None => {
                        let words = tl!("The account's server makes it when the meeting is sent; the link to join by comes back with it.");
                        ui.add(egui::Label::new(egui::RichText::new(words).color(t.text_dim)).truncate());
                    }
                });
            }
            // Other times that people invited proposed (a meeting the owner organizes).
            if self.proposals > 0 && !self.guest {
                row(ui, tl!("Proposals"), |ui| {
                    let words = tln!(self.proposals, "{n} person proposed another time.", "{n} people proposed another time.");
                    ui.label(egui::RichText::new(words).color(t.text));
                    if ui.link(tl!("View All Proposals…")).clicked() {
                        asked = Some("proposals");
                    }
                });
            }
            // How it repeats, in a line; the dialog changes it. (In a window the bar opens the
            // dialog too, and the line is only there for an item that repeats.)
            let line = self.recurrence_line(twelve);
            if matches!(self.scope, Some(Scope::One(_))) {
                // One occurrence: what it is one of, which is not changed from here.
                if let Some(line) = &line {
                    row(ui, tl!("Recurrence"), |ui| {
                        let words = crate::i18n::fmt(tl!("One of a series: {rule}"), &[("rule", &kit::shown(line, 200))]);
                        ui.add(egui::Label::new(egui::RichText::new(words).color(t.text_dim)).truncate());
                    });
                }
            } else if !self.guest && (!windowed || line.is_some()) {
                row(ui, tl!("Recurrence"), |ui| {
                    if let Some(line) = &line {
                        ui.add(egui::Label::new(egui::RichText::new(kit::shown(line, 200)).color(t.text)).truncate());
                    }
                    if ui.link(if line.is_some() { tl!("Change…") } else { tl!("Make it repeat…") }).clicked() {
                        asked = Some("recurrence");
                    }
                });
            }
            if !windowed {
                // In a window these are in its bar.
                row(ui, tl!("Reminder"), |ui| {
                    let current = self.reminder;
                    egui::ComboBox::from_id_salt("reminder").width(150.0).selected_text(reminder_label(current)).show_ui(ui, |ui| {
                        for m in REMINDERS.into_iter().chain((!REMINDERS.contains(&current)).then_some(current)) {
                            if ui.selectable_label(m == current, reminder_label(m)).clicked() {
                                self.reminder = m;
                                self.reminder_set = true;
                            }
                        }
                    });
                });
                row(ui, tl!("Show as"), |ui| {
                    egui::ComboBox::from_id_salt("show_as").width(150.0).selected_text(tl!(self.show_as.label())).show_ui(ui, |ui| {
                        for x in ShowAs::ALL {
                            if ui.selectable_label(x == self.show_as, tl!(x.label())).clicked() {
                                self.show_as = x;
                            }
                        }
                    });
                });
            }
            // A calendar that an account's server keeps says whose it is.
            let calendars: Vec<(Id, String)> = s.mailbox.calendars.iter().map(|c| (c.id, crate::calendar_view::calendar_label(s, c))).collect();
            if calendars.len() > 1 {
                row(ui, tl!("Calendar"), |ui| {
                    let current = calendars.iter().find(|(id, _)| *id == self.calendar).map(|(_, name)| name.clone()).unwrap_or_default();
                    egui::ComboBox::from_id_salt("calendar").width(150.0).selected_text(current).truncate().show_ui(ui, |ui| {
                        for (id, name) in &calendars {
                            if ui.selectable_label(*id == self.calendar, name.as_str()).clicked() {
                                self.calendar = *id;
                            }
                        }
                    });
                });
            }
            if !windowed {
                tall_row(ui, tl!("Categories"), |ui| category_chips(ui, &s.mailbox.categories, &mut self.categories));
                row(ui, "", |ui| {
                    ui.checkbox(&mut self.private, tl!("Private"));
                    ui.add_space(12.0);
                    let mut zones = self.zones && !self.all_day;
                    if ui.add_enabled(!self.all_day, egui::Checkbox::new(&mut zones, tl!("Time zones"))).changed() {
                        self.set_zones(zones);
                    }
                    ui.add_space(12.0);
                    if !self.guest && ui.link(if self.meeting { tl!("Remove attendees") } else { tl!("Invite attendees") }).clicked() {
                        asked = Some("invite");
                    }
                });
            } else if !self.categories.is_empty() {
                // The categories it has, each taken off with a click (the bar adds them).
                tall_row(ui, tl!("Categories"), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                        let mut off = None;
                        for name in self.categories.iter().take(100) {
                            let color =
                                s.mailbox.categories.iter().find(|c| c.name.eq_ignore_ascii_case(name)).map_or(t.text_disabled, |c| c32(c.color));
                            if chip(ui, &kit::shown(name, 60), true, color) {
                                off = Some(name.clone());
                            }
                        }
                        if let Some(off) = off {
                            self.categories.retain(|c| *c != off);
                        }
                    });
                });
            }
            tall_row(ui, tl!("Description"), |ui| {
                // The last of the fields: it has what is left of a window of its own.
                multiline_rest(ui, "description", &mut self.description, if windowed { 8 } else { 4 });
            });
            asked
        }

        /// The list a zone is picked from, beside a time: for the start, or with `end` for the end.
        fn zone_list(&mut self, ui: &mut Ui, s: &Session, end: bool) {
            let name = if end { &self.end_zone } else { &self.start_zone };
            let current = self.zone_of(name).unwrap_or(Zone::LOCAL);
            let key = if end { "end_zone" } else { "start_zone" };
            if let Some(zone) = crate::zones::picker(ui, kit::field_id(key), s, current, &mut self.zone_query, 200.0) {
                self.pick_zone(zone, end);
            }
        }

        /// The Start and End rows, with All day; and with Time Zones on, the zone of each.
        fn time_rows(&mut self, ui: &mut Ui, pass: &mut Pass, s: &Session) {
            let (t, twelve) = (pass.t, pass.twelve);
            let default_reminder = s.prefs.default_reminder;
            let zoned = self.zones && !self.all_day;
            row(ui, tl!("Start"), |ui| {
                let mut changed = pass.date(ui, "start_date", &mut self.start_date, true);
                if !self.all_day {
                    changed |= pass.time(ui, "start_time", &mut self.start_time, true, None);
                }
                if zoned {
                    self.zone_list(ui, s, false);
                }
                if changed {
                    self.start_moved(twelve);
                }
                date_note(ui, &self.start_date, true);
                ui.add_space(8.0);
                let mut all_day = self.all_day;
                if ui.checkbox(&mut all_day, tl!("All day")).changed() {
                    self.set_all_day(all_day, default_reminder, twelve);
                }
            });
            row(ui, tl!("End"), |ui| {
                let mut changed = pass.date(ui, "end_date", &mut self.end_date, true);
                if !self.all_day {
                    // On the start's own day the menu lists what comes after the start.
                    // (By two clocks, any time of the day may come after the start.)
                    let after =
                        self.start().filter(|s| Date::parse_iso(&self.end_date) == Some(s.date) && !self.two_clocks()).map(|s| s.minutes_of_day());
                    changed |= pass.time(ui, "end_time", &mut self.end_time, true, after);
                }
                if zoned {
                    self.zone_list(ui, s, true);
                }
                if changed {
                    self.end_moved();
                }
                date_note(ui, &self.end_date, true);
                if let Some(length) = self.length() {
                    ui.label(egui::RichText::new(format!("· {length}")).color(t.text_dim));
                }
            });
        }

        /// Lay out the form in the dialog. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            self.lay_out(app, ui, None) == Verdict::Close
        }

        /// Lay out the form in a window of its own, under that window's bar. `ask` is what the
        /// window's keys asked for.
        pub(crate) fn show_in_window(&mut self, app: &mut MailApp, ui: &mut Ui, ask: Option<Ask>) -> Verdict {
            self.lay_out(app, ui, ask)
        }

        fn lay_out(&mut self, app: &mut MailApp, ui: &mut Ui, ask: Option<Ask>) -> Verdict {
            let windowed = kit::windowed();
            // The zones that were picked in the form are listed near the top from now on.
            for zone in std::mem::take(&mut self.picked_zones) {
                let _ = app.execute("calendar.timeZones", json!({"used": zone, "limit": 1}));
            }
            // What came of a question that stood before a command of this form.
            match self.settle(app) {
                Verdict::Stay => {}
                settled => return settled,
            }
            // While the owner is being asked about what the form was to do, it waits as it is:
            // what would be changed in it now would not be what the answer is about.
            if self.waiting.is_some() {
                ui.disable();
            }
            let mut pass = Pass::new(ui, &app.session, &mut self.focus);
            let twelve = pass.twelve;
            let mut act = ask.map(|ask| match ask {
                Ask::Save => Act::Save { close: false },
                Ask::SaveAndClose => Act::Save { close: true },
                // (In an appointment's window the keys that send a meeting save and close.)
                Ask::Send if self.meeting && !self.guest => Act::Send { pressed: false },
                Ask::Send => Act::Save { close: true },
            });
            let mut asked = if windowed { self.bar(app, ui) } else { None };
            // In a window of its own the fields go down to the window's foot, less the room for
            // what a command refused.
            let margin = if windowed { MARGIN } else { 0 };
            let mut cancelled = false;
            egui::Frame::NONE.inner_margin(egui::Margin::same(margin)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                let refused = error_room(ui, &self.error);
                if windowed && self.page == Page::Scheduling {
                    self.scheduling_page(app, ui, &mut pass, refused);
                } else {
                    body_sized(ui, refused, !windowed, |ui| {
                        if let Some(more) = self.page_fields(ui, &mut pass, &app.session) {
                            asked = Some(more);
                        }
                    });
                }
                error(ui, &self.error);
                if !windowed {
                    let clicked = footer(ui, tl!("Save"), self.ready(), self.id.is_some());
                    cancelled = clicked.cancel;
                    if clicked.delete {
                        act = Some(Act::Delete);
                    } else if clicked.ok {
                        act = Some(Act::Save { close: true });
                    }
                }
            });
            if cancelled {
                return Verdict::Close;
            }
            match asked {
                Some("save") => act = Some(Act::Save { close: true }),
                Some("send") => act = Some(Act::Send { pressed: true }),
                Some("delete") => act = Some(Act::Delete),
                Some("appointment") => self.page = Page::Appointment,
                Some("scheduling") => self.page = Page::Scheduling,
                Some("recurrence") => self.open_recurrence(),
                Some("timeZones") => self.set_zones(!self.zones),
                Some("private") => self.private = !self.private,
                Some("importanceHigh") => self.importance = if self.importance == Importance::High { Importance::Normal } else { Importance::High },
                Some("importanceLow") => self.importance = if self.importance == Importance::Low { Importance::Normal } else { Importance::Low },
                // A Teams meeting is made by the server of the calendar the meeting is in: where
                // that can't make one, the button says so and nothing is switched on.
                Some("teams") => match mailcraft_engine::cmd::calendar::teams_refusal(&app.session, self.calendar).filter(|_| !self.online) {
                    Some(why) => self.error = tl!(why.as_str()).to_string(),
                    None => {
                        self.online = !self.online;
                        self.error.clear();
                    }
                },
                // Sent on, looked at and joined through the main window's own commands, which
                // ask what they need (who to send it to) and mail nobody by themselves.
                Some("forward") => {
                    if let Some(id) = self.id {
                        let _ = app.run("calendar.forward", self.scoped(json!({"id": id})));
                    }
                }
                Some("proposals") => {
                    if let Some(id) = self.id {
                        let _ = app.run("calendar.proposals", json!({"id": id}));
                    }
                }
                Some("join") => {
                    if let Some(id) = self.id {
                        let _ = app.run("ui.joinMeeting", json!({"event": id}));
                    }
                }
                Some("invite") => {
                    self.meeting = !self.meeting;
                    self.page = Page::Appointment;
                }
                Some("checkNames") => {
                    let unresolved = self.check_names(&app.session);
                    self.error = if unresolved.is_empty() {
                        String::new()
                    } else {
                        crate::i18n::fmt(tl!("Nobody, or more than one of your contacts, matches: {names}"), &[("names", &unresolved.join("; "))])
                    };
                }
                _ => {}
            }
            // The Appointment Recurrence dialog, over the form; while it is open the keys are its.
            let modal = self.recurrence.is_some();
            if let Some(mut dialog) = self.recurrence.take() {
                match dialog.show(ui.ctx(), &app.session) {
                    None => self.recurrence = Some(dialog),
                    Some(Left::Cancel) => {}
                    Some(Left::Ok) => self.apply_recurrence(&dialog, twelve),
                    Some(Left::Remove) => self.repeat = Repeat::default(),
                }
            }
            // Enter in a one-line field, or with nothing focused: what the window's first
            // button does (in the dialog: OK, when it can run).
            if act.is_none() && pass.submit && !modal && (windowed || self.ready()) {
                act = Some(if self.meeting && !self.guest && windowed { Act::Send { pressed: false } } else { Act::Save { close: true } });
            }
            let Some(act) = act.filter(|_| !modal && self.waiting.is_none()) else { return Verdict::Stay };
            let (command, params, after) = match act {
                Act::Delete => match self.id {
                    Some(id) => ("calendar.delete", self.scoped(json!({"id": id})), After::Close),
                    // What was never saved is given up.
                    None => return Verdict::Close,
                },
                Act::Save { close } => {
                    if let Some(missing) = self.missing() {
                        self.error = tl!(missing).to_string();
                        return Verdict::Stay;
                    }
                    let (command, params) = self.save_command();
                    (command, params, if close { After::Close } else { After::Stay })
                }
                Act::Send { pressed } => {
                    if let Some(missing) = self.missing().or((!self.invites() && !self.was_meeting).then_some("Name somebody to invite first")) {
                        self.error = tl!(missing).to_string();
                        return Verdict::Stay;
                    }
                    let (command, mut params) = self.command();
                    // Send on a new meeting sends, as it does in Outlook: the button was
                    // pressed for that. (A key asks first; and so does every update.)
                    if pressed
                        && self.id.is_none()
                        && command == "calendar.newMeeting"
                        && let Some(fields) = params.as_object_mut()
                    {
                        fields.insert("send".into(), json!(true));
                    }
                    (command, params, After::Close)
                }
            };
            self.run(app, command, params, after)
        }

        /// Run a command of the form. A change that reaches further than it looks (which of a
        /// series, somebody else's meeting, mail to the people invited) is asked about first, as
        /// it is when the event is dragged: the question comes up in the main window, a form in
        /// a window of its own waits for the answer, and one in the dialog gives way to it.
        fn run(&mut self, app: &mut MailApp, command: &'static str, params: Value, after: After) -> Verdict {
            // The command says itself which of a series is meant: what the main window noted of
            // that when the item was opened is used up, and is not applied to anything later.
            if self.scope.is_some() && app.calendar.opened.is_some_and(|(event, _)| Some(event) == self.id) {
                app.calendar.opened = None;
            }
            let seq = app.ran.0;
            let asks = crate::dialogs::confirmation(app, command, &params).is_some();
            let ran = if asks { app.run(command, params) } else { app.execute(command, params) };
            match ran {
                Ok(answer) if asks && answer.get("pending").is_some() => {
                    if !kit::windowed() {
                        return Verdict::Close;
                    }
                    self.waiting = Some(Waiting { seq, command, after });
                    self.error.clear();
                    Verdict::Stay
                }
                Ok(answer) => self.done(app, &answer, after),
                Err(e) => {
                    self.error = e;
                    Verdict::Stay
                }
            }
        }

        /// What came of the question a command of the form was waiting on, once it is answered:
        /// the command ran (and went through, or was refused), or the owner thought better of it
        /// and the form stays as it is.
        fn settle(&mut self, app: &mut MailApp) -> Verdict {
            let Some(waiting) = self.waiting else { return Verdict::Stay };
            if app.dialog.is_some() {
                return Verdict::Stay;
            }
            self.waiting = None;
            if app.ran.0 == waiting.seq || app.ran.1 != waiting.command {
                return Verdict::Stay;
            }
            match app.ran.2.clone() {
                Ok(answer) => self.done(app, &answer, waiting.after),
                Err(e) => {
                    self.error = e;
                    Verdict::Stay
                }
            }
        }

        /// A command of the form went through.
        fn done(&mut self, app: &mut MailApp, answer: &Value, after: After) -> Verdict {
            // A meeting or a series can't be on a server's calendar yet: say where it went.
            if answer.get("keptHere").and_then(Value::as_bool) == Some(true) {
                app.status(tl!("Kept on this computer only: invitations and repeating events aren't sent to the server yet"));
            }
            self.error.clear();
            if after == After::Close {
                return Verdict::Close;
            }
            // It stays open, on the item as it is now.
            let Some(id) = answer.get("id").and_then(Value::as_u64).or(self.id) else { return Verdict::Close };
            match EventForm::edit(&app.session, &json!({"id": id})) {
                Some(fresh) => {
                    let (page, assistant) = (self.page, std::mem::take(&mut self.assistant));
                    *self = EventForm { focus: false, assistant, ..fresh };
                    self.set_page(page);
                    Verdict::Now(id)
                }
                None => Verdict::Close,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;

        fn session() -> Session {
            Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
        }

        fn day(y: i32, m: u8, d: u8) -> Date {
            Date::new(y, m, d).unwrap()
        }

        fn event_id(s: &Session, title: &str) -> Id {
            s.mailbox.events.values().find(|e| e.title == title).map(|e| e.id).unwrap()
        }

        /// The form's four date and time fields.
        fn span(f: &EventForm) -> (&str, &str, &str, &str) {
            (&f.start_date, &f.start_time, &f.end_date, &f.end_time)
        }

        #[test]
        fn a_date_knows_which_of_its_weekday_it_is() {
            assert_eq!(nth_weekday(day(2026, 10, 14)), (2, Weekday::Wed));
            assert_eq!(nth_weekday(day(2026, 10, 1)), (1, Weekday::Thu));
            assert_eq!(nth_weekday(day(2026, 10, 21)), (3, Weekday::Wed));
            assert_eq!(nth_weekday(day(2026, 10, 30)), (-1, Weekday::Fri));
            assert_eq!(nth_choices(day(2026, 10, 14)), [2]);
            // A fourth one that is also the last can be counted either way; a fifth is the last.
            assert_eq!(nth_weekday(day(2026, 10, 28)), (-1, Weekday::Wed));
            assert_eq!(nth_choices(day(2026, 10, 28)), [4, -1]);
            assert_eq!(nth_choices(day(2026, 10, 29)), [-1]);
            // February 2026 has four of every day.
            assert_eq!(nth_weekday(day(2026, 2, 21)), (3, Weekday::Sat));
            assert_eq!(nth_choices(day(2026, 2, 22)), [4, -1]);
            // A count that fits the date stays; one that doesn't is read from the date.
            assert_eq!((fit_nth(day(2026, 10, 28), 4), fit_nth(day(2026, 10, 28), 2), fit_nth(day(2026, 10, 14), -1)), (4, -1, 2));
            assert_eq!((ordinal(2), ordinal(-1), ordinal(i8::MIN)), ("second", "last", "last"));
        }

        #[test]
        fn a_new_appointment_starts_where_the_command_would_put_it() {
            let mut s = session();
            let f = EventForm::new(&s, &json!({}), false);
            // The next half hour today.
            assert_eq!(span(&f), ("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"));
            assert_eq!(
                (f.reminder, f.all_day, f.meeting, f.minutes, f.calendar),
                (Some(15), false, false, 30, s.mailbox.default_calendar().unwrap())
            );
            assert_eq!((f.name(), f.title().as_str()), ("event", "New Appointment"));
            // Another day in view: the start of the working day.
            s.view.date = day(2026, 10, 20);
            let f = EventForm::new(&s, &json!({}), true);
            assert_eq!(span(&f), ("2026-10-20", "8:00 AM", "2026-10-20", "8:30 AM"));
            assert_eq!((f.name(), f.title().as_str(), f.meeting), ("meeting", "New Meeting", true));
            // The fields show times the way the clock preference says.
            s.prefs.twelve_hour = false;
            assert_eq!(EventForm::new(&s, &json!({}), false).start_time, "08:00");
            s.prefs.default_reminder = None;
            assert_eq!(EventForm::new(&s, &json!({}), false).reminder, None);
        }

        #[test]
        fn a_new_event_takes_what_the_params_say() {
            let s = session();
            let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Review", "location": "Room 4"}), false);
            assert_eq!((f.title.as_str(), f.location.as_str()), ("Review", "Room 4"));
            assert_eq!(span(&f), ("2026-10-16", "3:00 PM", "2026-10-16", "3:30 PM"));
            let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-17T09:15"}), false);
            assert_eq!((span(&f), f.minutes), (("2026-10-16", "3:00 PM", "2026-10-17", "9:15 AM"), 18 * 60 + 15));
            // A day alone gets the time the command would pick on that day.
            assert_eq!(span(&EventForm::new(&s, &json!({"start": "2026-10-16"}), false)), ("2026-10-16", "8:00 AM", "2026-10-16", "8:30 AM"));
            assert_eq!(span(&EventForm::new(&s, &json!({"start": "2026-10-14"}), false)), ("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"));
            // Whole days: `end` is the last day; the reminder is the afternoon before, and the days show as free.
            let f = EventForm::new(&s, &json!({"start": "2026-10-16", "end": "2026-10-18", "allDay": true}), false);
            assert_eq!(
                (f.all_day, f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder),
                (true, "2026-10-16", "2026-10-18", 2, Some(1080))
            );
            assert_eq!(f.show_as, ShowAs::Free);
            let f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "allDay": true, "reminder": 30}), false);
            assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder), ("2026-10-16", "2026-10-16", 0, Some(30)));
            // An end before the start, and nonsense, fall back to what a new event gets.
            assert_eq!(
                span(&EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-16T14:00"}), false)),
                ("2026-10-16", "3:00 PM", "2026-10-16", "3:30 PM")
            );
            let f = EventForm::new(
                &s,
                &json!({"start": "soon", "end": 7, "allDay": "yes", "title": 3, "reminder": "x", "categories": "Blue category"}),
                false,
            );
            assert_eq!(
                (span(&f), f.title.as_str(), f.reminder, f.categories.len()),
                (("2026-10-14", "4:30 PM", "2026-10-14", "5:00 PM"), "", Some(15), 0)
            );
            // Someone to invite makes it a meeting, whatever it was opened as.
            let f = EventForm::new(&s, &json!({"attendees": "maya@lumen.example", "showAs": "tentative", "private": true, "reminder": null}), false);
            assert_eq!(
                (f.meeting, f.name(), f.title().as_str(), f.show_as, f.private, f.reminder),
                (true, "event", "New Meeting", ShowAs::Tentative, true, None)
            );
        }

        #[test]
        fn a_new_appointment_sends_every_field() {
            let s = session();
            let calendar = s.mailbox.default_calendar().unwrap();
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00"}), false);
            f.title = "Design review".into();
            f.location = "Room 4".into();
            f.categories = vec!["Blue category".into()];
            assert_eq!(
                f.command(),
                (
                    "calendar.new",
                    json!({
                        "title": "Design review", "location": "Room 4", "description": "",
                        "start": "2026-10-16T15:00", "end": "2026-10-16T15:30", "allDay": false,
                        "reminder": 15, "showAs": "busy", "categories": ["Blue category"], "private": false, "calendar": calendar,
                        "importance": "normal",
                    })
                )
            );
            // The same as a meeting: all day, repeating, with everything set.
            f.meeting = true;
            f.required = "maya@lumen.example; Tomás <tomas@lumen.example>".into();
            f.optional = "priya@lumen.example".into();
            f.set_all_day(true, Some(15), true);
            f.repeat = Repeat {
                freq: Some(Freq::Weekly),
                interval: 2,
                days: vec![Weekday::Fri, Weekday::Mon],
                ends: Ends::After,
                count: 6,
                ..Default::default()
            };
            f.show_as = ShowAs::OutOfOffice;
            f.private = true;
            f.description = "Bring\nnotes".into();
            // And what a meeting has beyond an appointment: how much it matters, what the people
            // invited are asked for, and that it is to be a Teams meeting.
            f.importance = Importance::High;
            f.options.allow_proposals = false;
            f.online = true;
            assert_eq!(
                f.command(),
                (
                    "calendar.newMeeting",
                    json!({
                        "title": "Design review", "location": "Room 4", "description": "Bring\nnotes",
                        "start": "2026-10-16", "end": "2026-10-16", "allDay": true,
                        "attendees": "maya@lumen.example; Tomás <tomas@lumen.example>", "optional": "priya@lumen.example",
                        "recurrence": {"freq": "weekly", "interval": 2, "byDay": ["mon", "fri"], "count": 6},
                        // A new whole-day event reminds the afternoon before (18 hours).
                        "reminder": 1080, "showAs": "outOfOffice", "categories": ["Blue category"], "private": true, "calendar": calendar,
                        "importance": "high", "requestResponses": true, "allowProposals": false, "online": true,
                    })
                )
            );
        }

        #[test]
        fn importance_response_options_and_teams_are_in_the_bar_and_in_what_is_saved() {
            let mut s = session();
            let key = |f: &EventForm, key: &str| f.bar_items().into_iter().find(|item| item.key == key);
            // An appointment: High and Low Importance, each on or off; nothing of a meeting's.
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Dentist"}), false);
            assert!(key(&f, "importanceHigh").is_some_and(|item| !item.checked) && key(&f, "importanceLow").is_some());
            assert!(key(&f, "teams").is_none() && key(&f, "responseOptions").is_none() && key(&f, "forward").is_none());
            f.importance = Importance::Low;
            assert!(key(&f, "importanceLow").is_some_and(|item| item.checked) && key(&f, "importanceHigh").is_some_and(|item| !item.checked));
            let (command, params) = f.command();
            assert_eq!(
                (command, &params["importance"], params.get("online"), params.get("allowProposals")),
                ("calendar.new", &json!("low"), None, None)
            );
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            assert_eq!(s.mailbox.event(id).unwrap().importance, Importance::Low);
            // Opened again it says so, has Forward in its bar, and sends only what changes.
            let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
            assert!(key(&f, "importanceLow").is_some_and(|item| item.checked) && key(&f, "forward").is_some() && !f.dirty());
            f.importance = Importance::High;
            assert_eq!(f.command(), ("calendar.edit", json!({"id": id, "importance": "high"})));
            // A meeting the owner organizes: Teams Meeting and Response Options beside them.
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Sync"}), true);
            assert!(key(&f, "teams").is_some_and(|item| item.on && !item.checked) && key(&f, "responseOptions").is_some_and(|item| item.menu));
            f.online = true;
            assert!(key(&f, "teams").is_some_and(|item| item.checked));
            // One that its server has made a Teams meeting stays one: the button says so.
            let event = mailcraft_model::Event {
                title: "Made".into(),
                attendees: vec![mailcraft_model::Attendee { address: Address::new("Maya", "maya@lumen.example"), ..Default::default() }],
                online: Some(mailcraft_model::OnlineMeeting { join_url: "https://teams.example.org/l/meetup-join/1".into(), ..Default::default() }),
                response_options: ResponseOptions { request_responses: false, allow_proposals: true },
                proposals: vec![mailcraft_model::Proposal::default(); 2],
                ..Default::default()
            };
            let made = s.mailbox.add_event(event).unwrap();
            let f = EventForm::edit(&s, &json!({"id": made})).unwrap();
            assert_eq!(f.meeting_state(), (Importance::Normal, true, "https://teams.example.org/l/meetup-join/1"));
            assert_eq!((f.options.request_responses, f.options.allow_proposals, f.proposals, f.online_made), (false, true, 2, false));
            // (Kept here, no server made it: it can still be taken back.)
            assert!(key(&f, "teams").is_some_and(|item| item.on && item.checked));
            let made_there = EventForm { online_made: true, ..EventForm::edit(&s, &json!({"id": made})).unwrap() };
            assert!(key(&made_there, "teams").is_some_and(|item| !item.on && item.checked && item.tip.contains("stays one")));
            // Somebody else's meeting has neither: both are its organizer's to say.
            let theirs = EventForm { guest: true, ..EventForm::edit(&s, &json!({"id": made})).unwrap() };
            assert!(key(&theirs, "teams").is_none() && key(&theirs, "responseOptions").is_none() && key(&theirs, "importanceHigh").is_some());
            // Another time that was proposed goes into the fields, and is then a change to send.
            let mut f =
                EventForm::edit(&s, &json!({"id": made, "proposed": {"start": "2026-10-20T14:00:00", "end": "2026-10-20T15:30:00"}})).unwrap();
            assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.minutes, f.dirty()), ("2026-10-20", "2026-10-20", 90, true));
            let (_, params) = f.command();
            assert_eq!((&params["start"], &params["end"]), (&json!("2026-10-20T14:00"), &json!("2026-10-20T15:30")));
            // A proposal that is none (backwards, unreadable) changes nothing.
            f.take_proposal(DateTime::at_midnight(Date::new(2026, 10, 21).unwrap()), DateTime::at_midnight(Date::new(2026, 10, 20).unwrap()), true);
            assert_eq!(f.start_date, "2026-10-20");
            let f = EventForm::edit(&s, &json!({"id": made, "proposed": {"start": "soon", "end": 7}})).unwrap();
            assert!(!f.dirty());
        }

        #[test]
        fn the_commands_run_and_their_errors_come_back() {
            let mut s = session();
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Sync"}), true);
            f.required = "maya@lumen.example".into();
            f.repeat = Repeat { freq: Some(Freq::Monthly), nth: Some(3), ..Default::default() };
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let e = s.mailbox.event(id).unwrap();
            assert_eq!(
                (e.title.as_str(), e.start.iso().as_str(), e.duration_minutes(), e.attendees.len(), e.reminder_minutes),
                ("Sync", "2026-10-16T15:00:00", 30, 1, Some(15))
            );
            assert_eq!(e.recurrence.as_ref().map(|r| r.describe(e.start.date)).as_deref(), Some("Every month on the third Friday"));
            // Whole days: the end field names the last day.
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-19", "end": "2026-10-21", "allDay": true, "title": "Offsite"}), false);
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let e = s.mailbox.event(id).unwrap();
            assert_eq!(
                (e.all_day, e.start.iso().as_str(), e.end.iso().as_str(), e.reminder_minutes, e.show_as),
                (true, "2026-10-19T00:00:00", "2026-10-22T00:00:00", Some(1080), ShowAs::Free)
            );
            // An end before the start: the command says so and nothing is added.
            let before = s.mailbox.events.len();
            f.end_date = "2026-10-18".into();
            let (command, params) = f.command();
            assert_eq!(s.run(command, &params).unwrap_err().to_string(), "the end is before the start");
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "title": "Backwards"}), false);
            f.end_time = "1pm".into();
            let (command, params) = f.command();
            assert_eq!(s.run(command, &params).unwrap_err().to_string(), "the end is before the start");
            // Someone who isn't an address.
            f.end_time = "4pm".into();
            f.meeting = true;
            f.required = "maya".into();
            let (command, params) = f.command();
            assert_eq!(s.run(command, &params).unwrap_err().to_string(), "\"maya\" isn't an e-mail address");
            assert_eq!(s.mailbox.events.len(), before);
        }

        #[test]
        fn time_zones_keep_the_clock_time_and_go_with_the_times_they_are_for() {
            let zone = |name: &str| Zone::named(name).unwrap();
            // The calendar is shown in Chicago's time.
            let mut s = Session::in_zone(mailcraft_model::Mailbox::with_local_account("", ""), NOW, zone("America/Chicago"));
            let mut f = EventForm::new(&s, &json!({"title": "Call", "start": "2026-10-13T15:00", "end": "2026-10-13T16:00"}), false);
            // Without Time Zones nothing is said of zones, as ever.
            assert!(!f.zones && f.command().1.get("startZone").is_none());
            assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && !item.checked && item.on));
            // Turned on, the start and the end say the calendar's own zone.
            f.set_zones(true);
            assert_eq!((f.start_zone.as_str(), f.end_zone.as_str()), ("America/Chicago", "America/Chicago"));
            assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && item.checked));
            // Another zone for the start: the time on the clock stays (and so the event moves),
            // and the end's zone goes with the start's.
            f.pick_zone(zone("America/New_York"), false);
            assert_eq!(span(&f), ("2026-10-13", "3:00 PM", "2026-10-13", "4:00 PM"));
            let (command, params) = f.command();
            assert_eq!(command, "calendar.new");
            assert_eq!(
                (params["start"].as_str(), params["startZone"].as_str(), params["endZone"].is_null()),
                (Some("2026-10-13T15:00"), Some("America/New_York"), true)
            );
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let made = s.mailbox.event(id).unwrap().clone();
            assert_eq!((made.start.iso(), made.start_zone.as_str()), ("2026-10-13T14:00:00".to_string(), "America/New_York"));
            assert_eq!(s.prefs.recent_zones, Vec::<String>::new());

            // Opened again, it shows its own clock and its zones, and nothing is to be sent.
            let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
            assert!(f.zones && f.had_zones);
            assert_eq!(
                (span(&f), f.start_zone.as_str(), f.end_zone.as_str()),
                (("2026-10-13", "3:00 PM", "2026-10-13", "4:00 PM"), "America/New_York", "America/New_York")
            );
            assert_eq!(f.command(), ("calendar.edit", json!({"id": id})));
            // Opened to take up another time that somebody proposed, which is said by the clock
            // here: the fields show that moment by the event's own clock (four here is five there).
            let proposed = json!({"start": "2026-10-14T16:00:00", "end": "2026-10-14T17:00:00"});
            let taken = EventForm::edit(&s, &json!({"id": id, "proposed": proposed})).unwrap();
            assert_eq!(span(&taken), ("2026-10-14", "5:00 PM", "2026-10-14", "6:00 PM"));
            // Another title is only that.
            f.title = "Call with Bo".into();
            assert_eq!(f.command().1, json!({"id": id, "title": "Call with Bo"}));
            // Another time goes with the zone it is by.
            f.start_time = "5:00 PM".into();
            f.start_moved(true);
            assert_eq!(
                f.command().1,
                json!({"id": id, "title": "Call with Bo", "start": "2026-10-13T17:00", "end": "2026-10-13T18:00", "allDay": false, "startZone": "America/New_York", "endZone": null})
            );
            // The end in a zone of its own: how long it is, is counted in instants; and moving
            // the start keeps that length.
            // (Six in the evening in Berlin is before five in New York: no length to keep yet.)
            f.pick_zone(zone("Europe/Berlin"), true);
            assert_eq!((f.start_zone.as_str(), f.end_zone.as_str(), f.minutes, f.length()), ("America/New_York", "Europe/Berlin", 60, None));
            f.end_date = "2026-10-14".into();
            f.end_time = "7:00 AM".into();
            f.end_moved();
            assert_eq!((f.minutes, f.length().as_deref()), (8 * 60, Some("8 hours")));
            f.start_time = "6:00 PM".into();
            f.start_moved(true);
            assert_eq!(span(&f), ("2026-10-13", "6:00 PM", "2026-10-14", "8:00 AM"));
            let params = f.command().1;
            assert_eq!((params["startZone"].as_str(), params["endZone"].as_str()), (Some("America/New_York"), Some("Europe/Berlin")));
            s.run("calendar.edit", &params).unwrap();
            let flight = s.mailbox.event(id).unwrap().clone();
            assert_eq!((flight.start.iso(), flight.end.iso()), ("2026-10-13T17:00:00".to_string(), "2026-10-14T01:00:00".to_string()));
            // Time Zones turned off: no zones any more, and the times as they stand on the clock.
            let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
            f.set_zones(false);
            let params = f.command().1;
            assert_eq!(
                (params["start"].as_str(), params["startZone"].is_null(), params["endZone"].is_null()),
                (Some("2026-10-13T18:00"), true, true)
            );
            s.run("calendar.edit", &params).unwrap();
            let plain = s.mailbox.event(id).unwrap().clone();
            assert_eq!(
                (plain.start.iso(), plain.end.iso(), plain.start_zone.as_str()),
                ("2026-10-13T18:00:00".to_string(), "2026-10-14T08:00:00".to_string(), "")
            );
            // Whole days have no zones: the button is off, and none are sent.
            let mut f = EventForm::edit(&s, &json!({"id": id})).unwrap();
            f.set_zones(true);
            f.set_all_day(true, None, true);
            assert!(f.bar_items().iter().any(|item| item.key == "timeZones" && !item.checked && !item.on));
            assert!(f.command().1.get("startZone").is_none());
        }

        #[test]
        fn an_open_form_follows_the_calendar_to_another_zone() {
            let zone = |name: &str| Zone::named(name).unwrap();
            let (chicago, london) = (zone("America/Chicago"), zone("Europe/London"));
            let mut s = Session::in_zone(mailcraft_model::Mailbox::with_local_account("", ""), NOW, chicago);
            let id = s.run("calendar.new", &json!({"title": "Lunch", "start": "2026-10-13T12:00", "end": "2026-10-13T13:00"})).unwrap()["id"]
                .as_u64()
                .unwrap();
            let abroad = s.run("calendar.new", &json!({"title": "Call", "start": "2026-10-13T15:00", "startZone": "Asia/Tokyo"})).unwrap()["id"]
                .as_u64()
                .unwrap();
            let (mut lunch, mut call) = (EventForm::edit(&s, &json!({"id": id})).unwrap(), EventForm::edit(&s, &json!({"id": abroad})).unwrap());
            let mut typed = EventForm::edit(&s, &json!({"id": id})).unwrap();
            typed.title = "Long lunch".into();
            typed.end_time = "2:00 PM".into();
            typed.end_moved();
            // The computer is in London now; the mailbox and the forms are told.
            s.clock.zone = Some(london);
            assert!(s.settle_zone());
            for form in [&mut lunch, &mut call, &mut typed] {
                form.rezone(chicago, london, true);
            }
            // Lunch reads by London's clock, as the calendar shows it, and saving it changes nothing.
            assert_eq!(span(&lunch), ("2026-10-13", "6:00 PM", "2026-10-13", "7:00 PM"));
            assert!(!lunch.dirty());
            assert_eq!(lunch.command(), ("calendar.edit", json!({"id": id})));
            // The call is by Tokyo's clock, here as there.
            assert_eq!((span(&call), call.start_zone.as_str()), (("2026-10-13", "3:00 PM", "2026-10-13", "3:30 PM"), "Asia/Tokyo"));
            assert_eq!(call.command(), ("calendar.edit", json!({"id": abroad})));
            // What was typed stays typed, at the instant it was typed for.
            assert_eq!(span(&typed), ("2026-10-13", "6:00 PM", "2026-10-13", "8:00 PM"));
            let params = typed.command().1;
            assert_eq!(
                (params["title"].as_str(), params["start"].as_str(), params["end"].as_str()),
                (Some("Long lunch"), Some("2026-10-13T18:00"), Some("2026-10-13T20:00"))
            );
            s.run("calendar.edit", &params).unwrap();
            let saved = s.mailbox.event(id).unwrap();
            assert_eq!(
                (london.instant(saved.start), london.instant(saved.end)),
                (
                    chicago.instant(DateTime::new(day(2026, 10, 13), 12, 0, 0).unwrap()),
                    chicago.instant(DateTime::new(day(2026, 10, 13), 14, 0, 0).unwrap())
                )
            );
        }

        #[test]
        fn an_existing_event_fills_the_form() {
            let mut s = session();
            // Nothing selected and nothing named: no form.
            assert!(EventForm::edit(&s, &json!({})).is_none());
            assert!(EventForm::edit(&s, &json!({"id": 999_999})).is_none());
            let dentist = event_id(&s, "Dentist");
            s.sel.event = Some(dentist);
            let f = EventForm::edit(&s, &json!({})).unwrap();
            assert_eq!((f.name(), f.title().as_str(), f.id), ("editEvent", "Appointment", Some(dentist)));
            assert_eq!((span(&f), f.minutes), (("2026-10-17", "8:30 AM", "2026-10-17", "9:15 AM"), 45));
            assert_eq!((f.show_as, f.private, f.reminder, f.meeting, f.repeat.freq), (ShowAs::OutOfOffice, true, Some(60), false, None));
            // `id` wins over the selection. A meeting shows who is invited.
            let review = event_id(&s, "Design review: homepage");
            let f = EventForm::edit(&s, &json!({"id": review})).unwrap();
            assert_eq!((f.title().as_str(), f.meeting, f.optional.as_str()), ("Meeting", true, ""));
            assert_eq!(f.required, "Maya Okafor <maya@lumen.example>; Tomás Rivera <tomas@lumen.example>; Priya Nair <priya@lumen.example>");
            assert_eq!((f.categories.as_slice(), f.description.starts_with("Round three")), (["Blue category".to_string()].as_slice(), true));
            // Whole days: the end field is the last day, and times wait for All day to be cleared.
            let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Planning days")})).unwrap();
            assert_eq!((f.all_day, span(&f), f.days), (true, ("2026-10-21", "8:00 AM", "2026-10-22", "8:30 AM"), 1));
            // Series.
            let f = EventForm::edit(&s, &json!({"id": event_id(&s, "1:1 with Priya")})).unwrap();
            assert_eq!(f.repeat, Repeat { freq: Some(Freq::Weekly), days: vec![Weekday::Tue], ..Default::default() });
            let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Budget sync")})).unwrap();
            assert_eq!((f.repeat.freq, f.repeat.nth, f.repeat.ends), (Some(Freq::Monthly), Some(2), Ends::Never));
            let f = EventForm::edit(&s, &json!({"id": event_id(&s, "Dev's birthday")})).unwrap();
            assert_eq!((f.repeat.freq, f.repeat.nth, f.all_day, f.start_date.as_str()), (Some(Freq::Yearly), None, true, "2025-03-14"));
        }

        #[test]
        fn an_edit_sends_only_what_changed() {
            let mut s = session();
            let dentist = event_id(&s, "Dentist");
            let mut f = EventForm::edit(&s, &json!({"id": dentist})).unwrap();
            assert_eq!(f.command(), ("calendar.edit", json!({"id": dentist})));
            f.title = "Dentist (check-up)".into();
            f.private = false;
            assert_eq!(f.command(), ("calendar.edit", json!({"id": dentist, "title": "Dentist (check-up)", "private": false})));
            // A new start goes with its end, which kept its distance.
            f.start_time = "10am".into();
            f.start_moved(true);
            assert_eq!(f.end_time, "10:45 AM");
            assert_eq!(
                f.command().1,
                json!({"id": dentist, "title": "Dentist (check-up)", "private": false, "start": "2026-10-17T10:00", "end": "2026-10-17T10:45", "allDay": false})
            );
            // Repeating from now on, and inviting someone.
            f.repeat.set_freq(Some(Freq::Weekly), Date::parse_iso(&f.start_date));
            assert_eq!(f.command().1["recurrence"], json!({"freq": "weekly", "interval": 1, "byDay": ["sat"]}));
            f.meeting = true;
            f.required = "maya@lumen.example".into();
            assert_eq!((&f.command().1["attendees"], &f.command().1["optional"]), (&json!("maya@lumen.example"), &json!("")));
            let (command, params) = f.command();
            s.run(command, &params).unwrap();
            let e = s.mailbox.event(dentist).unwrap();
            assert_eq!(
                (e.title.as_str(), e.private, e.start.iso().as_str(), e.duration_minutes(), e.is_meeting()),
                ("Dentist (check-up)", false, "2026-10-17T10:00:00", 45, true)
            );
            // What the form doesn't show stays as it was.
            assert_eq!((e.show_as, e.reminder_minutes), (ShowAs::OutOfOffice, Some(60)));
        }

        #[test]
        fn ending_a_series_and_removing_attendees_say_so() {
            let mut s = session();
            let standup = event_id(&s, "Team stand-up");
            let mut f = EventForm::edit(&s, &json!({"id": standup})).unwrap();
            // A rule the form shows only in part isn't sent back unless Repeat changes.
            assert_eq!(f.command().1, json!({"id": standup}));
            f.repeat.interval = 2;
            assert_eq!(f.command().1["recurrence"], json!({"freq": "daily", "interval": 2, "byDay": ["mon", "tue", "wed", "thu", "fri"]}));
            f.repeat.set_freq(None, None);
            assert_eq!(f.command().1, json!({"id": standup, "recurrence": null}));
            let (command, params) = f.command();
            s.run(command, &params).unwrap();
            assert!(s.mailbox.event(standup).unwrap().recurrence.is_none());
            // A rule that doesn't match its start stays untouched too.
            let budget = event_id(&s, "Budget sync");
            assert_eq!(EventForm::edit(&s, &json!({"id": budget})).unwrap().command().1, json!({"id": budget}));
            // A meeting stops being one.
            let review = event_id(&s, "Design review: homepage");
            let mut f = EventForm::edit(&s, &json!({"id": review})).unwrap();
            f.meeting = false;
            assert_eq!((f.title().as_str(), f.command().1), ("Appointment", json!({"id": review, "attendees": "", "optional": ""})));
            let (command, params) = f.command();
            s.run(command, &params).unwrap();
            assert!(!s.mailbox.event(review).unwrap().is_meeting());
        }

        #[test]
        fn moving_the_start_keeps_the_length() {
            let s = session();
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T15:00", "end": "2026-10-16T16:30"}), false);
            assert_eq!(f.length().as_deref(), Some("1.5 hours"));
            f.start_time = "11:15pm".into();
            f.start_moved(true);
            assert_eq!(span(&f), ("2026-10-16", "11:15pm", "2026-10-17", "12:45 AM"));
            // A new end is the length from then on.
            f.end_date = "2026-10-16".into();
            f.end_time = "23:45".into();
            f.end_moved();
            assert_eq!(f.minutes, 30);
            f.start_date = "2026-11-02".into();
            f.start_moved(false);
            assert_eq!((f.end_date.as_str(), f.end_time.as_str(), f.last_start), ("2026-11-02", "23:45", Some(day(2026, 11, 2))));
            // An end before the start, or text that isn't a time, changes nothing.
            f.end_time = "9".into();
            f.end_moved();
            assert_eq!((f.minutes, f.length()), (30, None));
            f.start_time = "soon".into();
            f.start_moved(true);
            assert_eq!((f.end_time.as_str(), f.length()), ("9", None));
        }

        #[test]
        fn all_day_covers_the_days_and_gives_the_times_back() {
            let s = session();
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-16T22:00", "end": "2026-10-18T00:00"}), false);
            f.set_all_day(true, Some(15), true);
            // An end at midnight belongs to the day before. A new event's reminder becomes the
            // afternoon before, and the days show as free.
            assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.days, f.reminder), ("2026-10-16", "2026-10-17", 1, Some(1080)));
            assert_eq!(f.show_as, ShowAs::Free);
            assert_eq!(f.length().as_deref(), Some("2 days"));
            f.start_date = "2026-10-20".into();
            f.start_moved(true);
            assert_eq!(f.end_date, "2026-10-21");
            f.end_date = "2026-10-24".into();
            f.end_moved();
            assert_eq!((f.days, f.length().as_deref()), (4, Some("5 days")));
            // Back to times: the length it had before.
            f.set_all_day(false, Some(15), true);
            assert_eq!((span(&f), f.reminder), (("2026-10-20", "10:00 PM", "2026-10-22", "12:00 AM"), Some(15)));
            // A reminder chosen in the form is left alone.
            f.reminder = Some(60);
            f.reminder_set = true;
            f.set_all_day(true, Some(15), true);
            assert_eq!((f.reminder, f.end_date.as_str()), (Some(60), "2026-10-21"));
        }

        #[test]
        fn repeat_fields_make_rules() {
            let wed = day(2026, 10, 14);
            assert_eq!(Repeat::default().rule(wed), None);
            let json = |r: &Repeat, start: Date| r.rule(start).map(|r| rule_json(&r));
            let weekly = Repeat {
                freq: Some(Freq::Weekly),
                interval: 2,
                days: vec![Weekday::Wed, Weekday::Mon, Weekday::Wed],
                ends: Ends::After,
                count: 5,
                ..Default::default()
            };
            assert_eq!(json(&weekly, wed), Some(json!({"freq": "weekly", "interval": 2, "byDay": ["mon", "wed"], "count": 5})));
            // No day picked: the start's own.
            assert_eq!(
                json(&Repeat { freq: Some(Freq::Weekly), ..Default::default() }, wed),
                Some(json!({"freq": "weekly", "interval": 1, "byDay": ["wed"]}))
            );
            let monthly = Repeat { freq: Some(Freq::Monthly), nth: Some(2), ends: Ends::Until, until: "2027-03-31".into(), ..Default::default() };
            assert_eq!(json(&monthly, wed), Some(json!({"freq": "monthly", "interval": 1, "byDay": ["wed"], "nth": 2, "until": "2027-03-31"})));
            assert_eq!(monthly.rule(wed).unwrap().describe(wed), "Every month on the second Wednesday, until Mar 31, 2027");
            assert_eq!(json(&Repeat { freq: Some(Freq::Monthly), ..Default::default() }, wed), Some(json!({"freq": "monthly", "interval": 1})));
            // The weekday and its count come from the start date.
            let yearly = Repeat { freq: Some(Freq::Yearly), nth: Some(-1), ..Default::default() };
            assert_eq!(json(&yearly, day(2026, 10, 30)), Some(json!({"freq": "yearly", "interval": 1, "byDay": ["fri"], "nth": -1})));
            assert_eq!(json(&yearly, wed), Some(json!({"freq": "yearly", "interval": 1, "byDay": ["wed"], "nth": 2})));
            // Numbers out of range are brought back in.
            let wild =
                Repeat { freq: Some(Freq::Daily), interval: 0, ends: Ends::After, count: u32::MAX, until: "ignored".into(), ..Default::default() };
            assert_eq!(json(&wild, wed), Some(json!({"freq": "daily", "interval": 1, "count": 9999})));
        }

        #[test]
        fn rules_come_back_as_they_were_and_follow_the_start() {
            let tue = day(2026, 10, 13);
            let r = Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), count: Some(12), ..Recurrence::every(Freq::Monthly) };
            assert_eq!(Repeat::of(Some(&r), tue).rule(tue), Some(r.clone()));
            let until =
                Recurrence { interval: 3, until: Some(day(2027, 1, 1)), by_day: vec![Weekday::Mon, Weekday::Thu], ..Recurrence::every(Freq::Weekly) };
            assert_eq!(Repeat::of(Some(&until), tue).rule(tue), Some(until));
            assert_eq!(Repeat::of(None, tue), Repeat::default());
            // A weekly rule without days is on the start's day, and moves with it.
            let mut weekly = Repeat::of(Some(&Recurrence::every(Freq::Weekly)), tue);
            assert_eq!(weekly.days, [Weekday::Tue]);
            weekly.follow(Some(tue), day(2026, 10, 15));
            assert_eq!(weekly.days, [Weekday::Thu]);
            // Days that were picked stay.
            weekly.days = vec![Weekday::Mon, Weekday::Thu];
            weekly.follow(Some(day(2026, 10, 15)), tue);
            assert_eq!(weekly.days, [Weekday::Mon, Weekday::Thu]);
            // The second Tuesday moved to the last one.
            let mut monthly = Repeat::of(Some(&r), tue);
            monthly.follow(Some(tue), day(2026, 10, 27));
            assert_eq!(monthly.nth, Some(-1));
            monthly.set_freq(Some(Freq::Weekly), Some(tue));
            assert_eq!((monthly.days.as_slice(), monthly.nth, monthly.ends, monthly.count), ([Weekday::Tue].as_slice(), None, Ends::After, 12));
        }

        #[test]
        fn ok_waits_for_the_required_fields() {
            let s = session();
            let mut f = EventForm::new(&s, &json!({}), false);
            assert!(!f.ready());
            f.title = " x ".into();
            assert!(f.ready());
            f.start_time = "soon".into();
            assert!(!f.ready());
            f.all_day = true;
            assert!(f.ready());
            f.end_date = "2026-13-01".into();
            assert!(!f.ready());
            f.end_date = "2026-12-01".into();
            // A new meeting needs someone to invite.
            f.meeting = true;
            assert!(!f.ready());
            f.optional = "a@b.example".into();
            assert!(f.ready());
            // A series that ends on a date needs the date.
            f.repeat = Repeat { freq: Some(Freq::Daily), ends: Ends::Until, ..Default::default() };
            assert!(!f.ready());
            f.repeat.until = "2027-01-01".into();
            assert!(f.ready());
        }

        #[test]
        fn hostile_text_goes_through_without_harm() {
            let mut s = session();
            let junk = "\u{0}\u{202e}é\n<b>".repeat(40_000);
            let p = json!({"title": junk, "location": junk, "description": junk, "attendees": junk, "optional": junk, "start": junk, "end": junk,
                "categories": [junk, 7, null], "reminder": 1e300, "calendar": u64::MAX, "showAs": junk, "allDay": junk});
            let mut f = EventForm::new(&s, &p, false);
            assert_eq!((f.start_date.as_str(), f.reminder, f.calendar), ("2026-10-14", Some(15), s.mailbox.default_calendar().unwrap()));
            for text in [&mut f.start_date, &mut f.start_time, &mut f.end_date, &mut f.end_time, &mut f.repeat.until] {
                text.clone_from(&junk);
            }
            f.start_moved(true);
            f.end_moved();
            f.set_all_day(true, None, false);
            f.set_all_day(false, None, false);
            assert!(!f.ready() && f.length().is_none());
            // Dates that aren't dates go as typed, and the command refuses them.
            let (command, params) = f.command();
            assert!(s.run(command, &params).is_err());
            // With real dates the command takes it, cleaned.
            (f.start_date, f.start_time, f.end_date, f.end_time) = ("2026-10-16".into(), "9".into(), "2026-10-16".into(), "10".into());
            let (command, params) = f.command();
            let refused = s.run(command, &params).unwrap_err().to_string();
            assert!(refused.contains("isn't an e-mail address"));
            assert!(super::super::kit::shown(&refused, 400).chars().count() <= 401);
            f.meeting = false;
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let e = s.mailbox.event(id).unwrap();
            assert!(e.title.chars().count() <= 500 && !e.title.contains('\n') && e.duration_minutes() == 60);
        }

        /// The Appointment Recurrence dialog as it opens on a form.
        fn dialog(f: &EventForm) -> RecurrenceDialog {
            RecurrenceDialog::open(&f.repeat, f.series_start(), &f.start_time, &f.end_time, f.minutes, f.all_day)
        }

        /// A form for half an hour on Tuesday 2026-10-13, the second Tuesday of its month.
        fn tuesday(s: &Session) -> EventForm {
            EventForm::new(s, &json!({"start": "2026-10-13T09:30", "end": "2026-10-13T10:00", "title": "Sync"}), false)
        }

        #[test]
        fn the_recurrence_dialog_round_trips_every_kind_of_rule() {
            let s = session();
            let tue = day(2026, 10, 13);
            let rules = vec![
                Recurrence::every(Freq::Daily),
                Recurrence { interval: 3, count: Some(5), ..Recurrence::every(Freq::Daily) },
                Recurrence { by_day: WEEKDAYS.to_vec(), ..Recurrence::every(Freq::Daily) },
                Recurrence { by_day: vec![Weekday::Tue, Weekday::Sat], ..Recurrence::every(Freq::Daily) },
                Recurrence { by_day: vec![Weekday::Tue], ..Recurrence::every(Freq::Weekly) },
                Recurrence {
                    interval: 2,
                    by_day: vec![Weekday::Mon, Weekday::Tue, Weekday::Fri],
                    until: Some(day(2027, 3, 1)),
                    ..Recurrence::every(Freq::Weekly)
                },
                Recurrence::every(Freq::Monthly),
                Recurrence { interval: 6, count: Some(12), ..Recurrence::every(Freq::Monthly) },
                Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), ..Recurrence::every(Freq::Monthly) },
                Recurrence::every(Freq::Yearly),
                Recurrence { interval: 4, until: Some(day(2040, 1, 1)), ..Recurrence::every(Freq::Yearly) },
                Recurrence { by_day: vec![Weekday::Tue], nth: Some(2), count: Some(3), ..Recurrence::every(Freq::Yearly) },
            ];
            for rule in rules {
                let mut f = tuesday(&s);
                f.repeat = Repeat::of(Some(&rule), tue);
                assert_eq!(f.repeat.rule(tue), Some(rule.clone()), "{rule:?}");
                let before = f.clone();
                // Opened and left with OK, nothing touched: the dialog says the same rule for
                // the same day, and the form is as it was.
                let d = dialog(&f);
                assert!(!d.touched() && d.had_rule && d.wrong().is_none(), "{rule:?}");
                assert_eq!((d.first_day(tue), d.as_repeat().rule(tue)), (Some(tue), Some(rule.clone())), "{rule:?}");
                f.apply_recurrence(&d, true);
                assert_eq!(f, before, "{rule:?}");
                // Something else chosen for its end: that changes, and the rest of the rule stays.
                let mut d = dialog(&f);
                (d.repeat.ends, d.repeat.count) = (Ends::After, 7);
                assert!(d.touched());
                f.apply_recurrence(&d, true);
                assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: Some(7), until: None, ..rule.clone() }), "{rule:?}");
                assert_eq!(span(&f), span(&before), "{rule:?}");
                // And every way of ending, and none.
                let mut d = dialog(&f);
                (d.repeat.ends, d.repeat.until) = (Ends::Until, "2031-05-06".into());
                f.apply_recurrence(&d, true);
                assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: None, until: Some(day(2031, 5, 6)), ..rule.clone() }), "{rule:?}");
                let mut d = dialog(&f);
                d.repeat.ends = Ends::Never;
                f.apply_recurrence(&d, true);
                assert_eq!(f.repeat.rule(tue), Some(Recurrence { count: None, until: None, ..rule.clone() }), "{rule:?}");
            }
            // The last one of a month, and a fourth one that is also the last, each stay what
            // they were said to be.
            let last = day(2026, 10, 27);
            for nth in [4, -1] {
                let rule = Recurrence { by_day: vec![Weekday::Tue], nth: Some(nth), ..Recurrence::every(Freq::Monthly) };
                let mut f = tuesday(&s);
                f.start_date = last.iso();
                f.start_moved(true);
                f.repeat = Repeat::of(Some(&rule), last);
                let mut d = dialog(&f);
                assert_eq!((d.first_day(last), d.as_repeat().rule(last)), (Some(last), Some(rule.clone())));
                d.repeat.interval = 2;
                f.apply_recurrence(&d, true);
                assert_eq!(f.repeat.rule(last), Some(Recurrence { interval: 2, ..rule }));
            }
        }

        #[test]
        fn a_pattern_on_another_day_starts_the_series_on_the_first_such_day() {
            let s = session();
            let tue = day(2026, 10, 13);
            let mut f = tuesday(&s);
            assert_eq!(f.recurrence_line(true), None);
            // Nothing repeats yet: the dialog offers every week on the item's own day, and OK
            // makes it so.
            let d = dialog(&f);
            assert_eq!((d.freq(), d.repeat.days.as_slice(), d.had_rule, d.touched()), (Freq::Weekly, [Weekday::Tue].as_slice(), false, false));
            f.apply_recurrence(&d, true);
            assert_eq!(f.repeat.rule(tue).map(|r| r.describe(tue)).as_deref(), Some("Every week on Tuesday"));
            assert_eq!(f.recurrence_line(true).as_deref(), Some("Every week on Tuesday, from Oct 13, 2026, 9:30 AM – 10:00 AM"));
            assert_eq!(f.recurrence_line(false).as_deref(), Some("Every week on Tuesday, from Oct 13, 2026, 09:30 – 10:00"));
            // On Thursdays and Fridays instead: the series starts on Thursday the 15th, at the
            // same time of day.
            let mut d = dialog(&f);
            d.repeat.days = vec![Weekday::Fri, Weekday::Thu, Weekday::Fri];
            assert_eq!(d.first_day(tue), Some(day(2026, 10, 15)));
            f.apply_recurrence(&d, true);
            assert_eq!(span(&f), ("2026-10-15", "9:30 AM", "2026-10-15", "10:00 AM"));
            assert_eq!(f.command().1["recurrence"], json!({"freq": "weekly", "interval": 1, "byDay": ["thu", "fri"]}));
            // Monthly: by its date to begin with; the 31st is in the first month that has one.
            let mut d = dialog(&f);
            d.set_freq(Freq::Monthly);
            assert_eq!((d.day, d.repeat.nth, d.first_day(day(2026, 10, 15))), (15, None, Some(day(2026, 10, 15))));
            d.day = 31;
            assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 10, 31)));
            assert_eq!(d.first_day(day(2026, 11, 1)), Some(day(2026, 12, 31)));
            // The last Friday; and the first Monday, which this month is already past.
            (d.repeat.nth, d.weekday) = (Some(-1), Weekday::Fri);
            assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 10, 30)));
            (d.repeat.nth, d.weekday) = (Some(1), Weekday::Mon);
            assert_eq!(d.first_day(day(2026, 10, 15)), Some(day(2026, 11, 2)));
            f.apply_recurrence(&d, true);
            assert_eq!(span(&f), ("2026-11-02", "9:30 AM", "2026-11-02", "10:00 AM"));
            assert_eq!(f.command().1["recurrence"], json!({"freq": "monthly", "interval": 1, "byDay": ["mon"], "nth": 1}));
            assert_eq!(f.recurrence_line(true).as_deref(), Some("Every month on the first Monday, from Nov 2, 2026, 9:30 AM – 10:00 AM"));
            // Yearly: February 29th waits for a leap year; the third Thursday of November.
            let mut d = dialog(&f);
            d.set_freq(Freq::Yearly);
            (d.month, d.day) = (2, 29);
            assert_eq!(d.first_day(day(2026, 11, 2)), Some(day(2028, 2, 29)));
            (d.repeat.nth, d.weekday, d.month) = (Some(3), Weekday::Thu, 11);
            assert_eq!(d.first_day(day(2026, 11, 2)), Some(day(2026, 11, 19)));
            f.apply_recurrence(&d, true);
            assert_eq!(f.command().1["recurrence"], json!({"freq": "yearly", "interval": 1, "byDay": ["thu"], "nth": 3}));
            assert_eq!(f.start_date, "2026-11-19");
            // Daily on weekdays, from a Saturday: Monday.
            let mut d = dialog(&f);
            d.set_freq(Freq::Daily);
            d.repeat.days = WEEKDAYS.to_vec();
            assert_eq!((d.first_day(day(2026, 11, 21)), d.first_day(day(2026, 11, 19))), (Some(day(2026, 11, 23)), Some(day(2026, 11, 19))));
            // The time of day is the dialog's too: a new start keeps the length, a new end is it.
            d.start_time = "2pm".into();
            d.start_time_moved(true);
            assert_eq!(d.end_time, "2:30 PM");
            d.end_time = "3:30 PM".into();
            d.end_time_moved();
            f.apply_recurrence(&d, true);
            assert_eq!((span(&f), f.minutes), (("2026-11-19", "2:00 PM", "2026-11-19", "3:30 PM"), 90));
            // An end before the start is on the day after.
            let mut d = dialog(&f);
            (d.start_time, d.end_time) = ("11pm".into(), "1am".into());
            d.end_time_moved();
            assert_eq!(d.minutes, 120);
            // What can't be is said, and changes nothing.
            let before = f.clone();
            let mut d = dialog(&f);
            d.set_freq(Freq::Yearly);
            (d.month, d.day) = (2, 30);
            assert_eq!((d.first_day(tue), d.wrong()), (None, Some("No day fits this pattern")));
            f.apply_recurrence(&d, true);
            assert_eq!(f, before);
            for (start, time, until) in [("soon", "9", "2027-01-01"), ("2026-10-13", "late", "2027-01-01"), ("2026-10-13", "9", "never")] {
                let mut d = dialog(&f);
                (d.start, d.start_time, d.repeat.ends, d.repeat.until) = (start.into(), time.into(), Ends::Until, until.into());
                assert!(d.wrong().is_some(), "{start} {time} {until}");
            }
            // A whole day has no time of day to it.
            let mut f = EventForm::new(&s, &json!({"start": "2026-10-13", "allDay": true, "title": "Fair"}), false);
            let mut d = dialog(&f);
            d.start_time = "nonsense".into();
            assert!(d.wrong().is_none());
            d.repeat.days = vec![Weekday::Sat];
            f.apply_recurrence(&d, true);
            assert_eq!((f.start_date.as_str(), f.end_date.as_str(), f.all_day), ("2026-10-17", "2026-10-17", true));
            assert_eq!(f.recurrence_line(true).as_deref(), Some("Every week on Saturday, from Oct 17, 2026"));
            // Numbers and text from nowhere do no harm.
            let junk = "\u{0}\u{202e}é\n".repeat(10_000);
            let mut d = dialog(&f);
            (d.day, d.month, d.repeat.interval, d.repeat.count, d.repeat.nth) = (u32::MAX, u8::MAX, u32::MAX, u32::MAX, Some(i8::MIN));
            (d.start, d.start_time, d.end_time, d.repeat.until) = (junk.clone(), junk.clone(), junk.clone(), junk);
            d.start_time_moved(true);
            d.end_time_moved();
            for freq in [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly] {
                d.repeat.freq = Some(freq);
                let _ = (d.first_day(day(9999, 12, 31)), d.first_day(day(1, 1, 1)), d.wrong(), d.as_repeat());
                f.apply_recurrence(&d, false);
            }
        }

        #[test]
        fn names_on_the_attendee_lines_are_checked_as_the_composer_checks_them() {
            let s = session();
            // A name that is one contact's becomes that contact; an address stays as typed.
            let (line, unresolved) = check_names(&s, "maya; someone@else.example;  Tomás Rivera <tomas@lumen.example>");
            assert_eq!(line, "Maya Okafor <maya@lumen.example>; someone@else.example; Tomás Rivera <tomas@lumen.example>");
            assert!(unresolved.is_empty());
            // Nobody's name, and a name several have, stay and are said.
            let (line, unresolved) = check_names(&s, "zzzz; lumen");
            assert_eq!((line.as_str(), unresolved), ("zzzz; lumen", vec!["zzzz".to_string(), "lumen".to_string()]));
            assert_eq!(check_names(&s, " ; ;"), (String::new(), Vec::new()));
            // On the form: both lines, and only what changes is changed.
            let mut f = EventForm::new(&s, &json!({"attendees": "priya@lumen.example;maya", "optional": "nobody at all"}), true);
            assert_eq!(f.check_names(&s), ["nobody at all"]);
            assert_eq!((f.required.as_str(), f.optional.as_str()), ("priya@lumen.example; Maya Okafor <maya@lumen.example>", "nobody at all"));
            // What a line shows of the people on it: a name each, the contact's when the line
            // has only the address; what is nobody stands out.
            let t = crate::theme::Tokens::light();
            let job = names_job(&s, "priya@lumen.example; Maya Okafor <maya@lumen.example>; who@nowhere.example; bob", egui::FontId::default(), &t);
            assert_eq!(job.text, "Priya Nair; Maya Okafor; who@nowhere.example; bob");
            let colors: Vec<egui::Color32> = job.sections.iter().map(|section| section.format.color).collect();
            assert_eq!(colors.last(), Some(&t.red));
            assert!(colors.iter().filter(|c| **c == t.red).count() == 1);
            // Hostile lines are cut to size.
            let junk = "\u{0}\u{202e}é<b>;".repeat(20_000);
            let (line, unresolved) = check_names(&s, &junk);
            assert!(unresolved.len() <= 500 && unresolved.iter().all(|name| name.chars().count() <= 80) && !line.is_empty());
            assert!(names_job(&s, &junk, egui::FontId::default(), &t).text.chars().count() < 200_000);
        }

        #[test]
        fn one_occurrence_shows_on_its_own_day_and_is_saved_as_that_one() {
            let mut s = session();
            let standup = event_id(&s, "Team stand-up");
            let series = (**s.mailbox.events.get(&standup).unwrap()).clone();
            let friday = day(2026, 10, 16);
            // Opened as the one on Friday: the form shows Friday, and the command says which.
            let f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-16"})).unwrap();
            assert_eq!((f.scope, f.start_date.as_str(), f.end_date.as_str()), (Some(Scope::One(friday)), "2026-10-16", "2026-10-16"));
            assert_eq!(f.start_time, series.start.time_short(true));
            assert!(!f.dirty());
            assert_eq!(f.command(), ("calendar.edit", json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-16"})));
            // It has no rule of its own to change, and its bar has no Recurrence.
            let mut f = f;
            f.open_recurrence();
            assert!(f.recurrence.is_none() && !f.bar_items().iter().any(|item| item.key == "recurrence"));
            assert!(f.recurrence_line(true).is_some());
            // Another time that day, the title left alone: the command is told the change as
            // one to the series' first day, which is how one occurrence is changed, and that
            // occurrence becomes an event of its own on its own day.
            f.start_time = "11am".into();
            f.start_moved(true);
            let (command, params) = f.command();
            assert_eq!(
                (params["start"].as_str(), params["occurrence"].as_str(), params.get("title"), params.get("recurrence")),
                (Some(format!("{}T11:00", series.start.date.iso()).as_str()), Some("2026-10-16"), None, None)
            );
            let made = s.run(command, &params).unwrap();
            let one = (**s.mailbox.events.get(&made["id"].as_u64().unwrap()).unwrap()).clone();
            assert_eq!(
                (one.start.date, one.start.hour, one.duration_minutes(), one.recurrence.is_none()),
                (friday, 11, series.duration_minutes(), true)
            );
            let after = (**s.mailbox.events.get(&standup).unwrap()).clone();
            assert_eq!((after.start, after.recurrence.clone(), after.exceptions.contains(&friday)), (series.start, series.recurrence.clone(), true));
            // Moved to another day: by as many days.
            let mut f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-20"})).unwrap();
            let rule = f.repeat.clone();
            f.start_date = "2026-10-24".into();
            f.start_moved(true);
            assert_eq!(f.repeat, rule, "the series' rule is not the occurrence's to move");
            let (command, params) = f.command();
            let made = s.run(command, &params).unwrap();
            assert_eq!(s.mailbox.events.get(&made["id"].as_u64().unwrap()).unwrap().start.date, day(2026, 10, 24));
            // Deleting it deletes that one.
            let f = EventForm::edit(&s, &json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-21"})).unwrap();
            assert_eq!(f.scoped(json!({"id": standup})), json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-21"}));
            // The series: said so, and it shows its own first day.
            let f = EventForm::edit(&s, &json!({"id": standup, "scope": "series"})).unwrap();
            assert_eq!((f.scope, f.start_date.as_str(), f.apart), (Some(Scope::Series), series.start.date.iso().as_str(), 0));
            assert_eq!(f.command().1, json!({"id": standup, "scope": "series"}));
            assert!(f.bar_items().iter().any(|item| item.key == "recurrence"));
            // Nothing said, a day the series doesn't happen on, an item that doesn't repeat, and
            // nonsense: nothing is assumed, and the command says nothing of it.
            let dentist = event_id(&s, "Dentist");
            for (id, p) in [
                (standup, json!({"id": standup})),
                (standup, json!({"id": standup, "scope": "occurrence", "occurrence": "2026-10-17"})),
                (standup, json!({"id": standup, "scope": "occurrence", "occurrence": "never"})),
                (standup, json!({"id": standup, "scope": 7, "occurrence": [1]})),
                (standup, json!({"id": standup, "scope": "everything"})),
                (dentist, json!({"id": dentist, "scope": "occurrence", "occurrence": "2026-10-17"})),
                (dentist, json!({"id": dentist, "scope": "series"})),
            ] {
                let f = EventForm::edit(&s, &p).unwrap();
                assert_eq!((f.scope, f.apart, f.command().1), (None, 0, json!({"id": id})), "{p}");
            }
        }
    }
}

/// The contact form.
mod contact {
    use std::fmt::Debug;
    use std::hash::Hash;

    use egui::Ui;
    use mailcraft_engine::Session;
    use mailcraft_model::{Id, Labeled, PostalAddress};
    use mailcraft_time::Date;
    use serde_json::{Map, Value, json};

    use super::kit::{Pass, body, category_chips, changes, date_hint, date_ok, date_value, error, footer, multiline, object, row, shown, tall_row};
    use crate::MailApp;
    use crate::widgets::icon_button;

    const EMAIL_LABELS: [&str; 3] = ["work", "home", "other"];
    const PHONE_LABELS: [&str; 5] = ["mobile", "work", "home", "fax", "other"];
    const ADDRESS_LABELS: [&str; 3] = ["work", "home", "other"];
    /// How many e-mail rows, and phone rows, the form offers. A contact that has more keeps them.
    const MAX_ROWS: usize = 3;

    /// A label (`work`, `mobile`) as its menu shows it.
    fn label_text(label: &str) -> String {
        match label.trim().to_ascii_lowercase().as_str() {
            "work" => tl!("Work").to_string(),
            "home" => tl!("Home").to_string(),
            "other" => tl!("Other").to_string(),
            "mobile" => tl!("Mobile").to_string(),
            "fax" => tl!("Fax").to_string(),
            _ => shown(label, 40),
        }
    }

    /// The menu that labels an address or a number.
    fn label_combo(ui: &mut Ui, salt: impl Hash + Debug, label: &mut String, options: &[&str]) {
        egui::ComboBox::from_id_salt(salt).width(84.0).selected_text(label_text(label)).truncate().show_ui(ui, |ui| {
            for option in options {
                if ui.selectable_label(label.eq_ignore_ascii_case(option), label_text(option)).clicked() {
                    *label = (*option).to_string();
                }
            }
        });
    }

    /// The rows of e-mail addresses or phone numbers, with a button that adds one more.
    fn labeled_rows(ui: &mut Ui, pass: &mut Pass, title: &str, key: &'static str, rows: &mut Vec<Labeled>, labels: &[&str], more: &str) {
        let count = rows.len();
        let mut add = false;
        for (i, entry) in rows.iter_mut().enumerate() {
            row(ui, if i == 0 { title } else { "" }, |ui| {
                label_combo(ui, (key, "label", i), &mut entry.label, labels);
                let last = i + 1 == count && count < MAX_ROWS;
                // Every row leaves the room the button takes, so the fields line up.
                let width = ui.available_width() - 30.0;
                pass.text(ui, (key, i), &mut entry.value, width, "");
                if last && icon_button(ui, "plus", more, 12.0, true).clicked() {
                    add = true;
                }
            });
        }
        if add {
            // The next label nobody has yet.
            let label = labels.iter().find(|l| !rows.iter().any(|r| r.label.eq_ignore_ascii_case(l))).or(labels.last()).copied().unwrap_or_default();
            rows.push(Labeled::new(label, ""));
        }
    }

    /// The contact form: a new contact, or an existing one being edited.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct ContactForm {
        /// The contact being edited; `None` for a new one.
        id: Option<Id>,
        first: String,
        last: String,
        display: String,
        company: String,
        title: String,
        department: String,
        emails: Vec<Labeled>,
        phones: Vec<Labeled>,
        /// The first postal address.
        address: PostalAddress,
        /// The first website.
        website: String,
        birthday: String,
        notes: String,
        categories: Vec<String>,
        favorite: bool,
        error: String,
        focus: bool,
        /// The fields as the form opened; OK sends what differs from them.
        baseline: Map<String, Value>,
    }

    impl ContactForm {
        /// A new contact, prefilled from `p` (`first`, `last`, `email`, and the other one-line
        /// fields of `people.new`).
        pub fn new(p: &Value) -> ContactForm {
            let mut f = ContactForm {
                emails: vec![Labeled::new("work", "")],
                phones: vec![Labeled::new("mobile", "")],
                address: PostalAddress { label: "work".into(), ..Default::default() },
                focus: true,
                ..Default::default()
            };
            // What OK sends is what differs from an empty form.
            f.baseline = f.fields();
            let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            (f.first, f.last, f.display, f.company, f.title, f.department) =
                (text("first"), text("last"), text("displayName"), text("company"), text("title"), text("department"));
            if let Some(e) = f.emails.first_mut() {
                e.value = text("email");
            }
            if let Some(ph) = f.phones.first_mut() {
                ph.value = text("phone");
            }
            f
        }

        /// The form for an existing contact: the one `p.id` names, else the selected one.
        pub fn edit(s: &Session, p: &Value) -> Option<ContactForm> {
            let id = p.get("id").and_then(Value::as_u64).filter(|i| *i != 0).or(s.sel.contact)?;
            let c = s.mailbox.contact(id)?;
            let rows = |list: &[Labeled], label: &str| if list.is_empty() { vec![Labeled::new(label, "")] } else { list.to_vec() };
            let mut f = ContactForm {
                id: Some(id),
                first: c.first.clone(),
                last: c.last.clone(),
                display: c.display_name.clone(),
                company: c.company.clone(),
                title: c.title.clone(),
                department: c.department.clone(),
                emails: rows(&c.emails, "work"),
                phones: rows(&c.phones, "mobile"),
                address: c.addresses.first().cloned().unwrap_or_else(|| PostalAddress { label: "work".into(), ..Default::default() }),
                website: c.websites.first().cloned().unwrap_or_default(),
                birthday: c.birthday.map(Date::iso).unwrap_or_default(),
                notes: c.notes.clone(),
                categories: c.categories.clone(),
                favorite: c.favorite,
                focus: true,
                ..Default::default()
            };
            f.baseline = f.fields();
            Some(f)
        }

        /// The name the form was opened by.
        pub fn name(&self) -> &'static str {
            if self.id.is_some() { "editContact" } else { "contact" }
        }

        /// The dialog's title, in the interface language.
        pub fn title(&self) -> String {
            if self.id.is_some() { tl!("Contact") } else { tl!("New Contact") }.to_string()
        }

        /// Every field as `people.new` takes it. Rows left empty are not sent.
        fn fields(&self) -> Map<String, Value> {
            let list = |rows: &[Labeled]| {
                rows.iter().filter(|r| !r.value.trim().is_empty()).map(|r| json!({"label": r.label, "value": r.value})).collect::<Vec<_>>()
            };
            let a = &self.address;
            object(json!({
                "first": self.first,
                "last": self.last,
                "displayName": self.display,
                "company": self.company,
                "title": self.title,
                "department": self.department,
                "emails": list(&self.emails),
                "phones": list(&self.phones),
                "address": {"label": a.label, "street": a.street, "city": a.city, "region": a.region, "postalCode": a.postal_code, "country": a.country},
                "website": self.website,
                "birthday": date_value(&self.birthday),
                "notes": self.notes,
                "categories": self.categories,
                "favorite": self.favorite,
            }))
        }

        /// The command OK runs, with its params: the fields that were filled in for a new
        /// contact, the ones that changed for an existing one.
        pub fn command(&self) -> (&'static str, Value) {
            let mut m = changes(&self.baseline, &self.fields(), &[]);
            match self.id {
                Some(id) => {
                    m.insert("id".into(), json!(id));
                    ("people.edit", Value::Object(m))
                }
                None => ("people.new", Value::Object(m)),
            }
        }

        /// Whether OK can run: a new contact has a name, a company or an address, and the
        /// birthday, if any, reads as a date.
        fn ready(&self) -> bool {
            let named = [&self.first, &self.last, &self.display, &self.company].into_iter().any(|t| !t.trim().is_empty())
                || self.emails.iter().any(|e| !e.value.trim().is_empty());
            (named || self.id.is_some()) && date_ok(&self.birthday, false)
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            let mut pass = Pass::new(ui, &app.session, &mut self.focus);
            let categories = app.session.mailbox.categories.clone();
            body(ui, |ui| {
                row(ui, tl!("First name"), |ui| {
                    pass.text(ui, "first", &mut self.first, f32::INFINITY, "");
                });
                row(ui, tl!("Last name"), |ui| {
                    pass.text(ui, "last", &mut self.last, f32::INFINITY, "");
                });
                // Left empty, the name lists show is the first and last name.
                let full = [self.first.trim(), self.last.trim()].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" ");
                let hint = if full.is_empty() { tl!("Shown as First Last when empty").to_string() } else { shown(&full, 80) };
                row(ui, tl!("Display name"), |ui| {
                    pass.text(ui, "display", &mut self.display, f32::INFINITY, &hint);
                });
                row(ui, tl!("Company"), |ui| {
                    pass.text(ui, "company", &mut self.company, f32::INFINITY, "");
                });
                row(ui, tl!("Job title"), |ui| {
                    pass.text(ui, "job", &mut self.title, f32::INFINITY, "");
                });
                row(ui, tl!("Department"), |ui| {
                    pass.text(ui, "department", &mut self.department, f32::INFINITY, "");
                });
                labeled_rows(ui, &mut pass, tl!("E-mail"), "email", &mut self.emails, &EMAIL_LABELS, tl!("Add another e-mail address"));
                labeled_rows(ui, &mut pass, tl!("Phone"), "phone", &mut self.phones, &PHONE_LABELS, tl!("Add another phone number"));
                row(ui, tl!("Address"), |ui| label_combo(ui, "address_label", &mut self.address.label, &ADDRESS_LABELS));
                tall_row(ui, tl!("Street"), |ui| {
                    multiline(ui, "street", &mut self.address.street, 2);
                });
                row(ui, tl!("City"), |ui| {
                    pass.text(ui, "city", &mut self.address.city, f32::INFINITY, "");
                });
                row(ui, tl!("State or region"), |ui| {
                    pass.text(ui, "region", &mut self.address.region, f32::INFINITY, "");
                });
                row(ui, tl!("Postal code"), |ui| {
                    pass.text(ui, "postal", &mut self.address.postal_code, 140.0, "");
                });
                row(ui, tl!("Country"), |ui| {
                    pass.text(ui, "country", &mut self.address.country, f32::INFINITY, "");
                });
                row(ui, tl!("Website"), |ui| {
                    pass.text(ui, "website", &mut self.website, f32::INFINITY, "");
                });
                row(ui, tl!("Birthday"), |ui| {
                    pass.date_text(ui, "birthday", &mut self.birthday, false);
                    date_hint(ui, &self.birthday, false);
                });
                tall_row(ui, tl!("Categories"), |ui| category_chips(ui, &categories, &mut self.categories));
                row(ui, "", |ui| {
                    ui.checkbox(&mut self.favorite, tl!("Favorite"));
                });
                tall_row(ui, tl!("Notes"), |ui| {
                    multiline(ui, "notes", &mut self.notes, 3);
                });
            });
            error(ui, &self.error);
            let ready = self.ready();
            let clicked = footer(ui, tl!("Save"), ready, self.id.is_some());
            if clicked.cancel {
                return true;
            }
            let run = if let (true, Some(id)) = (clicked.delete, self.id) {
                // Asked about first, in the main window (#186): the form gives way to the question.
                return match app.run("people.delete", json!({"id": id})) {
                    Ok(_) => true,
                    Err(e) => {
                        self.error = e;
                        false
                    }
                };
            } else if clicked.ok || (pass.submit && ready) {
                Some(self.command())
            } else {
                None
            };
            match run.map(|(command, params)| app.execute(command, params)) {
                Some(Ok(_)) => true,
                Some(Err(e)) => {
                    self.error = e;
                    false
                }
                None => false,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;

        fn session() -> Session {
            Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
        }

        fn contact_id(s: &Session, name: &str) -> Id {
            s.mailbox.contacts.values().find(|c| c.name() == name).map(|c| c.id).unwrap()
        }

        #[test]
        fn a_new_contact_sends_what_was_filled_in() {
            let mut f = ContactForm::new(&json!({"first": "Ada", "last": "Lovelace", "email": "ada@example.org"}));
            assert_eq!((f.name(), f.title().as_str()), ("contact", "New Contact"));
            assert_eq!(
                f.command(),
                ("people.new", json!({"first": "Ada", "last": "Lovelace", "emails": [{"label": "work", "value": "ada@example.org"}]}))
            );
            f.display = "Countess of Lovelace".into();
            f.company = "Analytical Engines".into();
            f.title = "Programmer".into();
            f.department = "Research".into();
            f.emails.push(Labeled::new("home", "ada@home.example"));
            // A row left empty isn't sent.
            f.emails.push(Labeled::new("other", "  "));
            f.phones = vec![Labeled::new("mobile", "+44 20 7946 0000")];
            f.address = PostalAddress {
                label: "home".into(),
                street: "12 St James's Square\nSecond floor".into(),
                city: "London".into(),
                region: String::new(),
                postal_code: "SW1Y 4JH".into(),
                country: "UK".into(),
            };
            f.website = "https://example.org/ada".into();
            f.birthday = "1815-12-10".into();
            f.notes = "Met at the lecture.".into();
            f.categories = vec!["Blue category".into()];
            f.favorite = true;
            let (command, params) = f.command();
            assert_eq!(
                params,
                json!({
                    "first": "Ada", "last": "Lovelace", "displayName": "Countess of Lovelace",
                    "company": "Analytical Engines", "title": "Programmer", "department": "Research",
                    "emails": [{"label": "work", "value": "ada@example.org"}, {"label": "home", "value": "ada@home.example"}],
                    "phones": [{"label": "mobile", "value": "+44 20 7946 0000"}],
                    "address": {"label": "home", "street": "12 St James's Square\nSecond floor", "city": "London", "region": "", "postalCode": "SW1Y 4JH", "country": "UK"},
                    "website": "https://example.org/ada", "birthday": "1815-12-10", "notes": "Met at the lecture.",
                    "categories": ["Blue category"], "favorite": true,
                })
            );
            // And the command takes it.
            let mut s = session();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let c = s.mailbox.contact(id).unwrap();
            assert_eq!((c.name().as_str(), c.emails.len(), c.birthday, c.favorite), ("Countess of Lovelace", 2, Date::new(1815, 12, 10), true));
            assert_eq!(c.addresses.first().map(|a| a.one_line()).as_deref(), Some("12 St James's Square\nSecond floor, London SW1Y 4JH, UK"));
        }

        #[test]
        fn an_existing_contact_fills_the_form_and_sends_its_changes() {
            let mut s = session();
            // Nobody selected and nobody named: no form.
            assert!(ContactForm::edit(&s, &json!({})).is_none());
            assert!(ContactForm::edit(&s, &json!({"id": 999_999})).is_none());
            let dev = contact_id(&s, "Dev Patel");
            s.sel.contact = Some(dev);
            let mut f = ContactForm::edit(&s, &json!({})).unwrap();
            assert_eq!((f.name(), f.title().as_str(), f.id), ("editContact", "Contact", Some(dev)));
            assert_eq!(
                (f.first.as_str(), f.last.as_str(), f.company.as_str(), f.title.as_str(), f.birthday.as_str()),
                ("Dev", "Patel", "Patel & Rowe", "Architect", "1988-03-14")
            );
            assert_eq!(f.emails, [Labeled::new("work", "dev.patel@example.org"), Labeled::new("home", "dev@patel.example")]);
            assert_eq!(f.phones, [Labeled::new("mobile", "+1 555 0142")]);
            // Untouched, nothing is sent but who it is.
            assert_eq!(f.command(), ("people.edit", json!({"id": dev})));
            f.birthday.clear();
            f.phones[0].value = "+1 555 0199".into();
            f.phones.push(Labeled::new("work", "+1 555 0100"));
            f.favorite = true;
            let (command, params) = f.command();
            assert_eq!(
                params,
                json!({"id": dev, "birthday": null, "favorite": true,
                    "phones": [{"label": "mobile", "value": "+1 555 0199"}, {"label": "work", "value": "+1 555 0100"}]})
            );
            s.run(command, &params).unwrap();
            let c = s.mailbox.contact(dev).unwrap();
            assert_eq!((c.birthday, c.phones.len(), c.favorite, c.emails.len(), c.company.as_str()), (None, 2, true, 2, "Patel & Rowe"));
            // An address and a website; `id` wins over the selection.
            let lin = contact_id(&s, "Lin Zhao");
            let mut f = ContactForm::edit(&s, &json!({"id": lin})).unwrap();
            assert_eq!(
                (f.address.street.as_str(), f.address.city.as_str(), f.address.postal_code.as_str(), f.website.as_str()),
                ("40 Dock Road", "Port Haven", "20417", "https://harborworks.example")
            );
            assert_eq!(f.categories, ["Orange category"]);
            f.address.country = "Freedonia".into();
            assert_eq!(
                f.command().1,
                json!({"id": lin, "address": {"label": "work", "street": "40 Dock Road", "city": "Port Haven", "region": "", "postalCode": "20417", "country": "Freedonia"}})
            );
            // A contact without a phone still gets a row to type one in.
            let books = contact_id(&s, "Harbor Books");
            let f = ContactForm::edit(&s, &json!({"id": books})).unwrap();
            assert_eq!((f.phones.as_slice(), f.address.label.as_str()), ([Labeled::new("mobile", "")].as_slice(), "work"));
            assert_eq!(f.command().1, json!({"id": books}));
        }

        #[test]
        fn ok_waits_for_someone_to_save() {
            let mut f = ContactForm::new(&json!({}));
            assert!(!f.ready());
            assert_eq!(f.command(), ("people.new", json!({})));
            f.notes = "Only a note".into();
            assert!(!f.ready());
            f.company = "Harbor Books".into();
            assert!(f.ready());
            f.company.clear();
            f.emails[0].value = "x@example.org".into();
            assert!(f.ready());
            // A birthday that isn't a date holds OK back.
            f.birthday = "14 March".into();
            assert!(!f.ready());
            f.birthday = "1988-03-14".into();
            assert!(f.ready());
            assert_eq!((label_text("WORK").as_str(), label_text("pager").as_str(), label_text("").as_str()), ("Work", "pager", ""));
        }

        #[test]
        fn hostile_text_goes_through_without_harm() {
            let junk = "\u{0}\u{202e}é\n".repeat(40_000);
            let p = json!({"first": junk, "last": 7, "email": junk, "phone": junk, "displayName": junk, "company": [junk]});
            let mut f = ContactForm::new(&p);
            assert!(f.ready() && f.last.is_empty() && f.company.is_empty());
            (f.address.street, f.address.label, f.website, f.notes) = (junk.clone(), junk.clone(), junk.clone(), junk.clone());
            f.categories = vec![junk.clone(); 50];
            f.birthday.clone_from(&junk);
            assert!(!f.ready());
            // A birthday that isn't a date goes as typed, and the command refuses it.
            let mut s = session();
            let (command, params) = f.command();
            assert!(s.run(command, &params).unwrap_err().to_string().contains("`birthday` must be a date"));
            f.birthday.clear();
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            // The command cut the one-line fields down.
            assert!(s.mailbox.contact(id).unwrap().first.chars().count() <= 300);
            assert!(label_text(&junk).chars().count() <= 41);
        }
    }
}

/// The task form.
mod task {
    use egui::Ui;
    use mailcraft_engine::Session;
    use mailcraft_model::{Id, Importance};
    use mailcraft_time::{Date, time_short};
    use serde_json::{Map, Value, json};

    use super::kit::{
        Pass, body, category_chips, changes, date_note, date_ok, date_value, error, footer, multiline, object, row, segmented, stamp, tall_row,
        time_ok,
    };
    use crate::MailApp;

    /// The Monday after `today`'s week: where "Next week" puts a date.
    pub fn next_week(today: Date) -> Date {
        today.start_of_week(mailcraft_time::Weekday::Mon).add_days(7)
    }

    /// The quick choices after a date field: today, tomorrow, next week, no date.
    fn quick_dates(ui: &mut Ui, text: &mut String, today: Date) {
        let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 10.0);
        for (label, date) in
            [(tl!("Today"), Some(today)), (tl!("Tomorrow"), Some(today.add_days(1))), (tl!("Next week"), Some(next_week(today))), (tl!("None"), None)]
        {
            if ui.link(egui::RichText::new(label).size(11.5)).clicked() {
                *text = date.map(Date::iso).unwrap_or_default();
            }
        }
        ui.spacing_mut().item_spacing.x = gap;
    }

    /// The task form: a new task, or an existing one being edited.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct TaskForm {
        /// The task being edited; `None` for a new one.
        id: Option<Id>,
        title: String,
        due: String,
        start: String,
        importance: Importance,
        remind_date: String,
        remind_time: String,
        categories: Vec<String>,
        notes: String,
        completed: bool,
        /// The task was complete when the form opened.
        was_completed: bool,
        /// The hour a reminder gets when only its day is given (the start of the working day).
        day_start: u8,
        error: String,
        focus: bool,
        /// The fields as the form opened; OK sends what differs from them.
        baseline: Map<String, Value>,
    }

    impl TaskForm {
        /// A new task, prefilled from `p` (`title`, `due`, `start`, `notes`, `importance`).
        pub fn new(s: &Session, p: &Value) -> TaskForm {
            let mut f = TaskForm { day_start: s.prefs.work_start.min(23), focus: true, ..Default::default() };
            // What OK sends is what differs from an empty form.
            f.baseline = f.fields();
            let text = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
            (f.title, f.due, f.start, f.notes) = (text("title"), text("due"), text("start"), text("notes"));
            f.importance = p.get("importance").and_then(Value::as_str).and_then(Importance::parse).unwrap_or_default();
            f
        }

        /// The form for an existing task: the one `p.id` names, else the selected one.
        pub fn edit(s: &Session, p: &Value) -> Option<TaskForm> {
            let id = p.get("id").and_then(Value::as_u64).filter(|i| *i != 0).or(s.sel.task)?;
            let t = s.mailbox.task(id)?;
            let reminder = t.reminder.map(|r| s.local(r));
            let mut f = TaskForm {
                id: Some(id),
                title: t.title.clone(),
                due: t.due.map(Date::iso).unwrap_or_default(),
                start: t.start.map(Date::iso).unwrap_or_default(),
                importance: t.importance,
                remind_date: reminder.map(|r| r.date.iso()).unwrap_or_default(),
                remind_time: reminder.map(|r| r.time_short(s.prefs.twelve_hour)).unwrap_or_default(),
                categories: t.categories.clone(),
                notes: t.notes.clone(),
                completed: t.is_complete(),
                was_completed: t.is_complete(),
                day_start: s.prefs.work_start.min(23),
                focus: true,
                ..Default::default()
            };
            f.baseline = f.fields();
            Some(f)
        }

        /// The name the form was opened by.
        pub fn name(&self) -> &'static str {
            if self.id.is_some() { "editTask" } else { "task" }
        }

        /// The dialog's title, in the interface language.
        pub fn title(&self) -> String {
            if self.id.is_some() { tl!("Task") } else { tl!("New Task") }.to_string()
        }

        /// What `tasks.new` gets as the reminder: `2026-10-16T09:00`, or `null` for none. A day
        /// without a time is reminded at the start of the working day.
        fn reminder(&self) -> Value {
            let (date, time) = (self.remind_date.trim(), self.remind_time.trim());
            match (date.is_empty(), time.is_empty(), Date::parse_iso(date)) {
                (true, true, _) => Value::Null,
                (false, true, Some(day)) => json!(format!("{}T{:02}:00", day.iso(), self.day_start)),
                _ => json!(stamp(date, time)),
            }
        }

        /// Every field as `tasks.new` takes it.
        fn fields(&self) -> Map<String, Value> {
            object(json!({
                "title": self.title,
                "notes": self.notes,
                "due": date_value(&self.due),
                "start": date_value(&self.start),
                "importance": self.importance.code(),
                "categories": self.categories,
                "reminder": self.reminder(),
            }))
        }

        /// The command OK runs, with its params: the fields that were filled in for a new task,
        /// the ones that changed for an existing one.
        pub fn command(&self) -> (&'static str, Value) {
            let mut m = changes(&self.baseline, &self.fields(), &[]);
            match self.id {
                Some(id) => {
                    m.insert("id".into(), json!(id));
                    ("tasks.edit", Value::Object(m))
                }
                None => ("tasks.new", Value::Object(m)),
            }
        }

        /// The command that follows a successful edit when Completed was ticked or cleared.
        pub fn completion(&self) -> Option<(&'static str, Value)> {
            let id = self.id.filter(|_| self.completed != self.was_completed)?;
            Some(("tasks.complete", json!({"id": id, "value": self.completed})))
        }

        /// Whether OK can run: there is a title, and the dates and the time read as such. A
        /// reminder needs its day.
        fn ready(&self) -> bool {
            let timed = !self.remind_time.trim().is_empty();
            !self.title.trim().is_empty()
                && date_ok(&self.due, false)
                && date_ok(&self.start, false)
                && date_ok(&self.remind_date, timed)
                && time_ok(&self.remind_time, false)
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            let mut pass = Pass::new(ui, &app.session, &mut self.focus);
            let today = pass.today;
            let categories = app.session.mailbox.categories.clone();
            body(ui, |ui| {
                row(ui, tl!("Title"), |ui| {
                    pass.text(ui, "title", &mut self.title, f32::INFINITY, "");
                });
                row(ui, tl!("Due date"), |ui| {
                    pass.date(ui, "due", &mut self.due, false);
                    date_note(ui, &self.due, false);
                    quick_dates(ui, &mut self.due, today);
                });
                row(ui, tl!("Start date"), |ui| {
                    pass.date(ui, "start", &mut self.start, false);
                    date_note(ui, &self.start, false);
                    quick_dates(ui, &mut self.start, today);
                });
                row(ui, tl!("Importance"), |ui| {
                    segmented(
                        ui,
                        &mut self.importance,
                        &[(Importance::Low, tl!("Low")), (Importance::Normal, tl!("Normal")), (Importance::High, tl!("High"))],
                    );
                });
                row(ui, tl!("Reminder"), |ui| {
                    let timed = !self.remind_time.trim().is_empty();
                    // A day picked for the reminder brings a time with it.
                    if pass.date(ui, "remind_date", &mut self.remind_date, timed) && !timed && Date::parse_iso(&self.remind_date).is_some() {
                        self.remind_time = time_short(self.day_start, 0, pass.twelve);
                    }
                    pass.time(ui, "remind_time", &mut self.remind_time, false, None);
                    date_note(ui, &self.remind_date, timed);
                    if ui.link(egui::RichText::new(tl!("None")).size(11.5)).clicked() {
                        self.remind_date.clear();
                        self.remind_time.clear();
                    }
                });
                tall_row(ui, tl!("Categories"), |ui| category_chips(ui, &categories, &mut self.categories));
                if self.id.is_some() {
                    row(ui, "", |ui| {
                        ui.checkbox(&mut self.completed, tl!("Completed"));
                    });
                }
                tall_row(ui, tl!("Notes"), |ui| {
                    multiline(ui, "notes", &mut self.notes, 4);
                });
            });
            error(ui, &self.error);
            let ready = self.ready();
            let clicked = footer(ui, tl!("Save"), ready, self.id.is_some());
            if clicked.cancel {
                return true;
            }
            if let (true, Some(id)) = (clicked.delete, self.id) {
                return match app.execute("tasks.delete", json!({"id": id})) {
                    Ok(_) => true,
                    Err(e) => {
                        self.error = e;
                        false
                    }
                };
            }
            if !(clicked.ok || (pass.submit && ready)) {
                return false;
            }
            let (command, params) = self.command();
            let done = app.execute(command, params).and_then(|_| match self.completion() {
                Some((command, params)) => app.execute(command, params),
                None => Ok(Value::Null),
            });
            match done {
                Ok(_) => true,
                Err(e) => {
                    self.error = e;
                    false
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;

        fn session() -> Session {
            Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
        }

        fn task_id(s: &Session, title: &str) -> Id {
            s.mailbox.tasks.values().find(|t| t.title == title).map(|t| t.id).unwrap()
        }

        #[test]
        fn a_new_task_sends_what_was_filled_in() {
            let mut s = session();
            let mut f = TaskForm::new(&s, &json!({"title": "Book flights", "due": "2026-10-20"}));
            assert_eq!((f.name(), f.title().as_str(), f.ready()), ("task", "New Task", true));
            assert_eq!(f.command(), ("tasks.new", json!({"title": "Book flights", "due": "2026-10-20"})));
            assert_eq!(f.completion(), None);
            f.start = "20261015".into();
            f.importance = Importance::High;
            f.notes = "Aisle seat".into();
            f.categories = vec!["Red category".into()];
            // A reminder with a day and no time is for the start of the working day.
            f.remind_date = "2026-10-19".into();
            assert_eq!(
                f.command().1,
                json!({"title": "Book flights", "due": "2026-10-20", "start": "2026-10-15", "importance": "high", "notes": "Aisle seat",
                    "categories": ["Red category"], "reminder": "2026-10-19T08:00"})
            );
            f.remind_time = "5:30pm".into();
            assert_eq!(f.command().1["reminder"], "2026-10-19T17:30");
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let t = s.mailbox.task(id).unwrap();
            assert_eq!(
                (t.title.as_str(), t.due, t.start, t.importance),
                ("Book flights", Date::new(2026, 10, 20), Date::new(2026, 10, 15), Importance::High)
            );
            assert_eq!(t.reminder.map(|r| s.local(r).iso()).as_deref(), Some("2026-10-19T17:30:00"));
            // Prefilled the other ways.
            let f = TaskForm::new(&s, &json!({"title": 5, "importance": "LOW", "notes": "n", "start": "2026-10-15"}));
            assert_eq!((f.title.as_str(), f.importance, f.ready()), ("", Importance::Low, false));
            assert_eq!(f.command().1, json!({"importance": "low", "notes": "n", "start": "2026-10-15"}));
        }

        #[test]
        fn an_existing_task_fills_the_form_and_sends_its_changes() {
            let mut s = session();
            // Nothing selected and nothing named: no form.
            assert!(TaskForm::edit(&s, &json!({})).is_none());
            assert!(TaskForm::edit(&s, &json!({"id": 999_999})).is_none());
            let id = task_id(&s, "Review the Q4 roadmap draft");
            s.run("tasks.edit", &json!({"id": id, "reminder": "2026-10-15T09:30"})).unwrap();
            s.sel.task = Some(id);
            let mut f = TaskForm::edit(&s, &json!({})).unwrap();
            assert_eq!((f.name(), f.title().as_str(), f.id), ("editTask", "Task", Some(id)));
            assert_eq!(
                (f.title.as_str(), f.due.as_str(), f.start.as_str(), f.importance),
                ("Review the Q4 roadmap draft", "2026-10-14", "", Importance::High)
            );
            assert_eq!(
                (f.remind_date.as_str(), f.remind_time.as_str(), f.completed, f.notes.as_str()),
                ("2026-10-15", "9:30 AM", false, "Bring the Atlas estimate as a range.")
            );
            // Untouched, nothing is sent but which task it is, and it isn't completed.
            assert_eq!((f.command(), f.completion()), (("tasks.edit", json!({"id": id})), None));
            f.due.clear();
            f.remind_date.clear();
            f.remind_time.clear();
            f.importance = Importance::Normal;
            f.completed = true;
            assert_eq!(f.command().1, json!({"id": id, "due": null, "reminder": null, "importance": "normal"}));
            assert_eq!(f.completion(), Some(("tasks.complete", json!({"id": id, "value": true}))));
            let (command, params) = f.command();
            s.run(command, &params).unwrap();
            let (command, params) = f.completion().unwrap();
            s.run(command, &params).unwrap();
            let t = s.mailbox.task(id).unwrap();
            assert_eq!((t.due, t.reminder, t.importance, t.is_complete()), (None, None, Importance::Normal, true));
            // A finished task opens ticked, and clearing the box reopens it.
            let done = task_id(&s, "Renew the studio domain");
            let mut f = TaskForm::edit(&s, &json!({"id": done})).unwrap();
            assert!(f.completed && f.completion().is_none());
            f.completed = false;
            assert_eq!(f.completion(), Some(("tasks.complete", json!({"id": done, "value": false}))));
        }

        #[test]
        fn dates_and_reminders_are_checked_before_ok() {
            let s = session();
            let mut f = TaskForm::new(&s, &json!({"title": "x"}));
            assert!(f.ready());
            f.due = "next week".into();
            assert!(!f.ready());
            f.due = "2026-10-19".into();
            // A reminder time needs its day.
            f.remind_time = "9".into();
            assert!(!f.ready());
            f.remind_date = "2026-10-18".into();
            assert!(f.ready());
            assert_eq!(f.reminder(), json!("2026-10-18T09:00"));
            f.remind_time = "soon".into();
            assert!(!f.ready());
            assert_eq!(f.reminder(), json!("2026-10-18Tsoon"));
            f.title = "  ".into();
            f.remind_time.clear();
            assert!(!f.ready());
            // Next week starts on its Monday.
            let day = |d: u8| Date::new(2026, 10, d).unwrap();
            assert_eq!((next_week(day(14)), next_week(day(18)), next_week(day(19))), (day(19), day(19), day(26)));
        }

        #[test]
        fn hostile_text_goes_through_without_harm() {
            let mut s = session();
            let junk = "\u{0}\u{202e}é\n".repeat(40_000);
            let mut f = TaskForm::new(&s, &json!({"title": junk, "due": junk, "start": junk, "notes": junk, "importance": junk}));
            assert!(!f.ready());
            f.remind_date.clone_from(&junk);
            f.remind_time.clone_from(&junk);
            f.categories = vec![junk.clone(); 20];
            // Dates that aren't dates go as typed, and the command refuses them.
            let (command, params) = f.command();
            assert!(s.run(command, &params).unwrap_err().to_string().contains("must be a date"));
            (f.due, f.start, f.remind_date, f.remind_time) = (String::new(), String::new(), String::new(), String::new());
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            assert!(s.mailbox.task(id).unwrap().title.chars().count() <= 500);
        }
    }
}

/// Add Account and Account Settings: the wizard in `account_setup`.
mod account {
    pub use crate::account_setup::AccountSetup as AccountForm;
}

/// The Categories dialog: recolour, rename, delete, add.
mod categories {
    use egui::{Sense, Stroke, StrokeKind, Ui, vec2};
    use mailcraft_engine::Session;
    use mailcraft_model::{Category, Rgb};
    use serde_json::{Value, json};

    use super::kit::{ROW_H, body, close_button, error, field_id};
    use crate::MailApp;
    use crate::theme::{Tokens, c32};
    use crate::widgets::icon_button;

    /// The colours a category can take: our own twelve, mid-tones that read in both themes.
    pub const PALETTE: [Rgb; 12] = [
        Rgb(0xD1, 0x3B, 0x3B),
        Rgb(0xE0, 0x7B, 0x1F),
        Rgb(0xD9, 0xA4, 0x0C),
        Rgb(0x2E, 0x8B, 0x57),
        Rgb(0x1A, 0x9B, 0xA6),
        Rgb(0x2F, 0x6F, 0xD0),
        Rgb(0x7E, 0x57, 0xC2),
        Rgb(0xC2, 0x4E, 0x9A),
        Rgb(0x8A, 0x5A, 0x44),
        Rgb(0x6B, 0x7A, 0x1F),
        Rgb(0x4F, 0x6D, 0x8F),
        Rgb(0x7A, 0x7A, 0x7A),
    ];

    /// The first palette colour no category has yet; the first one when all are taken.
    pub fn unused_color(list: &[Category]) -> Rgb {
        PALETTE.iter().find(|p| !list.iter().any(|c| c.color == **p)).or(PALETTE.first()).copied().unwrap_or_default()
    }

    /// A colour swatch that opens the palette. Returns the colour picked there, if one was.
    fn swatch(ui: &mut Ui, color: Rgb) -> Option<Rgb> {
        let t = Tokens::get(ui.ctx());
        let (r, resp) = ui.allocate_exact_size(vec2(ROW_H, ROW_H), Sense::click());
        ui.painter().rect(
            r.shrink(2.0),
            5.0,
            c32(color),
            Stroke::new(1.0, if resp.hovered() { t.text_dim } else { t.border_strong }),
            StrokeKind::Inside,
        );
        let resp = resp.on_hover_text(tl!("Change the colour"));
        let mut picked = None;
        egui::Popup::menu(&resp).show(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for line in PALETTE.chunks(6) {
                ui.horizontal(|ui| {
                    for choice in line {
                        let (cell, cell_resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
                        let current = *choice == color;
                        ui.painter().rect(
                            cell,
                            4.0,
                            c32(*choice),
                            Stroke::new(if current { 2.0 } else { 1.0 }, if current || cell_resp.hovered() { t.text } else { t.border_strong }),
                            StrokeKind::Inside,
                        );
                        if cell_resp.clicked() {
                            picked = Some(*choice);
                        }
                    }
                });
            }
        });
        picked
    }

    /// The Categories dialog.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CategoriesForm {
        /// The name being retyped: the category it belongs to, and the text so far.
        editing: Option<(String, String)>,
        new_name: String,
        new_color: Rgb,
        error: String,
        focus: bool,
    }

    impl CategoriesForm {
        /// The dialog, with a colour no category has yet ready for a new one.
        pub fn new(s: &Session) -> CategoriesForm {
            CategoriesForm { new_color: unused_color(&s.mailbox.categories), focus: true, ..Default::default() }
        }

        /// The command that adds the category typed at the bottom.
        pub fn add_command(&self) -> (&'static str, Value) {
            ("category.set", json!({"name": self.new_name, "color": self.new_color.hex()}))
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            let t = Tokens::get(ui.ctx());
            let enter = crate::keys::enter(ui.ctx(), ui.layer_id());
            let list = app.session.mailbox.categories.clone();
            // Once Outlook's list of categories was read, those not on it say so (R3 spec S18).
            let outlook = &app.session.mailbox.outlook_categories;
            let not_outlooks: Vec<bool> =
                list.iter().map(|c| !outlook.is_empty() && !outlook.iter().any(|o| mailcraft_model::sync::same_category(o, &c.name))).collect();
            let mut run: Option<(&'static str, Value)> = None;
            let mut adding = false;
            body(ui, |ui| {
                for (i, c) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if let Some(color) = swatch(ui, c.color) {
                            run = Some(("category.set", json!({"name": c.name, "color": color.hex()})));
                        }
                        let mut text = match &self.editing {
                            Some((name, text)) if *name == c.name => text.clone(),
                            _ => c.name.clone(),
                        };
                        let width = (ui.available_width() - 44.0).max(40.0);
                        let resp = ui.add(egui::TextEdit::singleline(&mut text).id(field_id(("category", i))).desired_width(width));
                        if resp.changed() {
                            self.editing = Some((c.name.clone(), text));
                        }
                        // Enter, or leaving the field, renames.
                        if resp.lost_focus()
                            && let Some((from, to)) = self.editing.take_if(|e| e.0 == c.name)
                            && to.trim() != from
                        {
                            run = Some(("category.rename", json!({"from": from, "to": to})));
                        }
                        if icon_button(ui, "delete", tl!("Delete this category"), 14.0, true).clicked() {
                            run = Some(("category.remove", json!({"name": c.name})));
                        }
                    });
                    if not_outlooks.get(i).copied().unwrap_or(false) {
                        ui.horizontal(|ui| {
                            ui.add_space(ROW_H + ui.spacing().item_spacing.x);
                            ui.label(egui::RichText::new(tl!("Not in Outlook's list: no colour there")).small().color(t.text_dim));
                        });
                    }
                }
                if list.is_empty() {
                    ui.label(egui::RichText::new(tl!("No categories yet")).color(t.text_dim));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if let Some(color) = swatch(ui, self.new_color) {
                        self.new_color = color;
                    }
                    let width = (ui.available_width() - 80.0).max(40.0);
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.new_name)
                            .id(field_id("new_category"))
                            .desired_width(width)
                            .hint_text(tl!("New category")),
                    );
                    if std::mem::take(&mut self.focus) {
                        resp.request_focus();
                    }
                    let named = !self.new_name.trim().is_empty();
                    let add = ui.add_enabled(named, egui::Button::new(tl!("Add")).min_size(vec2(56.0, 22.0))).clicked();
                    if named && (add || (enter && resp.lost_focus())) {
                        run = Some(self.add_command());
                        adding = true;
                    }
                });
            });
            if let Some((command, params)) = run {
                match app.execute(command, params) {
                    Ok(_) => {
                        self.error.clear();
                        if adding {
                            // Ready for the next one.
                            self.new_name.clear();
                            self.new_color = unused_color(&app.session.mailbox.categories);
                            self.focus = true;
                        }
                    }
                    Err(e) => self.error = e,
                }
            }
            error(ui, &self.error);
            close_button(ui)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;

        #[test]
        fn the_palette_has_twelve_colours_and_offers_a_free_one() {
            let distinct: std::collections::BTreeSet<String> = PALETTE.iter().map(|c| c.hex()).collect();
            assert_eq!(distinct.len(), 12);
            // The six categories a mailbox starts with take red to green, blue and purple.
            let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
            assert_eq!(unused_color(&s.mailbox.categories), Rgb(0x1A, 0x9B, 0xA6));
            assert_eq!(unused_color(&[]), PALETTE[0]);
            let all: Vec<Category> = PALETTE.iter().map(|c| Category { name: c.hex(), color: *c }).collect();
            assert_eq!(unused_color(&all), PALETTE[0]);
            // The row at the bottom adds a category in that colour.
            let mut f = CategoriesForm::new(&s);
            f.new_name = "Travel".into();
            assert_eq!(f.add_command(), ("category.set", json!({"name": "Travel", "color": "#1A9BA6"})));
            let (command, params) = f.add_command();
            s.run(command, &params).unwrap();
            assert_eq!(s.mailbox.category("travel").map(|c| c.color), Some(Rgb(0x1A, 0x9B, 0xA6)));
            assert_eq!(unused_color(&s.mailbox.categories), Rgb(0xC2, 0x4E, 0x9A));
            // A hostile name is the command's to clean.
            f.new_name = "\u{0}\u{202e}é\n".repeat(40_000);
            let (command, params) = f.add_command();
            s.run(command, &params).unwrap();
            assert!(s.mailbox.categories.iter().all(|c| c.name.chars().count() <= mailcraft_model::MAX_NAME));
        }
    }
}

mod junk;
#[cfg(test)]
mod junk_tests;

/// The Rules dialog: the rules, Run Rules Now, and a builder for a new rule.
mod rules {
    use egui::{Align2, Rect, Sense, Ui, pos2, vec2};
    use mailcraft_engine::Session;
    use mailcraft_model::{Action, Condition, FolderKind, Id, Importance, Mailbox, Rule};
    use serde_json::{Value, json};

    use super::kit::{Pass, body, close_button, error, row, shown};
    use crate::MailApp;
    use crate::theme::{paint_clipped, regular, semibold};
    use crate::widgets::icon_button;

    /// Most conditions, and most actions, the builder takes.
    const MAX_PARTS: usize = 5;

    /// What a condition looks at.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum When {
        #[default]
        From,
        To,
        Subject,
        Body,
        Attachment,
        Importance,
    }

    impl When {
        const ALL: [When; 6] = [When::From, When::To, When::Subject, When::Body, When::Attachment, When::Importance];

        fn label(self) -> &'static str {
            match self {
                When::From => "From contains",
                When::To => "Sent to",
                When::Subject => "Subject contains",
                When::Body => "Body contains",
                When::Attachment => "Has an attachment",
                When::Importance => "Importance is",
            }
        }

        /// The condition's `type` in `rules.add`.
        fn code(self) -> &'static str {
            match self {
                When::From => "fromContains",
                When::To => "toContains",
                When::Subject => "subjectContains",
                When::Body => "bodyContains",
                When::Attachment => "hasAttachment",
                When::Importance => "importance",
            }
        }

        /// The condition is a text to look for.
        fn takes_text(self) -> bool {
            !matches!(self, When::Attachment | When::Importance)
        }
    }

    /// One condition of the rule being built.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Cond {
        kind: When,
        text: String,
        importance: Importance,
    }

    impl Default for Cond {
        fn default() -> Self {
            Cond { kind: When::From, text: String::new(), importance: Importance::High }
        }
    }

    /// What an action does.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Do {
        #[default]
        Move,
        Categorize,
        Flag,
        MarkRead,
        SetImportance,
        Delete,
    }

    impl Do {
        const ALL: [Do; 6] = [Do::Move, Do::Categorize, Do::Flag, Do::MarkRead, Do::SetImportance, Do::Delete];

        fn label(self) -> &'static str {
            match self {
                Do::Move => "Move to folder",
                Do::Categorize => "Categorize",
                Do::Flag => "Flag",
                Do::MarkRead => "Mark as read",
                Do::SetImportance => "Set importance",
                Do::Delete => "Delete",
            }
        }
    }

    /// One action of the rule being built. The folder, category and importance are kept for
    /// whichever kind is chosen.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Act {
        kind: Do,
        folder: Id,
        category: String,
        importance: Importance,
    }

    fn importance_label(i: Importance) -> &'static str {
        match i {
            Importance::Low => "Low",
            Importance::Normal => "Normal",
            Importance::High => "High",
        }
    }

    fn importance_combo(ui: &mut Ui, salt: (&'static str, usize), value: &mut Importance) {
        egui::ComboBox::from_id_salt(salt).width(110.0).selected_text(tl!(importance_label(*value))).show_ui(ui, |ui| {
            for i in [Importance::High, Importance::Normal, Importance::Low] {
                if ui.selectable_label(i == *value, tl!(importance_label(i))).clicked() {
                    *value = i;
                }
            }
        });
    }

    /// The button at the right end of a builder row that takes the row out. `true` when it was
    /// clicked.
    fn remove_button(ui: &mut Ui, tip: &str) -> bool {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| icon_button(ui, "close", tip, 10.0, true).clicked()).inner
    }

    /// The account whose folders a new rule can move mail to: the one in view.
    fn account_in_view(s: &Session) -> Id {
        s.mailbox.folder(s.view.folder).map(|f| f.account).or(s.mailbox.default_account().map(|a| a.id)).unwrap_or(0)
    }

    /// An account's folders as a rule can move mail to them: id and path, in the folder pane's
    /// order.
    pub fn folder_choices(mb: &Mailbox, account: Id) -> Vec<(Id, String)> {
        let mut out: Vec<(Id, String)> = Vec::new();
        let mut waiting: Vec<Id> = mb.children(account, None).iter().rev().map(|f| f.id).collect();
        while let Some(id) = waiting.pop() {
            // Every folder once, however the tree is tangled.
            if out.len() >= mb.folders.len() {
                break;
            }
            out.push((id, crate::nav::folder_path(mb, id)));
            waiting.extend(mb.children(account, Some(id)).iter().rev().map(|f| f.id));
        }
        out
    }

    /// The rule in words, naming the folder a move goes to:
    /// `When from contains "x": move it to Inbox/Projects, flag it`.
    pub fn describe(mb: &Mailbox, r: &Rule) -> String {
        let when = r.conditions.iter().map(Condition::describe).collect::<Vec<_>>().join(" and ");
        let then = r
            .actions
            .iter()
            .map(|a| match a {
                Action::MoveTo(f) if mb.folder(*f).is_some() => format!("move it to {}", crate::nav::folder_path(mb, *f)),
                Action::MoveTo(_) => "move it to a folder that is gone".to_string(),
                other => other.describe(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("When {when}: {then}")
    }

    /// The Rules dialog.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RulesForm {
        /// The New rule builder is unfolded.
        open: bool,
        name: String,
        conds: Vec<Cond>,
        acts: Vec<Act>,
        stop: bool,
        /// The folders a move can go to: id and path.
        folders: Vec<(Id, String)>,
        /// The folder and category a fresh action starts with.
        first_folder: Id,
        first_category: String,
        /// How many messages the last Run Rules Now matched.
        ran: Option<u64>,
        error: String,
        focus: bool,
    }

    impl RulesForm {
        /// The dialog, with the builder unfolded when there are no rules yet.
        pub fn new(s: &Session) -> RulesForm {
            let account = account_in_view(s);
            let folders = folder_choices(&s.mailbox, account);
            let first_folder = s.mailbox.folder_of_kind(account, FolderKind::Archive).or(folders.first().map(|f| f.0)).unwrap_or(0);
            let first_category = s.mailbox.categories.first().map(|c| c.name.clone()).unwrap_or_default();
            let mut f = RulesForm { open: s.mailbox.rules.is_empty(), folders, first_folder, first_category, focus: true, ..Default::default() };
            f.reset();
            f
        }

        /// Start the builder over: one empty condition, one action.
        fn reset(&mut self) {
            self.name.clear();
            self.stop = false;
            self.conds = vec![Cond::default()];
            self.acts = vec![self.fresh_action()];
        }

        fn fresh_action(&self) -> Act {
            Act { kind: Do::Move, folder: self.first_folder, category: self.first_category.clone(), importance: Importance::High }
        }

        /// The command Add runs, with its params.
        pub fn command(&self) -> (&'static str, Value) {
            let conditions: Vec<Value> = self
                .conds
                .iter()
                .map(|c| match c.kind {
                    When::Attachment => json!({"type": c.kind.code()}),
                    When::Importance => json!({"type": c.kind.code(), "value": c.importance.code()}),
                    kind => json!({"type": kind.code(), "value": c.text}),
                })
                .collect();
            let actions: Vec<Value> = self
                .acts
                .iter()
                .map(|a| match a.kind {
                    Do::Move => json!({"type": "moveTo", "value": a.folder}),
                    Do::Categorize => json!({"type": "categorize", "value": a.category}),
                    Do::Flag => json!({"type": "flag"}),
                    Do::MarkRead => json!({"type": "markRead"}),
                    Do::SetImportance => json!({"type": "setImportance", "value": a.importance.code()}),
                    Do::Delete => json!({"type": "delete"}),
                })
                .collect();
            ("rules.add", json!({"name": self.name, "conditions": conditions, "actions": actions, "stop": self.stop}))
        }

        /// Whether Add can run: the rule has a name, every text condition a text, every move a
        /// folder and every Categorize a category.
        fn ready(&self) -> bool {
            let conds = self.conds.iter().all(|c| !c.kind.takes_text() || !c.text.trim().is_empty());
            let acts = self.acts.iter().all(|a| match a.kind {
                Do::Move => a.folder != 0,
                Do::Categorize => !a.category.trim().is_empty(),
                _ => true,
            });
            !self.name.trim().is_empty() && !self.conds.is_empty() && !self.acts.is_empty() && conds && acts
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            let mut pass = Pass::new(ui, &app.session, &mut false);
            let t = pass.t;
            let rules = app.session.mailbox.rules.clone();
            let categories: Vec<String> = app.session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
            let folder_in_view = crate::nav::folder_path(&app.session.mailbox, app.session.view.folder);
            let mut run: Option<(&'static str, Value)> = None;
            let mut add = false;
            body(ui, |ui| {
                for r in &rules {
                    ui.horizontal(|ui| {
                        let mut on = r.enabled;
                        if ui.checkbox(&mut on, "").on_hover_text(tl!("Run this rule on mail as it arrives")).changed() {
                            run = Some(("rules.enable", json!({"id": r.id, "value": on})));
                        }
                        let width = (ui.available_width() - 34.0).max(40.0);
                        let (rect, resp) = ui.allocate_exact_size(vec2(width, 38.0), Sense::hover());
                        let words = shown(&describe(&app.session.mailbox, r), 600);
                        paint_clipped(
                            ui.painter(),
                            pos2(rect.min.x, rect.min.y + 11.0),
                            width,
                            &shown(&r.name, 200),
                            semibold(12.5),
                            if r.enabled { t.text } else { t.text_disabled },
                        );
                        paint_clipped(ui.painter(), pos2(rect.min.x, rect.min.y + 28.0), width, &words, regular(11.5), t.text_dim);
                        resp.on_hover_text(words);
                        if icon_button(ui, "delete", tl!("Delete this rule"), 14.0, true).clicked() {
                            run = Some(("rules.remove", json!({"id": r.id})));
                        }
                    });
                }
                if rules.is_empty() {
                    ui.label(egui::RichText::new(tl!("No rules yet. A rule files, flags or deletes mail as it arrives.")).color(t.text_dim));
                }
                ui.horizontal(|ui| {
                    let tip = crate::i18n::fmt(tl!("Run every rule that is switched on over {folder}"), &[("folder", &folder_in_view)]);
                    if ui.add_enabled(!rules.is_empty(), egui::Button::new(tl!("Run Rules Now"))).on_hover_text(tip).clicked() {
                        run = Some(("rules.run", json!({})));
                    }
                    if let Some(n) = self.ran {
                        let found = tln!(n, "{n} message matched", "{n} messages matched");
                        ui.label(egui::RichText::new(found).color(t.text_dim));
                    }
                });
                ui.separator();
                // The builder folds away under its heading.
                let (head, head_resp) = ui.allocate_exact_size(vec2(ui.available_width().max(40.0), 24.0), Sense::click());
                if head_resp.hovered() {
                    ui.painter().rect_filled(head, 4.0, t.hover);
                }
                crate::icons::paint(
                    ui.painter(),
                    Rect::from_center_size(pos2(head.min.x + 10.0, head.center().y), vec2(12.0, 12.0)),
                    if self.open { "expanded" } else { "collapsed" },
                    t.icon,
                    t.accent,
                );
                ui.painter().text(pos2(head.min.x + 24.0, head.center().y), Align2::LEFT_CENTER, tl!("New rule"), semibold(12.5), t.text);
                if head_resp.clicked() {
                    self.open = !self.open;
                    self.focus = self.open;
                }
                if self.open {
                    add = self.builder(ui, &mut pass, &categories);
                }
            });
            if add {
                run = Some(self.command());
            }
            if let Some((command, params)) = run {
                match app.execute(command, params) {
                    Ok(v) => {
                        self.error.clear();
                        // The count is of the last run; it goes when the rules change.
                        self.ran = v.get("matched").and_then(Value::as_u64).filter(|_| command == "rules.run");
                        if add {
                            self.reset();
                        }
                    }
                    Err(e) => self.error = e,
                }
            }
            error(ui, &self.error);
            close_button(ui)
        }

        /// The New rule builder. Returns `true` when Add was asked for and can run.
        fn builder(&mut self, ui: &mut Ui, pass: &mut Pass, categories: &[String]) -> bool {
            let t = pass.t;
            row(ui, tl!("Name"), |ui| {
                let resp = pass.text(ui, "rule_name", &mut self.name, f32::INFINITY, "");
                if std::mem::take(&mut self.focus) {
                    resp.request_focus();
                }
            });
            let mut drop_cond = None;
            let many = self.conds.len() > 1;
            for (i, c) in self.conds.iter_mut().enumerate() {
                row(ui, if i == 0 { tl!("When") } else { tl!("and") }, |ui| {
                    egui::ComboBox::from_id_salt(("when", i)).width(150.0).selected_text(tl!(c.kind.label())).show_ui(ui, |ui| {
                        for kind in When::ALL {
                            if ui.selectable_label(kind == c.kind, tl!(kind.label())).clicked() {
                                c.kind = kind;
                            }
                        }
                    });
                    match c.kind {
                        When::Attachment => {}
                        When::Importance => importance_combo(ui, ("when_importance", i), &mut c.importance),
                        _ => {
                            let width = (ui.available_width() - if many { 30.0 } else { 0.0 }).max(60.0);
                            pass.text(ui, ("when_text", i), &mut c.text, width, tl!("Text to look for"));
                        }
                    }
                    if many && remove_button(ui, tl!("Remove this condition")) {
                        drop_cond = Some(i);
                    }
                });
            }
            if let Some(i) = drop_cond.filter(|i| *i < self.conds.len()) {
                self.conds.remove(i);
            }
            row(ui, "", |ui| {
                if ui
                    .add_enabled(self.conds.len() < MAX_PARTS, egui::Button::new(tl!("+ condition")))
                    .on_hover_text(tl!("All the conditions must hold"))
                    .clicked()
                {
                    self.conds.push(Cond::default());
                }
            });
            let mut drop_act = None;
            let many = self.acts.len() > 1;
            let folders = &self.folders;
            for (i, a) in self.acts.iter_mut().enumerate() {
                row(ui, if i == 0 { tl!("Do") } else { tl!("and") }, |ui| {
                    egui::ComboBox::from_id_salt(("do", i)).width(150.0).selected_text(tl!(a.kind.label())).show_ui(ui, |ui| {
                        for kind in Do::ALL {
                            if ui.selectable_label(kind == a.kind, tl!(kind.label())).clicked() {
                                a.kind = kind;
                            }
                        }
                    });
                    let width = (ui.available_width() - if many { 30.0 } else { 0.0 }).max(60.0);
                    match a.kind {
                        Do::Move => {
                            let current =
                                folders.iter().find(|f| f.0 == a.folder).map_or_else(|| tl!("Choose a folder").to_string(), |f| shown(&f.1, 200));
                            egui::ComboBox::from_id_salt(("do_folder", i)).width(width).selected_text(current).truncate().show_ui(ui, |ui| {
                                for (id, path) in folders {
                                    if ui.selectable_label(*id == a.folder, shown(path, 200)).clicked() {
                                        a.folder = *id;
                                    }
                                }
                            });
                        }
                        Do::Categorize => {
                            let current = if a.category.is_empty() { tl!("Choose a category").to_string() } else { shown(&a.category, 200) };
                            egui::ComboBox::from_id_salt(("do_category", i)).width(width).selected_text(current).truncate().show_ui(ui, |ui| {
                                for name in categories {
                                    if ui.selectable_label(name.eq_ignore_ascii_case(&a.category), shown(name, 200)).clicked() {
                                        a.category = name.clone();
                                    }
                                }
                            });
                        }
                        Do::SetImportance => importance_combo(ui, ("do_importance", i), &mut a.importance),
                        Do::Flag | Do::MarkRead | Do::Delete => {}
                    }
                    if many && remove_button(ui, tl!("Remove this action")) {
                        drop_act = Some(i);
                    }
                });
            }
            if let Some(i) = drop_act.filter(|i| *i < self.acts.len()) {
                self.acts.remove(i);
            }
            let mut add = false;
            let ready = self.ready();
            row(ui, "", |ui| {
                if ui.add_enabled(self.acts.len() < MAX_PARTS, egui::Button::new(tl!("+ action"))).clicked() {
                    let fresh = self.fresh_action();
                    self.acts.push(fresh);
                }
            });
            row(ui, "", |ui| {
                ui.checkbox(&mut self.stop, tl!("Stop processing more rules"));
            });
            row(ui, "", |ui| {
                add = ui.add_enabled(ready, egui::Button::new(tl!("Add Rule")).min_size(vec2(84.0, 24.0))).clicked();
                if !ready {
                    ui.label(egui::RichText::new(tl!("A rule needs a name, and something to look for.")).color(t.text_dim).size(11.5));
                }
            });
            ready && (add || pass.submit)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Wednesday 2026-10-14 16:00 UTC.
        const NOW: i64 = 1_791_993_600;

        fn session() -> Session {
            Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
        }

        fn folder_id(s: &Session, path: &str) -> Id {
            s.mailbox.folders.iter().find(|f| s.mailbox.folder_path(f.id) == path).map(|f| f.id).unwrap()
        }

        #[test]
        fn a_new_rule_is_spelled_out_for_the_command() {
            let mut s = session();
            let mut f = RulesForm::new(&s);
            // The sample has a rule, so the builder starts folded, with one condition and a move to Archive.
            assert!(!f.open && !f.ready());
            assert_eq!((f.conds.len(), f.acts.len(), f.first_folder, f.first_category.as_str()), (1, 1, folder_id(&s, "Archive"), "Red category"));
            let receipts = folder_id(&s, "Receipts");
            f.name = "Invoices".into();
            f.conds = vec![
                Cond { kind: When::From, text: "billing".into(), ..Default::default() },
                Cond { kind: When::To, text: "accounts@".into(), ..Default::default() },
                Cond { kind: When::Subject, text: "invoice".into(), ..Default::default() },
                Cond { kind: When::Body, text: "amount due".into(), ..Default::default() },
                Cond { kind: When::Attachment, text: "left over".into(), ..Default::default() },
            ];
            f.acts = vec![
                Act { kind: Do::Move, folder: receipts, ..f.fresh_action() },
                Act { kind: Do::Categorize, category: "Green category".into(), ..f.fresh_action() },
                Act { kind: Do::Flag, ..f.fresh_action() },
                Act { kind: Do::MarkRead, ..f.fresh_action() },
                Act { kind: Do::SetImportance, importance: Importance::Low, ..f.fresh_action() },
            ];
            f.stop = true;
            assert!(f.ready());
            let (command, params) = f.command();
            assert_eq!(command, "rules.add");
            assert_eq!(
                params,
                json!({
                    "name": "Invoices",
                    "conditions": [
                        {"type": "fromContains", "value": "billing"}, {"type": "toContains", "value": "accounts@"},
                        {"type": "subjectContains", "value": "invoice"}, {"type": "bodyContains", "value": "amount due"}, {"type": "hasAttachment"},
                    ],
                    "actions": [
                        {"type": "moveTo", "value": receipts}, {"type": "categorize", "value": "Green category"},
                        {"type": "flag"}, {"type": "markRead"}, {"type": "setImportance", "value": "low"},
                    ],
                    "stop": true,
                })
            );
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let rule = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
            assert!(rule.enabled && rule.stop);
            assert_eq!(
                describe(&s.mailbox, rule),
                "When from contains \"billing\" and sent to \"accounts@\" and subject contains \"invoice\" and body contains \"amount due\" and has an attachment: \
                 move it to Receipts, categorize it as \"Green category\", flag it, mark it as read, set importance to low"
            );
            // The other kinds.
            f.conds = vec![Cond { kind: When::Importance, ..Default::default() }];
            f.acts = vec![Act { kind: Do::Delete, ..f.fresh_action() }];
            assert_eq!(
                f.command().1,
                json!({"name": "Invoices", "conditions": [{"type": "importance", "value": "high"}], "actions": [{"type": "delete"}], "stop": true})
            );
            // Starting over.
            f.reset();
            assert_eq!(
                (f.name.as_str(), f.stop, f.conds.as_slice(), f.acts.as_slice()),
                ("", false, [Cond::default()].as_slice(), [f.fresh_action()].as_slice())
            );
        }

        #[test]
        fn add_waits_for_what_a_rule_needs() {
            let s = session();
            let mut f = RulesForm::new(&s);
            f.name = "News".into();
            assert!(!f.ready());
            f.conds[0].text = " digest ".into();
            assert!(f.ready());
            // A move needs a folder, Categorize a category.
            f.acts[0].folder = 0;
            assert!(!f.ready());
            f.acts[0].kind = Do::Categorize;
            assert!(f.ready());
            f.acts[0].category.clear();
            assert!(!f.ready());
            f.acts[0].kind = Do::Flag;
            assert!(f.ready());
            // Conditions that take no text.
            f.conds = vec![Cond { kind: When::Attachment, ..Default::default() }, Cond { kind: When::Importance, ..Default::default() }];
            assert!(f.ready());
            f.name = " ".into();
            assert!(!f.ready());
            // On an empty mailbox the builder is unfolded: there is nothing else to show.
            assert!(RulesForm::new(&Session::empty()).open);
        }

        #[test]
        fn rules_and_folders_read_as_people_know_them() {
            let mut s = session();
            let account = account_in_view(&s);
            let paths: Vec<String> = folder_choices(&s.mailbox, account).into_iter().map(|f| f.1).collect();
            assert_eq!(
                paths,
                [
                    "Inbox",
                    "Inbox/Projects",
                    "Inbox/Projects/Atlas",
                    "Drafts",
                    "Sent Items",
                    "Deleted Items",
                    "Archive",
                    "Junk Email",
                    "Outbox",
                    "Receipts"
                ]
            );
            assert!(folder_choices(&s.mailbox, 999_999).is_empty());
            let atlas = folder_id(&s, "Inbox/Projects/Atlas");
            let rule = Rule {
                conditions: vec![Condition::SubjectContains("Atlas".into())],
                actions: vec![Action::MoveTo(atlas), Action::MarkRead],
                ..Default::default()
            };
            assert_eq!(describe(&s.mailbox, &rule), "When subject contains \"Atlas\": move it to Inbox/Projects/Atlas, mark it as read");
            // The folder went away.
            let gone = Rule { actions: vec![Action::MoveTo(999_999)], ..rule };
            assert_eq!(describe(&s.mailbox, &gone), "When subject contains \"Atlas\": move it to a folder that is gone");
            // The rule the sample comes with.
            let first = s.mailbox.rules.first().cloned().unwrap();
            assert_eq!(describe(&s.mailbox, &first), "When from contains \"harborbooks\": categorize it as \"Purple category\"");
            // Hostile text in a rule is the command's to cut down.
            let junk = "\u{0}\u{202e}é\n".repeat(40_000);
            let mut f = RulesForm::new(&s);
            f.name.clone_from(&junk);
            f.conds[0].text.clone_from(&junk);
            f.acts[0] = Act { kind: Do::Categorize, category: junk.clone(), ..f.fresh_action() };
            let (command, params) = f.command();
            let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
            let rule = s.mailbox.rules.iter().find(|r| r.id == id).cloned().unwrap();
            assert!(rule.name.chars().count() <= mailcraft_model::MAX_NAME);
            assert!(shown(&describe(&s.mailbox, &rule), 600).chars().count() <= 601);
        }
    }
}

/// The forms together: opening by name, and typing into them in a window without a screen.
#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;
    use serde_json::json;

    use super::Form;
    use crate::dialogs::Dialog;
    use crate::{MailApp, Services};

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        app
    }

    /// The whole window, laid out a few frames so the dialog is up and its first field has the keyboard.
    fn window(app: MailApp, size: (f32, f32)) -> Harness<'static, MailApp> {
        let mut harness =
            Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(size.0, size.1)).build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(6);
        harness
    }

    /// The window with the dialog `name` open.
    fn dialog(name: &str, params: serde_json::Value) -> Harness<'static, MailApp> {
        let mut a = app();
        let mut p = params;
        p["name"] = json!(name);
        a.execute("ui.dialog", p).unwrap();
        window(a, (1280.0, 800.0))
    }

    fn type_text(h: &mut Harness<'static, MailApp>, text: &str) {
        h.event(egui::Event::Text(text.to_string()));
        h.run_steps(3);
    }

    fn press(h: &mut Harness<'static, MailApp>, key: egui::Key) {
        h.key_press(key);
        h.run_steps(4);
    }

    fn open_dialog(h: &Harness<'static, MailApp>) -> Option<&'static str> {
        h.state().dialog.as_ref().map(Dialog::name)
    }

    #[test]
    fn forms_open_by_name() {
        let mut a = app();
        for (name, title, width) in [
            ("event", "New Appointment", 580.0),
            ("meeting", "New Meeting", 580.0),
            ("contact", "New Contact", 540.0),
            ("task", "New Task", 580.0),
            ("account", "Add Account", 520.0),
            ("categories", "Categories", 420.0),
            ("rules", "Rules", 620.0),
        ] {
            let f = Form::open(name, &a, &json!({})).unwrap_or_else(|| panic!("{name}"));
            assert_eq!((f.name(), f.title().as_str(), f.width()), (name, title, width));
        }
        assert!(Form::open("nonsense", &a, &json!({})).is_none());
        assert!(Form::open("", &a, &json!({"id": 1})).is_none());
        // The edit forms need something to edit: the selection, or `id`.
        for name in ["editEvent", "editContact", "editTask"] {
            assert!(Form::open(name, &a, &json!({})).is_none(), "{name}");
            assert!(Form::open(name, &a, &json!({"id": 0})).is_none(), "{name}");
        }
        let event = a.session.mailbox.events.keys().next().copied().unwrap();
        let contact = a.session.mailbox.contacts.keys().next().copied().unwrap();
        let task = a.session.mailbox.tasks.keys().next().copied().unwrap();
        for (name, id) in [("editEvent", event), ("editContact", contact), ("editTask", task)] {
            assert_eq!(Form::open(name, &a, &json!({"id": id})).map(|f| f.name()), Some(name));
        }
        (a.session.sel.event, a.session.sel.contact, a.session.sel.task) = (Some(event), Some(contact), Some(task));
        for (name, title) in [("editEvent", "Appointment"), ("editContact", "Contact"), ("editTask", "Task")] {
            let f = Form::open(name, &a, &json!({})).unwrap();
            // The first sample event is a meeting or an appointment; either way it says which.
            assert!(f.title() == title || (name == "editEvent" && f.title() == "Meeting"), "{name}: {}", f.title());
        }
        // The commands that ask for input open these forms.
        assert_eq!(a.run("tasks.new", json!({})).unwrap()["pending"], "task");
        assert_eq!(a.dialog.as_ref().map(Dialog::name), Some("task"));
        // An appointment's form opens in a window of its own; the dialog that was up stays.
        assert_eq!(a.run("calendar.edit", json!({})).unwrap()["pending"], "editEvent");
        assert_eq!((a.dialog.as_ref().map(Dialog::name), a.windows.of_event(event).is_some()), (Some("task"), true));
    }

    #[test]
    fn the_first_field_has_the_keyboard_and_enter_saves() {
        // A task: type its title, press Enter.
        let mut h = dialog("task", json!({}));
        type_text(&mut h, "Water the plants");
        assert_eq!(open_dialog(&h), Some("task"));
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), None);
        assert!(h.state().session.mailbox.tasks.values().any(|t| t.title == "Water the plants" && t.due.is_none()));

        // An appointment lands on the next half hour.
        let mut h = dialog("event", json!({}));
        type_text(&mut h, "Call the printer");
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), None);
        let s = &h.state().session;
        let e = s.mailbox.events.values().find(|e| e.title == "Call the printer").unwrap();
        assert_eq!(
            (e.start.iso().as_str(), e.duration_minutes(), e.reminder_minutes, s.sel.event),
            ("2026-10-14T16:30:00", 30, Some(15), Some(e.id))
        );

        // Tab moves on to the next field.
        let mut h = dialog("contact", json!({}));
        type_text(&mut h, "Noor");
        press(&mut h, egui::Key::Tab);
        type_text(&mut h, "Haddad");
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), None);
        assert!(h.state().session.mailbox.contacts.values().any(|c| c.first == "Noor" && c.last == "Haddad"));

        // An account is a wizard: Enter is Connect on the address page, Connect on the page with
        // the servers it suggests, and Done on the last one.
        let mut h = dialog("account", json!({}));
        type_text(&mut h, "noor@example.org");
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), Some("account"));
        assert!(!h.state().session.mailbox.accounts.iter().any(|a| a.email == "noor@example.org"));
        press(&mut h, egui::Key::Enter);
        assert!(h.state().session.mailbox.accounts.iter().any(|a| a.email == "noor@example.org"));
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), None);
    }

    #[test]
    fn enter_waits_while_ok_is_off_and_errors_stay_in_the_form() {
        // Nothing typed: Enter does nothing.
        let mut h = dialog("task", json!({}));
        let tasks = h.state().session.mailbox.tasks.len();
        press(&mut h, egui::Key::Enter);
        assert_eq!((open_dialog(&h), h.state().session.mailbox.tasks.len()), (Some("task"), tasks));
        // A meeting needs someone to invite.
        let mut h = dialog("meeting", json!({}));
        let events = h.state().session.mailbox.events.len();
        type_text(&mut h, "Kick-off");
        press(&mut h, egui::Key::Enter);
        assert_eq!((open_dialog(&h), h.state().session.mailbox.events.len()), (Some("meeting"), events));
        // What the command refuses is said in the form, which stays open.
        let mut h = dialog("account", json!({}));
        let accounts = h.state().session.mailbox.accounts.len();
        type_text(&mut h, "not an address");
        press(&mut h, egui::Key::Enter);
        assert_eq!((open_dialog(&h), h.state().session.mailbox.accounts.len()), (Some("account"), accounts));
        assert!(h.query_by_label_contains("isn't an e-mail address").is_some());
        // Escape closes without saving.
        press(&mut h, egui::Key::Escape);
        assert_eq!((open_dialog(&h), h.state().session.mailbox.accounts.len()), (None, accounts));
    }

    #[test]
    fn enter_in_a_multi_line_field_is_a_new_line() {
        use egui::accesskit::Role;
        let mut h = dialog("task", json!({"title": "Plan the week"}));
        // Notes is the form's one multi-line field.
        h.get_by_role(Role::MultilineTextInput).focus();
        h.run_steps(3);
        type_text(&mut h, "one");
        press(&mut h, egui::Key::Enter);
        type_text(&mut h, "two");
        assert_eq!(open_dialog(&h), Some("task"));
        assert!(!h.state().session.mailbox.tasks.values().any(|t| t.title == "Plan the week"));
        // Back in the title, Enter saves, notes and all.
        h.get_all_by_role(Role::TextInput).find(|n| n.value().as_deref() == Some("Plan the week")).unwrap().focus();
        h.run_steps(3);
        press(&mut h, egui::Key::Enter);
        assert_eq!(open_dialog(&h), None);
        let task = h.state().session.mailbox.tasks.values().find(|t| t.title == "Plan the week").unwrap();
        assert_eq!(task.notes, "one\ntwo");
    }

    #[test]
    fn categories_are_added_one_after_another() {
        let mut h = dialog("categories", json!({}));
        type_text(&mut h, "Travel");
        press(&mut h, egui::Key::Enter);
        // The dialog stays, ready for the next name.
        assert_eq!(open_dialog(&h), Some("categories"));
        type_text(&mut h, "Reading");
        press(&mut h, egui::Key::Enter);
        let s = &h.state().session;
        let color = |name: &str| s.mailbox.category(name).map(|c| c.color.hex());
        assert_eq!((color("Travel").as_deref(), color("Reading").as_deref()), (Some("#1A9BA6"), Some("#C24E9A")));
        assert_eq!(s.mailbox.categories.len(), 8);
    }

    /// Never-crash: every form lays out over a mailbox full of hostile text, small and large.
    #[test]
    fn forms_lay_out_over_hostile_data() {
        use mailcraft_model::{Action, Address, Attendee, Category, Condition, Contact, Event, Labeled, PostalAddress, Recurrence, Rgb, Rule, Task};
        let junk = "\u{0}\u{202e}é\t<b>&amp;\n".repeat(300);
        let lines = |n: usize| vec![Labeled::new(junk.clone(), junk.clone()); n];
        for (size, theme) in [((1280.0, 800.0), "light"), ((760.0, 480.0), "dark"), ((240.0, 160.0), "light")] {
            let mut a = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            let mb = &mut a.session.mailbox;
            mb.categories = (0..60).map(|i| Category { name: format!("{i}{junk}"), color: Rgb(i, 0, 255 - i) }).collect();
            let contact = mb.add_contact(Contact {
                display_name: junk.clone(),
                first: junk.clone(),
                emails: lines(40),
                phones: lines(40),
                addresses: vec![PostalAddress { label: junk.clone(), street: junk.clone(), city: junk.clone(), ..Default::default() }],
                websites: vec![junk.clone()],
                notes: junk.clone(),
                categories: vec![junk.clone(); 30],
                ..Default::default()
            });
            let at = |hour| mailcraft_time::DateTime::new(mailcraft_time::Date::new(9999, 12, 31).unwrap(), hour, 59, 59).unwrap();
            let event = mb
                .add_event(Event {
                    title: junk.clone(),
                    location: junk.clone(),
                    description: junk.clone(),
                    // It ends before it starts, at the end of time.
                    start: at(23),
                    end: at(1),
                    attendees: vec![Attendee { address: Address::new(junk.clone(), junk.clone()), ..Default::default() }; 50],
                    recurrence: Some(Recurrence {
                        interval: u32::MAX,
                        nth: Some(i8::MIN),
                        count: Some(u32::MAX),
                        by_day: vec![mailcraft_time::Weekday::Sun; 40],
                        ..Default::default()
                    }),
                    reminder_minutes: Some(u32::MAX),
                    categories: vec![junk.clone(); 30],
                    calendar: 999_999,
                    ..Default::default()
                })
                .unwrap();
            let task = mb.add_task(Task {
                title: junk.clone(),
                notes: junk.clone(),
                reminder: Some(i64::MAX),
                completed: Some(i64::MIN),
                categories: vec![junk.clone(); 30],
                ..Default::default()
            });
            for i in 0..40 {
                mb.rules.push(Rule {
                    id: 500_000 + i,
                    name: junk.clone(),
                    enabled: i % 2 == 0,
                    conditions: vec![Condition::BodyContains(junk.clone()); 20],
                    actions: vec![Action::MoveTo(i), Action::Categorize(junk.clone())],
                    stop: true,
                });
            }
            let p = json!({"title": junk, "first": junk, "email": junk, "start": junk, "attendees": junk, "displayName": junk});
            for name in ["event", "meeting", "contact", "task", "account", "categories", "rules", "editEvent", "editContact", "editTask"] {
                let mut b = MailApp::new(Session::at(a.session.mailbox.clone(), NOW, 0), Services::default());
                b.autosave = false;
                b.ui = a.ui.clone();
                (b.session.sel.event, b.session.sel.contact, b.session.sel.task) = (Some(event), Some(contact), Some(task));
                let mut params = p.clone();
                params["name"] = json!(name);
                b.execute("ui.dialog", params).unwrap_or_else(|e| panic!("{name}: {e}"));
                let mut h = window(b, size);
                // Keys reach whatever has the keyboard, and nothing falls over.
                type_text(&mut h, &junk);
                press(&mut h, egui::Key::Tab);
                press(&mut h, egui::Key::Enter);
                assert!(h.state().dialog.is_none() || open_dialog(&h) == Some(name), "{name}");
            }
        }
    }
}
