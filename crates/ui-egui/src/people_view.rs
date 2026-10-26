// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People: the contact list and the selected contact's card.
//!
//! The list is laid out once per change of the contacts or the search (not once per frame), and
//! only the rows in view are painted, so it stays quick with tens of thousands of contacts.

use std::sync::Arc;

use egui::{Align2, Color32, FontId, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::PeopleSort;
use mailcraft_engine::cmd::people;
use mailcraft_model::{Address, Contact, Id, Mailbox};
use mailcraft_time::Date;
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons, widgets};

const ROW_H: f32 = 52.0;
/// Height of a group heading in a list.
pub(crate) const GROUP_H: f32 = 26.0;
/// Messages listed under Recent mail.
const RECENT: usize = 8;
/// The widest the card's content gets.
const CARD_W: f32 = 720.0;
/// Characters of notes the card lays out; longer notes end with an ellipsis.
const NOTES_MAX: usize = 20_000;

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let layout = list_layout(app, ui.ctx());
    keyboard(app, ui, &layout);
    keep_picked(app);
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    let max = (ui.available_width() - 320.0).max(260.0);
    egui::Panel::left("people_list")
        .default_size(320.0f32.min(max))
        .size_range(240.0..=max)
        .resizable(true)
        .frame(frame)
        .show(ui, |ui| list(app, ui, &layout));
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| card(app, ui));
}

/// The contacts picked together are those still there, and only while the selected contact is
/// one of them: selecting another one alone (a click, the arrow keys, a command) ends the pick.
pub(crate) fn keep_picked(app: &mut MailApp) {
    let contacts = &app.session.mailbox.contacts;
    app.people_picked.retain(|id| contacts.contains_key(id));
    if app.people_picked.len() < 2 || app.session.sel.contact.is_none_or(|c| !app.people_picked.contains(&c)) {
        app.people_picked.clear();
    }
}

/// Ctrl-click (⌘ on a Mac) on `id`: add it to the contacts picked together, or take it out.
pub(crate) fn pick(app: &mut MailApp, id: Id) {
    if app.people_picked.is_empty()
        && let Some(current) = app.session.sel.contact.filter(|c| *c != id)
    {
        app.people_picked.push(current);
    }
    if let Some(at) = app.people_picked.iter().position(|x| *x == id) {
        app.people_picked.remove(at);
        // The selection moves to the last one still picked.
        if let Some(last) = app.people_picked.last().copied() {
            let _ = app.execute("people.select", json!({"id": last}));
        }
    } else if app.people_picked.len() < mailcraft_engine::cmd::people_merge::MAX_MERGE {
        app.people_picked.push(id);
        let _ = app.execute("people.select", json!({"id": id}));
    }
    keep_picked(app);
}

// ---- the list's layout ----

/// The heading a contact sits under in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Group {
    Favorites,
    /// The first letter of the name the list sorts by.
    Letter(char),
    /// Names that start with a digit or a symbol, and contacts without a name.
    Other,
    /// Arranged by categories: one set of them, by its place in [`PeopleList::names`].
    Categories(u32),
    /// Arranged by categories: the contacts that have none.
    NoCategory,
}

fn group_of(favorite: bool, file_as: &str) -> Group {
    if favorite {
        return Group::Favorites;
    }
    match file_as.trim_start().chars().next() {
        Some(c) if c.is_alphabetic() => Group::Letter(c.to_uppercase().next().unwrap_or(c)),
        _ => Group::Other,
    }
}

fn group_label(group: Group, names: &[String]) -> String {
    match group {
        Group::Favorites => tl!("Favorites").to_string(),
        Group::Letter(c) => c.to_string(),
        Group::Other => "#".to_string(),
        Group::Categories(i) => names.get(i as usize).map_or_else(String::new, |n| one_line(n, 200)),
        Group::NoCategory => tl!("No category").to_string(),
    }
}

/// One row of a grouped list: where it starts, and the heading above it when it opens a group.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Row<G> {
    pub id: Id,
    /// Which of the item's rows this is: arranged by categories, an item filed under two is
    /// listed under each, as Outlook lists it, and the second row is copy 1.
    pub copy: u16,
    pub top: f32,
    pub heading: Option<G>,
}

/// A list of same-height rows under group headings, top to bottom in list coordinates (0 is the
/// top of the first heading). The task list uses it too.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layout<G> {
    pub rows: Vec<Row<G>>,
    pub height: f32,
    row_h: f32,
}

impl<G: Copy + PartialEq> Layout<G> {
    /// A heading goes above the first row of each run of the same group.
    pub fn new(row_h: f32, items: impl IntoIterator<Item = (Id, G)>) -> Self {
        let row_h = if row_h.is_finite() { row_h.max(1.0) } else { 1.0 };
        let mut rows = Vec::new();
        let mut y = 0.0f32;
        let mut last = None;
        let mut copies: std::collections::HashMap<Id, u16> = std::collections::HashMap::new();
        for (id, group) in items {
            let heading = (last != Some(group)).then_some(group);
            if heading.is_some() {
                y += GROUP_H;
            }
            let copy = copies.entry(id).or_default();
            rows.push(Row { id, copy: *copy, top: y, heading });
            *copy = copy.saturating_add(1);
            y += row_h;
            last = Some(group);
        }
        Layout { rows, height: y, row_h }
    }

    /// The rows that show between `min` and `max` (list coordinates), with their headings.
    pub fn visible(&self, min: f32, max: f32) -> &[Row<G>] {
        let first = self.rows.partition_point(|r| r.top + self.row_h < min);
        let end = self.rows.partition_point(|r| r.top - GROUP_H <= max);
        self.rows.get(first..end.max(first)).unwrap_or(&[])
    }

    /// The row `step` rows from `current`; the first row (the last when going up) when nothing
    /// in the list is selected.
    #[cfg(test)]
    pub fn neighbour(&self, current: Option<Id>, step: i64) -> Option<Id> {
        self.neighbour_row(current.map(|id| (id, 0)), step).map(|(id, _)| id)
    }

    /// [`neighbour`](Self::neighbour) from one of an item's rows (`(id, copy)`), to a row.
    pub fn neighbour_row(&self, current: Option<(Id, u16)>, step: i64) -> Option<(Id, u16)> {
        let last = self.rows.len().checked_sub(1)?;
        let index = match current.and_then(|row| self.position(row)) {
            Some(i) => (i as i64).saturating_add(step).clamp(0, last as i64) as usize,
            None if step < 0 => last,
            None => 0,
        };
        self.rows.get(index).map(|r| (r.id, r.copy))
    }

    /// The row of an item that `(id, copy)` stands for in this list, found as [`position`](Self::position) finds it.
    pub fn resolve_row(&self, row: (Id, u16)) -> Option<(Id, u16)> {
        self.position(row).and_then(|i| self.rows.get(i)).map(|r| (r.id, r.copy))
    }

    /// Where one of an item's rows is: that copy, or the item's first row when it has fewer.
    fn position(&self, (id, copy): (Id, u16)) -> Option<usize> {
        self.rows.iter().position(|r| r.id == id && r.copy == copy).or_else(|| self.rows.iter().position(|r| r.id == id))
    }
}

