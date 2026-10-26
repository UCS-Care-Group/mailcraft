// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tasks: the filters, the to-do list and the selected task's details.
//!
//! The lists are sorted once per change of the tasks, the day or the filter (not once per frame),
//! and only the rows in view are painted.

use std::sync::Arc;

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::cmd::tasks;
use mailcraft_engine::{TaskFilter, TaskSort};
use mailcraft_model::{Id, Importance, Task};
use mailcraft_time::{Date, Weekday};
use serde_json::json;

use crate::people_view::{
    Layout, by_category, chips, choose_copy, chosen_row, clip_chars, listed, one_line, row_selected, section, selection_changed, show_rows,
};
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

/// Where the task list keeps which row of the selected task has the selection.
const TASK_ROW: &str = "task_list_row";

const ROW_H: f32 = 36.0;
/// Height of the quick-add row above the list.
const ADD_H: f32 = 44.0;
/// Where a row's title starts, right of the check box.
const TITLE_X: f32 = 44.0;
/// Where the values start in the details pane's field list.
const LABEL_W: f32 = 112.0;
/// Characters of notes the details pane lays out; longer notes end with an ellipsis.
const NOTES_MAX: usize = 20_000;

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let before = lists(app, ui.ctx());
    keyboard(app, ui, &before.layout);
    // Space may have ticked a task off: the lists follow in the same frame (nothing is sorted
    // again when nothing changed).
    let lists = lists(app, ui.ctx());
    filter_pane(app, ui, &lists.counts);
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    if let Some(task) = app.session.sel.task.and_then(|id| app.session.mailbox.tasks.get(&id).cloned()) {
        let max = (ui.available_width() - 320.0).max(260.0);
        egui::Panel::right("task_details")
            .default_size(340.0f32.min(max))
            .size_range(260.0..=max)
            .resizable(true)
            .frame(frame)
            .show(ui, |ui| details(app, ui, &task));
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| list(app, ui, &lists));
}

// ---- grouping ----

/// The heading a task sits under, in the order the list shows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bucket {
    Overdue,
    Today,
    Tomorrow,
    ThisWeek,
    Later,
    NoDate,
    Completed,
}

impl Bucket {
    /// Where a task goes, from its due date seen from `today`. This week runs to the end of the
    /// week that starts on `week_start`; a finished task is `Completed` whatever its date.
    fn of(due: Option<Date>, complete: bool, today: Date, week_start: Weekday) -> Bucket {
        if complete {
            return Bucket::Completed;
        }
        let Some(due) = due else { return Bucket::NoDate };
        match today.days_until(due) {
            n if n < 0 => Bucket::Overdue,
            0 => Bucket::Today,
            1 => Bucket::Tomorrow,
            _ if due <= today.start_of_week(week_start).add_days(6) => Bucket::ThisWeek,
            _ => Bucket::Later,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Bucket::Overdue => "Overdue",
            Bucket::Today => "Today",
            Bucket::Tomorrow => "Tomorrow",
            Bucket::ThisWeek => "This Week",
            Bucket::Later => "Later",
            Bucket::NoDate => "No Date",
            Bucket::Completed => "Completed",
        }
    }
}

/// The Due menu: a label and the date it sets (`None` clears it). This week and next week mean
/// their Fridays, as on the ribbon.
fn due_choices(today: Date) -> [(&'static str, Option<Date>); 5] {
    let to_friday = (4 - i64::from(today.weekday().index())).max(0);
    [
        ("Today", Some(today)),
        ("Tomorrow", Some(today.add_days(1))),
        ("This Week", Some(today.add_days(to_friday))),
        ("Next Week", Some(today.add_days(to_friday + 7))),
        ("No Date", None),
    ]
}

/// When a task is due, as a row says it; empty without a due date.
fn due_text(task: &Task, today: Date) -> String {
    if task.due.is_none() {
        return String::new();
    }
    let label = task.due_label(today);
    // Words (`Today`, `Friday`) are interface text; a date stays as it is.
    if label.chars().any(|c| c.is_ascii_digit()) { label } else { tl!(&label).to_string() }
}

/// The line under the title in the details pane, and whether it is a warning. `completed_on` is
/// the local day the task was finished.
fn status_text(task: &Task, today: Date, completed_on: Option<Date>) -> (String, bool) {
    let fmt = crate::i18n::fmt;
    match (completed_on, task.due) {
        (Some(day), _) => (fmt(tl!("Completed on {date}"), &[("date", &crate::dates::medium(day))]), false),
        (None, None) => (tl!("No due date").to_string(), false),
        (None, Some(_)) if task.is_overdue(today) => (fmt(tl!("Overdue: was due {when}"), &[("when", &due_text(task, today))]), true),
        (None, Some(_)) => (fmt(tl!("Due {when}"), &[("when", &due_text(task, today))]), false),
    }
}

fn importance_label(importance: Importance) -> &'static str {
    match importance {
        Importance::High => "High",
        Importance::Normal => "Normal",
        Importance::Low => "Low",
    }
}

/// The title a row and the details pane show.
fn shown_title(task: &Task) -> String {
    let title = one_line(&task.title, 300);
    if title.is_empty() { tl!("(no title)").to_string() } else { title }
}

/// The check box's click target inside a row: larger than the circle drawn in it.
fn check_rect(row: Rect) -> Rect {
    Rect::from_center_size(pos2(row.min.x + 24.0, row.center().y), vec2(30.0, row.height().min(30.0)))
}

fn filter_icon(filter: TaskFilter) -> &'static str {
    match filter {
        TaskFilter::Active => "tasks",
        TaskFilter::Today => "today",
        TaskFilter::Overdue => "clock",
        TaskFilter::Completed => "complete",
        TaskFilter::All => "list",
    }
}

// ---- the lists ----

/// What the panes show, kept between frames with what it was made from.
#[derive(Clone)]
struct Lists {
    key: (u64, usize, i64, TaskFilter, Weekday, TaskSort),
    /// How many tasks each filter shows, in `TaskFilter::ALL` order.
    counts: [usize; 5],
    /// The list for the filter in view.
    layout: Arc<Layout<Heading>>,
    /// The names of the categories its headings stand for, arranged by categories.
    names: Arc<Vec<String>>,
}

