//! Mail: the folder pane, the message list and the reading pane (or the message being written).
//!
//! The message list is laid out to the pixel from classic Outlook's at 100 %: every number below
//! is a distance measured there, in points (pixels at 100 %).

use std::sync::Arc;

use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::list::Row;
use mailcraft_engine::{Filter, Pane, SortBy};
use mailcraft_model::{FlagStatus, FolderKind, Id, Importance, Message};
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// What a drag out of the message list carries: the messages to file in the folder they are
/// let go over (`nav::folder_row` takes them).
pub(crate) struct MessageDrag(pub Vec<Id>);

/// A group heading in the list (Today, Yesterday…): a box with a line round it.
const GROUP_H: f32 = 25.0;
/// A group heading's text: 9 pt.
const GROUP_TEXT: f32 = 12.0;
/// The sender's name, and the tabs above the list: 11 pt.
const SENDER_TEXT: f32 = 15.0;
/// Subject, time and preview: 8 pt.
const LIST_TEXT: f32 = 11.0;
/// How the list is arranged, in its header: 9 pt.
const SORT_TEXT: f32 = 12.0;
/// The list's header (the filter's tabs, the arrangement). Its last pixel row is the line under
/// the tab in use.
const HEADER_H: f32 = 48.0;
/// From one tab in the header to the next.
const TAB_GAP: f32 = 14.0;
/// The least of a tab's name that is worth showing.
const TAB_MIN: f32 = 22.0;
/// What is in a row keeps this far from the list's left edge: the line under it, the box of a
/// group heading, the bar an unread message wears and the fill of a selected one start here.
const ROW_INSET: f32 = 2.0;
/// How wide the read/unread switch at a row's left edge is.
const READ_STRIP: f32 = 12.0;
/// A row's quick actions (flag, delete) under the pointer: the button, and the icon in it.
const QUICK_BUTTON: f32 = 24.0;
const QUICK_ICON: f32 = 18.0;
/// The marks on the sender's line (attachment, importance, flag…), and from one to the next.
const MARK: f32 = 16.0;
const MARK_PITCH: f32 = 20.0;
/// Where a row's text starts, from the list's left edge.
const TEXT_LEFT: f32 = 30.0;
/// Rows end this far from the list's right edge: a gap, the scroll bar's lane, and its margin.
const SCROLL_LANE: f32 = SCROLL_GAP + SCROLL_TRACK + SCROLL_MARGIN;
const SCROLL_GAP: f32 = 7.0;
const SCROLL_TRACK: f32 = 17.0;
const SCROLL_MARGIN: f32 = 5.0;
/// The scroll bar's handle, in the middle of its lane.
const SCROLL_HANDLE: f32 = 9.0;
/// The ends of the lane, each with an arrow in it: the handle travels between them.
const SCROLL_END: f32 = 19.0;
/// The time ends this far from a row's right end. What is right of it is the flag's column.
const TIME_RIGHT: f32 = 28.0;
/// Sender, subject and preview end this far from a row's right end, where the time's column
/// starts: every row's text stops at the same place, whatever its time.
const TEXT_RIGHT: f32 = 98.0;
/// The middle of the sender's capitals, from a row's top: they stand 15 below the line above.
const SENDER_Y: f32 = 21.0;
/// The middle of the marks and the quick actions on the sender's line, from a row's top: a little
/// above the sender's capitals, so a button clears the time under it.
const MARK_Y: f32 = SENDER_Y - 3.0;
/// The middle of the subject's (and the time's) capitals, from a row's top.
const SUBJECT_Y: f32 = 37.5;
/// The top of the preview's first line box, from a row's top: its text ends 15 above the line
/// under a row.
const PREVIEW_TOP: f32 = 45.0;
/// From one line of preview to the next.
const PREVIEW_PITCH: f32 = 13.0;
/// A row with one line of preview, the line under it included.
const ROW_H: f32 = 72.0;
/// What a row loses without a preview: the preview's line and the gap above it.
const PREVIEW_FIRST: f32 = 15.0;

/// While messages are being dragged: a small label by the pointer saying how many.
fn drag_ghost(ui: &Ui) {
    let Some(drag) = egui::DragAndDrop::payload::<MessageDrag>(ui.ctx()) else { return };
    let Some(pos) = ui.ctx().pointer_interact_pos() else { return };
    let t = Tokens::get(ui.ctx());
    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    let text = if drag.0.len() == 1 { tl!("1 message").to_string() } else { format!("{} {}", drag.0.len(), tl!("messages")) };
    egui::Area::new(egui::Id::new("message_drag_ghost")).order(egui::Order::Tooltip).fixed_pos(pos + vec2(14.0, 16.0)).interactable(false).show(
        ui.ctx(),
        |ui| {
            egui::Frame::NONE
                .fill(t.menu)
                .stroke(Stroke::new(1.0, t.border_strong))
                .corner_radius(4.0)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(text).font(regular(12.0)).color(t.text));
                });
        },
    );
}

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    drag_ghost(ui);
    if app.session.view.folder_pane {
        crate::nav::folder_pane(app, ui);
    } else {
        // Hidden: a narrow strip stays, with the button that brings the pane back.
        crate::nav::folder_pane_strip(app, ui);
    }
    if app.session.view.todo_bar {
        todo_bar(app, ui);
    }
    let composing = app.session.compose.is_some();
    let pane = app.session.view.reading_pane;
    // The list takes the whole width when the reading pane is off, unless a message is being written.
    if pane == Pane::Off && !composing {
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| list(app, ui));
        return;
    }
    let frame = egui::Frame::NONE.fill(t.surface).stroke(Stroke::new(1.0, t.border));
    if pane == Pane::Bottom {
        let h = (ui.available_height() * 0.42).clamp(140.0, 520.0);
        egui::Panel::top("message_list")
            .default_size(h)
            .size_range(120.0..=(ui.available_height() - 140.0).max(160.0))
            .resizable(true)
            .frame(frame)
            .show(ui, |ui| list(app, ui));
    } else {
        let max = (ui.available_width() - 260.0).max(260.0);
        let shown = crate::i18n::start_panel("message_list")
            .default_size(app.ui.list_width.min(max))
            .size_range(240.0..=max)
            .resizable(true)
            .frame(frame)
            .show(ui, |ui| list(app, ui));
        app.ui.list_width = shown.response.rect.width();
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| {
        if composing {
            crate::compose::show(app, ui);
        } else {
            crate::reading::show(app, ui);
        }
    });
}

/// The To-Do Bar down the right side: the appointments of the days ahead, then the tasks still
/// to do. A click opens what was clicked in its module.
fn todo_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let frame = egui::Frame::NONE.fill(t.panel).stroke(Stroke::new(1.0, t.border)).inner_margin(egui::Margin::same(10));
    crate::i18n::end_panel("todo_bar").default_size(240.0).size_range(180.0..=360.0).resizable(true).frame(frame).show(ui, |ui| {
        let (events, tasks) = mailcraft_engine::cmd::view::todo_items(&app.session, 30);
        let today = app.session.today();
        let twelve = app.session.prefs.twelve_hour;
        egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("todo_bar_scroll").show(ui, |ui| {
            ui.label(egui::RichText::new(tl!("Appointments")).strong());
            if events.is_empty() {
                ui.label(egui::RichText::new(tl!("Nothing in the next seven days")).color(t.text_dim).small());
            }
            let mut day = None;
            for (o, title, location, all_day) in &events {
                if day != Some(o.start.date) {
                    day = Some(o.start.date);
                    let label = if o.start.date == today { tl!("Today").to_string() } else { crate::i18n::date_medium(o.start.date) };
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(label).color(t.text_dim).small());
                }
                let when = if *all_day { tl!("All day").to_string() } else { o.start.time_short(twelve) };
                let mut text = format!("{when}  {}", crate::people_view::one_line(title, 60));
                if !location.trim().is_empty() {
                    text.push_str(&format!(" · {}", crate::people_view::one_line(location, 40)));
                }
                let resp = ui.add(egui::Button::new(egui::RichText::new(text).size(12.0)).frame(false));
                app.hit.add("todoEvent", o.event, resp.rect);
                if resp.clicked() {
                    let _ = app.run("view.calendar", json!({}));
                    let _ = app.run("view.goToDate", json!({"date": o.start.date.iso()}));
                    let _ = app.run("calendar.select", json!({"id": o.event}));
                }
            }
            ui.add_space(10.0);
            ui.label(egui::RichText::new(tl!("Tasks")).strong());
            if tasks.is_empty() {
                ui.label(egui::RichText::new(tl!("Nothing to do")).color(t.text_dim).small());
            }
            for id in tasks {
                let Some(task) = app.session.mailbox.task(id) else { continue };
                let late = task.is_overdue(today);
                let text = format!("{}  ·  {}", crate::people_view::one_line(&task.title, 60), crate::tasks_view::due_text(task, today));
                let color = if late { t.red } else { t.text };
                ui.horizontal(|ui| {
                    if ui.small_button("✓").on_hover_text(tl!("Mark Complete")).clicked() {
                        let _ = app.run("tasks.complete", json!({"id": id, "value": true}));
                    }
                    let resp = ui.add(egui::Button::new(egui::RichText::new(text).size(12.0).color(color)).frame(false));
                    app.hit.add("todoTask", id, resp.rect);
                    if resp.clicked() {
                        let _ = app.run("view.tasks", json!({}));
                        let _ = app.run("tasks.select", json!({"id": id}));
                    }
                });
            }
        });
    });
}