/// Items arranged by categories, as Outlook lists them: each under each of its categories, those
/// with none first, then the categories A to Z, in the order the items come in under each. Gives
/// each row's heading (an index into the names given back, `None` for no category) and the names.
pub(crate) fn by_category<'a>(
    s: &mailcraft_engine::Session,
    items: impl IntoIterator<Item = (Id, &'a [String])>,
) -> (Vec<(Id, Option<u32>)>, Vec<String>) {
    use mailcraft_engine::list::{category_names, category_spellings};
    let items: Vec<(Id, &[String])> = items.into_iter().collect();
    let spelling = category_spellings(s, items.iter().map(|(_, categories)| *categories));
    // (the heading's folded name, the item)
    let mut placed: Vec<(Option<String>, Id)> = Vec::new();
    for (id, categories) in items {
        let filed = category_names(categories);
        if filed.is_empty() {
            placed.push((None, id));
        }
        placed.extend(filed.into_iter().map(|name| (Some(name), id)));
    }
    // Stable, so under each heading the items keep the order they came in.
    placed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut names: Vec<String> = Vec::new();
    let mut last: Option<String> = None;
    let rows = placed
        .into_iter()
        .map(|(name, id)| match name {
            None => (id, None),
            Some(name) => {
                // Sorted by name, so a name not last seen is a new one, spelt as the mailbox spells it.
                if last.as_ref() != Some(&name) {
                    names.push(spelling.get(&name).map_or_else(|| name.clone(), |k| (*k).to_string()));
                    last = Some(name);
                }
                (id, Some(u32::try_from(names.len().saturating_sub(1)).unwrap_or(u32::MAX)))
            }
        })
        .collect();
    (rows, names)
}

/// How many items a list shows: one listed under several categories counts once.
pub(crate) fn listed<G>(layout: &Layout<G>) -> usize {
    layout.rows.iter().filter(|r| r.copy == 0).count()
}

/// Which of the selected item's rows has the selection, for an item a list shows more than once:
/// the row clicked or stepped to, kept for the list `slot`; the first otherwise.
pub(crate) fn chosen_copy(ctx: &egui::Context, slot: &'static str, selected: Option<Id>) -> u16 {
    match ctx.data(|d| d.get_temp::<(Id, u16)>(egui::Id::unique(slot))) {
        Some((id, copy)) if Some(id) == selected => copy,
        _ => 0,
    }
}

/// The selected item's row in `layout` ([`Layout::resolve_row`]), remembered so that the row
/// painted selected is the one the keys move from. Called once a frame, before the rows are drawn.
pub(crate) fn chosen_row<G: Copy + PartialEq>(
    ctx: &egui::Context,
    slot: &'static str,
    layout: &Layout<G>,
    selected: Option<Id>,
) -> Option<(Id, u16)> {
    let id = selected?;
    let copy = chosen_copy(ctx, slot, selected);
    let row = layout.resolve_row((id, copy))?;
    if row.1 != copy {
        choose_copy(ctx, slot, row.0, row.1);
    }
    Some(row)
}

/// Whether the row `(id, copy)` is the one with the selection.
pub(crate) fn row_selected(ctx: &egui::Context, slot: &'static str, selected: Option<Id>, (id, copy): (Id, u16)) -> bool {
    selected == Some(id) && chosen_copy(ctx, slot, selected) == copy
}

/// Remembers which of an item's rows was chosen ([`chosen_copy`]).
pub(crate) fn choose_copy(ctx: &egui::Context, slot: &'static str, id: Id, copy: u16) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique(slot), (id, copy)));
}

/// Whether the selected row is not the one this list showed last frame: a selection that changed
/// (the keyboard, a command, a new item) is scrolled into view. The row, not only the item: Down
/// from an item's last row under one category may reach its row under the next.
pub(crate) fn selection_changed(ui: &Ui, slot: &'static str, selected: Option<(Id, u16)>) -> bool {
    let slot = egui::Id::unique(slot);
    let changed = ui.data(|d| d.get_temp::<Option<(Id, u16)>>(slot)) != Some(selected);
    ui.data_mut(|d| d.insert_temp(slot, selected));
    changed
}

/// A scrolling list that paints its headings and only the rows in view. `reveal` is a row to
/// scroll to (`(id, copy)`); `row` paints one row (its item and copy) in the rectangle it is given.
pub(crate) fn show_rows<G: Copy + PartialEq>(
    ui: &mut Ui,
    layout: &Layout<G>,
    reveal: Option<(Id, u16)>,
    label: impl Fn(G) -> String,
    mut row: impl FnMut(&mut Ui, Id, u16, Rect),
) {
    let t = Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), layout.height), Sense::hover());
        let (origin, width) = (area.min, area.width());
        if let Some(r) = reveal.and_then(|row| layout.position(row)).and_then(|i| layout.rows.get(i)) {
            let heading = if r.heading.is_some() { GROUP_H } else { 0.0 };
            ui.scroll_to_rect(Rect::from_min_size(pos2(origin.x, origin.y + r.top - heading), vec2(width, layout.row_h + heading)), None);
        }
        for r in layout.visible(viewport.min.y, viewport.max.y) {
            if let Some(group) = r.heading {
                let bar = Rect::from_min_size(pos2(origin.x, origin.y + r.top - GROUP_H), vec2(width, GROUP_H));
                ui.painter().rect_filled(bar, 0.0, t.panel);
                paint_clipped(ui.painter(), pos2(bar.min.x + 12.0, bar.center().y), width - 24.0, &label(group), semibold(11.5), t.text_dim);
            }
            row(ui, r.id, r.copy, Rect::from_min_size(pos2(origin.x, origin.y + r.top), vec2(width, layout.row_h)));
        }
    });
}

/// The contact list as laid out, with the names of the categories its headings stand for when it
/// is arranged by categories.
#[derive(Clone, Debug, PartialEq)]
struct PeopleList {
    layout: Layout<Group>,
    names: Vec<String>,
}

impl std::ops::Deref for PeopleList {
    type Target = Layout<Group>;
    fn deref(&self) -> &Layout<Group> {
        &self.layout
    }
}

/// The layout kept between frames, with what it was made from.
#[derive(Clone)]
struct CachedLayout {
    key: (u64, usize, String, PeopleSort),
    list: Arc<PeopleList>,
}

/// The list for the search in view. Sorting every contact is the slow part, so the result is kept
/// until the mailbox, the search or the arrangement changes.
fn list_layout(app: &MailApp, ctx: &egui::Context) -> Arc<PeopleList> {
    let slot = egui::Id::unique("people_list_layout");
    let s = &app.session;
    let query = &s.view.people_search;
    let key = (s.rev(), s.mailbox.contacts.len(), query.clone(), s.view.people_sort);
    if let Some(cached) = ctx.data(|d| d.get_temp::<CachedLayout>(slot)).filter(|c| c.key == key) {
        return cached.list;
    }
    let ids = people::sorted(s, query);
    let contacts = ids.iter().filter_map(|id| s.mailbox.contact(*id));
    let mut names: Vec<String> = Vec::new();
    let layout = match s.view.people_sort {
        PeopleSort::Name => Layout::new(ROW_H, contacts.map(|c| (c.id, group_of(c.favorite, &c.file_as())))),
        // As Outlook lists them: a contact under each of its categories, those with none first,
        // then the categories A to Z, the contacts by name under each (the order they come in).
        PeopleSort::Categories => {
            let (rows, filed) = by_category(s, contacts.map(|c| (c.id, c.categories.as_slice())));
            names = filed;
            Layout::new(ROW_H, rows.into_iter().map(|(id, i)| (id, i.map_or(Group::NoCategory, Group::Categories))))
        }
    };
    let list = Arc::new(PeopleList { layout, names });
    ctx.data_mut(|d| d.insert_temp(slot, CachedLayout { key, list: list.clone() }));
    list
}

// ---- small pure pieces ----

/// Text for one line of a list or a card: trimmed, cut to `max` characters with an ellipsis, and
/// with line breaks and other control characters as spaces. Whatever a file holds, a line costs
/// no more than `max` characters to lay out.
pub(crate) fn one_line(s: &str, max: usize) -> String {
    let cut = clip_chars(s.trim(), max);
    let flat: String = cut.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.trim().to_string()
}

/// `s` cut to `max` characters, with an ellipsis when something was cut.
pub(crate) fn clip_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((end, _)) => format!("{}…", s.get(..end).unwrap_or(s)),
        None => s.to_string(),
    }
}

/// The name a row and the card show.
fn shown_name(c: &Contact) -> String {
    let name = one_line(&c.name(), 200);
    if name.is_empty() { tl!("(no name)").to_string() } else { name }
}

