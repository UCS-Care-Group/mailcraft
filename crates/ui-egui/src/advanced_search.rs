// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Advanced Search (#329): the panel under the search box, behind the button at the box's right,
//! as Outlook has it. Search in, From, To, Subject, the days a message was received, Attachments
//! (and words in a file name), Categories and Body; Add filters puts Cc, Read status and Importance
//! with them, and the choice is kept (`ui.json`). Opening the panel reads its fields out of the
//! box's query; Search writes the ones that were changed back where they stood, and leaves the
//! rest of the query as it was typed ([`Reading`]). Search in applies with Search.
//!
//! The panel is part of the main window, drawn just over it: Escape and Enter come to it through
//! the window's keys (#406), and a list open in it (Categories, Add filters) closes on Escape
//! before the panel does. It closes when the search changes under it, when Mail leaves the front,
//! when File or a dialog opens, and when another mailbox is opened.

use egui::{Area, Frame, Id, LayerId, Order, Rect, Response, Stroke, StrokeKind, Ui, pos2, vec2};
use mailcraft_engine::{Module, Scope};
use mailcraft_model::Importance;
use mailcraft_model::search::{Advanced, Reading, full_date};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::MailApp;
use crate::date_picker::{Calendar, Picked};
use crate::theme::{Tokens, regular};

/// The most of the panel's width its labels' column takes, the panel's width as it would have it,
/// and the narrowest the panel is before its labels go above the fields.
const LABEL_SHARE: f32 = 0.4;
const PANEL_W: f32 = 460.0;
const NARROW: f32 = 380.0;

/// The filters Add filters puts in the panel, as the owner left them (kept in `ui.json`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Filters {
    pub cc: bool,
    pub read: bool,
    pub importance: bool,
}

/// One of the filters of Add filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    Cc,
    Read,
    Importance,
}

impl Filter {
    fn of(self, f: Filters) -> bool {
        match self {
            Filter::Cc => f.cc,
            Filter::Read => f.read,
            Filter::Importance => f.importance,
        }
    }

    fn set(self, f: &mut Filters, on: bool) {
        match self {
            Filter::Cc => f.cc = on,
            Filter::Read => f.read = on,
            Filter::Importance => f.importance = on,
        }
    }

    /// Its field emptied.
    fn clear(self, fields: &mut Advanced) {
        match self {
            Filter::Cc => fields.cc.clear(),
            Filter::Read => fields.read = None,
            Filter::Importance => fields.importance = None,
        }
    }
}

/// What a day typed in the panel says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Day {
    None,
    Date(mailcraft_time::Date),
    NotADate,
}

fn day(text: &str) -> Day {
    match text.trim() {
        "" => Day::None,
        t => full_date(t).map_or(Day::NotADate, Day::Date),
    }
}

/// The panel while it is open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Panel {
    /// The query it read its fields from, and where each came from.
    reading: Reading,
    pub(crate) fields: Advanced,
    /// The days received as typed.
    pub(crate) received_from: String,
    pub(crate) received_to: String,
    pub(crate) scope: Scope,
    /// The filters of Add filters shown for this opening because the box holds them, ticked or not.
    held: Filters,
    /// The mailbox it was opened on ([`mailcraft_engine::Session::openings`]).
    openings: u64,
    /// Where the search looked when it was opened.
    looked: Scope,
}

/// What is wrong with the days received, if anything: the message under the first box, under the
/// second.
fn day_problems(from: &str, to: &str) -> (Option<&'static str>, Option<&'static str>) {
    let not = |d: Day| (d == Day::NotADate).then(|| tl!("Not a date"));
    let (a, b) = (day(from), day(to));
    let backwards = matches!((a, b), (Day::Date(a), Day::Date(b)) if b < a);
    (not(a), not(b).or(backwards.then(|| tl!("Ends before it starts"))))
}