/// A heading of the task list: when the tasks under it are due, or, arranged by categories, a
/// category (an index into [`Lists::names`]) or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heading {
    Due(Bucket),
    Category(u32),
    NoCategory,
}

#[cfg(test)]
impl Heading {
    /// When the tasks under it are due, for a heading arranged by due date.
    fn due(self) -> Option<Bucket> {
        match self {
            Heading::Due(bucket) => Some(bucket),
            Heading::Category(_) | Heading::NoCategory => None,
        }
    }
}

/// The counts beside the filters and the list in view. Sorting every task five times is the slow
/// part, so the result is kept until the mailbox, the day or the filter changes.
fn lists(app: &MailApp, ctx: &egui::Context) -> Lists {
    let slot = egui::Id::unique("task_lists");
    let today = app.session.today();
    let filter = app.session.view.task_filter;
    let week_start = app.session.prefs.week_start;
    let sort = app.session.view.task_sort;
    let key = (app.session.rev(), app.session.mailbox.tasks.len(), today.to_days(), filter, week_start, sort);
    if let Some(cached) = ctx.data(|d| d.get_temp::<Lists>(slot)).filter(|c| c.key == key) {
        return cached;
    }
    let s = &app.session;
    let mut counts = [0usize; 5];
    let mut layout = Layout::new(ROW_H, []);
    let mut names: Vec<String> = Vec::new();
    for (count, f) in counts.iter_mut().zip(TaskFilter::ALL) {
        let ids = tasks::sorted(s, f);
        *count = ids.len();
        if f != filter {
            continue;
        }
        let shown = ids.iter().filter_map(|id| s.mailbox.task(*id));
        layout = match sort {
            TaskSort::DueDate => Layout::new(ROW_H, shown.map(|x| (x.id, Heading::Due(Bucket::of(x.due, x.is_complete(), today, week_start))))),
            // As Outlook lists them: a task under each of its categories, those with none first,
            // then the categories A to Z, by when each is due under each (the order they come in).
            TaskSort::Categories => {
                let (rows, filed) = by_category(s, shown.map(|x| (x.id, x.categories.as_slice())));
                names = filed;
                Layout::new(ROW_H, rows.into_iter().map(|(id, i)| (id, i.map_or(Heading::NoCategory, Heading::Category))))
            }
        };
    }
    let lists = Lists { key, counts, layout: Arc::new(layout), names: Arc::new(names) };
    ctx.data_mut(|d| d.insert_temp(slot, lists.clone()));
    lists
}

// ---- keyboard ----

/// Up and Down move the selection, Space ticks the task off and Enter opens it, unless a text
/// field, a menu or a dialog has the keyboard.
fn keyboard(app: &mut MailApp, ui: &Ui, layout: &Layout<Heading>) {
    if app.dialog.is_some() || ui.ctx().egui_wants_keyboard_input() || egui::Popup::is_any_open(ui.ctx()) || !ui.input(|i| i.modifiers.is_none()) {
        return;
    }
    let none = egui::Modifiers::NONE;
    let (down, up, space) = ui.input_mut(|i| {
        (
            i.count_and_consume_key(none, egui::Key::ArrowDown),
            i.count_and_consume_key(none, egui::Key::ArrowUp),
            i.consume_key(none, egui::Key::Space),
        )
    });
    let step = down.min(1000) as i64 - up.min(1000) as i64;
    let selected = app.session.sel.task;
    let at = chosen_row(ui.ctx(), TASK_ROW, layout, selected);
    if step != 0
        && let Some((id, copy)) = layout.neighbour_row(at, step)
        && at != Some((id, copy))
    {
        choose_copy(ui.ctx(), TASK_ROW, id, copy);
        if selected != Some(id) {
            let _ = app.run("tasks.select", json!({"id": id}));
        }
    }
    if space && app.session.sel.task.is_some() {
        let _ = app.run("tasks.complete", json!({}));
    }
    // Only a new press that is the main window's: not the repeats of an Enter that closed a dialog
    // (#406).
    if app.session.sel.task.is_some() && crate::keys::take_enter_alone(ui.ctx(), ui.layer_id()) {
        let _ = app.run("tasks.edit", json!({}));
    }
}

// ---- the filter pane ----

fn filter_pane(app: &mut MailApp, ui: &mut Ui, counts: &[usize; 5]) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border));
    egui::Panel::left("task_filters").default_size(200.0).size_range(160.0..=320.0).resizable(true).frame(frame).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        ui.add_space(6.0);
        widgets::pane_heading(ui, tl!("My Tasks"));
        for (filter, count) in TaskFilter::ALL.into_iter().zip(counts.iter().copied()) {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
            let current = app.session.view.task_filter == filter;
            let row = r.shrink2(vec2(4.0, 1.0));
            if current {
                ui.painter().rect_filled(row, 4.0, t.selection);
            } else if resp.hovered() {
                ui.painter().rect_filled(row, 4.0, t.hover);
            }
            icons::paint(
                ui.painter(),
                Rect::from_center_size(pos2(r.min.x + 22.0, r.center().y), vec2(16.0, 16.0)),
                filter_icon(filter),
                t.icon,
                t.accent,
            );
            // What is late is worth a second look.
            let warn = filter == TaskFilter::Overdue && count > 0;
            let number = if count > 0 { count.to_string() } else { String::new() };
            let number_w = ui
                .painter()
                .text(pos2(r.max.x - 14.0, r.center().y), Align2::RIGHT_CENTER, &number, semibold(11.5), if warn { t.red } else { t.text_dim })
                .width();
            let x = r.min.x + 38.0;
            paint_clipped(
                ui.painter(),
                pos2(x, r.center().y),
                (r.max.x - 22.0 - number_w - x).max(0.0),
                tl!(filter.label()),
                if current { semibold(12.5) } else { regular(12.5) },
                t.text,
            );
            if resp.clicked() {
                let _ = app.run("view.taskFilter", json!({"value": filter}));
            }
        }
    });
}

// ---- the list ----