/// A row and the line under it: 72 with one line of preview (15 clear above the sender's
/// capitals and 15 under the preview's), 13 more for each further line, 57 with none. It depends
/// on nothing but the number of preview lines, so where a row is can be worked out without
/// laying out the rows before it.
fn row_height(preview_lines: u8) -> f32 {
    match preview_lines.min(3) {
        0 => ROW_H - PREVIEW_FIRST,
        n => ROW_H + f32::from(n - 1) * PREVIEW_PITCH,
    }
}

/// What the arrangement is called in the list's header.
fn sort_label(sort: SortBy) -> &'static str {
    match sort {
        SortBy::Date => tn!("By Date"),
        SortBy::From => tn!("By From"),
        SortBy::Subject => tn!("By Subject"),
        SortBy::Size => tn!("By Size"),
        SortBy::Importance => tn!("By Importance"),
    }
}

/// What is on top of the list, for the arrow that turns it over.
fn direction_label(sort: SortBy, descending: bool) -> &'static str {
    match (sort, descending) {
        (SortBy::Date, true) => tn!("Newest on top"),
        (SortBy::Date, false) => tn!("Oldest on top"),
        (SortBy::From | SortBy::Subject, false) => tn!("A on top"),
        (SortBy::From | SortBy::Subject, true) => tn!("Z on top"),
        (SortBy::Size, true) => tn!("Largest on top"),
        (SortBy::Size, false) => tn!("Smallest on top"),
        (SortBy::Importance, true) => tn!("High on top"),
        (SortBy::Importance, false) => tn!("Low on top"),
    }
}

/// The header above the list: All and Unread as tabs on the left (the filter in use in the
/// accent colour, with a line under it), and on the right how the list is arranged, with the
/// arrow that turns it over. The folder's name is in the window's title and in the folder pane.
fn list_header(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let rows_end = r.max.x - SCROLL_LANE;

    // The arrow: up while the list starts where its arrangement naturally starts (the newest,
    // A, the largest), down once it is turned over.
    let (sort, descending) = (app.session.view.sort, app.session.view.sort_descending);
    let arrow = Rect::from_center_size(pos2(rows_end - 11.0, r.min.y + 33.0), vec2(20.0, 22.0));
    let arrow_resp = ui.interact(arrow, ui.id().with("list_direction"), Sense::click()).on_hover_text(tl!(direction_label(sort, descending)));
    app.hit.add("listSort", 1, arrow);
    if arrow_resp.hovered() {
        ui.painter().rect_filled(arrow, 4.0, t.hover);
    }
    paint_direction(ui.painter(), arrow.center(), descending == sort.starts_high(), t.text);
    if arrow_resp.clicked() {
        let _ = app.run("view.reverseSort", json!({}));
    }

    // The arrangement and its menu.
    let label = tl!(sort_label(sort));
    let label_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), regular(SORT_TEXT), t.text).size().x);
    let label_end = rows_end - 46.0;
    let sr = Rect::from_min_max(pos2(label_end - label_w - 6.0, r.min.y + 22.0), pos2(rows_end - 25.0, r.min.y + 44.0));
    let sresp = ui.interact(sr, ui.id().with("list_sort"), Sense::click());
    app.hit.add("listSort", 0, sr);
    if sresp.hovered() {
        ui.painter().rect_filled(sr, 4.0, t.hover);
    }
    ui.painter().text(pos2(label_end, r.min.y + 33.5), Align2::RIGHT_CENTER, label, regular(SORT_TEXT), t.text);
    let chevron = pos2(rows_end - 36.0, r.min.y + 34.0);
    let line = Stroke::new(1.2, t.text);
    ui.painter().line(vec![chevron + vec2(-4.5, -2.5), chevron + vec2(0.0, 2.5), chevron + vec2(4.5, -2.5)], line);
    let filter = app.session.view.filter;
    egui::Popup::menu(&sresp).show(|ui| {
        ui.set_min_width(170.0);
        for s in SortBy::ALL {
            crate::widgets::mi_check(ui, app, s.label(), s == sort, "view.sort", json!({"by": s}));
        }
        ui.separator();
        crate::widgets::mi(ui, app, tn!("Reverse Sort"), "view.reverseSort", json!({}));
        let conv = app.session.view.conversations;
        crate::widgets::mi_check(ui, app, tn!("Show as Conversations"), conv, "view.conversations", json!({}));
        // The filters that have no tab of their own.
        ui.separator();
        for f in Filter::ALL {
            crate::widgets::mi_check(ui, app, f.label(), f == filter, "view.filter", json!({"value": f}));
        }
    });

    // The tabs. A filter that has none (Flagged, Has Attachments, To Me) shows as a third one
    // while it is in use.
    let width = |f: Filter| ui.ctx().fonts_mut(|fonts| fonts.layout_no_wrap(tl!(f.label()).to_string(), regular(SENDER_TEXT), t.text).size().x);
    let mut tabs = vec![(Filter::All, 0u64, width(Filter::All)), (Filter::Unread, 1, width(Filter::Unread))];
    if !matches!(filter, Filter::All | Filter::Unread) {
        tabs.push((filter, 2, width(filter)));
    }
    // A tab is its text with a little either side (the line under it is that wide), and the
    // next one starts 14 further on.
    let mut x = r.min.x + TEXT_LEFT - 1.0;
    // In a list too narrow for them all, the tab in use is the last to give: the others are
    // cut short, then left out (the arrangement's menu has every filter), then it is cut too.
    let mut over = tabs.iter().map(|(.., w)| w + 3.0).sum::<f32>() + TAB_GAP * tabs.len().saturating_sub(1) as f32 - (sr.min.x - 8.0 - x);
    for (.., w) in tabs.iter_mut().rev().filter(|(f, ..)| *f != filter) {
        let cut = over.clamp(0.0, (*w - TAB_MIN).max(0.0));
        *w -= cut;
        over -= cut;
    }
    while over > 0.0 {
        let Some(i) = tabs.iter().rposition(|(f, ..)| *f != filter) else { break };
        let (.., w) = tabs.remove(i);
        over -= w + 3.0 + TAB_GAP;
    }
    for (f, n, w) in tabs {
        let on = f == filter;
        let label = tl!(f.label());
        let w = if on { w - over.max(0.0) } else { w };
        if w < TAB_MIN / 2.0 {
            continue;
        }
        let tab = Rect::from_min_max(pos2(x, r.min.y + 18.0), pos2(x + w + 3.0, r.max.y));
        let resp = ui.interact(tab.expand2(vec2(5.0, 0.0)), ui.id().with(("list_tab", n)), Sense::click());
        app.hit.add("listTab", n, tab);
        paint_clipped(ui.painter(), pos2(x + 2.0, r.min.y + 31.5), w + 1.0, label, regular(SENDER_TEXT), if on { t.accent_text } else { t.text });
        let under = Rect::from_min_max(pos2(tab.min.x, r.max.y - 1.0), tab.max);
        if on {
            ui.painter().rect_filled(under, 0.0, t.accent_text);
        } else if resp.hovered() {
            ui.painter().rect_filled(under, 0.0, t.border_strong);
        }
        if resp.clicked() && !on {
            let _ = app.run("view.filter", json!({"value": f}));
        }
        x = tab.max.x + TAB_GAP;
    }
}