/// The second line of a row: company · e-mail, without repeating what the name already says.
fn subtitle(c: &Contact) -> String {
    let name = c.name();
    let company = c.company.trim();
    let email = c.primary_email().map(str::trim).unwrap_or("");
    let parts: Vec<&str> = [company, email].into_iter().filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(name.trim())).collect();
    one_line(&parts.join(" · "), 300)
}

/// Who the avatar stands for. A contact known only by its address takes its initials from the
/// address, not from the address used as a name.
fn avatar_address(c: &Contact) -> Address {
    let email = c.primary_email().unwrap_or("").trim();
    let name = c.name();
    let name = name.trim();
    Address::new(if name.eq_ignore_ascii_case(email) { "" } else { name }, email)
}

/// The recipient line for a message to one of the contact's addresses: `Name <address>`, or the
/// address alone for a contact known only by it.
fn recipient(c: &Contact, email: &str) -> String {
    let email = email.trim();
    let name = c.name();
    let name = name.trim();
    Address::new(if c.has_email(name) || name.eq_ignore_ascii_case(email) { "" } else { name }, email).full()
}

/// Title · company · department, skipping the empty ones and a company that is already the name.
fn role_line(c: &Contact) -> String {
    let name = c.name();
    let company = Some(c.company.trim()).filter(|company| !company.eq_ignore_ascii_case(name.trim())).unwrap_or("");
    let parts: Vec<&str> = [c.title.trim(), company, c.department.trim()].into_iter().filter(|p| !p.is_empty()).collect();
    one_line(&parts.join(" · "), 400)
}

/// A label as the card shows it (`work` reads `Work`).
fn label_text(label: &str) -> String {
    let label = label.trim();
    for known in ["Work", "Home", "Mobile", "Other", "Main", "Fax"] {
        if label.eq_ignore_ascii_case(known) {
            return tl!(known).to_string();
        }
    }
    let mut chars = label.chars().filter(|c| !c.is_control());
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).take(40).collect(),
        None => String::new(),
    }
}

/// The address a website line opens: `http` and `https` only, and a bare host name gets
/// `https://`. Anything else is shown but can't be clicked.
fn web_target(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let url = if raw.contains("://") {
        raw.to_string()
    } else if !raw.contains(':') && raw.contains('.') {
        format!("https://{raw}")
    } else {
        return None;
    };
    let web = url.split_once("://").is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) && rest.starts_with(|c: char| c.is_alphanumeric() || c == '[')
    });
    (web && mailcraft_html::is_safe_url(&url)).then_some(url)
}

/// `March 14, 1988`; the ISO form for a date whose month is out of range.
fn birthday_text(day: Date) -> String {
    if mailcraft_time::month_name(day.month).is_empty() { day.iso() } else { crate::dates::long_without_weekday(day) }
}

/// The next time the birthday comes round (today counts) and the age reached then. `None` when
/// that age isn't a believable one, as for a card that carries no real birth year.
fn next_birthday(birthday: Date, today: Date) -> Option<(Date, i32)> {
    for year in [today.year, today.year.saturating_add(1)] {
        // Someone born on February 29th has the 28th in other years.
        let day = birthday.day.min(mailcraft_time::days_in_month(year, birthday.month));
        let Some(next) = Date::new(year, birthday.month, day) else { continue };
        if next >= today {
            let age = year.saturating_sub(birthday.year);
            return (1..=130).contains(&age).then_some((next, age));
        }
    }
    None
}

/// The newest messages from or to any of the contact's addresses, newest first: one pass over
/// the mailbox.
fn recent_mail(mailbox: &Mailbox, c: &Contact, limit: usize) -> Vec<Id> {
    let emails: Vec<&str> = c.emails.iter().map(|e| e.value.trim()).filter(|e| !e.is_empty()).collect();
    if emails.is_empty() || limit == 0 {
        return Vec::new();
    }
    let theirs = |a: &Address| emails.iter().any(|e| a.email.trim().eq_ignore_ascii_case(e));
    let mut hits: Vec<(i64, Id)> =
        mailbox.messages.values().filter(|m| theirs(&m.from) || m.recipients().any(&theirs)).map(|m| (m.date, m.id)).collect();
    hits.sort_unstable_by(|a, b| b.cmp(a));
    hits.truncate(limit);
    hits.into_iter().map(|(_, id)| id).collect()
}

/// [`recent_mail`] for the selected contact, kept until the mailbox changes.
fn recent_for(app: &MailApp, ctx: &egui::Context, c: &Contact) -> Arc<Vec<Id>> {
    type Cached = ((u64, usize, Id), Arc<Vec<Id>>);
    let slot = egui::Id::unique("people_recent_mail");
    let key = (app.session.rev(), app.session.mailbox.messages.len(), c.id);
    if let Some((_, list)) = ctx.data(|d| d.get_temp::<Cached>(slot)).filter(|(k, _)| *k == key) {
        return list;
    }
    let list = Arc::new(recent_mail(&app.session.mailbox, c, RECENT));
    ctx.data_mut(|d| d.insert_temp::<Cached>(slot, (key, list.clone())));
    list
}

fn text_width(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), font, Color32::WHITE).size().x)
}

// ---- keyboard ----

/// Up and Down move the selection and Enter opens the contact, unless a text field, a menu or a
/// dialog has the keyboard.
fn keyboard(app: &mut MailApp, ui: &Ui, layout: &Layout<Group>) {
    if app.dialog.is_some() || ui.ctx().egui_wants_keyboard_input() || egui::Popup::is_any_open(ui.ctx()) || !ui.input(|i| i.modifiers.is_none()) {
        return;
    }
    let none = egui::Modifiers::NONE;
    let (down, up) = ui.input_mut(|i| (i.count_and_consume_key(none, egui::Key::ArrowDown), i.count_and_consume_key(none, egui::Key::ArrowUp)));
    let step = down.min(1000) as i64 - up.min(1000) as i64;
    let selected = app.session.sel.contact;
    let at = chosen_row(ui.ctx(), PEOPLE_ROW, layout, selected);
    if step != 0
        && let Some((id, copy)) = layout.neighbour_row(at, step)
        && at != Some((id, copy))
    {
        choose_copy(ui.ctx(), PEOPLE_ROW, id, copy);
        if selected != Some(id) {
            let _ = app.run("people.select", json!({"id": id}));
        }
    }
    // Only a new press that is the main window's: not the repeats of an Enter that closed a dialog
    // (#406).
    if app.session.sel.contact.is_some() && crate::keys::take_enter_alone(ui.ctx(), ui.layer_id()) {
        let _ = app.run("people.edit", json!({}));
    }
}

// ---- the list pane ----

/// Where the contact list keeps which row of the selected contact has the selection.
const PEOPLE_ROW: &str = "people_list_row";