impl Panel {
    /// The panel for the box's query, the scope in view, and the mailbox open.
    fn of(app: &MailApp) -> Panel {
        let reading = Reading::of(&app.session.view.search);
        let fields = reading.fields.clone();
        Panel {
            received_from: fields.received_from.map(|d| d.iso()).unwrap_or_default(),
            received_to: fields.received_to.map(|d| d.iso()).unwrap_or_default(),
            held: Filters { cc: !fields.cc.is_empty(), read: fields.read.is_some(), importance: fields.importance.is_some() },
            scope: app.session.view.scope,
            openings: app.session.openings(),
            looked: app.session.view.scope,
            fields,
            reading,
        }
    }

    /// The query Search writes; `None` while a day received isn't a date, or the days are the
    /// wrong way round.
    pub(crate) fn query(&self) -> Option<String> {
        if day_problems(&self.received_from, &self.received_to) != (None, None) {
            return None;
        }
        let date = |text: &str| if let Day::Date(d) = day(text) { Some(d) } else { None };
        let fields = Advanced { received_from: date(&self.received_from), received_to: date(&self.received_to), ..self.fields.clone() };
        Some(self.reading.write(&fields))
    }

    /// Whether a filter of Add filters shows: ticked, or held by the box.
    fn shows(&self, filters: Filters, which: Filter) -> bool {
        which.of(filters) || which.of(self.held)
    }
}

/// Whether the panel may open now: in Mail, with the search box drawn, and nothing over the
/// window.
fn can_open(app: &MailApp) -> Result<(), &'static str> {
    if app.session.view.module != Module::Mail || !crate::keys::takes_commands(app) {
        return Err("Advanced Search is Mail's: it opens with Mail in front and nothing over the window");
    }
    if !app.search_box_shown {
        return Err("the window is too narrow for the search box");
    }
    Ok(())
}

/// Opens the panel, or closes it (the button in the search box). The list of suggestions under the
/// box closes when it opens.
pub(crate) fn set_open(app: &mut MailApp, open: bool) -> Result<(), &'static str> {
    if open && app.advanced_search.is_none() {
        can_open(app)?;
        app.advanced_search = Some(Panel::of(app));
        if let Some(ctx) = &app.ctx {
            crate::search_suggest::Suggesting::close(ctx);
        }
    } else if !open {
        app.advanced_search = None;
    }
    Ok(())
}

/// Searches with what the open panel says, in where it says, and closes it.
pub(crate) fn search(app: &mut MailApp) -> Result<(), String> {
    let panel = app.advanced_search.as_ref().ok_or("Advanced Search isn't open")?;
    let query = panel.query().ok_or("a day received isn't a date, or the days are the wrong way round: write them YYYY-MM-DD")?;
    let scope = crate::chrome::scopes().into_iter().find(|(s, ..)| *s == panel.scope).map_or("folder", |(_, _, code)| code);
    app.execute("view.search", json!({"query": query, "scope": scope}))?;
    // Closed once it has searched: refused, it stays open with what was typed in it.
    app.advanced_search = None;
    // What the search took (it may have cut it short): the box shows that.
    app.search_text = app.session.view.search.clone();
    Ok(())
}

/// `ui.advancedSearch`: `{"open"?: bool, "search"?: bool}`.
pub(crate) fn command(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    if !p.is_object() && !p.is_null() {
        return Err("the parameters are an object: {\"open\"?: bool, \"search\"?: bool}".into());
    }
    let flag = |key: &str| match p.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(format!("`{key}` is true or false")),
    };
    let (open, search) = (flag("open")?, flag("search")?);
    match (open, search) {
        (Some(_), Some(_)) => return Err("`open` and `search` don't go together".into()),
        (_, Some(true)) => self::search(app)?,
        (_, Some(false)) => {}
        (open, None) => {
            let open = open.unwrap_or(app.advanced_search.is_none());
            set_open(app, open)?;
        }
    }
    Ok(json!({"open": app.advanced_search.is_some(), "query": app.session.view.search}))
}

/// Closes the panel when what it was opened for has gone: the search or where it looks changed
/// under it (a command, an agent, typing in the box), Mail left the front, File or a dialog
/// opened, another mailbox was opened, or the window became too narrow for the search box. Once
/// a frame, before anything is drawn.
pub(crate) fn follow(app: &mut MailApp) {
    let Some(panel) = &app.advanced_search else { return };
    let gone = panel.reading.query() != app.session.view.search
        || panel.looked != app.session.view.scope
        || !app.search_box_shown
        || panel.openings != app.session.openings()
        || app.session.view.module != Module::Mail
        || !crate::keys::takes_commands(app);
    if gone {
        app.advanced_search = None;
    }
}