/// The arrow that says which way the list runs: a stem with a head, 7 wide and 12 tall.
fn paint_direction(p: &egui::Painter, c: egui::Pos2, up: bool, color: Color32) {
    let line = Stroke::new(1.2, color);
    let tip = if up { -5.5 } else { 5.5 };
    let barb = if up { -2.0 } else { 2.0 };
    p.line_segment([c + vec2(0.0, -tip), c + vec2(0.0, tip)], line);
    p.line(vec![c + vec2(-3.5, barb), c + vec2(0.0, tip), c + vec2(3.5, barb)], line);
}

/// The colour of the scroll bar's handle and arrows: half way from the list to its text.
fn scroll_ink(t: &Tokens) -> Color32 {
    t.surface.lerp_to_gamma(t.text, 0.53)
}

/// The list's scroll bar: a slim rounded handle that is always there while the list is longer
/// than its pane. Its lane and arrows are painted by [`scroll_lane`] and [`scroll_arrows`].
fn scroll_style(ui: &mut Ui, t: &Tokens) {
    let style = ui.style_mut();
    style.spacing.scroll = egui::style::ScrollStyle {
        // Over the lane the rows leave free, not beside the content: rows are as wide whether
        // the list scrolls or not.
        floating: true,
        floating_allocated_width: 0.0,
        bar_width: SCROLL_HANDLE,
        floating_width: SCROLL_HANDLE,
        bar_inner_margin: 0.0,
        bar_outer_margin: SCROLL_MARGIN + (SCROLL_TRACK - SCROLL_HANDLE) / 2.0,
        handle_min_length: 17.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 1.0,
        active_handle_opacity: 1.0,
        interact_handle_opacity: 1.0,
        fade: egui::style::ScrollFadeStyle { strength: 0.0, ..Default::default() },
        ..egui::style::ScrollStyle::solid()
    };
    let ink = scroll_ink(t);
    let round = egui::CornerRadius::same(5);
    let widgets = &mut style.visuals.widgets;
    (widgets.inactive.bg_fill, widgets.inactive.corner_radius) = (ink, round);
    (widgets.hovered.bg_fill, widgets.hovered.corner_radius) = (ink.lerp_to_gamma(t.text, 0.35), round);
    (widgets.active.bg_fill, widgets.active.corner_radius) = (ink.lerp_to_gamma(t.text, 0.6), round);
}

/// The lane the scroll bar's handle runs in, right of the rows: a step darker than the list.
fn scroll_lane(outer: Rect) -> Rect {
    Rect::from_min_max(pos2(outer.max.x - SCROLL_MARGIN - SCROLL_TRACK, outer.min.y), pos2(outer.max.x - SCROLL_MARGIN, outer.max.y))
}

/// The arrows at the ends of the scroll bar's lane: a click moves the list by a row, and holding
/// one keeps it moving. They sit on top of the bar, so they are asked after the list is drawn.
fn scroll_arrows(app: &mut MailApp, ui: &mut Ui, lane: Rect, out: &egui::scroll_area::ScrollAreaOutput<()>, row_h: f32) {
    let t = Tokens::get(ui.ctx());
    let furthest = (out.content_size.y - out.inner_rect.height()).max(0.0);
    for up in [true, false] {
        let zone = if up {
            Rect::from_min_max(lane.min, pos2(lane.max.x, lane.min.y + SCROLL_END))
        } else {
            Rect::from_min_max(pos2(lane.min.x, lane.max.y - SCROLL_END), lane.max)
        };
        // It takes drags too, so that a press on it is not one on the bar underneath.
        let resp = ui.interact(zone, ui.id().with(("list_scroll_arrow", up)), Sense::click_and_drag());
        app.hit.add("listScroll", u64::from(!up), zone);
        // A triangle 9 wide and 7 tall, its point 5 from the end of the lane.
        let (tip, base) = if up { (zone.min.y + 5.0, zone.min.y + 12.0) } else { (zone.max.y - 5.0, zone.max.y - 12.0) };
        let x = zone.center().x;
        let color = if resp.hovered() { t.text } else { scroll_ink(&t) };
        ui.painter().add(egui::Shape::convex_polygon(vec![pos2(x, tip), pos2(x + 4.5, base), pos2(x - 4.5, base)], color, Stroke::NONE));
        let held = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.press_start_time().is_some_and(|since| i.time - since > 0.35));
        let by = if resp.clicked() {
            row_h
        } else if held {
            row_h * 14.0 * ui.input(|i| i.stable_dt).min(0.1)
        } else {
            0.0
        };
        if by > 0.0 {
            let mut state = out.state;
            state.offset.y = (state.offset.y + if up { -by } else { by }).clamp(0.0, furthest);
            state.store(ui.ctx(), out.id);
            ui.ctx().request_repaint();
        }
    }
}

fn list(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    list_header(app, ui);
    let rows = app.session.rows();
    // A search that reads the text of messages goes as far into older mail as its time allows,
    // and says how far that was.
    if app.session.unsearched > 0 && !app.session.view.search.trim().is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, t.panel);
        let n = app.session.unsearched;
        let text = if n == 1 {
            tl!("1 older message wasn't searched: its text isn't loaded yet").to_string()
        } else {
            crate::i18n::fmt(tl!("{n} older messages weren't searched: their text isn't loaded yet"), &[("n", &n.to_string())])
        };
        paint_clipped(ui.painter(), pos2(r.min.x + 10.0, r.min.y + 4.0), r.width() - 20.0, &text, regular(11.5), t.text_dim);
    }
    if rows.is_empty() {
        let r = ui.available_rect_before_wrap();
        let msg = if !app.session.view.search.trim().is_empty() {
            tl!("Nothing matches the search.")
        } else if app.session.view.filter != Filter::All {
            tl!("Nothing matches this filter.")
        } else {
            tl!("Nothing here yet.")
        };
        ui.painter().text(pos2(r.center().x, r.min.y + 60.0), Align2::CENTER_CENTER, msg, regular(12.5), t.text_dim);
        return;
    }
    let row_h = row_height(app.session.prefs.preview_lines);
    // Where each row starts: rows are the same height, group headings add theirs.
    let mut tops = Vec::with_capacity(rows.len());
    let mut y = 0.0f32;
    for row in rows.iter() {
        if row.group.is_some() {
            y += GROUP_H;
        }
        tops.push(y);
        y += row_h;
    }
    let total = y;
    let reveal = std::mem::take(&mut app.reveal_selection);
    let focus = app.session.sel.focus();
    let outer = ui.available_rect_before_wrap();
    let lane = scroll_lane(outer);
    let scrolls = total > outer.height();
    if scrolls {
        ui.painter().rect_filled(lane, SCROLL_TRACK / 2.0, if t.dark { t.panel } else { t.canvas });
    }
    // The handle keeps clear of the arrows at the ends of its lane (a lane too short for them
    // has none).
    let arrows = scrolls && lane.height() >= 4.0 * SCROLL_END;
    let travel = if arrows { outer.shrink2(vec2(0.0, SCROLL_END)) } else { outer };
    let plain = ui.style().clone();
    scroll_style(ui, &t);
    let out =
        egui::ScrollArea::vertical().id_salt("message_list").auto_shrink([false, false]).scroll_bar_rect(travel).show_viewport(ui, |ui, viewport| {
            // The bar's looks are the bar's alone.
            ui.set_style(plain);
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
            let origin = area.min;
            // Rows end where the scroll bar's lane starts, whether the list scrolls or not.
            let width = (area.width() - SCROLL_LANE).max(0.0);
            // Only the rows in view are laid out.
            let first = tops.partition_point(|top| top + row_h < viewport.min.y).saturating_sub(1);
            for (i, row) in rows.iter().enumerate().skip(first) {
                let Some(top) = tops.get(i).copied() else { break };
                if top - GROUP_H > viewport.max.y {
                    break;
                }
                if let Some(group) = &row.group {
                    // A box a step lighter than the list (a step darker on a light one), with a line round it.
                    let gr = Rect::from_min_max(pos2(origin.x + ROW_INSET, origin.y + top - GROUP_H), pos2(origin.x + width, origin.y + top));
                    let fill = t.surface.lerp_to_gamma(t.selection, if t.dark { 1.0 } else { 0.45 });
                    let line = Stroke::new(1.0, t.border_strong);
                    ui.painter().rect_filled(gr, 0.0, fill);
                    // The line under the row above is this box's top edge, except for the first box.
                    let (left, right, bottom) = (gr.min.x + 0.5, gr.max.x - 0.5, gr.max.y - 0.5);
                    let top = if i == 0 { gr.min.y + 0.5 } else { gr.min.y };
                    ui.painter().line_segment([pos2(left, top), pos2(left, gr.max.y)], line);
                    ui.painter().line_segment([pos2(right, top), pos2(right, gr.max.y)], line);
                    ui.painter().line_segment([pos2(gr.min.x, bottom), pos2(gr.max.x, bottom)], line);
                    if i == 0 {
                        ui.painter().line_segment([pos2(gr.min.x, top), pos2(gr.max.x, top)], line);
                    }
                    paint_clipped(ui.painter(), pos2(gr.min.x + 10.0, gr.center().y), gr.width() - 16.0, tl!(group), semibold(GROUP_TEXT), t.text);
                }
                let rect = Rect::from_min_size(pos2(origin.x, origin.y + top), vec2(width, row_h));
                if reveal && Some(row.id) == focus {
                    ui.scroll_to_rect(rect, None);
                }
                message_row(app, ui, row, rect);
            }
        });
    if arrows {
        scroll_arrows(app, ui, lane, &out, row_h);
    }
}