fn list(app: &mut MailApp, ui: &mut Ui, layout: &PeopleList) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let searching = !app.session.view.people_search.trim().is_empty();
    let total = app.session.mailbox.contacts.len();
    let count = if searching {
        crate::i18n::fmt(tl!("{n} of {total}"), &[("n", &listed(&layout.layout).to_string()), ("total", &total.to_string())])
    } else {
        total.to_string()
    };
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 34.0));
    crate::chrome::pane_title(ui, tl!("People"), &count);
    // New Contact, at the right of the heading.
    let button = Rect::from_center_size(pos2(header.max.x - 20.0, header.center().y), vec2(26.0, 24.0));
    let resp = ui.interact(button, ui.scope_id().with("people_new"), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(button, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    icons::paint(ui.painter(), Rect::from_center_size(button.center(), vec2(16.0, 16.0)), "newContact", t.icon, t.accent);
    if resp.on_hover_text(tl!("New Contact")).clicked() {
        let _ = app.run("people.new", json!({}));
    }
    ui.painter().line_segment([pos2(header.min.x, header.max.y), pos2(header.max.x, header.max.y)], Stroke::new(1.0, t.border));

    if layout.rows.is_empty() {
        empty_list(app, ui, searching);
        return;
    }
    let selected = app.session.sel.contact;
    let at = chosen_row(ui.ctx(), PEOPLE_ROW, &layout.layout, selected);
    let reveal = selection_changed(ui, "people_list_seen", at).then_some(at).flatten();
    show_rows(ui, layout, reveal, |g| group_label(g, &layout.names), |ui, id, copy, rect| contact_row(app, ui, id, copy, rect));
}

fn empty_list(app: &mut MailApp, ui: &mut Ui, searching: bool) {
    let t = Tokens::get(ui.ctx());
    ui.vertical_centered(|ui| {
        ui.add_space(56.0);
        let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
        icons::paint(ui.painter(), r, if searching { "search" } else { "people" }, t.border_strong, t.border_strong);
        ui.add_space(10.0);
        let text = if searching { tl!("Nobody matches the search") } else { tl!("No contacts yet") };
        ui.label(RichText::new(text).font(regular(12.5)).color(t.text_dim));
        if !searching {
            ui.add_space(14.0);
            if widgets::primary_button(ui, tl!("New Contact"), true).clicked() {
                let _ = app.run("people.new", json!({}));
            }
        }
    });
}

fn contact_row(app: &mut MailApp, ui: &mut Ui, id: Id, copy: u16, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    // The shared pointer, not a copy of the contact.
    let Some(c) = app.session.mailbox.contacts.get(&id).cloned() else { return };
    // A contact listed under two categories is two rows, each with an id of its own.
    let resp = ui.interact(rect, ui.scope_id().with(("contact", id, copy)), Sense::click());
    app.hit.add("contact", id, rect);
    // The selection is on one of a contact's rows: the one clicked or stepped to.
    let picked = app.people_picked.contains(&id);
    // The selection is the one contact commands act on; the others picked with it have a tick,
    // not the selection's colour (they are picked to merge, not selected).
    let selected = row_selected(ui.ctx(), PEOPLE_ROW, app.session.sel.contact, (id, copy));
    if resp.clicked() || resp.secondary_clicked() {
        choose_copy(ui.ctx(), PEOPLE_ROW, id, copy);
    }
    let p = ui.painter();
    if selected {
        p.rect_filled(rect, 0.0, t.selection);
    } else if resp.hovered() {
        p.rect_filled(rect, 0.0, t.hover.linear_multiply(0.6));
    }
    p.line_segment([pos2(rect.min.x + 54.0, rect.max.y), pos2(rect.max.x, rect.max.y)], Stroke::new(1.0, t.border.linear_multiply(0.7)));
    widgets::avatar(ui, pos2(rect.min.x + 28.0, rect.center().y), 16.0, &avatar_address(&c));
    let left = rect.min.x + 54.0;
    let mut right = rect.max.x - 12.0;
    if c.favorite {
        icons::paint(p, Rect::from_center_size(pos2(right - 7.0, rect.center().y), vec2(16.0, 16.0)), "starOn", t.icon, t.accent);
        right -= 20.0;
    }
    if picked {
        icons::paint(p, Rect::from_center_size(pos2(right - 7.0, rect.center().y), vec2(16.0, 16.0)), "check", t.icon, t.accent);
        right -= 20.0;
    }
    let width = (right - left).max(0.0);
    let second = subtitle(&c);
    let name_y = if second.is_empty() { rect.center().y } else { rect.min.y + 18.0 };
    paint_clipped(p, pos2(left, name_y), width, &shown_name(&c), if selected { semibold(13.0) } else { regular(13.0) }, t.text);
    paint_clipped(p, pos2(left, rect.min.y + 35.0), width, &second, regular(11.5), t.text_dim);

    // Ctrl-click (⌘ on a Mac) picks several contacts together, to merge (#506).
    let command = ui.input(|i| i.modifiers.command);
    if resp.clicked() && command {
        pick(app, id);
    } else if resp.clicked() {
        app.people_picked.clear();
        let _ = app.run("people.select", json!({"id": id}));
    }
    if resp.double_clicked() && !command {
        let _ = app.execute("people.select", json!({"id": id}));
        let _ = app.run("people.edit", json!({}));
    }
    // A right click acts on the row under the pointer, or on the contacts picked with it.
    if resp.secondary_clicked() && !selected && !picked {
        app.people_picked.clear();
        let _ = app.execute("people.select", json!({"id": id}));
    }
    let (favorite, has_email) = (c.favorite, c.address().is_some());
    let several = picked && app.people_picked.len() >= 2;
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        use widgets::mi;
        if several {
            let n = app.people_picked.len();
            let label = tln!(n as u64, "Merge {n} Contact…", "Merge {n} Contacts…");
            if ui.add(egui::Button::new(label).min_size(vec2(190.0, 0.0))).clicked() {
                let params = crate::ribbon::merge_params(app);
                let _ = app.run("people.merge", params);
                ui.close();
            }
            ui.separator();
        }
        if ui.add_enabled(has_email, egui::Button::new(tl!("Email")).min_size(vec2(190.0, 0.0))).clicked() {
            let _ = app.run("people.email", json!({"id": id}));
            ui.close();
        }
        mi(ui, app, "Edit", "people.edit", json!({}));
        ui.separator();
        mi(ui, app, if favorite { "Remove from Favorites" } else { "Add to Favorites" }, "people.favorite", json!({"id": id}));
        ui.menu_button(tl!("Categorize"), |ui| {
            crate::ribbon::category_menu(
                ui,
                app,
                "people.categorize",
                |app, name| app.session.mailbox.contact(id).is_some_and(|c| c.categories.iter().any(|x| x.eq_ignore_ascii_case(name))),
                None,
            );
        });
        ui.separator();
        mi(ui, app, "Export…", "file.exportContacts", json!({"ids": [id]}));
        mi(ui, app, "Delete", "people.delete", json!({"id": id}));
    });
}

// ---- the card ----

fn card(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(c) = app.session.sel.contact.and_then(|id| app.session.mailbox.contacts.get(&id).cloned()) else {
        // With nobody to select, the list's own empty state says what to do.
        if app.session.mailbox.contacts.is_empty() {
            return;
        }
        let r = ui.available_rect_before_wrap();
        let y = r.min.y + r.height() * 0.38;
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.center().x, y), vec2(56.0, 56.0)), "people", t.border_strong, t.border_strong);
        ui.painter().text(pos2(r.center().x, y + 50.0), Align2::CENTER_CENTER, tl!("Select a contact to see the details"), regular(13.0), t.text_dim);
        return;
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(("contact_card", c.id)).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 28, right: 28, top: 24, bottom: 28 }).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(CARD_W));
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            card_header(app, ui, &c);
            ui.add_space(12.0);
            card_actions(app, ui, &c);
            card_contact(app, ui, &c);
            card_places(ui, &c);
            card_birthday(app, ui, &c);
            if !c.notes.trim().is_empty() {
                section(ui, tl!("Notes"));
                ui.add(egui::Label::new(RichText::new(clip_chars(c.notes.trim(), NOTES_MAX)).font(regular(13.0)).color(t.text)).wrap());
            }
            card_recent(app, ui, &c);
        });
    });
}

/// The avatar, the name, what they do and where, and the categories.
fn card_header(app: &MailApp, ui: &mut Ui, c: &Contact) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(80.0, 80.0), Sense::hover());
        widgets::avatar(ui, r.center(), 40.0, &avatar_address(c));
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
            let role = role_line(c);
            // A name alone sits level with the middle of the avatar.
            ui.add_space(if role.is_empty() && c.categories.is_empty() {
                26.0
            } else if role.is_empty() || c.categories.is_empty() {
                16.0
            } else {
                6.0
            });
            ui.add(egui::Label::new(RichText::new(shown_name(c)).font(semibold(20.0)).color(t.text)).wrap());
            if !role.is_empty() {
                ui.add(egui::Label::new(RichText::new(role).font(regular(12.5)).color(t.text_dim)).wrap());
            }
            if !c.categories.is_empty() {
                ui.add_space(2.0);
                chips(app, ui, &c.categories);
            }
        });
    });
}