fn list(app: &mut MailApp, ui: &mut Ui, lists: &Lists) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let filter = app.session.view.task_filter;
    crate::chrome::pane_title(ui, tl!(filter.label()), &listed(&lists.layout).to_string());
    quick_add(app, ui);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment([pos2(r.min.x, r.min.y), pos2(r.max.x, r.min.y)], Stroke::new(1.0, t.border));
    if lists.layout.rows.is_empty() {
        let done = filter == TaskFilter::Completed;
        ui.vertical_centered(|ui| {
            ui.add_space(56.0);
            let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
            icons::paint(ui.painter(), r, "tasks", t.border_strong, t.border_strong);
            ui.add_space(10.0);
            let text = if done { tl!("No completed tasks yet.") } else { tl!("Nothing to do here.") };
            ui.label(RichText::new(text).font(regular(12.5)).color(t.text_dim));
        });
        return;
    }
    let selected = app.session.sel.task;
    let at = chosen_row(ui.ctx(), TASK_ROW, &lists.layout, selected);
    let reveal = selection_changed(ui, "task_list_seen", at).then_some(at).flatten();
    let today = app.session.today();
    let names = lists.names.clone();
    let label = move |h: Heading| match h {
        Heading::Due(bucket) => tl!(bucket.label()).to_string(),
        Heading::Category(i) => names.get(i as usize).map_or_else(String::new, |n| one_line(n, 200)),
        Heading::NoCategory => tl!("No category").to_string(),
    };
    show_rows(ui, &lists.layout, reveal, label, |ui, id, copy, rect| task_row(app, ui, id, copy, rect, today));
}

/// The field above the list: type a title, press Enter, and the task exists.
fn quick_add(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let filter = app.session.view.task_filter;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ADD_H), Sense::hover());
    let field = Rect::from_min_max(pos2(r.min.x + 10.0, r.min.y + 4.0), pos2(r.max.x - 10.0, r.max.y - 8.0));
    let id = egui::Id::unique("task_quick_add");
    let focused = ui.memory(|m| m.has_focus(id));
    ui.painter().rect(
        field,
        6.0,
        if focused { t.input } else { t.panel },
        Stroke::new(1.0, if focused { t.accent } else { t.border }),
        egui::StrokeKind::Inside,
    );
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(field.min.x + 16.0, field.center().y), vec2(12.0, 12.0)),
        "plus",
        if focused { t.accent_text } else { t.text_dim },
        t.accent,
    );
    let edit_rect = Rect::from_min_max(pos2(field.min.x + 30.0, field.min.y + 1.0), pos2(field.max.x - 6.0, field.max.y - 1.0));
    // A pane squeezed too narrow to type in shows the empty field.
    if edit_rect.width() < 24.0 || edit_rect.height() < 12.0 {
        return;
    }
    // What has been typed so far lives in egui's memory, not in the session.
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
    let hint = if filter == TaskFilter::Today { tl!("Add a task due today") } else { tl!("Add a task") };
    let edit = egui::TextEdit::singleline(&mut text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(hint)
        .font(regular(12.5))
        .char_limit(500)
        .desired_width(edit_rect.width())
        .min_size(vec2(0.0, edit_rect.height()))
        .align(egui::Align2::LEFT_CENTER);
    // In a child of its own, so the field doesn't move what is laid out after this row.
    let resp = ui.new_child(egui::UiBuilder::new().max_rect(edit_rect)).add(edit);
    // Enter and Escape both take the keyboard away from the field: Enter adds, Escape gives up.
    // (Each is the window's whose they are, #406.)
    let (enter, escape) = (crate::keys::enter(ui.ctx(), ui.layer_id()), crate::keys::escape(ui.ctx(), ui.layer_id()));
    if resp.lost_focus() && enter {
        let title = text.trim().to_string();
        if title.is_empty() {
            text.clear();
        } else {
            let params =
                if filter == TaskFilter::Today { json!({"title": title, "due": app.session.today().iso()}) } else { json!({"title": title}) };
            if app.run("tasks.new", params).is_ok() {
                text.clear();
                // A new task is open and undated: show it where it is listed.
                if matches!(filter, TaskFilter::Overdue | TaskFilter::Completed) {
                    let _ = app.execute("view.taskFilter", json!({"value": TaskFilter::Active}));
                }
            }
        }
        // Stay in the field for the next one.
        resp.request_focus();
    } else if resp.lost_focus() && escape {
        text.clear();
    }
    ui.data_mut(|d| d.insert_temp(id, text));
}

/// One line of text cut to `width` with an ellipsis, struck through when `strike`.
fn paint_title(p: &egui::Painter, pos: egui::Pos2, width: f32, text: &str, font: FontId, color: Color32, strike: bool) {
    if width <= 4.0 || text.is_empty() {
        return;
    }
    let format = egui::text::TextFormat {
        font_id: font,
        color,
        strikethrough: if strike { Stroke::new(1.0, color) } else { Stroke::NONE },
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), format);
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    p.galley(pos2(pos.x, pos.y - galley.size().y / 2.0), galley, color);
}

/// The round check box: filled with a tick when done, an outline (a faint tick under the pointer)
/// when not.
fn paint_check(p: &egui::Painter, center: egui::Pos2, done: bool, hot: bool, t: &Tokens) {
    let tick = |color: Color32, width: f32| {
        p.add(Shape::line(
            vec![pos2(center.x - 3.8, center.y + 0.3), pos2(center.x - 1.2, center.y + 2.9), pos2(center.x + 4.0, center.y - 2.9)],
            Stroke::new(width, color),
        ));
    };
    if done {
        p.circle_filled(center, 8.5, if hot { t.accent_text } else { t.accent });
        tick(t.on_accent, 1.6);
    } else {
        p.circle_stroke(center, 8.0, Stroke::new(1.4, if hot { t.accent } else { t.text_disabled }));
        if hot {
            tick(t.accent, 1.4);
        }
    }
}

