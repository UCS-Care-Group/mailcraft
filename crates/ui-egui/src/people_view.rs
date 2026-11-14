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
use mailcraft_model::{AccountChoice, Address, Contact, Id, Mailbox};
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
/// The widest the account beside a row's name gets.
const PLACE_W: f32 = 200.0;

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let layout = list_layout(app, ui.ctx());
    keyboard(app, ui, &layout);
    keep_picked(app);
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    let max = (ui.available_width() - 320.0).max(260.0);
    egui::Panel::left("people_list").default_size(320.0f32.min(max)).size_range(240.0..=max).resizable(true).frame(frame).show(ui, |ui| {
        crate::nav::module_foot(app, ui);
        list(app, ui, &layout);
    });
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| card(app, ui, layout.total));
}

/// The contacts picked together are those still there and in the list, and only while the
/// selected contact is one of them: selecting another one alone (a click, the arrow keys, a
/// command) ends the pick.
pub(crate) fn keep_picked(app: &mut MailApp) {
    let contacts = &app.session.mailbox.contacts;
    let shown = people::shown_account(&app.session);
    app.people_picked.retain(|id| contacts.get(id).is_some_and(|c| shown.shows(&c.origin)));
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
/// with none first, then the categories A to Z, in the order the items come in under each. Takes
/// each item's id, its account and its categories; gives each row's heading (an index into the
/// names given back, `None` for no category) and the names.
pub(crate) fn by_category<'a>(
    s: &mailcraft_engine::Session,
    items: impl IntoIterator<Item = (Id, Id, &'a [String])>,
) -> (Vec<(Id, Option<u32>)>, Vec<String>) {
    use mailcraft_engine::list::{category_names, category_spellings};
    let items: Vec<(Id, Id, &[String])> = items.into_iter().collect();
    let spelling = category_spellings(s, items.iter().map(|(_, account, categories)| (*account, *categories)));
    // (the heading's folded name, the item)
    let mut placed: Vec<(Option<String>, Id)> = Vec::new();
    for (id, _, categories) in items {
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
    /// How many contacts the account in view has, whatever the search.
    total: usize,
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
    key: (u64, usize, String, PeopleSort, AccountChoice),
    list: Arc<PeopleList>,
}

/// The list for the account and the search in view. Sorting every contact is the slow part, so the
/// result is kept until the mailbox, the account, the search or the arrangement changes.
fn list_layout(app: &MailApp, ctx: &egui::Context) -> Arc<PeopleList> {
    let slot = egui::Id::unique("people_list_layout");
    let s = &app.session;
    let query = &s.view.people_search;
    let shown = people::shown_account(s);
    let key = (s.rev(), s.mailbox.contacts.len(), query.clone(), s.view.people_sort, shown);
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
            let (rows, filed) = by_category(s, contacts.map(|c| (c.id, c.origin.account, c.categories.as_slice())));
            names = filed;
            Layout::new(ROW_H, rows.into_iter().map(|(id, i)| (id, i.map_or(Group::NoCategory, Group::Categories))))
        }
    };
    let total = s.mailbox.contacts.values().filter(|c| shown.shows(&c.origin)).count();
    let list = Arc::new(PeopleList { layout, names, total });
    ctx.data_mut(|d| d.insert_temp(slot, CachedLayout { key, list: list.clone() }));
    list
}

/// How many contacts the list has, whatever the search: the status bar's count.
pub(crate) fn shown_total(app: &MailApp, ctx: &egui::Context) -> usize {
    list_layout(app, ctx).total
}

// ---- small pure pieces ----