/// Escape for the open panel: it closes it, and the keyboard goes back to the search box. Asked
/// before the window's shortcuts and the search box ask, so that Escape closes the panel and
/// leaves the search. (Escape in a
/// list open in the panel is the list's: the window's keys give it to egui to close the list.)
pub(crate) fn escape(app: &mut MailApp, ctx: &egui::Context) {
    if app.advanced_search.is_some() && crate::keys::take_escape_any(ctx, crate::keys::main_layer()) {
        app.advanced_search = None;
        app.search_focus = true;
    }
}

fn panel_id() -> Id {
    Id::unique("advanced_search")
}

/// A field of the panel, named by `label` for screen readers; Enter in it searches.
fn field(ui: &mut Ui, label: egui::Id, id: &str, value: &mut String, width: f32, hint: &str, enter: &mut bool) -> Response {
    let resp = ui.add(egui::TextEdit::singleline(value).id_salt(id).desired_width(width).hint_text(hint.to_owned())).labelled_by(label);
    if resp.lost_focus() && crate::keys::enter(ui.ctx(), ui.layer_id()) {
        *enter = true;
    }
    resp
}

/// A choice of `options` (value, label) in a drop-down `width` wide, named by `label`.
fn choice<T: Clone + PartialEq>(ui: &mut Ui, label: egui::Id, id: &str, value: &mut T, options: &[(T, &str)], width: f32) {
    let shown = options.iter().find(|(v, _)| v == value).map_or("", |(_, l)| *l);
    let resp = egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for (v, l) in options {
                ui.selectable_value(value, v.clone(), *l);
            }
        })
        .response;
    resp.labelled_by(label);
}

/// One row of the panel: its label beside the field, in a column `label_w` wide, or above it
/// when the panel is narrow (`None`).
fn row(ui: &mut Ui, label_w: Option<f32>, label: &str, add: impl FnOnce(&mut Ui, egui::Id)) {
    let Some(label_w) = label_w else {
        let id = ui.label(label).id;
        add(ui, id);
        ui.add_space(4.0);
        return;
    };
    ui.horizontal(|ui| {
        let id = ui.add_sized(vec2(label_w, 22.0), egui::Label::new(label).halign(egui::Align::LEFT)).id;
        add(ui, id);
    });
    ui.add_space(4.0);
}