fn task_row(app: &mut MailApp, ui: &mut Ui, id: Id, copy: u16, rect: Rect, today: Date) {
    let t = Tokens::get(ui.ctx());
    // The shared pointer, not a copy of the task.
    let Some(task) = app.session.mailbox.tasks.get(&id).cloned() else { return };
    // A task listed under two categories is two rows, each with ids of its own.
    let resp = ui.interact(rect, ui.scope_id().with(("task", id, copy)), Sense::click());
    app.hit.add("task", id, rect);
    // The check box has its own click target, on top of the row's.
    let check = check_rect(rect);
    let check_resp = ui.interact(check, ui.scope_id().with(("task_check", id, copy)), Sense::click());
    // The selection is on one of a task's rows: the one clicked or stepped to.
    let selected = row_selected(ui.ctx(), TASK_ROW, app.session.sel.task, (id, copy));
    if resp.clicked() || resp.secondary_clicked() {
        choose_copy(ui.ctx(), TASK_ROW, id, copy);
    }
    let done = task.is_complete();
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 0.0, t.selection);
    } else if resp.hovered() || check_resp.hovered() {
        p.rect_filled(rect, 0.0, t.hover.linear_multiply(0.6));
    }
    p.line_segment([pos2(rect.min.x + TITLE_X, rect.max.y), pos2(rect.max.x, rect.max.y)], Stroke::new(1.0, t.border.linear_multiply(0.7)));
    paint_check(p, check.center(), done, check_resp.hovered(), &t);

    // From the right: when it is due, then the markers.
    let y = rect.center().y;
    let mut x = rect.max.x - 12.0;
    let due = due_text(&task, today);
    if !due.is_empty() {
        let color = if task.is_overdue(today) { t.red } else { t.text_dim };
        x -= p.text(pos2(x, y), Align2::RIGHT_CENTER, &due, regular(11.5), color).width() + 8.0;
    }
    let mut marker = |icon: &str, color: Color32| {
        icons::paint(p, Rect::from_center_size(pos2(x - 7.0, y), vec2(12.0, 12.0)), icon, color, t.accent);
        x -= 17.0;
    };
    match task.importance {
        Importance::High => marker("highImportance", t.icon),
        Importance::Low => marker("lowImportance", t.icon),
        Importance::Normal => {}
    }
    if task.message.is_some() {
        marker("mail", t.text_dim);
    }
    for c in task.categories.iter().take(3) {
        p.rect_filled(Rect::from_center_size(pos2(x - 5.0, y), vec2(10.0, 10.0)), 2.0, crate::theme::c32(app.session.mailbox.category_color(c)));
        x -= 13.0;
    }
    let left = rect.min.x + TITLE_X;
    let (font, color) = if done {
        (regular(13.0), t.text_dim)
    } else if selected {
        (semibold(13.0), t.text)
    } else {
        (regular(13.0), t.text)
    };
    paint_title(p, pos2(left, y), (x - left - 8.0).max(0.0), &shown_title(&task), font, color, done);

    if check_resp.clicked() {
        let _ = app.run("tasks.complete", json!({"id": id}));
    } else if resp.clicked() {
        let _ = app.run("tasks.select", json!({"id": id}));
    }
    if resp.double_clicked() {
        let _ = app.execute("tasks.select", json!({"id": id}));
        let _ = app.run("tasks.edit", json!({}));
    }
    // A right click acts on the row under the pointer.
    if (resp.secondary_clicked() || check_resp.secondary_clicked()) && !selected {
        let _ = app.execute("tasks.select", json!({"id": id}));
    }
    let importance = task.importance;
    resp.context_menu(|ui| task_menu(app, ui, id, done, importance, today));
    check_resp.context_menu(|ui| task_menu(app, ui, id, done, importance, today));
}

fn task_menu(app: &mut MailApp, ui: &mut Ui, id: Id, done: bool, importance: Importance, today: Date) {
    use widgets::{mi, mi_check};
    ui.set_min_width(200.0);
    mi(ui, app, if done { "Mark Incomplete" } else { "Mark Complete" }, "tasks.complete", json!({"id": id}));
    mi(ui, app, "Edit", "tasks.edit", json!({}));
    ui.separator();
    ui.menu_button(tl!("Due"), |ui| {
        for (label, due) in due_choices(today) {
            mi(ui, app, label, "tasks.edit", json!({"id": id, "due": due.map(Date::iso)}));
        }
    });
    ui.menu_button(tl!("Categorize"), |ui| {
        crate::ribbon::category_menu(
            ui,
            app,
            "tasks.categorize",
            |app, name| app.session.mailbox.task(id).is_some_and(|x| x.categories.iter().any(|c| c.eq_ignore_ascii_case(name))),
            None,
        );
    });
    ui.separator();
    for (label, value) in [("High Importance", Importance::High), ("Normal Importance", Importance::Normal), ("Low Importance", Importance::Low)] {
        mi_check(ui, app, label, importance == value, "tasks.edit", json!({"id": id, "importance": value.code()}));
    }
    ui.separator();
    mi(ui, app, "Delete", "tasks.delete", json!({"id": id}));
}

// ---- the details pane ----

