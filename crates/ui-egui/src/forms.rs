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
    /// `account`, `categories` and `rules` manage those (`rules` with a `folder` has the New rule filled
    /// in to move the selected message's sender there). `None` for an unknown name or nothing to edit.
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
            "categories" => Form::Categories(Box::new(categories::CategoriesForm::new(s, params)?)),
            "rules" => Form::Rules(Box::new(rules::RulesForm::new(s, params))),
            "junkOptions" => Form::Junk(Box::new(junk::JunkForm::new(s, params))),
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

    /// The appointment, contact or task the form changes, when it is one that exists.
    pub(crate) fn editing(&self) -> Option<mailcraft_model::Id> {
        match self {
            Form::Event(f) => f.editing(),
            Form::Contact(f) => f.editing(),
            Form::Task(f) => f.editing(),
            Form::Account(_) | Form::Categories(_) | Form::Rules(_) | Form::Junk(_) => None,
        }
    }

    /// Whether something was changed in the form that isn't saved: its window asks before it
    /// closes, and an agent asks the owner before it closes the dialog or opens another in its
    /// place (#494).
    pub(crate) fn dirty(&self) -> bool {
        match self {
            Form::Event(f) => f.dirty(),
            Form::Contact(f) => f.dirty(),
            Form::Task(f) => f.dirty(),
            Form::Account(_) | Form::Categories(_) | Form::Rules(_) | Form::Junk(_) => false,
        }
    }

    /// What an agent or a command that waited may not take away (#494, #513): changes that aren't
    /// saved, or the Appointment Recurrence dialog with something changed in it (what it holds
    /// reaches the form only on its OK).
    pub(crate) fn changed(&self) -> bool {
        self.dirty() || self.event().is_some_and(event::EventForm::recurrence_touched)
    }

    /// What an agent's question names of the form when it has changes that aren't saved (#494):
    /// the kind of item and what it is called, as typed. `None` when nothing in it is unsaved.
    pub(crate) fn unsaved(&self) -> Option<Value> {
        let (form, title) = match self {
            Form::Event(f) => (if f.is_meeting() { "meeting" } else { "appointment" }, f.subject().to_string()),
            Form::Contact(f) => ("contact", f.subject()),
            Form::Task(f) => ("task", f.subject().to_string()),
            Form::Account(_) | Form::Categories(_) | Form::Rules(_) | Form::Junk(_) => return None,
        };
        self.changed().then(|| serde_json::json!({"form": form, "title": title}))
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
    use mailcraft_time::{Date, Weekday, parse_time, time_short};
    use serde_json::{Map, Value, json};

    use crate::date_picker::{Calendar, Picked};
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
        /// The day weeks begin on, in a date field's calendar.
        pub week_start: Weekday,
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
                week_start: s.prefs.week_start,
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
            let resp = crate::text_menu::field(resp);
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
            let resp = crate::text_menu::field(resp);
            self.field(&resp);
            if !date_ok(text, required) {
                mark_wrong(ui, &resp, &self.t);
            }
            resp
        }

        /// A date field with buttons that step a day back and forward, and a calendar to pick
        /// one from (#329). Returns `true` when the text changed.
        pub fn date(&mut self, ui: &mut Ui, key: impl Hash + Debug + Copy, text: &mut String, required: bool) -> bool {
            let before = text.clone();
            let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.x, 2.0);
            self.date_text(ui, key, text, required);
            for (icon, step, tip) in [("previous", -1, tl!("The day before")), ("next", 1, tl!("The day after"))] {
                if icon_button(ui, icon, tip, 12.0, true).clicked() {
                    *text = Date::parse_iso(text).map_or(self.today, |d| d.add_days(step)).iso();
                }
            }
            self.calendar(ui, key, text, required);
            ui.spacing_mut().item_spacing.x = gap;
            *text != before
        }

        /// The calendar after the date field `key`: a day picked is written in, and None, offered
        /// only when the field may be empty, empties it. Returns `true` when something was picked.
        pub fn calendar(&self, ui: &mut Ui, key: impl Hash + Debug, text: &mut String, required: bool) -> bool {
            let calendar = Calendar {
                field: field_id(key),
                current: Date::parse_iso(text.trim()),
                today: self.today,
                week_start: self.week_start,
                offer_none: !required,
                name: tl!("Pick a date"),
            };
            match calendar.show(ui) {
                Some(Picked::Day(day)) => *text = day.iso(),
                Some(Picked::None) => text.clear(),
                None => return false,
            }
            true
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
            let resp = crate::text_menu::field(resp);
            self.field(&resp);
            if crate::text_menu::left(&resp)
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
            egui::ScrollArea::vertical().id_salt(("form_lines", key)).max_height(room).show(ui, |ui| crate::text_menu::field(ui.add(edit))).inner
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
            egui::ScrollArea::vertical().id_salt(("form_lines", key)).max_height(left).show(ui, |ui| crate::text_menu::field(ui.add(edit))).inner
        })
        .inner
    }

    /// A chip that is on or off, with its colour. `true` when it was clicked.
    pub fn chip(ui: &mut Ui, label: &str, on: bool, color: Color32) -> bool {
        let t = Tokens::get(ui.ctx());
        let text_w = ui.painter().layout_no_wrap(label.to_string(), regular(11.5), t.text).size().x.min(180.0);
        let (r, resp) = ui.allocate_exact_size(vec2(text_w + 32.0, 22.0), Sense::click());
        let fill = if on {
            t.data_fill(color)
        } else if resp.hovered() {
            t.hover
        } else {
            t.input
        };
        ui.painter().rect(r, 11.0, fill, Stroke::new(1.0, if on { t.data_edge(color) } else { t.input_border }), StrokeKind::Inside);
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

    /// Text from elsewhere for a label, as a name is shown (#454): one line, without control
    /// characters or characters that show nothing, spaces run together, at most `max` characters,
    /// the last an ellipsis where it was cut.
    pub use mailcraft_names::shown;

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
        strip(ui, |ui| close = crate::widgets::plain_button(ui, tl!("Close")).clicked());
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
            let line = t.ribbon_edge;
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
        mod tests;
    }

    #[cfg(test)]
    mod tests;
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
        let was_typed = ui.data(|d| d.get_temp::<bool>(typing)).unwrap_or(false);
        let typed = focused && was_typed;
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
        // (What was typed is kept until the line is left: its menu may have the keyboard meanwhile.)
        let typed = was_typed || resp.changed();
        let left = crate::text_menu::left(&resp);
        if left && typed {
            // Left, after something was typed: what stands for one contact becomes that contact.
            let (checked, _) = check_names(s, text);
            if Address::parse_list(text) != Address::parse_list(&checked) {
                *text = checked;
            }
        }
        ui.data_mut(|d| d.insert_temp(typing, typed && !left));
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
            egui::Modal::new(kit::field_id("recurrence_dialog")).frame(frame).backdrop_color(t.scrim).show(ctx, |ui| {
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
                            if crate::text_menu::field(ui.add(egui::DragValue::new(&mut self.repeat.count).range(1..=9999).speed(0.05))).changed() {
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
                crate::text_menu::field(ui.add(egui::DragValue::new(interval).range(1..=999).speed(0.05)));
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
                        if crate::text_menu::field(ui.add(egui::DragValue::new(&mut self.day).range(1..=31).speed(0.05))).changed() {
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
                            if crate::text_menu::field(ui.add(egui::DragValue::new(&mut self.day).range(1..=31).speed(0.05))).changed() {
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
            // As the command reads it (#553): one named wrongly opens nothing, never the selected one.
            let id = mailcraft_engine::p::id_strict(p, "id").ok()?.or(s.sel.event)?;
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

        /// The event it changes; `None` for a new one.
        pub fn editing(&self) -> Option<Id> {
            self.id
        }

        /// Whether the Appointment Recurrence dialog is open over it with something changed in it.
        pub fn recurrence_touched(&self) -> bool {
            self.recurrence.as_ref().is_some_and(RecurrenceDialog::touched)
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

        /// The account whose categories the event takes: its calendar's.
        pub(crate) fn category_account(&self, s: &Session) -> Id {
            s.mailbox.calendar(self.calendar).map_or(0, |c| c.origin.account)
        }

        /// What the window's Categorize menu offers: the list of the event's account, then in
        /// grey what the event has off it, as its chips do.
        pub(crate) fn offered_categories(&self, s: &Session) -> Vec<(String, egui::Color32)> {
            let account = self.category_account(s);
            let listed = s.mailbox.categories_of(account).iter().map(|c| (c.name.clone(), c32(c.color)));
            let off_list = self.categories.iter().filter(|name| s.mailbox.category_of(account, name).is_none());
            listed.chain(off_list.map(|name| (name.clone(), c32(mailcraft_model::GREY_CATEGORY)))).take(200).collect()
        }

        /// The window's own bar. The key of the button that was clicked.
        fn bar(&mut self, app: &mut MailApp, ui: &mut Ui) -> Option<&'static str> {
            let items = self.bar_items();
            let categories = self.offered_categories(&app.session);
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
                                ui.painter().text(
                                    r.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "✓",
                                    crate::theme::medium(10.0),
                                    Tokens::get(ui.ctx()).on_swatch,
                                );
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
                tall_row(ui, tl!("Categories"), |ui| category_chips(ui, s.mailbox.categories_of(self.category_account(s)), &mut self.categories));
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
                            let color = s.mailbox.category_of(self.category_account(s), name).map_or(t.text_disabled, |c| c32(c.color));
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
            if self.scope.is_some() && app.session.driver.calendar.opened.is_some_and(|(event, _)| Some(event) == self.id) {
                app.session.driver.calendar.opened = None;
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
    mod tests;
}

/// The contact form.
mod contact {
    use std::fmt::Debug;
    use std::hash::Hash;

    use egui::Ui;
    use mailcraft_engine::Session;
    use mailcraft_model::{ContactKey, Id, Labeled, PostalAddress};
    use mailcraft_time::Date;
    use serde_json::{Map, Value, json};

    use super::kit::{Pass, body, category_chips, changes, date_hint, date_ok, date_value, error, footer, multiline, object, row, shown, tall_row};
    use crate::MailApp;
    use crate::files::base64_lite::encode;
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

    /// One of the card's keys in the form (#30 R11): one it had, by its place on the card, or
    /// one added from a file (a Public key, checked as `people.edit` checks it), with its bytes
    /// as base64 for the command.
    #[derive(Clone, Debug, PartialEq)]
    enum CardKey {
        Had(usize, ContactKey),
        Added { key: ContactKey, data: String },
    }

    impl CardKey {
        fn key(&self) -> &ContactKey {
            match self {
                CardKey::Had(_, key) | CardKey::Added { key, .. } => key,
            }
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
        /// The card's certificates and keys.
        keys: Vec<CardKey>,
        /// The number of the Add from file… pick this form asked for: a key picked goes only to
        /// the form that asked for it.
        key_pick: Option<u64>,
        error: String,
        focus: bool,
        /// The fields as the form opened; OK sends what differs from them.
        baseline: Map<String, Value>,
        /// The fields as the form opened, prefilled or not: what it opened with is nothing to
        /// ask about (#494).
        opened: Map<String, Value>,
    }

    impl ContactForm {
        /// The number of the Add from file… pick this form asked for, if any.
        pub fn key_pick(&self) -> Option<u64> {
            self.key_pick
        }

        /// Add a Public key from a file to the card (Add from file…), when there is room and it
        /// isn't there already.
        pub fn add_key(&mut self, key: ContactKey) -> Result<(), String> {
            if self.keys.len() >= mailcraft_model::Contact::MAX_KEYS {
                return Err(tl!("A contact's card keeps at most 32 certificates and keys").to_string());
            }
            if self.keys.iter().any(|k| k.key().data == key.data) {
                return Err(tl!("That key is on the card already").to_string());
            }
            let data = encode(&key.data);
            self.keys.push(CardKey::Added { key, data });
            Ok(())
        }

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
            f.opened = f.fields();
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
                keys: c.keys.iter().take(mailcraft_model::Contact::MAX_KEYS).cloned().enumerate().map(|(i, k)| CardKey::Had(i, k)).collect(),
                focus: true,
                ..Default::default()
            };
            f.baseline = f.fields();
            f.opened = f.baseline.clone();
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

        /// Whether the fields differ from what the form opened with: there is something to save.
        pub fn dirty(&self) -> bool {
            self.fields() != self.opened
        }

        /// The contact it changes; `None` for a new one.
        pub fn editing(&self) -> Option<Id> {
            self.id
        }

        /// What the contact is called, as typed: its display name, else its first and last names.
        pub fn subject(&self) -> String {
            match self.display.trim() {
                "" => format!("{} {}", self.first.trim(), self.last.trim()).trim().to_string(),
                display => display.to_string(),
            }
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
                "keys": self.keys.iter().map(|k| match k {
                    CardKey::Had(i, _) => json!({"keep": i}),
                    CardKey::Added { data, .. } => json!({"data": data}),
                }).collect::<Vec<_>>(),
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
            let mut add_key = false;
            let mut pass = Pass::new(ui, &app.session, &mut self.focus);
            let account = self.id.and_then(|id| app.session.mailbox.contact(id)).map_or(0, |c| c.origin.account);
            let categories = app.session.mailbox.categories_of(account).to_vec();
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
                    pass.calendar(ui, "birthday", &mut self.birthday, false);
                    date_hint(ui, &self.birthday, false);
                });
                tall_row(ui, tl!("Categories"), |ui| category_chips(ui, &categories, &mut self.categories));
                row(ui, "", |ui| {
                    ui.checkbox(&mut self.favorite, tl!("Favorite"));
                });
                tall_row(ui, tl!("Notes"), |ui| {
                    multiline(ui, "notes", &mut self.notes, 3);
                });
                // The card's certificates and keys (#30 R11), and the Public keys kept for its
                // addresses, which are shown and not the card's to change.
                tall_row(ui, tl!("Certificates"), |ui| {
                    let t = crate::theme::Tokens::get(ui.ctx());
                    let mut gone = None;
                    for (i, k) in self.keys.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button(tl!("Remove")).on_hover_text(tl!("Take this key off the card")).clicked() {
                                gone = Some(i);
                            }
                            crate::security::card_key_line(app, ui, k.key(), &t);
                        });
                    }
                    if let Some(i) = gone {
                        self.keys.remove(i);
                    }
                    let emails: Vec<String> = self.emails.iter().map(|e| e.value.clone()).collect();
                    crate::security::kept_key_lines(app, ui, &emails, &t);
                    add_key |= ui
                        .button(tl!("Add from file…"))
                        .on_hover_text(tl!("An X.509 certificate (.cer, .pem) or an OpenPGP Public key (.asc) of this contact's"))
                        .clicked();
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
            if add_key {
                self.key_pick = Some(crate::security::ask_card_key(app));
            }
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
    mod tests;
}

/// The task form.
mod task {
    use egui::Ui;
    use mailcraft_engine::Session;
    use mailcraft_engine::cmd::{tasks, tasks_remote};
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
        /// Where the task is: an account, or 0 for this computer. `None` for a new one that
        /// nobody has said where to put: it stays here, like one that isn't put anywhere.
        pub(super) place: Option<Id>,
        /// The place as the form opened: choosing another is something to ask about.
        opened_place: Option<Id>,
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
        /// The fields as the form opened, prefilled or not: what it opened with is nothing to
        /// ask about (#494).
        opened: Map<String, Value>,
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
            f.place = p.get("account").and_then(Value::as_u64).or_else(|| tasks::new_task_place(s));
            f.opened_place = f.place;
            f.opened = f.fields();
            f
        }

        /// The form for an existing task: the one `p.id` names, else the selected one.
        pub fn edit(s: &Session, p: &Value) -> Option<TaskForm> {
            let id = p.get("id").and_then(Value::as_u64).filter(|i| *i != 0).or(s.sel.task)?;
            let t = s.mailbox.task(id)?;
            let reminder = t.reminder.map(|r| s.local(r));
            let mut f = TaskForm {
                id: Some(id),
                place: Some(t.origin.account),
                opened_place: Some(t.origin.account),
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
            f.opened = f.baseline.clone();
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

        /// Whether the fields differ from what the form opened with, Completed and the Account
        /// list too: there is something to save.
        pub fn dirty(&self) -> bool {
            self.fields() != self.opened || self.completed != self.was_completed || self.place != self.opened_place
        }

        /// The task it changes; `None` for a new one.
        pub fn editing(&self) -> Option<Id> {
            self.id
        }

        /// What the task is called, as its title field has it.
        pub fn subject(&self) -> &str {
            self.title.trim()
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
                None => {
                    if let Some(place) = self.place {
                        m.insert("account".into(), json!(place));
                    }
                    ("tasks.new", Value::Object(m))
                }
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

        /// The account whose categories the task takes: the one its Account list says.
        pub(crate) fn category_account(&self) -> Id {
            self.place.unwrap_or(0)
        }

        /// A new task that nobody had said a place for when the form opened ("On this computer
        /// for now") stays on this computer once something says one meanwhile (the list moved to
        /// an account, a task put somewhere else): OK puts it where the form says, not there.
        pub(crate) fn hold_place(&mut self, s: &Session) {
            if self.id.is_none() && self.place.is_none() && tasks::new_task_place(s).is_some() {
                (self.place, self.opened_place) = (Some(0), Some(0));
            }
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            self.hold_place(&app.session);
            let mut pass = Pass::new(ui, &app.session, &mut self.focus);
            let today = pass.today;
            let mailbox = &app.session.mailbox;
            let categories = mailbox.categories_of(self.category_account()).to_vec();
            let place = self.place.unwrap_or(0);
            let places: Vec<(Id, String)> =
                tasks_remote::task_accounts(mailbox).into_iter().chain([0]).map(|a| (a, crate::widgets::place_name(mailbox, a))).collect();
            // Nobody has said: it can still be offered to an account.
            let place_name =
                if self.place.is_some() { crate::widgets::place_name(mailbox, place) } else { tl!("On this computer for now").to_string() };
            let held = place != 0 && app.tasks_held(place);
            body(ui, |ui| {
                row(ui, tl!("Title"), |ui| {
                    pass.text(ui, "title", &mut self.title, f32::INFINITY, "");
                });
                if self.id.is_some() && (places.len() > 1 || place != 0) {
                    // Where it is, as it stays there.
                    row(ui, tl!("Account"), |ui| {
                        ui.add(egui::Label::new(place_name.as_str()).truncate());
                    });
                } else if places.len() > 1 {
                    row(ui, tl!("Account"), |ui| {
                        egui::ComboBox::from_id_salt("task_account").width(220.0).selected_text(place_name.as_str()).truncate().show_ui(ui, |ui| {
                            for (account, name) in &places {
                                if ui.selectable_label(self.place == Some(*account), name.as_str()).clicked() {
                                    self.place = Some(*account);
                                }
                            }
                        });
                    });
                    if held {
                        row(ui, "", |ui| {
                            ui.label(egui::RichText::new(tl!("It waits on this computer until To Do is allowed for this account")).size(11.5));
                        });
                    }
                }
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
    mod tests;
}

/// Add Account and Account Settings: the wizard in `account_setup`.
mod account {
    pub use crate::account_setup::AccountSetup as AccountForm;
}

/// The Categories dialog: recolour, rename, delete, add.
mod categories {
    use egui::{Ui, vec2};
    use mailcraft_engine::Session;
    use mailcraft_model::{AccountKind, Category, Id, Mailbox, Rgb};
    use serde_json::{Value, json};

    use super::kit::{ROW_H, body, close_button, error, field_id, shown};
    use crate::MailApp;
    use crate::theme::Tokens;
    use crate::widgets::icon_button;

    /// The colours a category can take: the theme's twelve.
    pub const PALETTE: [Rgb; 12] = crate::theme::CATEGORY_PALETTE;

    /// The first palette colour no category has yet; the first one when all are taken.
    pub fn unused_color(list: &[Category]) -> Rgb {
        PALETTE.iter().find(|p| !list.iter().any(|c| c.color == **p)).or(PALETTE.first()).copied().unwrap_or_default()
    }

    /// A colour swatch that opens the palette. Returns the colour picked there, if one was.
    fn swatch(ui: &mut Ui, color: Rgb) -> Option<Rgb> {
        crate::widgets::color_swatch(ui, ROW_H, Some(color), None).flatten()
    }

    /// The lists the Categories dialog can show (#633): each account's own, then the one the rest
    /// share, whose account is 0.
    fn lists(mailbox: &Mailbox) -> Vec<(Id, String)> {
        let own = mailbox.accounts.iter().filter(|a| mailbox.account_categories.contains_key(&a.id));
        let mut lists: Vec<(Id, String)> = own.map(|a| (a.id, shown(if a.email.is_empty() { &a.name } else { &a.email }, 60))).collect();
        let others = mailbox.accounts.iter().any(|a| a.kind != AccountKind::Local && mailbox.category_list_owner(a.id).is_none());
        lists.push((0, if others { tl!("Other accounts and On this computer") } else { tl!("On this computer") }.to_string()));
        lists
    }

    /// The Categories dialog.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CategoriesForm {
        /// The account whose list is in view; 0 for the list that accounts without their own share.
        account: Id,
        /// The name being retyped: the category it belongs to, and the text so far.
        editing: Option<(String, String)>,
        /// What is typed to be added.
        pub new_name: String,
        new_color: Rgb,
        error: String,
        focus: bool,
    }

    impl CategoriesForm {
        /// The dialog, on the list of `params.account`, else of the selected message's account,
        /// else of the first, with a colour no category has yet ready for a new one. An `account`
        /// of 0 is the shared list, as an item of no account says it. `None` when `params.account`
        /// is anything else that isn't an account.
        pub fn new(s: &Session, params: &Value) -> Option<CategoriesForm> {
            let named = match params.get("account") {
                None | Some(Value::Null) => None,
                Some(a) if a.as_u64() == Some(0) => Some(0),
                Some(_) => Some(mailcraft_engine::cmd::category::list_account(s, params).ok()?),
            };
            let account = named
                .or_else(|| s.sel.messages.last().and_then(|m| s.mailbox.message(*m)).map(|m| m.account))
                .or_else(|| s.mailbox.default_account().map(|a| a.id))
                .and_then(|a| s.mailbox.category_list_owner(a))
                .unwrap_or(0);
            Some(CategoriesForm { account, new_color: unused_color(s.mailbox.categories_of(account)), focus: true, ..Default::default() })
        }

        /// Put the list of `account` in view; nothing begun on the last one carries over.
        fn show_list(&mut self, mailbox: &Mailbox, account: Id) {
            self.account = account;
            self.editing = None;
            self.new_name.clear();
            self.error.clear();
            self.new_color = unused_color(mailbox.categories_of(account));
        }

        /// A command's `params` for the list in view.
        fn on_list(&self, mut params: Value) -> Value {
            if let Some(params) = params.as_object_mut().filter(|_| self.account != 0) {
                params.insert("account".to_string(), self.account.into());
            }
            params
        }

        /// The command that adds the category typed at the bottom.
        pub fn add_command(&self) -> (&'static str, Value) {
            ("category.set", self.on_list(json!({"name": self.new_name, "color": self.new_color.hex()})))
        }

        /// Lay out the form. Returns `true` when the dialog should close.
        pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui) -> bool {
            let t = Tokens::get(ui.ctx());
            let enter = crate::keys::enter(ui.ctx(), ui.layer_id());
            let lists = lists(&app.session.mailbox);
            if !lists.iter().any(|(id, _)| *id == self.account) {
                self.show_list(&app.session.mailbox, 0);
            }
            let list = app.session.mailbox.categories_of(self.account).to_vec();
            // Once Outlook's list of categories was read, those not on it say so (R3 spec S18).
            let outlook = app.session.mailbox.server_categories.get(&self.account);
            let not_outlooks: Vec<bool> =
                list.iter().map(|c| outlook.is_some_and(|names| !names.iter().any(|o| mailcraft_model::sync::same_category(o, &c.name)))).collect();
            let mut run: Option<(&'static str, Value)> = None;
            let mut adding = false;
            let mut picked = None;
            body(ui, |ui| {
                if lists.len() > 1 {
                    ui.horizontal(|ui| {
                        ui.label(tl!("Account"));
                        let now = lists.iter().find(|(id, _)| *id == self.account).map(|(_, name)| name.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt("categories_account").selected_text(now).width(280.0).show_ui(ui, |ui| {
                            for (id, name) in &lists {
                                if ui.selectable_label(self.account == *id, name).clicked() && self.account != *id {
                                    picked = Some(*id);
                                }
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
                for (i, c) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if let Some(color) = swatch(ui, c.color) {
                            run = Some(("category.set", self.on_list(json!({"name": c.name, "color": color.hex()}))));
                        }
                        let mut text = match &self.editing {
                            Some((name, text)) if *name == c.name => text.clone(),
                            _ => c.name.clone(),
                        };
                        let width = (ui.available_width() - 44.0).max(40.0);
                        let resp = ui.add(egui::TextEdit::singleline(&mut text).id(field_id(("category", i))).desired_width(width));
                        let resp = crate::text_menu::field(resp);
                        if resp.changed() {
                            self.editing = Some((c.name.clone(), text));
                        }
                        // Enter, or leaving the field, renames.
                        if crate::text_menu::left(&resp)
                            && let Some((from, to)) = self.editing.take_if(|e| e.0 == c.name)
                            && to.trim() != from
                        {
                            run = Some(("category.rename", self.on_list(json!({"from": from, "to": to}))));
                        }
                        if icon_button(ui, "delete", tl!("Delete this category"), 14.0, true).clicked() {
                            run = Some(("category.remove", self.on_list(json!({"name": c.name}))));
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
                    let resp = crate::text_menu::field(resp);
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
            if let Some(account) = picked {
                self.show_list(&app.session.mailbox, account);
            }
            if let Some((command, params)) = run {
                match app.execute(command, params) {
                    Ok(_) => {
                        self.error.clear();
                        if adding {
                            // Ready for the next one.
                            self.new_name.clear();
                            self.new_color = unused_color(app.session.mailbox.categories_of(self.account));
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
    mod tests;
}

#[cfg(test)]
mod categories_tests;
mod junk;
#[cfg(test)]
mod junk_tests;
#[cfg(test)]
mod task_place_tests;

/// The Rules dialog: the rules, Run Rules Now, and a builder for a new rule.
pub(crate) mod rules {
    use egui::{Align2, Rect, Sense, Ui, pos2, vec2};
    use mailcraft_engine::Session;
    use mailcraft_engine::cmd::rules::{NotOnServer, server_cannot, server_cannot_draft};
    use mailcraft_model::{Action, FolderKind, Id, Importance, Keep, Mailbox, Rule};
    use serde_json::{Value, json};

    use super::kit::{Pass, body, close_button, error, row, shown, tall_row};
    use crate::MailApp;
    use crate::theme::{paint_clipped, regular, semibold};
    use crate::widgets::icon_button;

    /// Most conditions, and most actions, the builder takes.
    const MAX_PARTS: usize = 5;
    /// The width of a rule's button (an icon of 14 points, as `icon_button` lays it out).
    const BUTTON: f32 = 22.0;

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

        /// The label for a rule of an account whose server looks for a sender's or a recipient's
        /// words in their addresses alone (#566).
        fn what(self) -> &'static str {
            match self {
                When::From => "Sender's address contains",
                When::To => "Recipient's address contains",
                other => other.label(),
            }
        }

        fn label_as(self, addresses: bool) -> &'static str {
            if addresses { self.what() } else { self.label() }
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

    /// The accounts the dialog shows the rules of (#566): every account but a shared mailbox and
    /// one of a kind this version doesn't know, with what it is called, in the order the owner
    /// put them in the folder pane (#702).
    pub fn rule_accounts(mb: &Mailbox) -> Vec<(Id, String)> {
        mb.accounts_in_order()
            .into_iter()
            .filter(|a| has_rules(a))
            .map(|a| (a.id, if a.email.trim().is_empty() { a.name.clone() } else { a.email.trim().to_string() }))
            .collect()
    }

    /// Whether the dialog shows an account's rules: not a shared mailbox's, nor one of a kind this
    /// version doesn't know.
    fn has_rules(a: &mailcraft_model::Account) -> bool {
        !a.is_shared() && a.kind != mailcraft_model::AccountKind::Unknown
    }

    /// The account the dialog falls back on when none is in view: the first added that has
    /// rules, whatever order the owner put the accounts in (#702).
    fn first_with_rules(mb: &Mailbox) -> Option<Id> {
        mb.accounts.iter().find(|a| has_rules(a)).map(|a| a.id)
    }

    /// The account the dialog opens on: the one in view, as Outlook's "Apply changes to this
    /// folder" starts on the mailbox in view.
    fn account_in_view(s: &Session) -> Id {
        let shown = rule_accounts(&s.mailbox);
        let in_view = s.mailbox.folder(s.view.folder).map(|f| f.account).or(s.mailbox.default_account().map(|a| a.id));
        in_view.filter(|a| shown.iter().any(|(id, _)| id == a)).or_else(|| first_with_rules(&s.mailbox)).unwrap_or(0)
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

    /// The folders a new rule of `account` can move mail to: the account's own, then those of the
    /// owner's other accounts, named with their account (a rule moving mail there is kept on this
    /// computer).
    pub fn move_choices(mb: &Mailbox, account: Id) -> Vec<(Id, String)> {
        let mut out = folder_choices(mb, account);
        for (other, name) in rule_accounts(mb).into_iter().filter(|(id, _)| *id != account) {
            let theirs = folder_choices(mb, other);
            out.extend(
                theirs.into_iter().map(|(id, path)| (id, crate::i18n::fmt(tl!("{folder} - {mailbox}"), &[("folder", &path), ("mailbox", &name)]))),
            );
        }
        out
    }

    /// The rule in words, naming the folder a move goes to:
    /// `When from contains "x": move it to Inbox/Projects, flag it`.
    pub fn describe(mb: &Mailbox, r: &Rule) -> String {
        let addresses = mb.rule_reads_addresses(r);
        let when = r.conditions.iter().map(|c| c.describe_as(addresses)).collect::<Vec<_>>().join(" and ");
        let then = r
            .actions
            .iter()
            .map(|a| match a {
                Action::MoveTo(f) if mb.folder(*f).is_some_and(|f| f.account == r.account || r.account == 0) => {
                    format!("move it to {}", crate::nav::folder_path(mb, *f))
                }
                Action::MoveTo(f) if let Some(other) = mb.folder(*f).and_then(|f| mb.account(f.account)) => {
                    let who = if other.email.trim().is_empty() { other.name.trim() } else { other.email.trim() };
                    format!("move it to {} - {who}", crate::nav::folder_path(mb, *f))
                }
                Action::MoveTo(_) => "move it to a folder that is gone".to_string(),
                other => other.describe(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("When {when}: {then}")
    }

    /// Where a rule is kept, and why, as the dialog says it under the rule (#566).
    pub(crate) fn mark(app: &MailApp, r: &Rule, account: Id) -> String {
        let mb = &app.session.mailbox;
        let why = |n: NotOnServer| tl!(n.said()).to_string();
        let only_here = |why: &str| crate::i18n::fmt(tl!("Only on this computer: {why}"), &[("why", why)]);
        if r.account == 0 {
            return tl!("On this computer, for the mail of every account").to_string();
        }
        // A change the server isn't told of while MailCraft may not change its rules: the server
        // still has the rule as it was, and runs it so.
        if r.changed_here() && app.rules_held(r.account) {
            return tl!("On the server, still as it was: your change waits for permission (File › Info)").to_string();
        }
        if let Some(server) = r.server.as_ref().filter(|_| r.on_server()) {
            return if !server.unknown.is_empty() {
                tl!("On the server: here it can be switched on or off, moved or deleted").to_string()
            } else if server.read_only {
                tl!("On the server, which doesn't let it be changed").to_string()
            } else if server.error {
                tl!("On the server, which says something in it is wrong: Outlook on the web shows what").to_string()
            } else {
                tl!("On the server").to_string()
            };
        }
        if r.waiting() {
            let reason = server_cannot(mb, r, account).map_or_else(|| app.rules_wait_reason(account), why);
            return crate::i18n::fmt(tl!("Only on this computer for now: {why}"), &[("why", &reason)]);
        }
        if !mb.keeps_rules(r.account) {
            return tl!("On this computer").to_string();
        }
        if !r.refused.is_empty() {
            let said = crate::i18n::fmt(tl!("the server didn't take it ({why})"), &[("why", &shown(&r.refused, 120))]);
            return only_here(&said);
        }
        match server_cannot(mb, r, r.account) {
            Some(NotOnServer::NoServer) | None => tl!("Only on this computer").to_string(),
            Some(n) => only_here(&why(n)),
        }
    }

    /// A list's heading, with what it means on hover.
    fn heading(ui: &mut Ui, t: crate::theme::Tokens, title: &str, tip: &str) {
        ui.label(egui::RichText::new(title).font(semibold(12.0)).color(t.text_dim)).on_hover_text(tip);
    }

    /// One rule of a list: switched on or off, its name, what it does and where it is kept; to the
    /// right, a place up or down, keeping it on the server, deleting it.
    #[allow(clippy::too_many_arguments)]
    fn rule_row(
        ui: &mut Ui,
        app: &MailApp,
        r: &Rule,
        mark: &str,
        (up, down): (bool, bool),
        account: Id,
        servers: &[(Id, String)],
        run: &mut Option<(&'static str, Value)>,
    ) {
        let t = crate::theme::Tokens::get(ui.ctx());
        let mb = &app.session.mailbox;
        let fixed = r.read_only();
        // The servers that could keep it: its own account's, or for a rule for every account, any.
        let to: Vec<&(Id, String)> = if r.keep == Keep::Here && !r.on_server() {
            servers.iter().filter(|(id, _)| (r.account == 0 || r.account == *id) && server_cannot(mb, r, *id).is_none()).collect()
        } else {
            Vec::new()
        };
        ui.horizontal(|ui| {
            let mut on = r.enabled;
            let tip = if fixed { tl!("The server doesn't let this rule be changed") } else { tl!("Run this rule on mail as it arrives") };
            if ui.add_enabled(!fixed, egui::Checkbox::without_text(&mut on)).on_hover_text(tip).on_disabled_hover_text(tip).changed() {
                *run = Some(("rules.enable", json!({"id": r.id, "value": on})));
            }
            // Four buttons to the right, the same in every row, so that they stand in a column.
            let buttons = 4.0 * BUTTON + 4.0 * ui.spacing().item_spacing.x;
            let width = (ui.available_width() - buttons).max(40.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 54.0), Sense::hover());
            let words = if r.known() {
                shown(&describe(mb, r), 600)
            } else {
                tl!("Made in another program or on the web, with parts MailCraft can't show").to_string()
            };
            let shade = if r.enabled { t.text } else { t.text_disabled };
            paint_clipped(ui.painter(), pos2(rect.min.x, rect.min.y + 11.0), width, &shown(&r.name, 200), semibold(12.5), shade);
            paint_clipped(ui.painter(), pos2(rect.min.x, rect.min.y + 27.0), width, &words, regular(11.5), t.text_dim);
            let held = r.changed_here() && app.rules_held(r.account);
            let mark_colour = if r.waiting() || !r.refused.is_empty() || held { t.orange } else { t.text_dim };
            paint_clipped(ui.painter(), pos2(rect.min.x, rect.min.y + 43.0), width, mark, regular(11.0), mark_colour);
            resp.on_hover_text(format!("{words}\n{mark}"));
            let place = |direction: &str| json!({"id": r.id, "direction": direction, "account": account});
            if icon_button(ui, "moveUp", tl!("Move this rule up: it runs earlier"), 14.0, up && !fixed).clicked() && up && !fixed {
                *run = Some(("rules.move", place("up")));
            }
            if icon_button(ui, "moveDown", tl!("Move this rule down: it runs later"), 14.0, down && !fixed).clicked() && down && !fixed {
                *run = Some(("rules.move", place("down")));
            }
            match to.as_slice() {
                [] => {
                    ui.add_space(BUTTON);
                }
                [(only, address)] => {
                    let tip = crate::i18n::fmt(
                        tl!("Keep this rule on the server of {address}: it then runs there whether MailCraft is open or not, for that account's mail"),
                        &[("address", &shown(address, 80))],
                    );
                    if icon_button(ui, "server", &tip, 14.0, true).clicked() {
                        *run = Some(("rules.keepOnServer", json!({"id": r.id, "account": only})));
                    }
                }
                several => {
                    let tip = tl!("Keep this rule on an account's server: choose which. It then runs there whether MailCraft is open or not, for that account's mail");
                    let resp = icon_button(ui, "server", tip, 14.0, true);
                    egui::Popup::menu(&resp).show(|ui| {
                        for (id, address) in several {
                            let label = crate::i18n::fmt(tl!("Keep on the server of {address}"), &[("address", &shown(address, 80))]);
                            if ui.button(label).clicked() {
                                *run = Some(("rules.keepOnServer", json!({"id": r.id, "account": id})));
                                ui.close();
                            }
                        }
                    });
                }
            }
            if icon_button(ui, "delete", tl!("Delete this rule"), 14.0, !fixed).clicked() && !fixed {
                *run = Some(("rules.remove", json!({"id": r.id})));
            }
        });
    }

    /// The Rules dialog.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RulesForm {
        /// The account whose rules are shown, and whose arriving mail a new rule is for.
        account: Id,
        /// The New rule builder is unfolded.
        open: bool,
        name: String,
        conds: Vec<Cond>,
        acts: Vec<Act>,
        stop: bool,
        /// Keep on the server: ticked when the builder starts, the owner's to untick; while the
        /// server can't do the rule it is greyed and the rule stays here.
        server: bool,
        /// Opened by Always Move Messages From This Sender: the folder the message is in, and its
        /// path. Once the rule is added, the rules are run over that folder while this is ticked.
        run_in: Option<(Id, String)>,
        then_run: bool,
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
        /// The dialog, with the builder unfolded when there are no rules yet. With a `folder`, as
        /// Always Move Messages From This Sender opens it, the builder is unfolded and filled in to
        /// move the mail of the selected message's sender (or message `id`'s) there.
        pub fn new(s: &Session, params: &Value) -> RulesForm {
            let first_category = s.mailbox.every_category().into_iter().next().map(|c| c.name).unwrap_or_default();
            let mut f = RulesForm { open: s.mailbox.rules.is_empty(), first_category, focus: true, ..Default::default() };
            f.choose(&s.mailbox, account_in_view(s));
            f.reset();
            if let Some(folder) = params.get("folder").and_then(Value::as_u64) {
                f.fill_from_message(s, folder, params.get("id").and_then(Value::as_u64).or(s.sel.focus()));
            }
            f
        }

        /// Fill the builder in for the mail of message `id`'s sender to go to `folder`, for the
        /// account the message came to, as `rules.fromMessage` makes it. Nothing for a message or
        /// folder that isn't there, a send queue, a sender with no address, or a shared mailbox's
        /// message (its rules are its own; one for its owner's account would move the owner's mail
        /// into it).
        fn fill_from_message(&mut self, s: &Session, folder: Id, id: Option<Id>) {
            let mb = &s.mailbox;
            let Some(m) = id.and_then(|id| mb.message(id)) else { return };
            let Some(target) = mb.folder(folder).filter(|_| !mb.is_send_queue(folder)) else { return };
            let sender = m.from.email.trim();
            let account = m.account;
            if sender.is_empty() || !rule_accounts(mb).iter().any(|(id, _)| *id == account) {
                return;
            }
            self.choose(mb, account);
            let name = crate::i18n::fmt(tl!("{sender} to {folder}"), &[("sender", m.from.display()), ("folder", &target.name)]);
            self.name = name.chars().take(mailcraft_model::MAX_NAME).collect();
            self.conds = vec![Cond { kind: When::From, text: sender.to_string(), ..Default::default() }];
            self.acts = vec![Act { kind: Do::Move, folder, ..self.fresh_action() }];
            self.open = true;
            self.run_in = Some((m.folder, crate::nav::folder_path(mb, m.folder)));
            self.then_run = true;
        }

        /// Show the rules of `account`: a new rule is for its mail, and moves it to its folders.
        fn choose(&mut self, mb: &Mailbox, account: Id) {
            self.account = account;
            self.folders = move_choices(mb, account);
            self.first_folder = mb.folder_of_kind(account, FolderKind::Archive).or(self.folders.first().map(|f| f.0)).unwrap_or(0);
            let (folders, first) = (&self.folders, self.first_folder);
            for a in &mut self.acts {
                if !folders.iter().any(|f| f.0 == a.folder) {
                    a.folder = first;
                }
            }
            // The mail already there that the new rule may run over is the account's own.
            if self.run_in.as_ref().is_some_and(|(f, _)| mb.folder(*f).is_none_or(|f| f.account != account)) {
                self.run_in = None;
                self.then_run = false;
            }
        }

        /// Start the builder over: one empty condition, one action.
        fn reset(&mut self) {
            self.name.clear();
            self.stop = false;
            self.server = true;
            self.run_in = None;
            self.then_run = false;
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
            let mut params = json!({"name": self.name, "conditions": conditions, "actions": actions, "stop": self.stop});
            if self.account != 0 {
                params["account"] = json!(self.account);
                if !self.server {
                    params["server"] = json!(false);
                }
            }
            ("rules.add", params)
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
            // A held Space on a rule's button is one press: its repeats would reach the button
            // that takes its place, the next rule's, and delete or move that one too.
            ui.input_mut(|i| crate::keys::drop_repeats(i, &[egui::Key::Space]));
            let mut pass = Pass::new(ui, &app.session, &mut false);
            let t = pass.t;
            let accounts = rule_accounts(&app.session.mailbox);
            if !accounts.iter().any(|(id, _)| *id == self.account)
                && let Some(first) = first_with_rules(&app.session.mailbox)
            {
                self.choose(&app.session.mailbox, first);
            }
            let account = self.account;
            let mb = &app.session.mailbox;
            let on_server = mb.keeps_rules(account);
            let rules: Vec<Rule> = mb.rules_in_order(account).into_iter().filter_map(|i| mb.rules.get(i).cloned()).collect();
            let (there, here): (Vec<Rule>, Vec<Rule>) = rules.into_iter().partition(|r| r.account == account && r.keep == Keep::Server);
            let servers = rule_accounts(mb).into_iter().filter(|(id, _)| mb.keeps_rules(*id)).collect::<Vec<_>>();
            let categories: Vec<String> = mb.every_category().into_iter().map(|c| c.name).collect();
            let folder_in_view = crate::nav::folder_path(mb, app.session.view.folder);
            let marks: Vec<(Id, String)> = there.iter().chain(&here).map(|r| (r.id, mark(app, r, account))).collect();
            let held = app.rules_held(account);
            let gone = if held { mb.rule_gone.iter().filter(|g| g.account == account && !g.id.is_empty()).count() } else { 0 };
            // Whether the rule being made could be kept on the server, and why not (an account on
            // this computer alone has none).
            let place = mb
                .account(account)
                .filter(|a| self.open && a.kind != mailcraft_model::AccountKind::Local)
                .map(|a| (a.email.clone(), server_cannot_draft(&app.session, &self.command().1)));
            let mut run: Option<(&'static str, Value)> = None;
            let mut chosen = None;
            let mut add = false;
            body(ui, |ui| {
                // Whose rules: Outlook's "Apply changes to this folder".
                if accounts.len() > 1 {
                    row(ui, tl!("Rules for"), |ui| {
                        let current = accounts.iter().find(|(id, _)| *id == account).map(|(_, name)| shown(name, 120)).unwrap_or_default();
                        egui::ComboBox::from_id_salt("rules_account").width(260.0).selected_text(current).truncate().show_ui(ui, |ui| {
                            for (id, name) in &accounts {
                                if ui.selectable_label(*id == account, shown(name, 120)).clicked() {
                                    chosen = Some(*id);
                                }
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                let mark_of = |id: Id| marks.iter().find(|(m, _)| *m == id).map(|(_, m)| m.as_str()).unwrap_or("");
                if on_server {
                    heading(ui, t, tl!("On the server"), tl!("Run by the server as mail arrives, whether MailCraft is open or not"));
                    for (i, r) in there.iter().enumerate() {
                        // A rule waiting to go stands after those the server has, and moves among them once it is there.
                        let up = i > 0 && there.get(i - 1).is_some_and(|p| p.on_server() == r.on_server());
                        let down = there.get(i + 1).is_some_and(|n| n.on_server() == r.on_server());
                        rule_row(ui, app, r, mark_of(r.id), (up, down), account, &servers, &mut run);
                    }
                    if there.is_empty() {
                        ui.label(egui::RichText::new(tl!("None yet.")).color(t.text_dim).size(11.5));
                    }
                    // Deleted here, and still run by the server while it isn't told.
                    if gone > 0 {
                        let said = tln!(
                            gone,
                            "{n} rule deleted here is still on the server until MailCraft may change the server's rules (File › Info › Ask again).",
                            "{n} rules deleted here are still on the server until MailCraft may change the server's rules (File › Info › Ask again)."
                        );
                        ui.label(egui::RichText::new(said).color(t.orange).size(11.5));
                    }
                    ui.add_space(6.0);
                    heading(ui, t, tl!("On this computer"), tl!("Run by MailCraft as mail arrives, while it is open"));
                }
                for (i, r) in here.iter().enumerate() {
                    rule_row(ui, app, r, mark_of(r.id), (i > 0, i + 1 < here.len()), account, &servers, &mut run);
                }
                if there.is_empty() && here.is_empty() {
                    ui.label(egui::RichText::new(tl!("No rules yet. A rule files, flags or deletes mail as it arrives.")).color(t.text_dim));
                }
                ui.horizontal(|ui| {
                    let tip = crate::i18n::fmt(tl!("Run every rule that is switched on over {folder}"), &[("folder", &folder_in_view)]);
                    let any = !(there.is_empty() && here.is_empty());
                    if ui.add_enabled(any, egui::Button::new(tl!("Run Rules Now"))).on_hover_text(tip).clicked() {
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
                    add = self.builder(ui, &mut pass, &categories, place.as_ref(), held);
                }
            });
            if let Some(id) = chosen {
                self.choose(&app.session.mailbox, id);
                self.ran = None;
                self.error.clear();
            }
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
                            // Then the new rule alone over the mail already in the folder, as the
                            // menu that opened it says and Outlook offers.
                            let made = v.get("id").and_then(Value::as_u64);
                            if let Some(((folder, _), rule)) = self.run_in.take().filter(|_| self.then_run).zip(made) {
                                match app.execute("rules.run", json!({"folder": folder, "id": rule})) {
                                    Ok(v) => self.ran = v.get("matched").and_then(Value::as_u64),
                                    Err(e) => self.error = e,
                                }
                            }
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
        fn builder(
            &mut self,
            ui: &mut Ui,
            pass: &mut Pass,
            categories: &[String],
            place: Option<&(String, Option<NotOnServer>)>,
            held: bool,
        ) -> bool {
            let t = pass.t;
            // A rule kept on the server looks at addresses alone, as the server does; one kept here
            // at names too.
            let on_server = self.server && place.is_some_and(|(_, cannot)| cannot.is_none());
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
                    egui::ComboBox::from_id_salt(("when", i)).width(190.0).selected_text(tl!(c.kind.label_as(on_server))).show_ui(ui, |ui| {
                        for kind in When::ALL {
                            if ui.selectable_label(kind == c.kind, tl!(kind.label_as(on_server))).clicked() {
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
            // Where it is kept: the owner says, while the server can do all of it; greyed, with why,
            // when it can't.
            if let Some((address, cannot)) = place {
                let can = cannot.is_none();
                let mut ticked = self.server && can;
                let label = crate::i18n::fmt(tl!("Keep on the server of {address}"), &[("address", address)]);
                row(ui, "", |ui| {
                    if ui.add_enabled(can, egui::Checkbox::new(&mut ticked, label)).changed() {
                        self.server = ticked;
                    }
                });
                let (said, warn) = match cannot {
                    Some(NotOnServer::NoServer) => {
                        (tl!("MailCraft keeps this account's rules on this computer: the rule runs while MailCraft is open.").to_string(), false)
                    }
                    Some(why) => (
                        crate::i18n::fmt(
                            tl!("This rule can't be kept on the server: {why}. It runs on this computer, while MailCraft is open."),
                            &[("why", tl!(why.said()))],
                        ),
                        true,
                    ),
                    None if ticked && held => (
                        tl!("It goes to the server once MailCraft may change the server's rules (File › Info › Ask again); until then it runs on this computer, while MailCraft is open.")
                            .to_string(),
                        true,
                    ),
                    None if ticked => {
                        (tl!("It runs there whether MailCraft is open or not, and shows in Outlook on the web and on your phone.").to_string(), false)
                    }
                    None => (tl!("It runs on this computer, while MailCraft is open.").to_string(), false),
                };
                // Wrapped within the dialog: a reason can be long.
                tall_row(ui, "", |ui| {
                    ui.label(egui::RichText::new(said).color(if warn { t.orange } else { t.text_dim }).size(11.5));
                });
            }
            if let Some(label) = self
                .run_in
                .as_ref()
                .map(|(_, path)| crate::i18n::fmt(tl!("Then run it over the mail already in {folder}"), &[("folder", &shown(path, 80))]))
            {
                row(ui, "", |ui| {
                    ui.checkbox(&mut self.then_run, label);
                });
            }
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
    mod keep_tests;

    #[cfg(test)]
    mod tests;
}

#[cfg(test)]
mod tests;