/// Draws the open panel under the search box (`anchor`); `button` is the button that opens and
/// closes it. Search (or Enter in a field) runs the search; a click outside it closes it.
pub(crate) fn show(app: &mut MailApp, ctx: &egui::Context, anchor: Rect, button: Rect) {
    let Some(mut panel) = app.advanced_search.take() else { return };
    let t = Tokens::get(ctx);
    let names: Vec<String> = app.session.mailbox.categories.iter().map(|c| c.name.clone()).collect();
    let today = app.session.today();
    let today_iso = today.iso();
    let week_start = app.session.prefs.week_start;
    let mut filters = app.ui.advanced_filters;
    let (mut search, mut clear, mut enter) = (false, false, false);
    let problems = day_problems(&panel.received_from, &panel.received_to);
    let ok = problems == (None, None);
    // Taken before drawing: a list in the panel that a click closes in this frame was open.
    let list_open = egui::Popup::is_any_open(ctx);
    let screen = ctx.content_rect();
    let width = PANEL_W.max(anchor.width()).min((screen.width() - 16.0).max(0.0));
    let x = anchor.min.x.min(screen.max.x - 8.0 - width).max(screen.min.x + 8.0);
    let top = anchor.max.y + 2.0;
    let narrow = width < NARROW;
    let layer = LayerId::new(Order::Background, panel_id());
    // Just over the main window, as a part of it: under its dialogs and lists.
    ctx.set_sublayer(LayerId::background(), layer);
    let area = Area::new(panel_id()).order(Order::Background).fixed_pos(pos2(x, top)).show(ctx, |ui| {
        Frame::popup(ui.style()).fill(t.menu).stroke(Stroke::new(1.0, t.border_strong)).inner_margin(12.0).show(ui, |ui| {
            ui.set_width(width - 24.0);
            // The labels' column is as wide as the widest of them, in any language; when that
            // leaves too little for the fields, the labels go above them.
            let labels = [
                tl!("Search in"),
                tl!("From"),
                tl!("To"),
                tl!("Cc"),
                tl!("Subject"),
                tl!("Date received"),
                tl!("Read status"),
                tl!("Attachments"),
                tl!("Attachment keywords"),
                tl!("Categories"),
                tl!("Importance"),
                tl!("Body"),
            ];
            let font = egui::TextStyle::Body.resolve(ui.style());
            let widest = labels
                .iter()
                .map(|l| ui.ctx().fonts_mut(|f| f.layout_no_wrap((*l).to_string(), font.clone(), egui::Color32::WHITE).size().x))
                .fold(0.0_f32, f32::max);
            let label_w = (!narrow && widest + 4.0 <= ui.available_width() * LABEL_SHARE).then_some(widest + 4.0);
            let field_w = match label_w {
                Some(w) => (ui.available_width() - w - 8.0).max(80.0),
                None => ui.available_width(),
            };
            egui::ScrollArea::vertical().max_height((screen.max.y - top - 80.0).max(120.0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
                row(ui, label_w, tl!("Search in"), |ui, l| {
                    let scopes: Vec<(Scope, &str)> = crate::chrome::scopes().into_iter().map(|(s, name, _)| (s, name)).collect();
                    choice(ui, l, "advanced_scope", &mut panel.scope, &scopes, field_w);
                });
                row(ui, label_w, tl!("From"), |ui, l| {
                    field(ui, l, "advanced_from", &mut panel.fields.from, field_w, "", &mut enter);
                });
                row(ui, label_w, tl!("To"), |ui, l| {
                    field(ui, l, "advanced_to", &mut panel.fields.to, field_w, "", &mut enter);
                });
                if panel.shows(filters, Filter::Cc) {
                    row(ui, label_w, tl!("Cc"), |ui, l| {
                        field(ui, l, "advanced_cc", &mut panel.fields.cc, field_w, "", &mut enter);
                    });
                }
                row(ui, label_w, tl!("Subject"), |ui, l| {
                    field(ui, l, "advanced_subject", &mut panel.fields.subject, field_w, "", &mut enter);
                });
                row(ui, label_w, tl!("Date received"), |ui, l| {
                    ui.vertical(|ui| {
                        // Each day with its calendar (#329); the two on one line when they fit.
                        let button = 26.0;
                        let side = ((field_w - 24.0) / 2.0 - button).max(60.0);
                        let one_line = side >= 100.0;
                        let mut day = |ui: &mut Ui, id: &str, text: &mut String, problem: Option<&str>, width: f32, name: &str| {
                            let resp = field(ui, l, id, text, width, &today_iso, &mut enter);
                            // (What is wrong is said in words under the two, for screen readers too.)
                            if problem.is_some() {
                                ui.painter().rect_stroke(resp.rect, 4.0, Stroke::new(1.0, t.red), StrokeKind::Inside);
                            }
                            let calendar = Calendar { field: resp.id, current: full_date(text), today, week_start, offer_none: true, name };
                            match calendar.show(ui) {
                                Some(Picked::Day(d)) => *text = d.iso(),
                                Some(Picked::None) => text.clear(),
                                None => {}
                            }
                        };
                        let (first, last) = (tl!("Pick the first day"), tl!("Pick the last day"));
                        if one_line {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                day(ui, "advanced_received_from", &mut panel.received_from, problems.0, side, first);
                                ui.add_space(4.0);
                                ui.label("–");
                                ui.add_space(4.0);
                                day(ui, "advanced_received_to", &mut panel.received_to, problems.1, side, last);
                            });
                        } else {
                            for (id, text, problem, name) in [
                                ("advanced_received_from", &mut panel.received_from, problems.0, first),
                                ("advanced_received_to", &mut panel.received_to, problems.1, last),
                            ] {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 2.0;
                                    day(ui, id, text, problem, (field_w - button).max(60.0), name);
                                });
                            }
                        }
                        for problem in [problems.0, problems.1].into_iter().flatten() {
                            ui.label(egui::RichText::new(problem).color(t.red).font(regular(11.5)));
                        }
                    });
                });
                if panel.shows(filters, Filter::Read) {
                    row(ui, label_w, tl!("Read status"), |ui, l| {
                        let options = [(None, tl!("All")), (Some(false), tl!("Unread")), (Some(true), tl!("Read"))];
                        choice(ui, l, "advanced_read", &mut panel.fields.read, &options, field_w);
                    });
                }
                row(ui, label_w, tl!("Attachments"), |ui, l| {
                    let options = [(None, ""), (Some(true), tl!("Yes")), (Some(false), tl!("No"))];
                    choice(ui, l, "advanced_attachments", &mut panel.fields.attachments, &options, field_w);
                });
                row(ui, label_w, tl!("Attachment keywords"), |ui, l| {
                    field(ui, l, "advanced_attachment", &mut panel.fields.attachment, field_w, "", &mut enter);
                });
                row(ui, label_w, tl!("Categories"), |ui, l| {
                    let mut options: Vec<(String, &str)> = vec![(String::new(), "")];
                    options.extend(names.iter().map(|n| (n.clone(), n.as_str())));
                    // A category typed into the box that the mailbox doesn't list stays a choice.
                    let typed = panel.fields.category.clone();
                    if !typed.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(&typed)) {
                        options.push((typed.clone(), typed.as_str()));
                    }
                    let mut picked = names.iter().find(|n| n.eq_ignore_ascii_case(&typed)).cloned().unwrap_or(typed.clone());
                    let was = picked.clone();
                    choice(ui, l, "advanced_category", &mut picked, &options, field_w);
                    // (Read as typed, `category:red`, unless another is picked.)
                    if picked != was {
                        panel.fields.category = picked;
                    }
                });
                if panel.shows(filters, Filter::Importance) {
                    row(ui, label_w, tl!("Importance"), |ui, l| {
                        let options = [
                            (None, ""),
                            (Some(Importance::Low), tl!("Low")),
                            (Some(Importance::Normal), tl!("Normal")),
                            (Some(Importance::High), tl!("High")),
                        ];
                        choice(ui, l, "advanced_importance", &mut panel.fields.importance, &options, field_w);
                    });
                }
                row(ui, label_w, tl!("Body"), |ui, l| {
                    field(ui, l, "advanced_body", &mut panel.fields.body, field_w, "", &mut enter);
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.menu_button(format!("+ {}", tl!("Add filters")), |ui| {
                    for (label, which) in [(tl!("Cc"), Filter::Cc), (tl!("Read status"), Filter::Read), (tl!("Importance"), Filter::Importance)] {
                        let mut on = panel.shows(filters, which);
                        if ui.checkbox(&mut on, label).changed() {
                            which.set(&mut filters, on);
                            // Taken out of the panel, a filter asks for nothing any more.
                            if !on {
                                which.set(&mut panel.held, false);
                                which.clear(&mut panel.fields);
                            }
                        }
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    clear = ui.add(egui::Button::new(tl!("Clear filters")).min_size(vec2(0.0, 26.0))).clicked();
                    search = crate::widgets::primary_button(ui, tl!("Search"), ok).clicked();
                });
            });
        });
    });
    let rect = area.response.rect;
    app.hit.add("advancedSearch", 0, rect);
    app.ui.advanced_filters = filters;
    if clear {
        panel.fields = Advanced::default();
        panel.received_from.clear();
        panel.received_to.clear();
    }
    if (search || enter) && panel.query().is_some() {
        app.advanced_search = Some(panel);
        let _ = self::search(app);
        return;
    }
    // A click outside the panel (and its lists) closes it.
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !button.contains(p)));
    if !(outside && !list_open) {
        app.advanced_search = Some(panel);
    }
}

#[cfg(test)]
#[path = "advanced_search_tests.rs"]
mod advanced_search_tests;