/// Category chips, wrapping onto more lines when they don't fit on one.
pub(crate) fn chips(app: &MailApp, ui: &mut Ui, categories: &[String]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        for name in categories.iter().take(40) {
            let shown = one_line(name, 40);
            let w = text_width(ui, &shown, regular(10.5)) + 12.0;
            let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
            widgets::category_chip(ui, pos2(r.min.x, r.center().y), &shown, crate::theme::c32(app.session.mailbox.category_color(name)));
        }
    });
}

fn card_actions(app: &mut MailApp, ui: &mut Ui, c: &Contact) {
    let id = c.id;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        if c.address().is_some() && widgets::primary_button(ui, tl!("Email"), true).clicked() {
            let _ = app.run("people.email", json!({"id": id}));
        }
        // The card shows the selected contact, so Edit opens its form.
        if widgets::icon_text_button(ui, "edit", tl!("Edit"), true).clicked() {
            let _ = app.run("people.edit", json!({}));
        }
        let tip = if c.favorite { tl!("Remove from Favorites") } else { tl!("Add to Favorites") };
        if widgets::icon_text_button(ui, if c.favorite { "starOn" } else { "star" }, tl!("Favorite"), true).on_hover_text(tip).clicked() {
            let _ = app.run("people.favorite", json!({"id": id}));
        }
        if widgets::icon_text_button(ui, "delete", tl!("Delete"), true).clicked() {
            let _ = app.run("people.delete", json!({"id": id}));
        }
    });
}

/// A section's heading: a small accent title over a hairline.
pub(crate) fn section(ui: &mut Ui, title: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(14.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    paint_clipped(ui.painter(), pos2(r.min.x, r.center().y - 1.0), r.width(), title, semibold(11.5), t.accent_text);
    ui.painter().line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
    ui.add_space(4.0);
}

/// One line of a section: an icon, a value and its label. A `link` is a button.
fn field(ui: &mut Ui, icon: &str, value: &str, label: &str, link: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), if link { Sense::click() } else { Sense::hover() });
    if link && resp.hovered() {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(16.0, 16.0)), icon, t.text_dim, t.accent);
    let label_w = if label.is_empty() { 0.0 } else { text_width(ui, label, regular(11.5)) + 12.0 };
    let x = r.min.x + 30.0;
    let used = paint_clipped(
        ui.painter(),
        pos2(x, r.center().y),
        (r.max.x - x - label_w - 6.0).max(0.0),
        value,
        regular(13.0),
        if link { t.accent_text } else { t.text },
    );
    let label_x = x + used + 12.0;
    paint_clipped(ui.painter(), pos2(label_x, r.center().y + 0.5), (r.max.x - label_x).max(0.0), label, regular(11.5), t.text_dim);
    resp
}

/// E-mail addresses (a click writes to that address) and phone numbers.
fn card_contact(app: &mut MailApp, ui: &mut Ui, c: &Contact) {
    let emails: Vec<_> = c.emails.iter().filter(|e| !e.value.trim().is_empty()).take(100).collect();
    let phones: Vec<_> = c.phones.iter().filter(|p| !p.value.trim().is_empty()).take(100).collect();
    if emails.is_empty() && phones.is_empty() {
        return;
    }
    section(ui, tl!("Contact"));
    for e in emails {
        let shown = one_line(&e.value, 300);
        let tip = crate::i18n::fmt(tl!("Write to {address}"), &[("address", &shown)]);
        if field(ui, "mail", &shown, &label_text(&e.label), true).on_hover_text(tip).clicked() {
            // The address as it is stored, not as it is shown.
            let _ = app.run("mail.new", json!({"to": recipient(c, &e.value)}));
        }
    }
    for p in phones {
        field(ui, "phone", &one_line(&p.value, 200), &label_text(&p.label), false);
    }
}

/// Postal addresses and websites.
fn card_places(ui: &mut Ui, c: &Contact) {
    let addresses: Vec<_> = c.addresses.iter().filter(|a| !a.is_empty()).take(100).collect();
    if !addresses.is_empty() {
        section(ui, tl!("Address"));
        for a in addresses {
            let line = one_line(&a.one_line(), 500);
            field(ui, "address", &line, &label_text(&a.label), false).on_hover_text(line.as_str());
        }
    }
    let sites: Vec<&str> = c.websites.iter().map(|w| w.trim()).filter(|w| !w.is_empty()).take(100).collect();
    if !sites.is_empty() {
        section(ui, tl!("Websites"));
        for site in sites {
            let shown = one_line(site, 300);
            // Only plain web addresses open; anything else is text. What opens is the address as
            // it is stored, not as it is shown.
            match web_target(site) {
                Some(url) => {
                    if field(ui, "web", &shown, "", true).on_hover_text(one_line(&url, 300)).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
                None => {
                    field(ui, "web", &shown, "", false);
                }
            }
        }
    }
}

fn card_birthday(app: &MailApp, ui: &mut Ui, c: &Contact) {
    let Some(birthday) = c.birthday else { return };
    section(ui, tl!("Birthday"));
    let today = app.session.today();
    let note = match next_birthday(birthday, today) {
        Some((next, age)) if next == today => crate::i18n::fmt(tl!("Turns {age} today"), &[("age", &age.to_string())]),
        Some((next, age)) => {
            let on = crate::dates::month_day_long(next);
            crate::i18n::fmt(tl!("Turns {age} on {date}"), &[("age", &age.to_string()), ("date", &on)])
        }
        None => String::new(),
    };
    field(ui, "calendar", &birthday_text(birthday), &note, false);
}

/// The newest mail from or to this contact; a click opens the message in Mail.
fn card_recent(app: &mut MailApp, ui: &mut Ui, c: &Contact) {
    let recent = recent_for(app, ui.ctx(), c);
    if recent.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    section(ui, tl!("Recent mail"));
    for id in recent.iter() {
        let Some(m) = app.session.mailbox.messages.get(id).cloned() else { continue };
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        app.hit.add("recentMail", *id, r);
        if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
        }
        let p = ui.painter();
        // Mail they sent comes in; mail to them goes out.
        let from_them = c.has_email(&m.from.email);
        icons::paint(
            p,
            Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(16.0, 16.0)),
            if from_them { "mail" } else { "sent" },
            t.text_dim,
            t.accent,
        );
        let date = crate::dates::list_label(m.date, app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
        let date_w = p.text(pos2(r.max.x - 8.0, r.center().y), Align2::RIGHT_CENTER, &date, regular(11.0), t.text_dim).width();
        let folder = app.session.mailbox.folder(m.folder).map(|f| one_line(crate::nav::folder_name(f), 60)).unwrap_or_default();
        let folder_w = text_width(ui, &folder, regular(11.0)).min(140.0);
        let folder_x = r.max.x - 8.0 - date_w.max(64.0) - 14.0 - folder_w;
        paint_clipped(p, pos2(folder_x, r.center().y), folder_w + 1.0, &folder, regular(11.0), t.text_dim);
        let subject = if m.subject.trim().is_empty() { tl!("(no subject)").to_string() } else { one_line(&m.subject, 300) };
        let x = r.min.x + 30.0;
        paint_clipped(
            p,
            pos2(x, r.center().y),
            (folder_x - x - 14.0).max(0.0),
            &subject,
            if m.read { regular(12.5) } else { semibold(12.5) },
            t.text,
        );
        if resp.clicked() {
            app.open_in_mail(*id);
        }
    }
}

/// A headless window on the sample mailbox, for the tests that click and type (here and in the
/// task list's).
#[cfg(test)]
pub(crate) mod testing {
    use egui::{Event, Key, Modifiers, PointerButton, Pos2};
    use egui_kittest::Harness;
    use mailcraft_engine::Session;
    use serde_json::json;

    use crate::{MailApp, Services};

    pub type Window = Harness<'static, MailApp>;

