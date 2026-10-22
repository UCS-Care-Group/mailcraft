// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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

mod server_rows;

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
/// Where a row's text starts, from the list's left edge.
const TEXT_LEFT: f32 = 30.0;
/// Rows end this far from the list's right edge: a gap, the scroll bar's lane, and its margin.
const SCROLL_LANE: f32 = SCROLL_GAP + SCROLL_TRACK + SCROLL_MARGIN;
const SCROLL_GAP: f32 = 7.0;
const SCROLL_TRACK: f32 = 17.0;
const SCROLL_MARGIN: f32 = 5.0;
/// The scroll bar's handle, in the middle of its lane.
const SCROLL_HANDLE: f32 = 9.0;
/// Where scroll bars float over what they scroll (a Mac's): the handle, a little wider while the
/// pointer is on it, and how far it keeps from the list's right edge.
const OVERLAY_HANDLE: f32 = 7.0;
const OVERLAY_HANDLE_HOVERED: f32 = 9.0;
const OVERLAY_MARGIN: f32 = 2.0;
/// The ends of the lane, each with an arrow in it: the handle travels between them.
const SCROLL_END: f32 = 19.0;
/// The time ends this far from a row's right end. What is right of it is the flag's column.
const TIME_RIGHT: f32 = 24.0;
/// Sender, subject and preview end this far from a row's right end, where the time's column
/// starts: every row's text stops at the same place, whatever its time.
const TEXT_RIGHT: f32 = 98.0;
/// Where a quick action (delete, flag) on the row under the pointer takes a click.
const QUICK_TARGET: egui::Vec2 = egui::vec2(28.0, 22.0);
/// The clear space between the quick actions, and between them and the marks before them.
const QUICK_GAP: f32 = 6.0;
/// The middle of the sender's capitals, from a row's top: they stand 15 below the line above.
const SENDER_Y: f32 = 21.0;
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
    let text = tln!(drag.0.len(), "{n} message", "{n} messages");
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
        let shown = egui::Panel::left("message_list")
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

/// A row of the single-line list: the sender, the subject, the marks and the time on one line.
const ONE_LINE_H: f32 = 32.0;

/// The least a single-line row leaves for the sender's name.
const ONE_LINE_SENDER_MIN: f32 = 70.0;

/// How wide the sender's name is in a single-line row, of the room the row has for text: a third,
/// not less than 90 or more than 220 (and never more than the room).
fn one_line_sender_width(room: f32) -> f32 {
    (room / 3.0).clamp(ONE_LINE_SENDER_MIN, 220.0).min(room.max(0.0))
}

/// A group heading of the list in the interface language: the engine's dates (`Today`) are
/// names the catalogue has, and so is the heading of what is pinned.
fn group_name(group: &str) -> &str {
    if group == "Pinned" { tl!("Pinned") } else { tl!(group) }
}

/// What the arrangement is called in the list's header, in the interface language.
fn sort_label(sort: SortBy) -> &'static str {
    match sort {
        SortBy::Date => tl!("By Date"),
        SortBy::From => tl!("By From"),
        SortBy::Subject => tl!("By Subject"),
        SortBy::Size => tl!("By Size"),
        SortBy::Importance => tl!("By Importance"),
    }
}

/// What is on top of the list, for the arrow that turns it over, in the interface language.
fn direction_label(sort: SortBy, descending: bool) -> &'static str {
    match (sort, descending) {
        (SortBy::Date, true) => tl!("Newest on top"),
        (SortBy::Date, false) => tl!("Oldest on top"),
        (SortBy::From | SortBy::Subject, false) => tl!("A on top"),
        (SortBy::From | SortBy::Subject, true) => tl!("Z on top"),
        (SortBy::Size, true) => tl!("Largest on top"),
        (SortBy::Size, false) => tl!("Smallest on top"),
        (SortBy::Importance, true) => tl!("High on top"),
        (SortBy::Importance, false) => tl!("Low on top"),
    }
}