fn details(app: &mut MailApp, ui: &mut Ui, task: &Task) {
    let t = Tokens::get(ui.ctx());
    let id = task.id;
    let today = app.session.today();
    let done = task.is_complete();
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("task_details", id)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 14, top: 12, bottom: 20 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            // The title, with Close on the right.
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if widgets::icon_button(ui, "close", tl!("Close"), 13.0, true).clicked() {
                        let _ = app.run("tasks.select", json!({"id": null}));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        let title = RichText::new(shown_title(task)).font(semibold(16.0)).color(if done { t.text_dim } else { t.text });
                        ui.add(egui::Label::new(if done { title.strikethrough() } else { title }).wrap());
                    });
                });
            });
            let (status, warn) = status_text(task, today, task.completed.map(|at| app.session.local(at).date));
            ui.horizontal(|ui| {
                if done {
                    let (r, _) = ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), "complete", t.green, t.accent);
                }
                ui.label(RichText::new(status).font(regular(12.0)).color(if warn { t.red } else { t.text_dim }));
            });
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if widgets::primary_button(ui, if done { tl!("Mark Incomplete") } else { tl!("Mark Complete") }, true).clicked() {
                    let _ = app.run("tasks.complete", json!({"id": id}));
                }
                // The pane shows the selected task, so Edit opens its form.
                if widgets::icon_text_button(ui, "edit", tl!("Edit"), true).clicked() {
                    let _ = app.run("tasks.edit", json!({}));
                }
                if widgets::icon_text_button(ui, "delete", tl!("Delete"), true).clicked() {
                    let _ = app.run("tasks.delete", json!({"id": id}));
                }
            });

            section(ui, tl!("Details"));
            let none = tl!("None");
            let date = |day: Option<Date>| day.map(Date::medium);
            detail(ui, "calendar", tl!("Start"), date(task.start).as_deref().unwrap_or(none), if task.start.is_some() { t.text } else { t.text_dim });
            let due_color = if task.is_overdue(today) {
                t.red
            } else if task.due.is_some() {
                t.text
            } else {
                t.text_dim
            };
            detail(ui, "today", tl!("Due"), date(task.due).as_deref().unwrap_or(none), due_color);
            let importance_icon = match task.importance {
                Importance::High => "highImportance",
                Importance::Normal => "minus",
                Importance::Low => "lowImportance",
            };
            detail(ui, importance_icon, tl!("Importance"), tl!(importance_label(task.importance)), t.text);
            let reminder = task.reminder.map(|at| {
                let local = app.session.local(at);
                crate::dates::medium_time(local, app.session.prefs.twelve_hour)
            });
            detail(ui, "reminder", tl!("Reminder"), reminder.as_deref().unwrap_or(none), if reminder.is_some() { t.text } else { t.text_dim });
            if task.categories.is_empty() {
                detail(ui, "categorize", tl!("Categories"), none, t.text_dim);
            } else {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    let (r, _) = ui.allocate_exact_size(vec2(LABEL_W, 26.0), Sense::hover());
                    detail_label(ui, r, "categorize", tl!("Categories"));
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        chips(app, ui, &task.categories);
                    });
                });
            }

            if !task.notes.trim().is_empty() {
                section(ui, tl!("Notes"));
                ui.add(egui::Label::new(RichText::new(clip_chars(task.notes.trim(), NOTES_MAX)).font(regular(13.0)).color(t.text)).wrap());
            }
            // A to-do made from a message leads back to it, while the message exists.
            if let Some(m) = task.message.and_then(|m| app.session.mailbox.messages.get(&m).cloned()) {
                section(ui, tl!("Message"));
                let subject = if m.subject.trim().is_empty() { tl!("(no subject)").to_string() } else { one_line(&m.subject, 300) };
                let from = crate::i18n::fmt(tl!("From {name}"), &[("name", &one_line(m.from.display(), 200))]);
                ui.add(egui::Label::new(RichText::new(subject).font(regular(12.5)).color(t.text)).truncate());
                ui.add(egui::Label::new(RichText::new(from).font(regular(11.5)).color(t.text_dim)).truncate());
                ui.add_space(4.0);
                if widgets::icon_text_button(ui, "mail", tl!("Open the message"), true).clicked() {
                    let _ = app.run("mail.select", json!({"id": m.id}));
                    let _ = app.run("view.mail", json!({}));
                }
            }
        });
    });
}

/// The icon and name of a field, at the left of its line.
fn detail_label(ui: &Ui, r: Rect, icon: &str, label: &str) {
    let t = Tokens::get(ui.ctx());
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 9.0, r.center().y), vec2(16.0, 16.0)), icon, t.text_dim, t.accent);
    paint_clipped(ui.painter(), pos2(r.min.x + 26.0, r.center().y), LABEL_W - 30.0, label, regular(12.0), t.text_dim);
}

/// One line of the field list: an icon, the field's name and its value.
fn detail(ui: &mut Ui, icon: &str, label: &str, value: &str, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    detail_label(ui, r, icon, label);
    paint_clipped(ui.painter(), pos2(r.min.x + LABEL_W, r.center().y), (r.width() - LABEL_W).max(0.0), value, regular(12.5), color);
}