/// Text for one line of a list or a card, as a name is shown (#454): one line, without control
/// characters or characters that show nothing, spaces run together, at most `max` characters,
/// the last an ellipsis where it was cut. Whatever a file holds, a line costs no more than `max`
/// characters to lay out.
pub(crate) use mailcraft_names::shown as one_line;

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
    let total = layout.total;
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
    let accounts = people::contact_accounts(&app.session.mailbox);
    let shown = people::shown_account(&app.session);
    if !accounts.is_empty() {
        ui.add_space(2.0);
        if let Some(choice) = widgets::account_choice(ui, &app.session.mailbox, &accounts, shown) {
            let _ = app.run("view.peopleAccount", json!({"value": choice.code()}));
        }
        ui.add_space(8.0);
    }
    let line = ui.cursor().min.y;
    ui.painter().line_segment([pos2(header.min.x, line), pos2(header.max.x, line)], Stroke::new(1.0, t.border));

    if layout.rows.is_empty() {
        empty_list(app, ui, searching);
        return;
    }
    let selected = app.session.sel.contact;
    let at = chosen_row(ui.ctx(), PEOPLE_ROW, &layout.layout, selected);
    let reveal = selection_changed(ui, "people_list_seen", at).then_some(at).flatten();
    let labelled = labelled(app);
    show_rows(ui, layout, reveal, |g| group_label(g, &layout.names), |ui, id, copy, rect| contact_row(app, ui, id, copy, rect, labelled));
}

/// Whether each row says where its contact is: every account's contacts are in one list.
fn labelled(app: &MailApp) -> bool {
    people::shown_account(&app.session) == AccountChoice::All && !people::contact_accounts(&app.session.mailbox).is_empty()
}

fn empty_list(app: &mut MailApp, ui: &mut Ui, searching: bool) {
    let t = Tokens::get(ui.ctx());
    // A new contact is kept on this computer, so it isn't offered as what this list lacks.
    let of_account = matches!(people::shown_account(&app.session), AccountChoice::Account(_));
    ui.vertical_centered(|ui| {
        ui.add_space(56.0);
        let (r, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
        icons::paint(ui.painter(), r, if searching { "search" } else { "people" }, t.border_strong, t.border_strong);
        ui.add_space(10.0);
        let text = if searching {
            tl!("Nobody matches the search")
        } else if of_account {
            // (Its contacts may not have come down from its server yet.)
            tl!("No contacts from this account yet")
        } else {
            tl!("No contacts yet")
        };
        ui.label(RichText::new(text).font(regular(12.5)).color(t.text_dim));
        if !searching && !of_account {
            ui.add_space(14.0);
            if widgets::primary_button(ui, tl!("New Contact"), true).clicked() {
                let _ = app.run("people.new", json!({}));
            }
        }
    });
}

fn contact_row(app: &mut MailApp, ui: &mut Ui, id: Id, copy: u16, rect: Rect, labelled: bool) {
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
        p.rect_filled(rect, 0.0, t.hover_subtle);
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
    // From the right of the name's line: where it is.
    let mut name_w = width;
    if labelled {
        let place = widgets::place_name(&app.session.mailbox, c.origin.account);
        let wide = p.layout_no_wrap(place.clone(), regular(11.5), t.text_dim).size().x.min(PLACE_W).min(width * 0.5);
        name_w = (width - paint_clipped(p, pos2(right - wide, name_y), wide, &place, regular(11.5), t.text_dim) - 8.0).max(0.0);
    }
    paint_clipped(p, pos2(left, name_y), name_w, &shown_name(&c), if selected { semibold(13.0) } else { regular(13.0) }, t.text);
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
            let contact = app.session.mailbox.contact(id);
            let (account, carried) = (contact.map(|c| c.origin.account), contact.map(|c| c.categories.clone()).unwrap_or_default());
            crate::ribbon::category_menu(
                ui,
                app,
                account.as_slice(),
                &carried,
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

/// The selected contact, with `total` contacts in the list.
fn card(app: &mut MailApp, ui: &mut Ui, total: usize) {
    let t = Tokens::get(ui.ctx());
    let Some(c) = app.session.sel.contact.and_then(|id| app.session.mailbox.contacts.get(&id).cloned()) else {
        // With nobody to select, the list's own empty state says what to do.
        if total == 0 {
            return;
        }
        let r = ui.available_rect_before_wrap();
        let y = r.min.y + r.height() * 0.38;
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.center().x, y), vec2(56.0, 56.0)), "people", t.border_strong, t.border_strong);
        ui.painter().text(pos2(r.center().x, y + 50.0), Align2::CENTER_CENTER, tl!("Select a contact to see the details"), regular(13.0), t.text_dim);
        return;
    };
    // One scroll state for the card, at the top again for another contact: not one in egui's
    // memory for every contact ever shown.
    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("contact_card");
    let shown = egui::Id::unique("mailcraft-contact-card-shown");
    if ui.ctx().data(|d| d.get_temp::<Id>(shown)) != Some(c.id) {
        ui.ctx().data_mut(|d| d.insert_temp(shown, c.id));
        area = area.vertical_scroll_offset(0.0);
    }
    area.show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 28, right: 28, top: 24, bottom: 28 }).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(CARD_W));
            ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
            card_header(app, ui, &c);
            ui.add_space(12.0);
            card_actions(app, ui, &c);
            card_contact(app, ui, &c);
            card_account(app, ui, &c);
            card_places(ui, &c);
            card_birthday(app, ui, &c);
            crate::security::card_section(app, ui, &c);
            if !c.notes.trim().is_empty() {
                section(ui, tl!("Notes"));
                ui.add(egui::Label::new(RichText::new(clip_chars(c.notes.trim(), NOTES_MAX)).font(regular(13.0)).color(t.text)).wrap());
            }
            if !c.imported_from.is_empty() {
                ui.add_space(8.0);
                let from = one_line(&c.imported_from, 200);
                ui.add(
                    egui::Label::new(
                        RichText::new(crate::i18n::fmt(tl!("Imported from {file}"), &[("file", &from)])).font(regular(12.0)).color(t.text_dim),
                    )
                    .wrap(),
                );
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
                chips(app, ui, c.origin.account, &c.categories);
            }
        });
    });
}