/// The header above the list: All and Unread as tabs on the left (the filter in use in the
/// accent colour, with a line under it), and on the right how the list is arranged, with the
/// arrow that turns it over. The folder's name is in the window's title and in the folder pane.
fn list_header(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let rows_end = r.max.x - lane_width(app);

    // The arrow: up while the list starts where its arrangement naturally starts (the newest,
    // A, the largest), down once it is turned over.
    let (sort, descending) = (app.session.view.sort, app.session.view.sort_descending);
    let arrow = Rect::from_center_size(pos2(rows_end - 11.0, r.min.y + 33.0), vec2(20.0, 22.0));
    let arrow_resp = ui.interact(arrow, ui.id().with("list_direction"), Sense::click()).on_hover_text(direction_label(sort, descending));
    app.hit.add("listSort", 1, arrow);
    if arrow_resp.hovered() {
        ui.painter().rect_filled(arrow, 4.0, t.hover);
    }
    paint_direction(ui.painter(), arrow.center(), descending == sort.starts_high(), t.text);
    if arrow_resp.clicked() {
        let _ = app.run("view.reverseSort", json!({}));
    }

    // The arrangement and its menu.
    let label = sort_label(sort);
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
        crate::widgets::mi(ui, app, "Reverse Sort", "view.reverseSort", json!({}));
        let conv = app.session.view.conversations;
        crate::widgets::mi_check(ui, app, "Show as Conversations", conv, "view.conversations", json!({}));
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

/// How much of the list's right edge its scroll bar keeps for itself: the lane, or nothing where
/// scroll bars float over what they scroll ([`MailApp::overlay_scroll_bars`]).
fn lane_width(app: &MailApp) -> f32 {
    if app.overlay_scroll_bars { 0.0 } else { SCROLL_LANE }
}

/// The list's scroll bar where scroll bars float over what they scroll, as a Mac's do: a slim
/// rounded handle over the rows' right end, there while the list is scrolled or the pointer is
/// over it and gone when it is left alone, with no lane and no arrows.
fn overlay_scroll_style(ui: &mut Ui, t: &Tokens) {
    let style = ui.style_mut();
    style.spacing.scroll = egui::style::ScrollStyle {
        bar_width: OVERLAY_HANDLE_HOVERED,
        floating_width: OVERLAY_HANDLE,
        bar_inner_margin: 0.0,
        bar_outer_margin: OVERLAY_MARGIN,
        handle_min_length: 24.0,
        foreground_color: false,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.85,
        interact_handle_opacity: 1.0,
        ..egui::style::ScrollStyle::floating()
    };
    let ink = scroll_ink(t);
    let round = egui::CornerRadius::same(5);
    let widgets = &mut style.visuals.widgets;
    (widgets.inactive.bg_fill, widgets.inactive.corner_radius) = (ink, round);
    (widgets.hovered.bg_fill, widgets.hovered.corner_radius) = (ink.lerp_to_gamma(t.text, 0.35), round);
    (widgets.active.bg_fill, widgets.active.corner_radius) = (ink.lerp_to_gamma(t.text, 0.6), round);
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

/// While a search is on, a bar over the list says so: how many it found and where, with a way
/// back to the folder (✕, or Esc). Search is quick enough that the list changing by itself
/// went unnoticed (#115).
fn search_bar(app: &mut MailApp, ui: &mut Ui, found: usize) {
    let on = !app.session.view.search.trim().is_empty();
    // It fades in when a search starts; with no search it is gone at once, ready to fade again.
    let shown = ui.ctx().animate_bool_with_time(ui.id().with("search_bar_fade"), on, if on { 0.15 } else { 0.0 });
    if !on {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let scope = if app.session.view.scope == mailcraft_engine::Scope::Everywhere {
        tl!("All Mailboxes").to_string()
    } else {
        // (A shared mailbox's folder with the mailbox it is in, #70.)
        app.session.mailbox.folder(app.session.view.folder).map(|f| crate::nav::folder_label(&app.session.mailbox, f)).unwrap_or_default()
    };
    let said = match found {
        0 => crate::i18n::fmt(tl!("No results in {scope}"), &[("scope", &scope)]),
        n => crate::i18n::fmt(&tln!(n, "{n} result in {scope}", "{n} results in {scope}"), &[("scope", &scope)]),
    };
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width() - lane_width(app), 30.0), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &said));
    let fill = t.selection.gamma_multiply(shown);
    ui.painter().rect_filled(r.shrink2(vec2(6.0, 3.0)), 4.0, fill);
    let close = Rect::from_center_size(pos2(r.max.x - 22.0, r.center().y), vec2(22.0, 22.0));
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 16.0, r.center().y), close.min.x - r.min.x - 24.0, &said, semibold(12.0), t.text);
    let x = ui.interact(close, ui.id().with("search_bar_close"), Sense::click()).on_hover_text(tl!("Clear search (Esc)"));
    x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Clear search")));
    app.hit.add("searchBarClose", 0, close);
    if x.hovered() {
        ui.painter().rect_filled(close, 4.0, t.hover);
    }
    let ink = if x.hovered() { t.text } else { t.text_dim };
    let c = close.center();
    for (a, b) in [(vec2(-4.5, -4.5), vec2(4.5, 4.5)), (vec2(-4.5, 4.5), vec2(4.5, -4.5))] {
        ui.painter().line_segment([c + a, c + b], Stroke::new(1.4, ink));
    }
    if x.clicked() {
        app.search_text.clear();
        let _ = app.run("view.clearSearch", json!({}));
    }
}