#[cfg(test)]
mod arrange_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::people_view::GROUP_H;

    fn day(y: i32, m: u8, d: u8) -> Date {
        Date::new(y, m, d).unwrap()
    }

    /// Wednesday.
    fn today() -> Date {
        day(2026, 10, 14)
    }

    fn task(due: Option<Date>) -> Task {
        Task { title: "Write the release notes".into(), due, ..Default::default() }
    }

    #[test]
    fn tasks_are_grouped_by_when_they_are_due() {
        let of = |due: Date| Bucket::of(Some(due), false, today(), Weekday::Sun);
        assert_eq!(of(day(2026, 10, 13)), Bucket::Overdue);
        assert_eq!(of(day(2020, 1, 1)), Bucket::Overdue);
        assert_eq!(of(today()), Bucket::Today);
        assert_eq!(of(day(2026, 10, 15)), Bucket::Tomorrow);
        // The week of Sunday the 11th ends on Saturday the 17th.
        assert_eq!(of(day(2026, 10, 16)), Bucket::ThisWeek);
        assert_eq!(of(day(2026, 10, 17)), Bucket::ThisWeek);
        assert_eq!(of(day(2026, 10, 18)), Bucket::Later);
        assert_eq!(of(day(2027, 3, 1)), Bucket::Later);
        assert_eq!(Bucket::of(None, false, today(), Weekday::Sun), Bucket::NoDate);
        // Done is done, whenever it was due.
        for due in [None, Some(day(2026, 10, 13)), Some(today()), Some(day(2030, 1, 1))] {
            assert_eq!(Bucket::of(due, true, today(), Weekday::Sun), Bucket::Completed);
        }
    }

    #[test]
    fn this_week_follows_the_first_day_of_the_week() {
        // Weeks from Monday: Sunday the 18th still belongs to this one.
        assert_eq!(Bucket::of(Some(day(2026, 10, 18)), false, today(), Weekday::Mon), Bucket::ThisWeek);
        assert_eq!(Bucket::of(Some(day(2026, 10, 19)), false, today(), Weekday::Mon), Bucket::Later);
        // On the last day of the week, tomorrow is next week but still reads Tomorrow.
        let saturday = day(2026, 10, 17);
        assert_eq!(Bucket::of(Some(day(2026, 10, 18)), false, saturday, Weekday::Sun), Bucket::Tomorrow);
        assert_eq!(Bucket::of(Some(day(2026, 10, 19)), false, saturday, Weekday::Sun), Bucket::Later);
        // Whatever the week, the groups come in the order the engine sorts tasks: by due date.
        for first in Weekday::ALL {
            for start in 0..7 {
                let now = today().add_days(start);
                let buckets: Vec<Bucket> = (-3..40).map(|n| Bucket::of(Some(now.add_days(n)), false, now, first)).collect();
                assert!(buckets.is_sorted(), "{first:?} {now:?}: {buckets:?}");
            }
        }
        assert!(Bucket::Later < Bucket::NoDate && Bucket::NoDate < Bucket::Completed);
    }

    #[test]
    fn dates_at_the_edges_group_without_panicking() {
        let (first, last) = (day(1, 1, 1), day(9999, 12, 31));
        assert_eq!(Bucket::of(Some(last), false, today(), Weekday::Sun), Bucket::Later);
        assert_eq!(Bucket::of(Some(first), false, today(), Weekday::Sun), Bucket::Overdue);
        assert_eq!(Bucket::of(Some(last), false, last, Weekday::Mon), Bucket::Today);
        assert_eq!(Bucket::of(Some(first), false, last, Weekday::Mon), Bucket::Overdue);
        assert_eq!(Bucket::of(Some(last), false, first, Weekday::Sat), Bucket::Later);
        assert_eq!(Bucket::of(Some(first), false, first, Weekday::Sat), Bucket::Today);
        // Dates a damaged file could carry: the fields are public.
        let broken = [Date { year: 0, month: 0, day: 0 }, Date { year: i32::MAX, month: 255, day: 255 }, Date { year: i32::MIN, month: 13, day: 32 }];
        for due in broken {
            for now in broken.into_iter().chain([today(), first, last]) {
                for week in Weekday::ALL {
                    let _ = Bucket::of(Some(due), false, now, week);
                    let _ = Bucket::of(Some(now), false, due, week);
                }
                let _ = due_choices(now);
                let hostile = task(Some(due));
                let _ = (due_text(&hostile, now), status_text(&hostile, now, None), status_text(&hostile, now, Some(due)));
            }
        }
    }

    #[test]
    fn group_headings_have_their_names() {
        let all = [Bucket::Overdue, Bucket::Today, Bucket::Tomorrow, Bucket::ThisWeek, Bucket::Later, Bucket::NoDate, Bucket::Completed];
        let names: Vec<&str> = all.iter().map(|b| b.label()).collect();
        assert_eq!(names, ["Overdue", "Today", "Tomorrow", "This Week", "Later", "No Date", "Completed"]);
        assert!(all.is_sorted());
    }

    #[test]
    fn rows_are_offset_by_the_headings_above_them() {
        let l = Layout::new(ROW_H, [(1, Bucket::Overdue), (2, Bucket::Today), (3, Bucket::Today), (4, Bucket::NoDate), (5, Bucket::Completed)]);
        let tops: Vec<f32> = l.rows.iter().map(|r| r.top).collect();
        assert_eq!(tops, [GROUP_H, 2.0 * GROUP_H + ROW_H, 2.0 * GROUP_H + 2.0 * ROW_H, 3.0 * GROUP_H + 3.0 * ROW_H, 4.0 * GROUP_H + 4.0 * ROW_H]);
        assert_eq!(l.height, 4.0 * GROUP_H + 5.0 * ROW_H);
        let headings: Vec<Option<Bucket>> = l.rows.iter().map(|r| r.heading).collect();
        assert_eq!(headings, [Some(Bucket::Overdue), Some(Bucket::Today), None, Some(Bucket::NoDate), Some(Bucket::Completed)]);
        // A long list lays out only what a window shows.
        let long = Layout::new(ROW_H, (1..=50_000u64).map(|id| (id, Bucket::Later)));
        assert_eq!(long.height, GROUP_H + 50_000.0 * ROW_H);
        assert!(long.visible(900_000.0, 900_800.0).len() <= 26);
        assert_eq!(long.visible(0.0, 100.0).iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(long.neighbour(Some(50_000), 1), Some(50_000));
        assert_eq!(long.neighbour(Some(1), -1), Some(1));
        assert_eq!(long.neighbour(None, 1), Some(1));
    }

    #[test]
    fn the_check_box_sits_inside_its_row_left_of_the_title() {
        let row = Rect::from_min_size(pos2(248.0, 300.0), vec2(600.0, ROW_H));
        let check = check_rect(row);
        assert!(row.contains_rect(check), "{check:?}");
        assert_eq!(check.center(), pos2(272.0, 300.0 + ROW_H / 2.0));
        assert!(check.max.x <= row.min.x + TITLE_X);
        // A row squeezed thin keeps the target inside it.
        let thin = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 10.0));
        assert!(thin.contains_rect(check_rect(thin)));
    }

    #[test]
    fn the_due_menu_sets_the_dates_the_ribbon_does() {
        let dates = |now: Date| due_choices(now).map(|(_, due)| due.map(Date::iso).unwrap_or_default());
        // Wednesday: this week is Friday, next week the Friday after.
        assert_eq!(dates(today()), ["2026-10-14", "2026-10-15", "2026-10-16", "2026-10-23", ""]);
        // Friday is its own end of the week; on the weekend this week means today.
        assert_eq!(dates(day(2026, 10, 16)), ["2026-10-16", "2026-10-17", "2026-10-16", "2026-10-23", ""]);
        assert_eq!(dates(day(2026, 10, 18)), ["2026-10-18", "2026-10-19", "2026-10-18", "2026-10-25", ""]);
        assert_eq!(due_choices(today()).map(|(label, _)| label), ["Today", "Tomorrow", "This Week", "Next Week", "No Date"]);
        // The last day there is: nothing runs past it.
        assert_eq!(dates(day(9999, 12, 31)), ["9999-12-31", "9999-12-31", "9999-12-31", "9999-12-31", ""]);
    }

    #[test]
    fn due_and_status_lines() {
        assert_eq!(due_text(&task(None), today()), "");
        assert_eq!(due_text(&task(Some(today())), today()), "Today");
        assert_eq!(due_text(&task(Some(day(2026, 10, 16))), today()), "Friday");
        assert_eq!(due_text(&task(Some(day(2026, 11, 13))), today()), "Nov 13, 2026");
        assert_eq!(due_text(&task(Some(day(9999, 12, 31))), today()), "Dec 31, 9999");

        assert_eq!(status_text(&task(None), today(), None), ("No due date".to_string(), false));
        assert_eq!(status_text(&task(Some(day(2026, 10, 15))), today(), None), ("Due Tomorrow".to_string(), false));
        assert_eq!(status_text(&task(Some(day(2026, 12, 1))), today(), None), ("Due Dec 1, 2026".to_string(), false));
        assert_eq!(status_text(&task(Some(day(2026, 10, 13))), today(), None), ("Overdue: was due Yesterday".to_string(), true));
        assert_eq!(status_text(&task(Some(day(2026, 10, 8))), today(), None), ("Overdue: was due Oct 8, 2026".to_string(), true));
        // Finished late is finished, not overdue.
        let mut done = task(Some(day(2026, 10, 8)));
        done.completed = Some(1_791_367_200);
        assert_eq!(status_text(&done, today(), Some(day(2026, 10, 7))), ("Completed on Oct 7, 2026".to_string(), false));
    }

    #[test]
    fn titles_stay_on_one_line() {
        assert_eq!(shown_title(&task(None)), "Write the release notes");
        assert_eq!(shown_title(&Task::default()), "(no title)");
        assert_eq!(shown_title(&Task { title: " \n\t ".into(), ..Default::default() }), "(no title)");
        assert_eq!(shown_title(&Task { title: "Call\nLin\u{0}".into(), ..Default::default() }), "Call Lin");
        // However long a title in a file is, a row lays out a bounded piece of it.
        assert_eq!(shown_title(&Task { title: "t".repeat(1_000_000), ..Default::default() }).chars().count(), 301);
        assert_eq!(importance_label(Importance::High), "High");
        for filter in TaskFilter::ALL {
            assert!(icons::NAMES.contains(&filter_icon(filter)), "{filter:?}");
        }
    }

    #[test]
    fn clicks_and_keys_work_the_list() {
        use egui::{Key, PointerButton};

        use crate::people_view::testing;
        type Window<'a> = egui_kittest::Harness<'a, MailApp>;
        let mut w = testing::window("view.tasks");
        let ids = tasks::sorted(&w.state().session, TaskFilter::Active);
        let row = |w: &Window<'_>, id: Id| w.state().hit.rect("task", id).unwrap();
        let selected = |w: &Window<'_>| w.state().session.sel.task;
        let complete = |w: &Window<'_>, id: Id| w.state().session.mailbox.task(id).unwrap().is_complete();
        assert!(ids.len() >= 4 && selected(&w).is_none());

        // A click on a row selects it; Up and Down walk the list and stop at its ends.
        let pos = row(&w, ids[1]).center();
        testing::click(&mut w, pos, PointerButton::Primary, 1);
        assert_eq!((selected(&w), complete(&w, ids[1])), (Some(ids[1]), false));
        testing::key(&mut w, Key::ArrowUp);
        testing::key(&mut w, Key::ArrowUp);
        assert_eq!(selected(&w), Some(ids[0]));
        testing::key(&mut w, Key::ArrowDown);
        testing::key(&mut w, Key::ArrowDown);
        assert_eq!(selected(&w), Some(ids[2]));

        // A click on the check box ticks that task off and leaves the selection alone. Under
        // Active, a finished task leaves the list.
        let pos = check_rect(row(&w, ids[3])).center();
        testing::click(&mut w, pos, PointerButton::Primary, 1);
        assert_eq!((complete(&w, ids[3]), selected(&w)), (true, Some(ids[2])));
        assert!(w.state().hit.rect("task", ids[3]).is_none());

        // Space ticks the selected task off, and on again.
        testing::key(&mut w, Key::Space);
        assert!(complete(&w, ids[2]));
        testing::key(&mut w, Key::Space);
        assert!(!complete(&w, ids[2]));

        // Enter opens the task; while its form is up, the keys are the form's.
        testing::key(&mut w, Key::Enter);
        assert_eq!(testing::dialog(&w), Some("editTask"));
        testing::key(&mut w, Key::Space);
        testing::key(&mut w, Key::ArrowUp);
        assert_eq!((complete(&w, ids[2]), selected(&w)), (false, Some(ids[2])));
        w.state_mut().dialog = None;
        w.step();

        // A double click selects the row and opens it.
        let pos = row(&w, ids[0]).center();
        testing::click(&mut w, pos, PointerButton::Primary, 2);
        assert_eq!((selected(&w), testing::dialog(&w)), (Some(ids[0]), Some("editTask")));
        w.state_mut().dialog = None;
        w.step();

        // A right click selects the row under the pointer and opens its menu, which keeps the keys.
        let pos = row(&w, ids[1]).center();
        testing::click(&mut w, pos, PointerButton::Secondary, 1);
        assert_eq!(selected(&w), Some(ids[1]));
        let ctx = w.state().ctx.clone().unwrap();
        assert!(egui::Popup::is_any_open(&ctx));
        testing::key(&mut w, Key::Space);
        assert!(!complete(&w, ids[1]));
    }

    #[test]
    fn the_quick_add_field_makes_tasks() {
        use egui::Key;

        use crate::people_view::testing;
        let mut w = testing::window("view.tasks");
        let ctx = w.state().ctx.clone().unwrap();
        let field = egui::Id::unique("task_quick_add");
        let typed = || ctx.data(|d| d.get_temp::<String>(field)).unwrap_or_default();
        let count = w.state().session.mailbox.tasks.len();
        let first = tasks::sorted(&w.state().session, TaskFilter::Active)[0];
        w.state_mut().execute("tasks.select", json!({"id": first})).unwrap();
        ctx.memory_mut(|m| m.request_focus(field));
        w.run_steps(2);

        // While the field has the keyboard, the list's keys are its own: Space doesn't tick.
        testing::text(&mut w, "  Book the Lisbon flights ");
        testing::key(&mut w, Key::Space);
        assert!(!w.state().session.mailbox.task(first).unwrap().is_complete());
        assert_eq!(typed(), "  Book the Lisbon flights ");

        // Enter adds the task, selects it, empties the field and stays in it.
        testing::key(&mut w, Key::Enter);
        let added =
            |w: &egui_kittest::Harness<'_, MailApp>| w.state().session.sel.task.and_then(|id| w.state().session.mailbox.task(id)).cloned().unwrap();
        assert_eq!(w.state().session.mailbox.tasks.len(), count + 1);
        assert_eq!((added(&w).title.as_str(), added(&w).due), ("Book the Lisbon flights", None));
        assert_eq!(typed(), "");
        assert!(ctx.memory(|m| m.has_focus(field)));

        // Under Today, what is added is due today.
        w.state_mut().execute("view.taskFilter", json!({"value": "today"})).unwrap();
        w.run_steps(2);
        testing::text(&mut w, "Call Lin");
        testing::key(&mut w, Key::Enter);
        assert_eq!((added(&w).title.as_str(), added(&w).due), ("Call Lin", Some(day(2026, 10, 14))));

        // Nothing but spaces adds nothing.
        testing::text(&mut w, "   ");
        testing::key(&mut w, Key::Enter);
        assert_eq!((w.state().session.mailbox.tasks.len(), typed()), (count + 2, String::new()));

        // Under Overdue a new task wouldn't be listed: the view moves to where it is.
        w.state_mut().execute("view.taskFilter", json!({"value": "overdue"})).unwrap();
        w.run_steps(2);
        testing::text(&mut w, "Renew the passport");
        testing::key(&mut w, Key::Enter);
        assert_eq!((added(&w).title.as_str(), w.state().session.view.task_filter), ("Renew the passport", TaskFilter::Active));
        assert!(w.state().hit.rect("task", added(&w).id).is_some());

        // Escape drops what was typed and leaves the field.
        testing::text(&mut w, "never mind");
        testing::key(&mut w, Key::Escape);
        assert_eq!((typed(), ctx.memory(|m| m.has_focus(field))), (String::new(), false));
        assert_eq!(w.state().session.mailbox.tasks.len(), count + 3);
    }

    #[test]
    fn the_lists_are_sorted_again_only_when_something_changes() {
        use crate::people_view::testing;
        let mut w = testing::window("view.tasks");
        let ctx = w.state().ctx.clone().unwrap();
        let first = lists(w.state(), &ctx);
        // Active, Today, Overdue, Completed, All in the sample mailbox.
        assert_eq!(first.counts, [5, 2, 1, 1, 6]);
        assert_eq!(first.layout.rows.len(), 5);
        let buckets: Vec<Bucket> = first.layout.rows.iter().filter_map(|r| r.heading.and_then(Heading::due)).collect();
        assert_eq!(buckets, [Bucket::Overdue, Bucket::Today, Bucket::Tomorrow, Bucket::ThisWeek, Bucket::NoDate]);
        // More frames and a selection: the same list.
        let id = first.layout.rows[1].id;
        w.state_mut().execute("tasks.select", json!({"id": id})).unwrap();
        w.run_steps(3);
        assert!(Arc::ptr_eq(&first.layout, &lists(w.state(), &ctx).layout));
        // A task is ticked off: it leaves Active and Today for Completed.
        w.state_mut().execute("tasks.complete", json!({"id": id})).unwrap();
        let ticked = lists(w.state(), &ctx);
        assert_eq!(ticked.counts, [4, 1, 1, 2, 6]);
        assert!(ticked.layout.rows.iter().all(|r| r.id != id));
        // Another filter is another list.
        w.state_mut().execute("view.taskFilter", json!({"value": "completed"})).unwrap();
        let done = lists(w.state(), &ctx);
        assert_eq!(done.layout.rows.len(), 2);
        assert_eq!(done.layout.rows.iter().filter_map(|r| r.heading.and_then(Heading::due)).collect::<Vec<_>>(), [Bucket::Completed]);
        // Undo, and the first list is back.
        w.state_mut().execute("view.taskFilter", json!({"value": "active"})).unwrap();
        w.state_mut().execute("edit.undo", json!({})).unwrap();
        assert_eq!((lists(w.state(), &ctx).counts, &*lists(w.state(), &ctx).layout), (first.counts, &*first.layout));
        // Midnight passes: what was due today is late, with nothing else changed.
        w.state_mut().session.clock.now = Some(testing::now() + mailcraft_time::DAY);
        let tomorrow = lists(w.state(), &ctx);
        assert_eq!(tomorrow.counts, [5, 3, 2, 1, 6]);
        let buckets: Vec<Bucket> = tomorrow.layout.rows.iter().filter_map(|r| r.heading.and_then(Heading::due)).collect();
        assert_eq!(buckets, [Bucket::Overdue, Bucket::Today, Bucket::Tomorrow, Bucket::NoDate]);
    }

    #[test]
    fn a_very_long_list_paints_only_what_shows() {
        use crate::people_view::testing;
        let mut mb = mailcraft_model::Mailbox::with_local_account("Me", "me@example.org");
        let start = day(2026, 10, 1);
        for i in 0..50_000u32 {
            mb.add_task(Task {
                title: format!("Task {i:05}"),
                due: (i % 7 != 0).then(|| start.add_days(i64::from(i % 400))),
                completed: (i % 10 == 0).then_some(testing::now() - i64::from(i)),
                categories: if i % 3 == 0 { vec!["Blue category".into()] } else { Vec::new() },
                ..Default::default()
            });
        }
        let mut w = testing::window_on(mb, "view.tasks");
        w.state_mut().execute("view.taskFilter", json!({"value": "all"})).unwrap();
        w.run_steps(3);
        let ctx = w.state().ctx.clone().unwrap();
        let all = lists(w.state(), &ctx);
        assert_eq!((all.counts[4], all.counts[3], all.counts[0]), (50_000, 5_000, 45_000));
        assert_eq!(all.layout.rows.len(), 50_000);
        let drawn = |w: &egui_kittest::Harness<'_, MailApp>| all.layout.rows.iter().filter(|r| w.state().hit.rect("task", r.id).is_some()).count();
        assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
        // The far end of the list: selecting it scrolls it into view, and still only a screenful
        // is painted.
        let (first, last) = (all.layout.rows[0].id, all.layout.rows[49_999].id);
        w.state_mut().execute("tasks.select", json!({"id": last})).unwrap();
        w.run_steps(40);
        assert!(w.state().hit.rect("task", last).is_some() && w.state().hit.rect("task", first).is_none());
        assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
    }
}