/// The category chips of an item of `account`, wrapping onto more lines when they don't fit on
/// one.
pub(crate) fn chips(app: &MailApp, ui: &mut Ui, account: Id, categories: &[String]) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        for name in categories.iter().take(40) {
            let shown = one_line(name, 40);
            let w = text_width(ui, &shown, regular(10.5)) + 12.0;
            let (r, _) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
            widgets::category_chip(ui, pos2(r.min.x, r.center().y), &shown, crate::theme::c32(app.session.mailbox.category_color_of(account, name)));
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

/// Where the contact is kept, where there is more than one place it could be.
fn kept_in(app: &MailApp, c: &Contact) -> Option<String> {
    let mailbox = &app.session.mailbox;
    (c.origin.is_remote() || !people::contact_accounts(mailbox).is_empty()).then(|| widgets::place_name(mailbox, c.origin.account))
}

/// Whether the contact is a copy of what its account's server keeps, which can't be changed here
/// (`remote::changeable`): the card says so beside whose it is, before Edit or Delete is tried.
fn read_only_here(app: &MailApp, c: &Contact) -> bool {
    mailcraft_engine::cmd::remote::keeper(&app.session.mailbox, &c.origin).is_some()
}

fn card_account(app: &MailApp, ui: &mut Ui, c: &Contact) {
    if let Some(place) = kept_in(app, c) {
        section(ui, tl!("Account"));
        if read_only_here(app, c) {
            field(ui, "account", &place, tl!("Read-only"), false).on_hover_text(tl!(
                "This account's server keeps this contact. Changes made here aren't sent back yet, so it can't be edited or deleted here. Favorite and categories are kept on this computer."
            ));
        } else {
            field(ui, "account", &place, "", false);
        }
    }
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
mod account_tests;
#[cfg(test)]
mod arrange_tests;

#[cfg(test)]
mod tests;