    /// Wednesday 2026-10-14 16:00 UTC.
    pub fn now() -> i64 {
        mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
    }

    /// The window on the sample mailbox as of [`now`], with `module` (`view.people`,
    /// `view.tasks`) in front.
    pub fn window(module: &str) -> Harness<'static, MailApp> {
        window_on(mailcraft_engine::sample::mailbox(now(), 0), module)
    }

    /// The window on `mailbox`. Frames are 1/60 s apart, so a double click is one.
    pub fn window_on(mailbox: mailcraft_model::Mailbox, module: &str) -> Harness<'static, MailApp> {
        let mut app = MailApp::new(Session::at(mailbox, now(), 0), Services::default());
        app.autosave = false;
        app.execute(module, json!({})).unwrap();
        let mut window = Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        window.run_steps(5);
        window
    }

    /// Click (`times` 2 is a double click) at `pos`, one pointer event per frame. A pause comes
    /// first, so the click doesn't count as one more of an earlier click's.
    pub fn click(window: &mut Harness<'_, MailApp>, pos: Pos2, button: PointerButton, times: usize) {
        window.run_steps(45);
        window.input_mut().events.push(Event::PointerMoved(pos));
        window.step();
        for _ in 0..times {
            for pressed in [true, false] {
                window.input_mut().events.push(Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
                window.step();
            }
        }
        window.step();
    }

    pub fn key(window: &mut Harness<'_, MailApp>, key: Key) {
        for pressed in [true, false] {
            window.input_mut().events.push(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
        }
        window.run_steps(2);
    }

    /// `key` held down: the press, then the repeats the system sends, a frame apart, then the
    /// release. A held key is one press (#391, #393).
    pub fn held(window: &mut Harness<'_, MailApp>, key: Key) {
        for repeat in [false, true, true, true] {
            window.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat, modifiers: Modifiers::NONE });
            window.run_steps(2);
        }
        window.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
        window.run_steps(2);
    }

    pub fn text(window: &mut Harness<'_, MailApp>, text: &str) {
        window.input_mut().events.push(Event::Text(text.to_string()));
        window.run_steps(2);
    }

    /// The name of the dialog that is open.
    pub fn dialog(window: &Harness<'_, MailApp>) -> Option<&'static str> {
        window.state().dialog.as_ref().map(|d| d.name())
    }
}

#[cfg(test)]
mod arrange_tests;

#[cfg(test)]
mod tests {
    use mailcraft_model::{Labeled, Message};

    use super::*;

    fn contact(first: &str, last: &str) -> Contact {
        Contact { first: first.into(), last: last.into(), ..Default::default() }
    }

    fn day(y: i32, m: u8, d: u8) -> Date {
        Date::new(y, m, d).unwrap()
    }

    #[test]
    fn groups_come_from_the_name_the_list_sorts_by() {
        assert_eq!(group_of(true, "Chen, Maya"), Group::Favorites);
        assert_eq!(group_of(false, "Chen, Maya"), Group::Letter('C'));
        assert_eq!(group_of(false, "  okafor"), Group::Letter('O'));
        assert_eq!(group_of(false, "élan"), Group::Letter('É'));
        // A letter whose capital is two letters (ß → SS) still gives one.
        assert_eq!(group_of(false, "ßtraße"), Group::Letter('S'));
        for odd in ["", "   ", "42 Wallaby Way", "@home", "\u{0}", "(Acme)"] {
            assert_eq!(group_of(false, odd), Group::Other, "{odd:?}");
        }
        assert_eq!(group_label(Group::Favorites, &[]), "Favorites");
        assert_eq!(group_label(Group::Letter('C'), &[]), "C");
        assert_eq!(group_label(Group::Other, &[]), "#");
    }

    #[test]
    fn rows_are_offset_by_the_headings_above_them() {
        let l = Layout::new(
            ROW_H,
            [(1, Group::Favorites), (2, Group::Favorites), (3, Group::Letter('A')), (4, Group::Letter('B')), (5, Group::Letter('B'))],
        );
        let tops: Vec<f32> = l.rows.iter().map(|r| r.top).collect();
        assert_eq!(tops, [GROUP_H, GROUP_H + ROW_H, 2.0 * GROUP_H + 2.0 * ROW_H, 3.0 * GROUP_H + 3.0 * ROW_H, 3.0 * GROUP_H + 4.0 * ROW_H]);
        assert_eq!(l.height, 3.0 * GROUP_H + 5.0 * ROW_H);
        let headings: Vec<Option<Group>> = l.rows.iter().map(|r| r.heading).collect();
        assert_eq!(headings, [Some(Group::Favorites), None, Some(Group::Letter('A')), Some(Group::Letter('B')), None]);
        let empty = Layout::<Group>::new(ROW_H, []);
        assert_eq!((empty.rows.len(), empty.height), (0, 0.0));
        assert!(empty.visible(0.0, 900.0).is_empty());
        assert_eq!(empty.neighbour(Some(7), 1), None);
    }

    #[test]
    fn only_the_rows_in_view_are_picked() {
        let l = Layout::new(ROW_H, (1..=50_000u64).map(|id| (id, Group::Letter('A'))));
        assert_eq!(l.height, GROUP_H + 50_000.0 * ROW_H);
        let ids = |min: f32, max: f32| l.visible(min, max).iter().map(|r| r.id).collect::<Vec<_>>();
        // The top of the list: the heading and the rows a 200 px window reaches.
        assert_eq!(ids(0.0, 200.0), [1, 2, 3, 4]);
        // Far down: a screenful, not the whole list.
        let far = l.visible(1_000_000.0, 1_000_900.0);
        assert!((17..=20).contains(&far.len()), "{}", far.len());
        assert!(far.iter().all(|r| r.top + ROW_H >= 1_000_000.0 && r.top - GROUP_H <= 1_000_900.0));
        // Windows off either end, upside down or not numbers at all are empty, never a panic.
        assert!(ids(l.height + 100.0, l.height + 900.0).is_empty());
        assert!(ids(-900.0, -100.0).is_empty());
        assert!(ids(500.0, 100.0).is_empty());
        let _ = ids(f32::NAN, f32::NAN);
        let _ = ids(f32::NEG_INFINITY, f32::INFINITY);
    }

    #[test]
    fn the_keyboard_moves_through_the_rows_and_stops_at_the_ends() {
        let l = Layout::new(ROW_H, [(10, Group::Favorites), (20, Group::Letter('A')), (30, Group::Letter('B'))]);
        assert_eq!(l.neighbour(Some(10), 1), Some(20));
        assert_eq!(l.neighbour(Some(20), -1), Some(10));
        assert_eq!(l.neighbour(Some(10), -1), Some(10));
        assert_eq!(l.neighbour(Some(30), 1), Some(30));
        assert_eq!(l.neighbour(Some(10), i64::MAX), Some(30));
        assert_eq!(l.neighbour(Some(30), i64::MIN), Some(10));
        // Nothing selected, or a selection the search has filtered out.
        assert_eq!(l.neighbour(None, 1), Some(10));
        assert_eq!(l.neighbour(None, -1), Some(30));
        assert_eq!(l.neighbour(Some(99), 1), Some(10));
    }