/// The previews of the rows the list drew last, so that drawing them again reads no message a
/// second time. Kept for as long as the mailbox stays as it is.
#[derive(Clone, Default)]
struct Previews {
    /// The mailbox's revision the lines were made at.
    rev: u64,
    /// Per message: what its line was made from, and the line.
    lines: std::collections::HashMap<Id, (PreviewOf, Arc<str>)>,
}

/// What a preview was made from: where the message and its body are and how long they are (a
/// body that is written again is somewhere else, or another length), and how much was asked for.
type PreviewOf = (usize, usize, usize, usize);

/// Most previews kept: a few screens of rows.
const MAX_PREVIEWS: usize = 512;

/// The first words of `m` for its row: [`mailcraft_engine::list::preview`], remembered.
fn preview(ui: &Ui, rev: u64, m: &Arc<Message>, max_chars: usize) -> Arc<str> {
    let of: PreviewOf = (Arc::as_ptr(m) as usize, m.body.text.len(), m.body.html.as_ref().map_or(0, String::len), max_chars);
    ui.data_mut(|data| {
        let kept = data.get_temp_mut_or_default::<Previews>(egui::Id::new("message_list_previews"));
        if kept.rev != rev || kept.lines.len() > MAX_PREVIEWS {
            kept.lines.clear();
            kept.rev = rev;
        }
        match kept.lines.get(&m.id) {
            Some((made_of, line)) if *made_of == of => line.clone(),
            _ => {
                let line: Arc<str> = mailcraft_engine::list::preview(m, max_chars).into();
                kept.lines.insert(m.id, (of, line.clone()));
                line
            }
        }
    })
}

/// A line of dots round `r`, one pixel on and one off: the mark of the row the keys act on.
fn dotted_frame(p: &egui::Painter, r: Rect, color: Color32) {
    // A dot is a pixel of the screen, on the screen's own grid, so the line stays one of dots
    // at any scale. They go out as one mesh: shapes this small are smoothed away otherwise.
    let dot = 1.0 / p.pixels_per_point().max(0.25);
    let r = egui::emath::GuiRounding::round_to_pixels(r, p.pixels_per_point());
    let steps = |length: f32| ((length / (2.0 * dot)).ceil() as usize).min(4000);
    let mut dots = egui::Mesh::default();
    let mut put = |x: f32, y: f32| dots.add_colored_rect(Rect::from_min_size(pos2(x, y), vec2(dot, dot)), color);
    for i in 0..steps(r.width()) {
        let x = r.min.x + i as f32 * 2.0 * dot;
        put(x, r.min.y);
        put(x, r.max.y - dot);
    }
    for i in 0..steps(r.height()) {
        let y = r.min.y + i as f32 * 2.0 * dot;
        put(r.min.x, y);
        put(r.max.x - dot, y);
    }
    p.add(egui::Shape::mesh(dots));
}