fn list(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    list_header(app, ui);
    let rows = app.session.rows();
    search_bar(app, ui, rows.len());
    // A search covers what is here: of a message not fetched yet, its headers and preview (R3.2).
    if !app.session.view.search.trim().is_empty() {
        let folder = app.session.view.folder;
        let everywhere = app.session.view.scope == mailcraft_engine::Scope::Everywhere;
        let partly = app.session.mailbox.messages.values().filter(|m| m.unfetched && (everywhere || m.folder == folder)).count();
        if partly > 0 {
            let said = crate::i18n::fmt(
                tl!("{count} messages weren't searched in full: only their beginning is here until they download."),
                &[("count", &partly.to_string())],
            );
            egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 4, bottom: 4 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(egui::RichText::new(said).font(regular(11.5)).color(t.text_dim));
            });
        }
    }
    // While the word index is being built, what it hasn't got yet is searched more slowly (R6.2).
    if !app.session.view.search.trim().is_empty() && matches!(app.session.word_index(), mailcraft_engine::WordIndex::Building { .. }) {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, t.panel);
        let text = tl!("Still indexing: some older mail is searched more slowly");
        paint_clipped(ui.painter(), pos2(r.min.x + 10.0, r.min.y + 4.0), r.width() - 20.0, text, regular(11.5), t.text_dim);
        app.hit.add("stillIndexing", 0, r);
    }
    // A search that reads the text of messages goes as far into older mail as its time allows,
    // and says how far that was.
    if app.session.unsearched > 0 && !app.session.view.search.trim().is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
        ui.painter().rect_filled(r, 0.0, t.panel);
        let n = app.session.unsearched;
        let text = tln!(
            n,
            "{n} older message wasn't searched: its text isn't loaded yet",
            "{n} older messages weren't searched: their text isn't loaded yet"
        );
        paint_clipped(ui.painter(), pos2(r.min.x + 10.0, r.min.y + 4.0), r.width() - 20.0, &text, regular(11.5), t.text_dim);
    }
    let one_line = app.session.prefs.one_line;
    // The rows of a search on the server keep the height the preview lines ask for.
    let server_h = row_height(app.session.prefs.preview_lines);
    let row_h = if one_line { ONE_LINE_H } else { server_h };
    // Below the local results of a search: More results on the server, and what came (R6.2).
    let searching = !app.session.view.search.trim().is_empty();
    let local: std::collections::HashSet<Id> =
        if searching { rows.iter().flat_map(|r| r.members.iter().copied()).collect() } else { Default::default() };
    let below = if searching { server_rows::height(app, &local, server_h) } else { 0.0 };
    let overlay = app.overlay_scroll_bars;
    let lane_w = lane_width(app);
    if rows.is_empty() {
        let r = ui.available_rect_before_wrap();
        let msg = if searching {
            tl!("Nothing matches the search.")
        } else if app.session.view.filter != Filter::All {
            tl!("Nothing matches this filter.")
        } else {
            tl!("Nothing here yet.")
        };
        ui.painter().text(pos2(r.center().x, r.min.y + 60.0), Align2::CENTER_CENTER, msg, regular(12.5), t.text_dim);
        if below > 0.0 {
            let room = Rect::from_min_max(pos2(r.min.x, r.min.y + 90.0), r.max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
                egui::ScrollArea::vertical().id_salt("message_list_server").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
                    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), below), Sense::hover());
                    let width = (area.width() - lane_w).max(0.0);
                    server_rows::show(app, ui, &local, area.min.x, area.min.y, width, server_h, viewport.translate(area.min.to_vec2()));
                });
            });
        }
        return;
    }
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
    let rows_end = y;
    let total = y + below;
    let reveal = std::mem::take(&mut app.reveal_selection);
    let focus = app.session.sel.focus();
    let outer = ui.available_rect_before_wrap();
    let lane = scroll_lane(outer);
    let scrolls = total > outer.height();
    // (A bar that floats over the rows has no lane to paint and no arrows.)
    if scrolls && !overlay {
        ui.painter().rect_filled(lane, SCROLL_TRACK / 2.0, if t.dark { t.panel } else { t.canvas });
    }
    // The handle keeps clear of the arrows at the ends of its lane (a lane too short for them
    // has none).
    let arrows = scrolls && !overlay && lane.height() >= 4.0 * SCROLL_END;
    let travel = if arrows { outer.shrink2(vec2(0.0, SCROLL_END)) } else { outer };
    let plain = ui.style().clone();
    if overlay {
        overlay_scroll_style(ui, &t);
    } else {
        scroll_style(ui, &t);
    }
    let out =
        egui::ScrollArea::vertical().id_salt("message_list").auto_shrink([false, false]).scroll_bar_rect(travel).show_viewport(ui, |ui, viewport| {
            // The bar's looks are the bar's alone.
            ui.set_style(plain);
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
            let origin = area.min;
            // Rows end where the scroll bar's lane starts, whether the list scrolls or not (at the
            // list's right edge where the bar floats over them).
            let width = (area.width() - lane_w).max(0.0);
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
                    paint_clipped(
                        ui.painter(),
                        pos2(gr.min.x + 10.0, gr.center().y),
                        gr.width() - 16.0,
                        group_name(group),
                        semibold(GROUP_TEXT),
                        t.text,
                    );
                }
                let rect = Rect::from_min_size(pos2(origin.x, origin.y + top), vec2(width, row_h));
                if reveal && Some(row.id) == focus {
                    ui.scroll_to_rect(rect, None);
                }
                message_row(app, ui, row, rect);
            }
            if below > 0.0 {
                server_rows::show(app, ui, &local, origin.x, origin.y + rows_end, width, server_h, viewport.translate(origin.to_vec2()));
            }
        });
    if arrows {
        scroll_arrows(app, ui, lane, &out, row_h);
    }
}

/// How wide the folder a search result is in may be drawn: the time's column.
const FOLDER_W: f32 = TEXT_RIGHT - TIME_RIGHT;