    #[test]
    fn rows_say_who_and_where() {
        let mut c = contact("Maya", "Chen");
        c.company = "Lumen Studio".into();
        c.emails = vec![Labeled::new("work", ""), Labeled::new("home", "maya@lumen.example")];
        assert_eq!(shown_name(&c), "Maya Chen");
        assert_eq!(subtitle(&c), "Lumen Studio · maya@lumen.example");
        assert_eq!(avatar_address(&c), Address::new("Maya Chen", "maya@lumen.example"));
        c.title = "Design Lead".into();
        c.department = " ".into();
        assert_eq!(role_line(&c), "Design Lead · Lumen Studio");
        // A company, or an address, standing in for the name isn't said twice.
        let org = Contact { company: "Harbor Books".into(), emails: vec![Labeled::new("work", "hello@harborbooks.example")], ..Default::default() };
        assert_eq!((shown_name(&org).as_str(), subtitle(&org).as_str()), ("Harbor Books", "hello@harborbooks.example"));
        assert_eq!(role_line(&org), "");
        assert_eq!(role_line(&Contact { department: "Orders".into(), ..org.clone() }), "Orders");
        let bare = Contact { emails: vec![Labeled::new("", "grace.hopper@example.org")], ..Default::default() };
        assert_eq!((shown_name(&bare).as_str(), subtitle(&bare).as_str()), ("grace.hopper@example.org", ""));
        assert_eq!(avatar_address(&bare).initials(), "GH");
        // Writing to them: the name with the address, or the address alone when that is the name.
        assert_eq!(recipient(&c, " maya@home.example "), "Maya Chen <maya@home.example>");
        assert_eq!(recipient(&bare, "grace.hopper@example.org"), "grace.hopper@example.org");
        assert_eq!(recipient(&org, "orders@harborbooks.example"), "Harbor Books <orders@harborbooks.example>");
        assert_eq!(recipient(&Contact::default(), ""), "");
    }

    #[test]
    fn hostile_names_stay_on_one_line() {
        let empty = Contact::default();
        assert_eq!(shown_name(&empty), "(no name)");
        assert_eq!((subtitle(&empty).as_str(), role_line(&empty).as_str()), ("", ""));
        assert_eq!(avatar_address(&empty), Address::default());
        assert_eq!(avatar_address(&empty).initials(), "");
        let odd = Contact { display_name: "Line\none\ttwo\u{0}".into(), company: "A\r\nB".into(), ..Default::default() };
        assert_eq!(shown_name(&odd), "Line one two");
        assert_eq!(subtitle(&odd), "A  B");
        assert_eq!(clip_chars("héllo wörld", 5), "héllo…");
        assert_eq!(clip_chars("héllo", 5), "héllo");
        assert_eq!(clip_chars("", 0), "");
        assert_eq!(clip_chars("abc", 0), "…");
        assert_eq!(clip_chars(&"é".repeat(100_000), 3), "ééé…");
        // However long the text in a file is, a line of the list lays out a bounded piece of it.
        assert_eq!(one_line(&"x".repeat(1_000_000), 10), format!("{}…", "x".repeat(10)));
        assert_eq!(one_line("  a\r\nb\u{7}  ", 100), "a  b");
        assert_eq!(one_line("\n\n", 100), "");
        let long = Contact { display_name: "n".repeat(100_000), company: "c".repeat(100_000), title: "t".repeat(100_000), ..Default::default() };
        assert!(shown_name(&long).chars().count() <= 201 && subtitle(&long).chars().count() <= 301 && role_line(&long).chars().count() <= 401);
    }

    #[test]
    fn labels_read_like_words() {
        assert_eq!(label_text("work"), "Work");
        assert_eq!(label_text(" MOBILE "), "Mobile");
        assert_eq!(label_text("assistant"), "Assistant");
        assert_eq!(label_text("élan"), "Élan");
        assert_eq!(label_text(""), "");
        assert_eq!(label_text("a\nb"), "Ab");
        assert_eq!(label_text(&"x".repeat(500)).chars().count(), 40);
    }