/// One message in the list: the sender in the larger size, the subject with the time at the end
/// of its line, then the preview in the dimmer colour; marks at the right of the sender's line,
/// category marks and the size of a conversation at the right of the preview's.
fn message_row(app: &mut MailApp, ui: &mut Ui, row: &Row, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    // The shared pointer, not a copy of the message.
    let Some(m) = app.session.mailbox.messages.get(&row.id).cloned() else { return };
    let resp = ui.interact(rect, ui.id().with(("msg", row.id)), Sense::click_and_drag());
    app.hit.add("message", row.id, rect);
    let unread = row.unread > 0;
    // The strip down the left edge is a switch between read and unread. It sits on top of the
    // row, so a click there changes the mark and leaves the selection alone.
    let strip = Rect::from_min_max(rect.min, pos2(rect.min.x + READ_STRIP, rect.max.y));
    let read_resp = ui
        .interact(strip, ui.id().with(("msg_read", row.id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(if unread { tl!("Mark as read") } else { tl!("Mark as unread") });
    app.hit.add("messageRead", row.id, strip);
    let line1 = rect.min.y + SENDER_Y;
    let marks_y = rect.min.y + MARK_Y;
    let line2 = rect.min.y + SUBJECT_Y;
    // Quick actions on the row under the pointer, at the right of the sender's line: flag (where
    // a flagged message wears its flag) and delete. Like the read switch they sit on top of the
    // row and leave the selection alone.
    let over = ui.rect_contains_pointer(rect) && egui::DragAndDrop::payload::<MessageDrag>(ui.ctx()).is_none();
    let flag_rect = Rect::from_center_size(pos2(rect.max.x - TIME_RIGHT / 2.0, marks_y), vec2(QUICK_BUTTON, QUICK_BUTTON));
    let delete_rect = flag_rect.translate(vec2(-(QUICK_BUTTON + 2.0), 0.0));
    let flagged = m.flag == FlagStatus::Flagged;
    let quick = over.then(|| {
        let delete = ui.interact(delete_rect, ui.id().with(("msg_delete", row.id)), Sense::click()).on_hover_text(tl!("Delete"));
        let flag = ui.interact(flag_rect, ui.id().with(("msg_flag", row.id)), Sense::click()).on_hover_text(if flagged {
            tl!("Clear flag")
        } else {
            tl!("Flag for follow-up")
        });
        app.hit.add("messageDelete", row.id, delete_rect);
        app.hit.add("messageFlag", row.id, flag_rect);
        (delete, flag)
    });
    let selected = app.session.sel.messages.contains(&row.id);
    let focus = app.session.sel.focus() == Some(row.id);
    let p = ui.painter();
    // The row is what is above the line under it; its content starts a little in from the list's edge.
    let body = Rect::from_min_max(pos2(rect.min.x + ROW_INSET, rect.min.y), pos2(rect.max.x, rect.max.y - 1.0));
    if selected {
        p.rect_filled(body, 0.0, if focus { t.selection } else { t.selection.linear_multiply(0.7) });
    } else if resp.hovered() || read_resp.hovered() || quick.is_some() {
        p.rect_filled(body, 0.0, t.hover.linear_multiply(0.6));
    }
    // Unread wears the bar; under the pointer it widens, and a read message shows a faint one
    // so the switch can be found.
    let bar = Rect::from_min_max(body.min, pos2(body.min.x + if read_resp.hovered() { 6.0 } else { 4.0 }, body.max.y));
    if unread {
        p.rect_filled(bar, 0.0, t.accent);
    } else if read_resp.hovered() {
        p.rect_filled(bar, 0.0, t.accent.linear_multiply(0.4));
    }
    if selected && focus {
        // The message the keys act on has a line of dots round it.
        dotted_frame(p, body, t.text_dim);
    }
    let rule = if t.dark { t.border_strong } else { t.border };
    p.line_segment([pos2(body.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)], Stroke::new(1.0, rule));

    // Sent, drafts and outbox show who it went to rather than who wrote it.
    let outgoing = app.session.mailbox.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox));
    let who = if outgoing {
        let names: Vec<&str> = m.to.iter().map(|a| a.display()).collect();
        if names.is_empty() { tl!("(no recipients)").to_string() } else { names.join("; ") }
    } else {
        m.from.display().to_string()
    };
    let left = rect.min.x + TEXT_LEFT;
    // Where the time ends, and where the text of every line does.
    let time_end = rect.max.x - TIME_RIGHT;
    let text_end = rect.max.x - TEXT_RIGHT;

    // The sender's line. On its right, the marks: the flag in its own column at the row's end
    // (the quick actions take that place while the pointer is over the row), the others before it.
    let mut x = time_end;
    if let Some((delete, flag)) = &quick {
        for (resp, at, icon, color) in
            [(delete, delete_rect, "delete", t.text_dim), (flag, flag_rect, "flag", if flagged { t.icon } else { t.text_dim })]
        {
            if resp.hovered() {
                p.rect_filled(at, 4.0, t.pressed);
            }
            icons::paint(p, Rect::from_center_size(at.center(), vec2(QUICK_ICON, QUICK_ICON)), icon, color, t.accent);
        }
        x = delete_rect.min.x - 2.0;
    } else {
        let flag = Rect::from_center_size(flag_rect.center(), vec2(MARK, MARK));
        match m.flag {
            FlagStatus::Flagged => icons::paint(p, flag, "flag", t.icon, t.accent),
            FlagStatus::Complete => icons::paint(p, flag, "complete", t.icon, t.accent),
            FlagStatus::None => {}
        }
    }
    let mut marker = |icon: &str, color: Color32| {
        icons::paint(p, Rect::from_center_size(pos2(x - MARK / 2.0, marks_y), vec2(MARK, MARK)), icon, color, t.accent);
        x -= MARK_PITCH;
    };
    // A meeting that came by mail wears its mark; its calendar part is not one of its files.
    let (meeting, files) = crate::reading::list_marks(&m);
    if let Some(meeting) = meeting {
        marker(meeting, t.icon);
    }
    if files {
        marker("attach", t.icon);
    }
    match m.importance {
        Importance::High => marker("highImportance", t.icon),
        Importance::Low => marker("lowImportance", t.icon),
        Importance::Normal => {}
    }
    if m.pinned {
        marker("pin", t.text_dim);
    }
    if m.answered {
        marker("reply", t.text_dim);
    } else if m.forwarded {
        marker("forward", t.text_dim);
    }
    let who_font = if unread { semibold(SENDER_TEXT) } else { regular(SENDER_TEXT) };
    let who_end = text_end.min(x - 4.0);
    let mut who_x = left;
    if m.draft {
        let w = paint_clipped(p, pos2(left, line1), (who_end - left).max(0.0), tl!("[Draft]"), regular(SENDER_TEXT), t.red);
        who_x += w + 5.0;
    }
    // A message the server wouldn't send as the account waits in the Outbox for the owner
    // (`Message::hold`): changed or sent again, it goes.
    if m.hold.is_some() {
        let w = paint_clipped(p, pos2(who_x, line1), (who_end - who_x).max(0.0), tl!("[Not sent]"), regular(SENDER_TEXT), t.red);
        who_x += w + 5.0;
    }
    paint_clipped(p, pos2(who_x, line1), (who_end - who_x).max(0.0), &who, who_font, t.text);

    // The subject's line, with the time at its end.
    let date =
        crate::i18n::list_label(mailcraft_engine::list::when(&m), app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
    let (date_font, date_color) = if unread { (semibold(LIST_TEXT), t.accent_text) } else { (regular(LIST_TEXT), t.text) };
    let date_w = p.text(pos2(time_end, line2), Align2::RIGHT_CENTER, &date, date_font, date_color).width();
    let mut subject_end = text_end.min(time_end - date_w - 8.0);

    // Category marks and the size of a conversation: at the right of the preview's first line,
    // under the time, or before the time when no preview is shown.
    let lines = app.session.prefs.preview_lines.min(3);
    let preview_y = rect.min.y + PREVIEW_TOP + PREVIEW_PITCH / 2.0 + 0.5;
    let (mut sx, tail_y) = if lines > 0 { (time_end, preview_y) } else { (time_end - date_w - 8.0, line2) };
    for c in m.categories.iter().take(4) {
        let color = crate::theme::c32(app.session.mailbox.category_color(c));
        p.rect_filled(Rect::from_center_size(pos2(sx - 5.0, tail_y), vec2(10.0, 10.0)), 2.0, color);
        sx -= 13.0;
    }
    if row.members.len() > 1 {
        let n = row.members.len().to_string();
        let w = n.len() as f32 * 6.5 + 10.0;
        let br = Rect::from_min_max(pos2(sx - w, tail_y - 7.0), pos2(sx, tail_y + 7.0));
        p.rect_filled(br, 7.0, t.pressed);
        p.text(br.center(), Align2::CENTER_CENTER, n, regular(10.0), t.text_dim);
        sx -= w + 5.0;
    }
    if lines == 0 {
        subject_end = subject_end.min(sx - 6.0);
    }
    let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { m.subject.as_str() };
    let (sfont, scolor) = if unread { (semibold(LIST_TEXT), t.accent_text) } else { (regular(LIST_TEXT), t.text) };
    paint_clipped(p, pos2(left, line2), (subject_end - left).max(0.0), subject, sfont, scolor);

    // Then the preview: whole words, as many lines as asked for, and no mark where it stops.
    let preview_end = text_end.min(sx - 6.0);
    if lines > 0 && preview_end - left > 4.0 {
        let said = preview(ui, app.session.rev(), &m, usize::from(lines) * 200);
        let format =
            egui::text::TextFormat { font_id: regular(LIST_TEXT), color: t.text_dim, line_height: Some(PREVIEW_PITCH), ..Default::default() };
        let mut job = egui::text::LayoutJob::single_section(said.to_string(), format);
        job.wrap =
            egui::text::TextWrapping { max_width: preview_end - left, max_rows: usize::from(lines), break_anywhere: false, overflow_character: None };
        let galley = p.layout_job(job);
        // A word too long for the column runs on: it stops at the column's edge.
        let clip = Rect::from_min_max(pos2(left, rect.min.y), pos2(preview_end, rect.max.y - 1.0)).intersect(p.clip_rect());
        p.with_clip_rect(clip).galley(pos2(left, rect.min.y + PREVIEW_TOP), galley, t.text_dim);
    }

    // Quick actions over the row under the pointer.
    if resp.hovered() && !selected {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(400));
    }
    // Dragging a row carries it to a folder: the whole selection when the row is part of it,
    // otherwise just what the row stands for.
    if resp.drag_started() {
        let ids: Vec<Id> = if selected { app.session.sel.messages.to_vec() } else { row.members.clone() };
        resp.dnd_set_drag_payload(MessageDrag(ids));
    }
    if let Some((delete, flag)) = &quick {
        if delete.clicked() {
            let _ = app.run("mail.delete", json!({"ids": row.members}));
            return;
        }
        if flag.clicked() {
            let _ = app.run("mail.toggleFlag", json!({"ids": [row.id]}));
        }
    }
    if read_resp.clicked() {
        // A conversation is read once all of it is; its newest message makes it unread again.
        let (command, ids) = if unread { ("mail.markRead", row.members.clone()) } else { ("mail.markUnread", vec![row.id]) };
        let _ = app.run(command, json!({"ids": ids}));
    }
    clicks(app, ui, &resp, row.id, &m);
}

fn clicks(app: &mut MailApp, ui: &Ui, resp: &egui::Response, id: Id, m: &Message) {
    let mods = ui.input(|i| i.modifiers);
    if resp.clicked() {
        let _ = app.run("mail.select", json!({"id": id, "extend": mods.command, "range": mods.shift}));
    }
    // A double click opens the message: a draft in the composer, any other in a window of its own.
    if resp.double_clicked() {
        let _ = if m.draft { app.run("mail.editDraft", json!({"id": id})) } else { app.run("ui.openWindow", json!({"message": id})) };
    }
    // A right click on a row outside the selection acts on that row.
    if resp.secondary_clicked() && !app.session.sel.messages.contains(&id) {
        let _ = app.execute("mail.select", json!({"id": id}));
    }
    let (read, flagged, pinned) = (m.read, m.flag == FlagStatus::Flagged, m.pinned);
    let in_junk = app.session.mailbox.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Junk);
    resp.context_menu(|ui| {
        ui.set_min_width(200.0);
        use crate::widgets::mi;
        if m.draft {
            mi(ui, app, tn!("Open Draft"), "mail.editDraft", json!({}));
            ui.separator();
        } else {
            mi(ui, app, tn!("Open in a Window"), "ui.openWindow", json!({"message": id}));
            ui.separator();
        }
        mi(ui, app, tn!("Reply"), "mail.reply", json!({}));
        mi(ui, app, tn!("Reply All"), "mail.replyAll", json!({}));
        mi(ui, app, tn!("Forward"), "mail.forward", json!({}));
        ui.separator();
        mi(ui, app, if read { "Mark as Unread" } else { "Mark as Read" }, if read { "mail.markUnread" } else { "mail.markRead" }, json!({}));
        mi(ui, app, if flagged { "Clear Flag" } else { "Follow Up" }, if flagged { "mail.clearFlag" } else { "mail.flag" }, json!({}));
        mi(ui, app, if pinned { "Unpin" } else { "Pin" }, "mail.pin", json!({}));
        ui.menu_button(tl!("Categorize"), |ui| {
            let ids = app.session.sel.messages.clone();
            crate::ribbon::category_menu(
                ui,
                app,
                "mail.categorize",
                |app, name| ids.iter().all(|id| app.session.mailbox.message(*id).is_some_and(|m| m.has_category(name))),
                Some("mail.clearCategories"),
            );
        });
        ui.separator();
        ui.menu_button(tl!("Move"), |ui| {
            crate::ribbon::mailbox_folder_menu(ui, app, |app, folder| {
                if let Err(e) = app.run("mail.move", json!({"folder": folder})) {
                    app.status(e);
                }
            });
        });
        mi(ui, app, tn!("Archive"), "mail.archive", json!({}));
        if in_junk {
            mi(ui, app, tn!("Not Junk"), "mail.notJunk", json!({}));
        } else {
            mi(ui, app, tn!("Junk"), "mail.junk", json!({}));
        }
        mi(ui, app, tn!("Delete"), "mail.delete", json!({}));
        ui.separator();
        mi(ui, app, tn!("Add to Tasks"), "tasks.fromMessage", json!({}));
        mi(ui, app, tn!("Add Sender to Contacts"), "people.fromMessage", json!({}));
        mi(ui, app, tn!("Find Related Messages"), "mail.findRelated", json!({}));
        mi(ui, app, tn!("Save Message As…"), "file.exportMessage", json!({}));
    });
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;
    use mailcraft_model::Body;

    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    type Window = egui_kittest::Harness<'static, MailApp>;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        app.auto_sync = false;
        app
    }

    /// The whole window, through real frames.
    fn window(app: MailApp, size: (f32, f32)) -> Window {
        let mut harness = egui_kittest::Harness::builder().with_size(vec2(size.0, size.1)).with_step_dt(1.0 / 60.0).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
        harness.run_steps(4);
        let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        harness.run_steps(3);
        harness
    }

    fn hover(harness: &mut Window, pos: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.run_steps(3);
    }

    fn click_at(harness: &mut Window, pos: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(3);
    }

    /// Something of the list (`kind`, `id` as the control channel names them), where it was drawn.
    fn at(harness: &Window, kind: &str, id: u64) -> Rect {
        harness.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} {id} on screen"))
    }

    /// The rows that were drawn, top to bottom, each with where it is.
    fn drawn(harness: &mut Window) -> Vec<(Row, Rect)> {
        let rows = harness.state_mut().session.rows();
        rows.iter().filter_map(|row| harness.state().hit.rect("message", row.id).map(|rect| (row.clone(), rect))).collect()
    }

    fn message(harness: &Window, id: Id) -> Message {
        harness.state().session.mailbox.message(id).cloned().unwrap()
    }

    #[test]
    fn a_rows_height_is_its_preview_lines_and_nothing_else() {
        // One line of preview: 15 clear above the sender's capitals (11 tall), 7 to the subject's
        // (8 tall), 7 to the preview's (8 tall), 15 under them, and the line under the row.
        assert_eq!(row_height(1), 15.0 + 11.0 + 7.0 + 8.0 + 7.0 + 8.0 + 15.0 + 1.0);
        assert_eq!([row_height(0), row_height(1), row_height(2), row_height(3)], [57.0, 72.0, 85.0, 98.0]);
        assert_eq!(row_height(200), row_height(3));
        assert_eq!(SCROLL_LANE, 29.0);
    }

    /// Whatever a row shows (read or unread, flagged, with a file, with category marks, pinned,
    /// a whole conversation, selected, under the pointer), it is as tall as the number of preview
    /// lines says, in a narrow list and in one as wide as the window: where a row is follows
    /// from the rows before it without laying any of them out.
    #[test]
    fn every_row_is_as_tall_as_the_preview_asks_for() {
        for lines in 0..=3u8 {
            for conversations in [false, true] {
                for (pane, size) in [("right", (1280.0, 1100.0)), ("off", (1280.0, 1100.0)), ("right", (640.0, 1100.0))] {
                    let what = format!("{lines} lines, conversations {conversations}, pane {pane}, {size:?}");
                    let mut a = app();
                    a.execute("view.previewLines", json!({"value": lines})).unwrap();
                    a.execute("view.conversations", json!({"value": conversations})).unwrap();
                    a.execute("view.readingPane", json!({"value": pane})).unwrap();
                    let mut harness = window(a, size);
                    // More marks on the rows in view than the sample has: a second and a third
                    // category, a flag that is done with.
                    let ids: Vec<Id> = drawn(&mut harness).iter().map(|(row, _)| row.id).collect();
                    assert!(ids.len() >= 6, "{what}: {} rows", ids.len());
                    for (id, command, params) in [
                        (ids[1], "mail.categorize", json!({"category": "Red category", "value": true})),
                        (ids[1], "mail.categorize", json!({"category": "Green category", "value": true})),
                        (ids[2], "mail.complete", json!({})),
                    ] {
                        let mut params = params;
                        params["ids"] = json!([id]);
                        harness.state_mut().execute(command, params).unwrap_or_else(|e| panic!("{what}: {command}: {e}"));
                    }
                    harness.run_steps(3);

                    let want = row_height(lines);
                    let check = |harness: &mut Window, state: &str| -> Vec<(Row, Rect)> {
                        let rows = drawn(harness);
                        let (first, _) = rows.first().cloned().unwrap();
                        assert!(first.group.is_some(), "{what}, {state}: the list starts with a heading");
                        for pair in rows.windows(2) {
                            let ((_, above), (row, rect)) = (&pair[0], &pair[1]);
                            assert_eq!(rect.height(), want, "{what}, {state}: row {}", row.id);
                            let heading = if row.group.is_some() { GROUP_H } else { 0.0 };
                            assert_eq!(rect.min.y - above.min.y, want + heading, "{what}, {state}: row {}", row.id);
                            assert_eq!((rect.min.x, rect.max.x), (above.min.x, above.max.x), "{what}, {state}: row {}", row.id);
                        }
                        rows
                    };
                    let rows = check(&mut harness, "at rest");
                    // What the rows in view show between them.
                    let shown: Vec<Message> = rows.iter().map(|(row, _)| message(&harness, row.id)).collect();
                    let any = |test: &dyn Fn(&Message) -> bool| shown.iter().any(test);
                    assert!(any(&|m| m.read) && any(&|m| !m.read), "{what}: read and unread");
                    assert!(any(&|m| m.flag == FlagStatus::Flagged) && any(&|m| m.flag == FlagStatus::Complete), "{what}: flags");
                    assert!(any(&|m| m.has_attachments()) && any(&|m| m.categories.len() >= 2) && any(&|m| m.pinned), "{what}: marks");
                    assert!(any(&|m| m.importance != Importance::Normal), "{what}: importance");
                    assert_eq!(rows.iter().any(|(row, _)| row.members.len() > 1), conversations, "{what}: conversations");

                    // Selected, with the keys on it; and another under the pointer, with its buttons.
                    let (selected, hovered) = (rows[4].0.id, rows[2].0.id);
                    harness.state_mut().execute("mail.select", json!({"id": selected})).unwrap();
                    hover(&mut harness, rows[2].1.center());
                    let after = check(&mut harness, "selected and hovered");
                    assert_eq!(
                        after.iter().map(|(_, rect)| *rect).collect::<Vec<_>>(),
                        rows.iter().map(|(_, rect)| *rect).collect::<Vec<_>>(),
                        "{what}"
                    );
                    assert_eq!(harness.state().session.sel.focus(), Some(selected));
                    assert!(
                        rows[2].1.contains_rect(at(&harness, "messageFlag", hovered))
                            && rows[2].1.contains_rect(at(&harness, "messageDelete", hovered))
                    );
                }
            }
        }
    }

    /// The parts of a row a script (or a hand) can reach are where the row shows them: the row
    /// itself from the list's edge to the scroll bar's lane, the read switch down its left edge,
    /// and flag and delete at the right of the sender's line while the pointer is over it.
    #[test]
    fn the_parts_of_a_row_are_where_it_shows_them() {
        let mut harness = window(app(), (1280.0, 800.0));
        let rows = drawn(&mut harness);
        let (row, rect) = rows[3].clone();
        let list = at(&harness, "listTab", 0).min.x - (TEXT_LEFT - 1.0);
        assert_eq!(rect.min.x, list, "a row starts at the list's edge");
        assert_eq!(rect.height(), ROW_H);
        // Under the header, with its heading: nothing of the list is above the header's end.
        let header_end = at(&harness, "listTab", 0).max.y;
        assert_eq!(rows[0].1.min.y, header_end + GROUP_H);

        let strip = at(&harness, "messageRead", row.id);
        assert_eq!((strip.min, strip.max), (rect.min, pos2(rect.min.x + READ_STRIP, rect.max.y)));
        assert!(harness.state().hit.rect("messageFlag", row.id).is_none(), "nothing is offered until the pointer is there");

        hover(&mut harness, rect.center());
        let (flag, delete) = (at(&harness, "messageFlag", row.id), at(&harness, "messageDelete", row.id));
        // The flag is in its own column at the row's end, delete before it, both on the sender's line.
        assert!(flag.center().distance(pos2(rect.max.x - TIME_RIGHT / 2.0, rect.min.y + MARK_Y)) < 0.01);
        assert_eq!(delete.center().y, flag.center().y);
        assert!(delete.max.x <= flag.min.x && delete.min.x > rect.max.x - TEXT_RIGHT, "the buttons keep out of the text's column");
        assert!(flag.max.y < rect.min.y + SUBJECT_Y - 7.0, "and well off the subject's line, where the time is");

        // They do what they say, and the selection stays where it was.
        let selected = harness.state().session.sel.messages.clone();
        click_at(&mut harness, flag.center());
        assert_eq!(message(&harness, row.id).flag, FlagStatus::Flagged);
        let was_read = message(&harness, row.id).read;
        click_at(&mut harness, strip.center());
        assert_eq!(message(&harness, row.id).read, !was_read);
        assert_eq!(harness.state().session.sel.messages, selected);
        // The rest of the row selects.
        click_at(&mut harness, pos2(rect.min.x + TEXT_LEFT + 40.0, rect.min.y + SUBJECT_Y));
        assert_eq!(harness.state().session.sel.focus(), Some(row.id));

        // The scroll bar's lane is right of every row, inside the list.
        let rows_end = rect.max.x;
        let sort = at(&harness, "listSort", 1);
        assert!(sort.max.x <= rows_end && sort.min.x > at(&harness, "listSort", 0).min.x);
    }

    /// The header: All and Unread are tabs that run `view.filter`; a filter without a tab shows
    /// as a third one while it is in use; the arrangement opens its menu (`view.sort`, and the
    /// other filters), and the arrow turns the list over (`view.reverseSort`).
    #[test]
    fn the_header_filters_and_arranges_the_list() {
        let mut harness = window(app(), (1280.0, 800.0));
        let filter = |harness: &Window| harness.state().session.view.filter;
        let tab = |harness: &Window, n: u64| harness.state().hit.rect("listTab", n);
        assert_eq!(filter(&harness), Filter::All);
        assert!(tab(&harness, 0).is_some() && tab(&harness, 1).is_some() && tab(&harness, 2).is_none());
        let (all, unread) = (at(&harness, "listTab", 0), at(&harness, "listTab", 1));
        assert!(all.max.x < unread.min.x && unread.max.x < at(&harness, "listSort", 0).min.x, "tabs on the left, the arrangement on the right");

        let everything = drawn(&mut harness).len();
        click_at(&mut harness, unread.center());
        assert_eq!(filter(&harness), Filter::Unread);
        let rows = drawn(&mut harness);
        assert!(!rows.is_empty() && rows.len() < everything && rows.iter().all(|(row, _)| row.unread > 0));
        click_at(&mut harness, all.center());
        assert_eq!((filter(&harness), drawn(&mut harness).len()), (Filter::All, everything));

        // A filter that has no tab of its own.
        harness.state_mut().execute("view.filter", json!({"value": "flagged"})).unwrap();
        harness.run_steps(3);
        let third = at(&harness, "listTab", 2);
        assert!(third.min.x > at(&harness, "listTab", 1).max.x && third.max.x < at(&harness, "listSort", 0).min.x);
        click_at(&mut harness, all.center());
        assert_eq!(filter(&harness), Filter::All);
        assert!(tab(&harness, 2).is_none());

        // The arrow.
        let order = |harness: &mut Window| drawn(harness).iter().map(|(row, _)| row.id).collect::<Vec<_>>();
        let (newest_first, was) = (order(&mut harness), harness.state().session.view.sort_descending);
        let arrow = at(&harness, "listSort", 1).center();
        click_at(&mut harness, arrow);
        assert_eq!(harness.state().session.view.sort_descending, !was);
        assert_ne!(order(&mut harness), newest_first);
        click_at(&mut harness, arrow);
        assert_eq!(order(&mut harness), newest_first);

        // The arrangement's menu: how the list is arranged, and every filter.
        let open = |harness: &mut Window| {
            let arrangement = at(harness, "listSort", 0).center();
            click_at(harness, arrangement);
        };
        open(&mut harness);
        for label in [
            "Date",
            "From",
            "Subject",
            "Size",
            "Importance",
            "Reverse Sort",
            "Show as Conversations",
            "Unread",
            "Flagged",
            "Has Attachments",
            "To Me",
        ] {
            assert!(harness.query_all_by_label_contains(label).next().is_some(), "the menu offers {label}");
        }
        harness.get_by_label_contains("Subject").click();
        harness.run_steps(4);
        assert_eq!((harness.state().session.view.sort, harness.state().session.view.sort_descending), (SortBy::Subject, false));
        open(&mut harness);
        harness.get_by_label_contains("Has Attachments").click();
        harness.run_steps(4);
        assert_eq!(filter(&harness), Filter::Attachments);
        assert!(drawn(&mut harness).iter().all(|(row, _)| message(&harness, row.id).has_attachments()));
        assert!(tab(&harness, 2).is_some());

        // In a list too narrow for every tab, the one in use is the one that stays.
        let mut a = app();
        a.ui.list_width = 240.0;
        a.execute("view.filter", json!({"value": "attachments"})).unwrap();
        let narrow = window(a, (760.0, 800.0));
        let in_use = at(&narrow, "listTab", 2);
        assert!(in_use.width() > TAB_MIN && in_use.max.x < at(&narrow, "listSort", 0).min.x, "{in_use:?}");
        assert!(narrow.state().hit.rect("listTab", 1).is_none(), "Unread makes room for it");
    }

    /// The header says how the list is arranged and which way it runs, in words for each.
    #[test]
    fn the_arrangement_has_a_name_and_a_direction() {
        for sort in SortBy::ALL {
            assert!(sort_label(sort).starts_with("By ") && sort_label(sort).ends_with(sort.label()));
            assert_ne!(direction_label(sort, true), direction_label(sort, false));
        }
        assert_eq!(direction_label(SortBy::Date, true), "Newest on top");
        assert_eq!(direction_label(SortBy::From, false), "A on top");
    }

    /// The arrows at the ends of the scroll bar move the list by a row, and stop at its ends.
    #[test]
    fn the_scroll_bars_arrows_move_the_list_by_a_row() {
        let mut a = app();
        let inbox = a.session.view.folder;
        for i in 0..60u32 {
            let from = mailcraft_model::Address::new(format!("Sender {i}"), format!("s{i}@example.org"));
            let m = Message {
                folder: inbox,
                subject: format!("Message {i}"),
                from,
                date: NOW - 3600,
                received: NOW - 3600 - i64::from(i),
                ..Default::default()
            };
            a.session.mailbox.add_message(m).unwrap();
        }
        a.session.mailbox_changed();
        let mut harness = window(a, (1280.0, 800.0));
        let (up, down) = (at(&harness, "listScroll", 0), at(&harness, "listScroll", 1));
        let rows = drawn(&mut harness);
        // In the lane right of the rows, at its two ends.
        assert!((up.min.x - rows[0].1.max.x - SCROLL_GAP).abs() < 0.01 && (up.width() - SCROLL_TRACK).abs() < 0.01 && up.max.y < down.min.y);
        let (row, was) = rows[5].clone();
        click_at(&mut harness, down.center());
        assert_eq!(at(&harness, "message", row.id).min.y, was.min.y - ROW_H);
        click_at(&mut harness, down.center());
        assert_eq!(at(&harness, "message", row.id).min.y, was.min.y - 2.0 * ROW_H);
        for _ in 0..4 {
            click_at(&mut harness, up.center());
        }
        assert_eq!(at(&harness, "message", row.id), was, "back at the top, and no further");
        // A list that fits has no bar and no arrows, and its rows are just as wide.
        harness.state_mut().execute("view.filter", json!({"value": "flagged"})).unwrap();
        harness.run_steps(3);
        assert!(harness.state().hit.rect("listScroll", 0).is_none());
        let few = drawn(&mut harness);
        assert_eq!((few[0].1.min.x, few[0].1.max.x), (was.min.x, was.max.x));
    }

    /// The preview under a subject is what the message says to its reader, worked out when the
    /// row is drawn: mail that was here before reads as cleanly as mail that comes now, and
    /// nothing in the message is touched. A line is read once, until the mailbox changes.
    #[test]
    fn previews_are_what_the_messages_say() {
        let kept = |harness: &Window, id: Id| -> Option<String> {
            harness
                .ctx
                .data(|data| data.get_temp::<Previews>(egui::Id::new("message_list_previews")))
                .and_then(|p| p.lines.get(&id).map(|(_, line)| line.to_string()))
        };
        let mut a = app();
        let inbox = a.session.view.folder;
        // As a newsletter is stored: a text part full of what its sender's program left in it,
        // and the HTML a reader sees.
        let body = Body {
            text: "&zwnj; &zwnj; &zwnj; &zwnj; &zwnj;\n[image: Fernhollow]\nhttps://click.fernhollow.example/t/9f3a7c?u=88213\n".into(),
            html: Some(format!(
                "<html><head><style>.x {{ color: red }}</style></head><body><div style=\"display:none\">Wool layers are back.{}</div>\
                 <p><a href=\"https://click.fernhollow.example/t/9f3a7c?u=88213\">See what is new</a></p></body></html>",
                "&zwnj;&nbsp;&#847;".repeat(120)
            )),
        };
        let from = mailcraft_model::Address::new("Fernhollow Outfitters", "news@fernhollow.example");
        let id = a
            .session
            .mailbox
            .add_message(Message {
                folder: inbox,
                subject: "New this week".into(),
                from,
                date: NOW,
                received: NOW,
                body: body.clone(),
                ..Default::default()
            })
            .unwrap();
        a.session.mailbox_changed();
        let mut harness = window(a, (1280.0, 800.0));
        assert!(harness.state().hit.rect("message", id).is_some(), "the message is in view");
        assert_eq!(kept(&harness, id).as_deref(), Some("Wool layers are back. See what is new"));
        assert_eq!(message(&harness, id).body, body, "the message is as it was stored");
        // Every row's line is clean, whatever its message holds.
        for (row, _) in drawn(&mut harness) {
            let line = kept(&harness, row.id).unwrap();
            assert!(!line.is_empty() && !line.contains("&zwnj") && !line.contains("http") && !line.contains("  "), "{line:?}");
        }
        // With more lines to fill it is read again, for a longer line that says the same.
        harness.state_mut().execute("view.previewLines", json!({"value": 3})).unwrap();
        harness.run_steps(3);
        assert_eq!(kept(&harness, id).as_deref(), Some("Wool layers are back. See what is new"));
        // The body changes (a draft is written on, a message comes down in full): so does its line.
        if let Ok(m) = harness.state_mut().session.mailbox.message_mut(id) {
            m.body = Body { text: "Plain words now.".into(), html: None };
        }
        harness.state_mut().session.mailbox_changed();
        harness.run_steps(3);
        assert_eq!(kept(&harness, id).as_deref(), Some("Plain words now."));
    }

    /// Lines are remembered for the rows in view, not for a mailbox's worth of scrolling.
    #[test]
    fn remembered_previews_are_few() {
        let messages: Vec<Arc<Message>> = (1..=3 * MAX_PREVIEWS as u64)
            .map(|id| Arc::new(Message { id, body: Body { text: format!("Message number {id} says hello"), html: None }, ..Default::default() }))
            .collect();
        egui::__run_test_ui(|ui| {
            for m in &messages {
                assert_eq!(&*preview(ui, 7, m, 40), format!("Message number {} says hello", m.id).as_str());
            }
            let kept = ui.data(|data| data.get_temp::<Previews>(egui::Id::new("message_list_previews"))).unwrap();
            assert!(kept.lines.len() <= MAX_PREVIEWS + 1, "{}", kept.lines.len());
            // Asked again, the same line; asked for less, a shorter one; after a change, a new one.
            let first = messages.last().unwrap();
            assert!(Arc::ptr_eq(&preview(ui, 7, first, 40), &preview(ui, 7, first, 40)));
            assert_eq!(preview(ui, 7, first, 14).chars().count(), 15);
            let changed = Arc::new(Message { body: Body { text: "Other words".into(), html: None }, ..(**first).clone() });
            assert_eq!(&*preview(ui, 8, &changed, 40), "Other words");
        });
    }
}