/// The folder a search result is in (#195), in the time's column: a folder's mark and its name
/// in `text_dim` (4.5:1 or more on every row's background, selected, flagged or under the
/// pointer, in both palettes), ending at `end` and cut short with an ellipsis to the column's
/// width. Where it starts.
fn paint_result_folder(p: &egui::Painter, end: egui::Pos2, label: &str, t: &Tokens) -> f32 {
    const ICON: f32 = 12.0;
    const GAP: f32 = 3.0;
    let width = FOLDER_W - ICON - GAP;
    if label.is_empty() {
        return end.x;
    }
    let mut job = egui::text::LayoutJob::single_section(
        label.to_string(),
        egui::text::TextFormat { font_id: regular(LIST_TEXT), color: t.text_dim, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let galley = p.layout_job(job);
    let size = galley.size();
    let x = end.x - size.x;
    p.galley(pos2(x, end.y - size.y / 2.0), galley, t.text_dim);
    let icon = x - GAP - ICON;
    icons::paint(p, Rect::from_min_size(pos2(icon, end.y - ICON / 2.0), vec2(ICON, ICON)), "folder", t.text_dim, t.accent);
    icon
}

/// What a row says of its message's size (#273): only while the list is arranged by size, and
/// only when the size is known ([`Message::list_size`]); never "900 bytes", but KB at the least,
/// as the size of a picture is said.
fn size_label(s: &mailcraft_engine::Session, m: &Message) -> Option<String> {
    let bytes = (s.view.sort == SortBy::Size).then(|| m.list_size()).flatten()?;
    Some(mailcraft_model::size_label_in(crate::i18n::current(), bytes.max(1024)))
}

/// Whether a row names the folder its message is in, under the time: in a search of every
/// mailbox, as Outlook's rows do (#195); in one folder the owner knows which it is.
fn shows_folder(s: &mailcraft_engine::Session) -> bool {
    s.view.scope == mailcraft_engine::Scope::Everywhere && !mailcraft_model::search::Query::parse(&s.view.search).is_empty()
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

/// Where a message's follow-up flag stands, as its row in the list shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FollowUp {
    /// Not flagged, or the flag is done with: the row is as any other.
    None,
    /// Flagged: the row wears a faint tint.
    Flagged,
    /// Flagged, and due on a day before today: its text is red as well.
    Overdue,
}

impl FollowUp {
    pub(crate) fn of(m: &mailcraft_model::Message, session: &mailcraft_engine::Session) -> FollowUp {
        match (m.flag, m.flag_due) {
            (FlagStatus::Flagged, Some(due)) if session.local(due).date < session.today() => FollowUp::Overdue,
            (FlagStatus::Flagged, _) => FollowUp::Flagged,
            (FlagStatus::Complete | FlagStatus::None, _) => FollowUp::None,
        }
    }
}

/// What a row is filled with, under the pointer's shade: a selected row's colour, a flagged
/// one's tint, or nothing (the list shows through).
pub(crate) fn row_fill(t: &Tokens, follow_up: FollowUp, selected: bool, focus: bool) -> Option<Color32> {
    match (selected, follow_up) {
        (true, _) => Some(if focus { t.selection } else { t.selection.linear_multiply(0.7) }),
        (false, FollowUp::Flagged | FollowUp::Overdue) => Some(t.flagged_row),
        (false, FollowUp::None) => None,
    }
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
    let one = app.session.prefs.one_line;
    let line1 = if one { rect.center().y } else { rect.min.y + SENDER_Y };
    let line2 = if one { rect.center().y } else { rect.min.y + SUBJECT_Y };
    // Quick actions on the row under the pointer, at the right of the sender's line: flag (where
    // a flagged message wears its flag) and delete. Like the read switch they sit on top of the
    // row and leave the selection alone.
    let over = ui.rect_contains_pointer(rect) && egui::DragAndDrop::payload::<MessageDrag>(ui.ctx()).is_none();
    // Wide enough to hit without aiming, a gap between them, and short enough to stay off the
    // time on the subject's line.
    let flag_rect = Rect::from_center_size(pos2(rect.max.x - QUICK_TARGET.x / 2.0, line1), QUICK_TARGET);
    let delete_rect = flag_rect.translate(vec2(-(QUICK_TARGET.x + QUICK_GAP), 0.0));
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
    let follow_up = FollowUp::of(&m, &app.session);
    let fill = row_fill(&t, follow_up, selected, focus);
    if let Some(fill) = fill {
        p.rect_filled(body, 0.0, fill);
        if fill == t.flagged_row {
            app.hit.add("messageFlagShade", row.id, body);
        }
    }
    if !selected && (resp.hovered() || read_resp.hovered() || quick.is_some()) {
        // The pointer's shade goes over a flagged row's tint.
        p.rect_filled(body, 0.0, t.hover.linear_multiply(0.6));
    }
    // Past its due date, its text is red.
    let overdue = follow_up == FollowUp::Overdue;
    if overdue {
        app.hit.add("messageOverdue", row.id, body);
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
    let date =
        crate::dates::list_label(mailcraft_engine::list::when(&m), app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
    let (date_font, date_color) = match (unread, overdue) {
        (true, false) => (semibold(LIST_TEXT), t.accent_text),
        (true, true) => (semibold(LIST_TEXT), t.overdue),
        (false, _) => (regular(LIST_TEXT), if overdue { t.overdue } else { t.text }),
    };
    if let Some((delete, flag)) = &quick {
        for (resp, at, icon, color) in
            [(delete, delete_rect, "delete", t.text_dim), (flag, flag_rect, "flag", if flagged { t.icon } else { t.text_dim })]
        {
            if resp.hovered() {
                p.rect_filled(at, 4.0, t.pressed);
            }
            icons::paint(p, Rect::from_center_size(at.center(), vec2(16.0, 16.0)), icon, color, t.accent);
        }
        // The marks keep clear of delete, so a click meant for the row doesn't land on it.
        x = delete_rect.min.x - QUICK_GAP;
    } else {
        let flag = Rect::from_center_size(flag_rect.center(), vec2(12.0, 12.0));
        match m.flag {
            FlagStatus::Flagged => icons::paint(p, flag, "flag", t.icon, t.accent),
            FlagStatus::Complete => icons::paint(p, flag, "complete", t.icon, t.accent),
            FlagStatus::None => {}
        }
    }
    // In a single line the time comes before the marks, and the quick actions take its place
    // while the pointer is over the row.
    let mut one_date_w = 0.0;
    if one && quick.is_none() {
        one_date_w = p.text(pos2(time_end, line1), Align2::RIGHT_CENTER, &date, date_font.clone(), date_color).width();
        x = time_end - one_date_w - 10.0;
    }
    let mut marker = |icon: &str, color: Color32| {
        icons::paint(p, Rect::from_center_size(pos2(x - 6.0, line1), vec2(12.0, 12.0)), icon, color, t.accent);
        x -= 15.0;
    };
    // A meeting that came by mail wears its mark; its calendar part is not one of its files.
    let (meeting, files) = crate::reading::list_marks(&m);
    if let Some(meeting) = meeting {
        marker(meeting, t.icon);
    }
    if files {
        marker("attach", t.text_dim);
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
    // Arranged by size, the message's size (#273); in a single line it comes before the marks, so
    // the sender's name stops short of it there, and a line with no room for both keeps the name.
    let size = size_label(&app.session, &m)
        .map(|label| p.layout_no_wrap(label, regular(LIST_TEXT), t.text_dim))
        .filter(|g| !one || x - 4.0 - (g.size().x + 6.0) >= left + ONE_LINE_SENDER_MIN);
    let size_room = if one { size.as_ref().map_or(0.0, |g| g.size().x + 6.0) } else { 0.0 };
    let who_font = if unread { semibold(SENDER_TEXT) } else { regular(SENDER_TEXT) };
    let who_end = if one { (left + one_line_sender_width(time_end - left)).min(x - 4.0 - size_room) } else { text_end.min(x - 4.0) };
    let mut who_x = left;
    if m.draft {
        let w = paint_clipped(p, pos2(left, line1), (who_end - left).max(0.0), tl!("[Draft]"), regular(SENDER_TEXT), t.red);
        who_x += w + 5.0;
    }
    let w = paint_clipped(p, pos2(who_x, line1), (who_end - who_x).max(0.0), &who, who_font, if overdue { t.overdue } else { t.text });
    app.hit.add("messageSender", row.id, Rect::from_min_max(pos2(left, line1 - 7.0), pos2(who_x + w, line1 + 7.0)));

    // The subject's line, with the time at its end.
    let date_w = if one { one_date_w } else { p.text(pos2(time_end, line2), Align2::RIGHT_CENTER, &date, date_font, date_color).width() };
    let mut subject_end = if one { time_end } else { text_end.min(time_end - date_w - 8.0) };

    // The size; in a search of every mailbox the folder it is in; then category marks and the
    // size of a conversation: at the right of the preview's first line, under the time, before
    // the time when no preview is shown, or before the marks in a single line.
    let lines = if one { 0 } else { app.session.prefs.preview_lines.min(3) };
    let preview_y = rect.min.y + PREVIEW_TOP + PREVIEW_PITCH / 2.0 + 0.5;
    let (mut sx, tail_y) = if one {
        (x, line1)
    } else if lines > 0 {
        (time_end, preview_y)
    } else {
        (time_end - date_w - 8.0, line2)
    };
    if let Some(galley) = size {
        let at = Align2::RIGHT_CENTER.anchor_size(pos2(sx, tail_y), galley.size());
        p.galley(at.min, galley, t.text_dim);
        app.hit.add("messageSize", row.id, at);
        sx = at.min.x - 6.0;
    }
    if let Some(folder) = shows_folder(&app.session).then(|| app.session.mailbox.folder(m.folder)).flatten() {
        let label = crate::nav::result_folder_label(&app.session.mailbox, folder);
        let end = sx;
        sx = paint_result_folder(p, pos2(end, tail_y), &label, &t) - 6.0;
        app.hit.add("messageFolder", row.id, Rect::from_min_max(pos2(sx + 6.0, tail_y - 7.0), pos2(end, tail_y + 7.0)));
    }
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
    let (sfont, scolor) = match (unread, overdue) {
        (true, false) => (semibold(LIST_TEXT), t.accent_text),
        (true, true) => (semibold(LIST_TEXT), t.overdue),
        (false, _) => (regular(LIST_TEXT), if overdue { t.overdue } else { t.text }),
    };
    let subject_x = if one { who_end + 10.0 } else { left };
    paint_clipped(p, pos2(subject_x, line2), (subject_end - subject_x).max(0.0), subject, sfont, scolor);

    // Then the preview: whole words, as many lines as asked for, and no mark where it stops.
    let preview_end = text_end.min(sx - 6.0);
    if lines > 0 && preview_end - left > 4.0 {
        let said = preview(ui, app.session.rev(), &m, usize::from(lines) * 200);
        let format =
            egui::text::TextFormat { font_id: regular(LIST_TEXT), color: t.text_dim, line_height: Some(PREVIEW_PITCH), ..Default::default() };
        let mut job = egui::text::LayoutJob::default();
        job.append(&said, 0.0, format);
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
            mi(ui, app, "Open Draft", "mail.editDraft", json!({}));
            ui.separator();
        } else {
            mi(ui, app, "Open in a Window", "ui.openWindow", json!({"message": id}));
            ui.separator();
        }
        mi(ui, app, "Reply", "mail.reply", json!({}));
        mi(ui, app, "Reply All", "mail.replyAll", json!({}));
        mi(ui, app, "Forward", "mail.forward", json!({}));
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
            crate::ribbon::folder_menu(ui, app, |app, folder| {
                let _ = app.run("mail.move", json!({"folder": folder}));
            });
        });
        mi(ui, app, "Archive", "mail.archive", json!({}));
        if in_junk {
            mi(ui, app, "Not Junk", "mail.notJunk", json!({}));
        } else {
            mi(ui, app, "Junk", "mail.junk", json!({}));
        }
        mi(ui, app, "Delete", "mail.delete", json!({}));
        ui.separator();
        mi(ui, app, "Add to Tasks", "tasks.fromMessage", json!({}));
        mi(ui, app, "Add Sender to Contacts", "people.fromMessage", json!({}));
        mi(ui, app, "Find Related Messages", "mail.findRelated", json!({}));
        mi(ui, app, "Save Message As…", "file.exportMessage", json!({}));
    });
}

#[cfg(test)]
mod overlay_scroll_tests;

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
        // The lane and its arrows, on a Mac too (`overlay_scroll_tests` has the bar that floats).
        app.overlay_scroll_bars = false;
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

    /// Show in Groups off draws no headings, and the single-line list draws every row as tall
    /// as one line, with more rows in the same window.
    #[test]
    fn the_list_without_groups_and_in_a_single_line() {
        let mut a = app();
        a.execute("view.groups", json!({"value": false})).unwrap();
        let mut harness = window(a, (1280.0, 1100.0));
        let rows = drawn(&mut harness);
        assert!(rows.len() >= 6);
        assert!(rows.iter().all(|(row, _)| row.group.is_none()));
        for pair in rows.windows(2) {
            assert_eq!(pair[1].1.min.y - pair[0].1.min.y, row_height(1));
        }
        let before = rows.len();
        harness.state_mut().execute("view.oneLine", json!({"value": true})).unwrap();
        harness.run_steps(3);
        let rows = drawn(&mut harness);
        assert!(rows.len() >= before);
        for (_, rect) in &rows {
            assert_eq!(rect.height(), ONE_LINE_H);
        }
        assert_eq!(one_line_sender_width(0.0), 0.0);
        assert_eq!(one_line_sender_width(10_000.0), 220.0);
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
        assert!(flag.max.x <= rect.max.x && (flag.center().y - (rect.min.y + SENDER_Y)).abs() < 0.01);
        assert_eq!(delete.center().y, flag.center().y);
        assert!(delete.max.x <= flag.min.x && delete.min.x > rect.max.x - TEXT_RIGHT, "the buttons keep out of the text's column");
        // Big enough to hit without aiming, and apart, so a click meant for one doesn't take the other.
        for b in [flag, delete] {
            assert!(b.width() >= 28.0 && b.height() >= 22.0, "{b:?}");
        }
        assert!(flag.min.x - delete.max.x >= 6.0);
        assert!(flag.max.y < rect.min.y + SUBJECT_Y - 4.0, "and off the subject's line, where the time is");

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

    /// A flagged message's row wears a faint tint, as in Outlook; one whose follow-up was due on
    /// a day before today has red text as well. A completed flag wears neither, and a selected
    /// row is the selection's colour whatever its flag.
    #[test]
    fn flagged_rows_are_tinted_and_overdue_ones_red() {
        let mut a = app();
        let ids: Vec<Id> = a.session.rows().iter().take(5).map(|r| r.id).collect();
        a.execute("mail.clearFlag", json!({"ids": ids})).unwrap();
        a.execute("mail.flag", json!({"ids": [ids[0]], "due": "2026-10-09"})).unwrap();
        a.execute("mail.flag", json!({"ids": [ids[1]], "due": "2026-10-14"})).unwrap();
        a.execute("mail.flag", json!({"ids": [ids[2]]})).unwrap();
        a.execute("mail.flag", json!({"ids": [ids[3]], "due": "2026-10-01"})).unwrap();
        a.execute("mail.complete", json!({"ids": [ids[3]]})).unwrap();
        let follow_ups: Vec<FollowUp> = ids.iter().map(|id| FollowUp::of(&a.session.mailbox.messages[id], &a.session)).collect();
        assert_eq!(follow_ups, [FollowUp::Overdue, FollowUp::Flagged, FollowUp::Flagged, FollowUp::None, FollowUp::None], "due today is not overdue");
        a.execute("mail.select", json!({"id": ids[4]})).unwrap();
        let mut harness = window(a, (1440.0, 900.0));
        harness.run_steps(3);
        let look = |harness: &Window, id: Id| {
            let hit = &harness.state().hit;
            (hit.rect("messageFlagShade", id).is_some(), hit.rect("messageOverdue", id).is_some())
        };
        let looks: Vec<(bool, bool)> = ids.iter().map(|id| look(&harness, *id)).collect();
        assert_eq!(looks, [(true, true), (true, false), (true, false), (false, false), (false, false)]);
        // Selected, the row is the selection's colour (its text stays red when it is overdue).
        harness.state_mut().execute("mail.select", json!({"id": ids[0]})).unwrap();
        harness.run_steps(3);
        assert_eq!(look(&harness, ids[0]), (false, true));
        // The tint is neither the list, the selection nor the pointer's shade, in either palette.
        for t in [Tokens::light(), Tokens::dark()] {
            assert_eq!(row_fill(&t, FollowUp::Flagged, false, false), Some(t.flagged_row));
            assert_eq!(row_fill(&t, FollowUp::Overdue, true, true), Some(t.selection));
            assert_eq!(row_fill(&t, FollowUp::None, false, false), None);
            assert!(![t.surface, t.selection, t.hover].contains(&t.flagged_row));
        }
    }
    /// #195: in a search of every mailbox a row names its folder, as a server's result does; in
    /// one folder, or with nothing searched, it doesn't. Two accounts' Inboxes are told apart.
    #[test]
    fn a_search_of_every_mailbox_names_each_rows_folder() {
        let mut a = app();
        assert!(!shows_folder(&a.session));
        a.execute("view.search", json!({"query": "budget", "scope": "folder"})).unwrap();
        assert!(!shows_folder(&a.session));
        a.execute("view.search", json!({"query": "budget", "scope": "everywhere"})).unwrap();
        assert!(shows_folder(&a.session));
        a.execute("view.search", json!({"query": "- @", "scope": "everywhere"})).unwrap();
        assert!(!shows_folder(&a.session), "nothing asked for yet: the folder's own list");
        let mb = &mut a.session.mailbox;
        let first = mb.inbox().unwrap();
        let projects = mb.folders.iter().find(|f| f.name == "Projects").unwrap().id;
        assert_eq!(crate::nav::result_folder_label(mb, mb.folder(first).unwrap()), "Inbox", "one account: the name alone");
        let work = mb.add_account(mailcraft_model::Account { name: "Work".into(), email: "me@work.example".into(), ..Default::default() });
        let second = mb.folders.iter().find(|f| f.account == work && f.kind == FolderKind::Inbox).unwrap().id;
        assert_eq!(crate::nav::result_folder_label(mb, mb.folder(second).unwrap()), "Inbox - Work");
        assert!(crate::nav::result_folder_label(mb, mb.folder(first).unwrap()).starts_with("Inbox - "));
        assert_eq!(crate::nav::result_folder_label(mb, mb.folder(projects).unwrap()), "Projects", "a name only one has");
    }

    /// #273: arranged by size, each row says its message's size where the folder of a search
    /// result goes: under the time, or before it with no preview; in a search of every mailbox
    /// the size is nearest the time and the folder before it. Otherwise no row says it.
    #[test]
    fn arranged_by_size_each_row_says_its_size() {
        let mut a = app();
        // Of a Microsoft account's message only the start is here; another has the mark of a
        // message too large to download, which any sender could have written.
        let ids: Vec<Id> = a.session.rows().iter().map(|row| row.id).collect();
        let (partial, marked, whole) = (ids[1], ids[2], ids[3]);
        let m = a.session.mailbox.message_mut(partial).unwrap();
        m.body = mailcraft_model::Body { text: "Only the start".into(), html: None };
        m.attachments.clear();
        m.unfetched = true;
        m.server = Some(mailcraft_model::sync::ServerMessage { size: 5 << 19, ..Default::default() });
        a.session.mailbox.message_mut(marked).unwrap().headers.push((mailcraft_model::NOT_DOWNLOADED.into(), "62914560".into()));
        let said = |a: &MailApp, id: Id| size_label(&a.session, a.session.mailbox.message(id).unwrap());
        assert_eq!((said(&a, partial), said(&a, whole)), (None, None), "arranged by date");
        a.execute("view.sort", json!({"by": "size", "descending": true})).unwrap();
        assert_eq!((said(&a, partial), said(&a, marked), said(&a, whole)), (Some("2.5 MB".into()), None, Some("1 KB".into())));
        a.execute("view.sort", json!({"by": "date"})).unwrap();

        let mut harness = window(a, (1200.0, 800.0));
        let sizes = |harness: &mut Window| -> Vec<(Row, Rect, Option<Rect>)> {
            drawn(harness).into_iter().map(|(row, rect)| (row.clone(), rect, harness.state().hit.rect("messageSize", row.id))).collect()
        };
        assert!(sizes(&mut harness).iter().all(|(_, _, size)| size.is_none()), "arranged by date");

        harness.state_mut().execute("view.sort", json!({"by": "size", "descending": true})).unwrap();
        harness.run_steps(3);
        let rows = sizes(&mut harness);
        assert!(rows.len() > 3);
        // Largest on top after what is pinned: the message whose server gave its size.
        assert_eq!(rows.iter().find(|(row, _, _)| !message(&harness, row.id).pinned).map(|(row, _, _)| row.id), Some(partial));
        // The one with the mark says nothing; every other row says its size.
        assert!(harness.state().hit.rect("messageSize", marked).is_none());
        for (row, rect, size) in rows.iter().filter(|(row, _, _)| row.id != marked) {
            let size = size.unwrap_or_else(|| panic!("row {} says no size", row.id));
            // Right-aligned at the time's end, on the preview's first line: under the time.
            assert!((size.max.x - (rect.max.x - TIME_RIGHT)).abs() <= 1.0, "row {}: {size:?} in {rect:?}", row.id);
            assert!(size.min.y > rect.min.y + PREVIEW_TOP - 2.0 && size.max.y < rect.max.y, "row {}", row.id);
            assert!(size.width() < FOLDER_W, "row {}: the time's column", row.id);
        }

        // With no preview, on the subject's line before the time.
        harness.state_mut().execute("view.previewLines", json!({"value": 0})).unwrap();
        harness.run_steps(3);
        for (row, rect, size) in sizes(&mut harness).into_iter().filter(|(row, _, _)| row.id != marked) {
            let size = size.unwrap_or_else(|| panic!("row {} says no size", row.id));
            assert!(size.max.x < rect.max.x - TIME_RIGHT - 20.0, "row {}: before the time", row.id);
            assert!(size.center().y > rect.center().y, "row {}: on the subject's line", row.id);
        }
        harness.state_mut().execute("view.previewLines", json!({"value": 1})).unwrap();

        // In a single line, on that line before the marks and the time.
        harness.state_mut().execute("view.oneLine", json!({"value": true})).unwrap();
        harness.run_steps(3);
        for (row, rect, size) in sizes(&mut harness).into_iter().filter(|(row, _, _)| row.id != marked) {
            let size = size.unwrap_or_else(|| panic!("row {} says no size", row.id));
            assert!(size.max.x < rect.max.x - TIME_RIGHT - 20.0, "row {}: before the time", row.id);
            assert!((size.center().y - rect.center().y).abs() <= 2.0, "row {}: on its one line", row.id);
        }
        harness.state_mut().execute("view.oneLine", json!({"value": false})).unwrap();

        // A search of every mailbox: the size, then the folder before it.
        harness.state_mut().execute("view.search", json!({"query": "the", "scope": "everywhere"})).unwrap();
        harness.run_steps(3);
        let rows = sizes(&mut harness);
        assert!(!rows.is_empty());
        for (row, _, size) in rows.iter().filter(|(row, _, _)| row.id != marked) {
            let size = size.unwrap_or_else(|| panic!("row {} says no size", row.id));
            let folder = at(&harness, "messageFolder", row.id);
            assert!(folder.max.x <= size.min.x, "row {}: {folder:?} before {size:?}", row.id);
        }
    }

    /// #273: in a single line, in a list as narrow as it goes, the sender's name stops short of
    /// the size, and a row with every mark, which has no room for both, keeps the name.
    #[test]
    fn in_a_narrow_single_line_the_sender_stops_short_of_the_size() {
        let mut a = app();
        a.ui.list_width = 240.0;
        let ids: Vec<Id> = a.session.rows().iter().map(|row| row.id).collect();
        // Every other row with every mark.
        for id in ids.iter().step_by(2) {
            let m = a.session.mailbox.message_mut(*id).unwrap();
            m.from.name = "Bartholomew Featherstonehaugh".into();
            (m.importance, m.answered) = (Importance::High, true);
            m.attachments.push(mailcraft_model::Attachment::new("a.pdf", "application/pdf", vec![1]));
        }
        a.execute("view.sort", json!({"by": "size", "descending": true})).unwrap();
        a.execute("view.oneLine", json!({"value": true})).unwrap();
        let mut harness = window(a, (760.0, 800.0));
        let rows = drawn(&mut harness);
        assert!(rows.len() > 3);
        let (mut said, mut left_out) = (0, 0);
        for (row, rect) in rows {
            assert!(rect.width() < 260.0, "{rect:?}");
            let sender = at(&harness, "messageSender", row.id);
            if harness.state().hit.rect("messageSize", row.id).is_none() {
                assert!(sender.width() > 0.0, "row {}: the name is drawn", row.id);
                left_out += 1;
            }
            if let Some(size) = harness.state().hit.rect("messageSize", row.id) {
                assert!(sender.max.x <= size.min.x, "row {}: {sender:?} runs into {size:?}", row.id);
                // Never for less than a single line's least room for the name.
                if message(&harness, row.id).answered {
                    assert!(sender.width() >= ONE_LINE_SENDER_MIN - 12.0, "row {}: {sender:?}", row.id);
                }
                said += 1;
            }
        }
        // Some rows have room for it, and some, crowded with marks, haven't.
        assert!(said > 0 && left_out > 0, "{said} said, {left_out} left out");
    }
}