    #[test]
    fn only_plain_web_addresses_open() {
        assert_eq!(web_target("https://harborworks.example/a?b=1").as_deref(), Some("https://harborworks.example/a?b=1"));
        assert_eq!(web_target(" HTTP://example.org ").as_deref(), Some("HTTP://example.org"));
        assert_eq!(web_target("harborworks.example/team").as_deref(), Some("https://harborworks.example/team"));
        for bad in [
            "",
            "   ",
            "localhost",
            "javascript:alert(1)",
            "javascript://example.org/%0aalert(1)",
            "file:///C:/Windows/system32/calc.exe",
            "data:text/html,<script>1</script>",
            "mailto:a@example.org",
            "example.org:8080",
            "https://",
            "https://[::1",
            "https://[zz]/",
            "https://exa mple.org",
            "https://example.org/\u{0}",
            "ms-settings://x",
        ] {
            assert_eq!(web_target(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn birthdays_read_well_and_survive_bad_dates() {
        assert_eq!(birthday_text(day(1988, 3, 14)), "March 14, 1988");
        assert_eq!(birthday_text(day(9999, 12, 31)), "December 31, 9999");
        // A date a damaged file could carry: the fields are public.
        assert_eq!(birthday_text(Date { year: -5, month: 0, day: 0 }), "-005-00-00");
        assert_eq!(birthday_text(Date { year: i32::MAX, month: 13, day: 255 }), format!("{}-13-255", i32::MAX));

        let today = day(2026, 10, 14);
        assert_eq!(next_birthday(day(1988, 3, 14), today), Some((day(2027, 3, 14), 39)));
        assert_eq!(next_birthday(day(1988, 12, 1), today), Some((day(2026, 12, 1), 38)));
        assert_eq!(next_birthday(day(1988, 10, 14), today), Some((today, 38)));
        // February 29th falls on the 28th outside leap years.
        assert_eq!(next_birthday(day(2000, 2, 29), today), Some((day(2027, 2, 28), 27)));
        assert_eq!(next_birthday(day(2000, 2, 29), day(2028, 1, 1)), Some((day(2028, 2, 29), 28)));
        // No believable age: born this year, in the future, or a card without a real year.
        assert_eq!(next_birthday(day(2026, 12, 1), today), None);
        assert_eq!(next_birthday(day(9999, 12, 31), today), None);
        assert_eq!(next_birthday(day(1, 1, 1), today), None);
        // Clocks and dates at the edges of what a date can hold.
        assert_eq!(next_birthday(day(9990, 1, 1), day(9999, 12, 31)), None);
        assert_eq!(next_birthday(day(9990, 12, 31), day(9999, 12, 31)), Some((day(9999, 12, 31), 9)));
        assert_eq!(next_birthday(Date { year: 1990, month: 0, day: 0 }, today), None);
        assert_eq!(next_birthday(Date { year: i32::MIN, month: 200, day: 200 }, Date { year: i32::MAX, month: 12, day: 31 }), None);
    }

    #[test]
    fn recent_mail_is_the_newest_from_or_to_any_address() {
        let mut mb = Mailbox::with_local_account("Me", "me@example.org");
        let inbox = mb.inbox().unwrap();
        let mut add = |from: &str, to: &str, cc: &str, date: i64| {
            let list = |s: &str| if s.is_empty() { Vec::new() } else { vec![Address::new("", s)] };
            mb.add_message(Message { folder: inbox, from: Address::new("", from), to: list(to), cc: list(cc), date, ..Default::default() }).unwrap()
        };
        let a = add("dev@patel.example", "me@example.org", "", 100);
        let b = add("me@example.org", "DEV@Patel.example", "", 300);
        let c = add("someone@example.org", "me@example.org", " dev@home.example ", 200);
        add("someone@example.org", "me@example.org", "", 400);
        // A message without a sender or recipients, for the contact without an address below.
        add("", "", "", 500);
        let mut dev = contact("Dev", "Patel");
        dev.emails = vec![Labeled::new("work", " dev@patel.example"), Labeled::new("home", "dev@home.example"), Labeled::new("old", "  ")];
        assert_eq!(recent_mail(&mb, &dev, 8), [b, c, a]);
        assert_eq!(recent_mail(&mb, &dev, 2), [b, c]);
        assert!(recent_mail(&mb, &dev, 0).is_empty());
        // No address, no mail: an empty address must not match the message nobody sent.
        assert!(recent_mail(&mb, &contact("No", "Mail"), 8).is_empty());
        assert!(recent_mail(&mb, &Contact::default(), 8).is_empty());
    }

    #[test]
    fn the_contact_forms_delete_asks_first() {
        // #186: Delete in a contact's form gives way to the question, and only its answer deletes.
        let mut w = testing::window("view.people");
        let id = people::sorted(&w.state().session, "")[0];
        w.state_mut().run("people.select", serde_json::json!({"id": id})).unwrap();
        w.run_steps(2);
        testing::key(&mut w, egui::Key::Enter);
        assert_eq!(testing::dialog(&w), Some("editContact"));
        use egui_kittest::kittest::Queryable;
        let press = |w: &mut egui_kittest::Harness<'_, MailApp>| {
            // (A window that has just opened finds its place first.)
            w.run_steps(5);
            let pos = w.query_all_by_label("Delete").last().expect("a Delete button").rect().center();
            testing::click(w, pos, egui::PointerButton::Primary, 1);
        };
        press(&mut w);
        assert_eq!(testing::dialog(&w), Some("confirm"));
        assert!(w.state().session.mailbox.contact(id).is_some());
        press(&mut w);
        assert_eq!(testing::dialog(&w), None);
        assert!(w.state().session.mailbox.contact(id).is_none());
    }

    #[test]
    fn clicks_and_keys_work_the_list() {
        use egui::{Key, PointerButton};
        let mut w = testing::window("view.people");
        let ids = people::sorted(&w.state().session, "");
        let at = |w: &egui_kittest::Harness<'_, MailApp>, id: Id| w.state().hit.rect("contact", id).unwrap().center();
        let selected = |w: &egui_kittest::Harness<'_, MailApp>| w.state().session.sel.contact;
        assert!(ids.len() >= 4 && selected(&w).is_none());

        // A click selects; Up and Down walk the list in its order and stop at its ends.
        let pos = at(&w, ids[1]);
        testing::click(&mut w, pos, PointerButton::Primary, 1);
        assert_eq!(selected(&w), Some(ids[1]));
        testing::key(&mut w, Key::ArrowUp);
        testing::key(&mut w, Key::ArrowUp);
        assert_eq!(selected(&w), Some(ids[0]));
        testing::key(&mut w, Key::ArrowDown);
        testing::key(&mut w, Key::ArrowDown);
        assert_eq!(selected(&w), Some(ids[2]));

        // Enter opens the contact; while its form is up, the keys are the form's.
        testing::key(&mut w, Key::Enter);
        assert_eq!(testing::dialog(&w), Some("editContact"));
        testing::key(&mut w, Key::ArrowDown);
        assert_eq!(selected(&w), Some(ids[2]));
        w.state_mut().dialog = None;
        w.step();

        // A double click selects the row and opens it.
        let pos = at(&w, ids[3]);
        testing::click(&mut w, pos, PointerButton::Primary, 2);
        assert_eq!((selected(&w), testing::dialog(&w)), (Some(ids[3]), Some("editContact")));
        w.state_mut().dialog = None;
        w.step();

        // A right click selects the row under the pointer and opens its menu, which keeps the keys.
        let pos = at(&w, ids[0]);
        testing::click(&mut w, pos, PointerButton::Secondary, 1);
        assert_eq!(selected(&w), Some(ids[0]));
        let ctx = w.state().ctx.clone().unwrap();
        assert!(egui::Popup::is_any_open(&ctx));
        testing::key(&mut w, Key::ArrowDown);
        assert_eq!(selected(&w), Some(ids[0]));
    }

    #[test]
    fn the_search_narrows_the_list_and_the_keys_stay_inside_it() {
        let mut w = testing::window("view.people");
        let all = people::sorted(&w.state().session, "");
        w.state_mut().execute("people.select", json!({"id": all[0]})).unwrap();
        w.state_mut().execute("people.search", json!({"query": "harbor"})).unwrap();
        w.run_steps(2);
        let found = people::sorted(&w.state().session, "harbor");
        assert!(!found.is_empty() && found.len() < all.len() && !found.contains(&all[0]));
        // Only what matches is drawn.
        for id in &all {
            assert_eq!(w.state().hit.rect("contact", *id).is_some(), found.contains(id), "{id}");
        }
        // The selection is outside the list: Down starts at its top.
        testing::key(&mut w, egui::Key::ArrowDown);
        assert_eq!(w.state().session.sel.contact, found.first().copied());
        // Nothing matches: nothing is drawn, and the keys do nothing.
        w.state_mut().execute("people.search", json!({"query": "zzzz"})).unwrap();
        w.run_steps(2);
        assert!(all.iter().all(|id| w.state().hit.rect("contact", *id).is_none()));
        testing::key(&mut w, egui::Key::ArrowDown);
        assert_eq!(w.state().session.sel.contact, found.first().copied());
    }

    #[test]
    fn the_list_is_sorted_again_only_when_it_changes() {
        let mut w = testing::window("view.people");
        let ctx = w.state().ctx.clone().unwrap();
        let first = list_layout(w.state(), &ctx);
        let last = first.rows.last().unwrap().id;
        assert_eq!(first.rows.len(), w.state().session.mailbox.contacts.len());
        // More frames, a selection, a change somewhere else in the mailbox's view: the same list.
        w.state_mut().execute("people.select", json!({"id": last})).unwrap();
        w.run_steps(3);
        assert!(Arc::ptr_eq(&first, &list_layout(w.state(), &ctx)));
        // A contact changes: it moves up under Favorites.
        w.state_mut().execute("people.favorite", json!({"id": last})).unwrap();
        let starred = list_layout(w.state(), &ctx);
        assert!(!Arc::ptr_eq(&first, &starred));
        assert!(starred.rows.iter().take_while(|r| r.id != last).all(|r| r.heading != Some(Group::Letter('B'))));
        assert_ne!(starred.rows.last().map(|r| r.id), Some(last));
        // Undo puts the list back; a search narrows it; clearing the search brings it back.
        w.state_mut().execute("edit.undo", json!({})).unwrap();
        assert_eq!(*list_layout(w.state(), &ctx), *first);
        w.state_mut().execute("people.search", json!({"query": "harbor"})).unwrap();
        assert!(list_layout(w.state(), &ctx).rows.len() < first.rows.len());
        w.state_mut().execute("people.search", json!({"query": ""})).unwrap();
        assert_eq!(*list_layout(w.state(), &ctx), *first);
        // Recent mail is looked up again only when the mailbox changes, too.
        let c = w.state().session.mailbox.contact(last).cloned().unwrap();
        let recent = recent_for(w.state(), &ctx, &c);
        assert!(Arc::ptr_eq(&recent, &recent_for(w.state(), &ctx, &c)));
        assert_eq!(*recent, recent_mail(&w.state().session.mailbox, &c, RECENT));
    }

    #[test]
    fn a_very_long_list_paints_only_what_shows() {
        let mut mb = Mailbox::with_local_account("Me", "me@example.org");
        for i in 0..50_000u32 {
            mb.add_contact(Contact {
                first: format!("Person {i:05}"),
                last: format!("Family {:03}", i % 700),
                emails: vec![Labeled::new("work", format!("p{i}@example.org"))],
                favorite: i % 5000 == 0,
                ..Default::default()
            });
        }
        let mut w = testing::window_on(mb, "view.people");
        let ids = people::sorted(&w.state().session, "");
        assert_eq!(ids.len(), 50_000);
        let drawn = |w: &egui_kittest::Harness<'_, MailApp>| ids.iter().filter(|id| w.state().hit.rect("contact", **id).is_some()).count();
        assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
        assert!(w.state().hit.rect("contact", ids[0]).is_some());
        // The far end of the list: selecting it scrolls it into view, and still only a screenful
        // is painted.
        let last = ids[49_999];
        w.state_mut().execute("people.select", json!({"id": last})).unwrap();
        w.run_steps(40);
        assert!(w.state().hit.rect("contact", last).is_some());
        assert!(w.state().hit.rect("contact", ids[0]).is_none());
        assert!((5..40).contains(&drawn(&w)), "{}", drawn(&w));
        // And back, a row at a time from the keyboard.
        testing::key(&mut w, egui::Key::ArrowUp);
        assert_eq!(w.state().session.sel.contact, Some(ids[49_998]));
    }
}
