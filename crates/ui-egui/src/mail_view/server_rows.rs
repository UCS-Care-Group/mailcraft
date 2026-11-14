// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hits a search found on a mailbox's server, drawn as rows of the same list
//! ([`crate::server_search`]). Laid out as the list's rows are, so that one reads as any other
//! message: the sender, the subject and the time,
//! then its preview, and in a search of every mailbox the folder it is in under the time; read,
//! flag, importance, categories, paperclip.

use std::collections::HashSet;

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{FlagStatus, FolderKind, Id, Importance};

use super::{LIST_TEXT, PREVIEW_TOP, ROW_INSET, SENDER_TEXT, SENDER_Y, SUBJECT_Y, TEXT_LEFT, TEXT_RIGHT, TIME_RIGHT};
use crate::server_search::Link;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// A line of words in the list.
const NOTE_H: f32 = 22.0;
/// Most errors said at once.
const MAX_SAID: usize = 3;

/// One line of what is drawn below the local results.
enum Line {
    /// More results on the server, or Not connected, or still searching.
    Ask,
    /// The search stopped at its limit.
    Note(String),
    Problem(String),
    /// The server result at this index of the session's.
    Result(usize),
}

impl Line {
    fn height(&self, row_h: f32) -> f32 {
        match self {
            Line::Ask | Line::Note(_) | Line::Problem(_) => NOTE_H,
            Line::Result(_) => row_h,
        }
    }
}

/// The messages the servers found, as ordinary rows of the list. `local` is what the list above
/// already shows, so a message is not drawn twice. Nothing here says where a message was found.
fn lines(app: &MailApp, local: &HashSet<Id>) -> Vec<Line> {
    let link = app.server_link();
    if link == Link::None {
        return Vec::new();
    }
    let mut out = Vec::new();
    if matches!(link, Link::Ask | Link::NotConnected { .. } | Link::Searching) {
        out.push(Line::Ask);
    }
    let search = &app.session.server_search;
    if !mailcraft_engine::cmd::search_server::is_current(&app.session) || !search.asked() {
        return out;
    }
    if search.capped {
        out.push(Line::Note(tl!("The server search stopped after 40 messages. There may be more.").to_string()));
    }
    out.extend(search.errors.iter().take(MAX_SAID).map(|e| Line::Problem(e.clone())));
    for (i, r) in search.results.iter().enumerate() {
        // Opened and then deleted: the row must not bring it back. Downloaded since by a
        // Send/Receive: it is in the local list, and a second copy must not be added.
        let deleted = r.opened.is_some_and(|id| app.session.mailbox.message(id).is_none());
        let downloaded = r.opened.is_none()
            && mailcraft_engine::cmd::search_server::imap_uid(&r.found.server.id)
                .is_some_and(|uid| app.session.mailbox.messages.values().any(|m| m.account == r.account && m.folder == r.folder && m.uid == uid));
        let already = r.opened.is_some_and(|id| local.contains(&id));
        if !deleted && !downloaded && !already {
            out.push(Line::Result(i));
        }
    }
    out
}

/// How tall what is below the local results is.
pub(super) fn height(app: &MailApp, local: &HashSet<Id>, row_h: f32) -> f32 {
    lines(app, local).iter().map(|l| l.height(row_h)).sum()
}

/// Draw what is below the local results from `top` down, in the list's columns (`width` wide
/// from `left`), the part in `viewport` only.
pub(super) fn show(app: &mut MailApp, ui: &mut Ui, local: &HashSet<Id>, left: f32, top: f32, width: f32, row_h: f32, viewport: Rect) {
    let t = Tokens::get(ui.ctx());
    let mut y = top;
    for line in lines(app, local) {
        let h = line.height(row_h);
        let rect = Rect::from_min_size(pos2(left, y), vec2(width, h));
        y += h;
        if rect.max.y < viewport.min.y || rect.min.y > viewport.max.y {
            continue;
        }
        match line {
            Line::Ask => link_line(app, ui, rect),
            Line::Note(text) => {
                paint_clipped(ui.painter(), pos2(rect.min.x + 10.0, rect.center().y), rect.width() - 20.0, &text, regular(11.5), t.text_dim);
                app.hit.add("serverNote", 0, rect);
            }
            Line::Problem(text) => {
                let r = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(12.0, 12.0));
                icons::paint(ui.painter(), r, "highImportance", t.orange, t.accent);
                let w = paint_clipped(ui.painter(), pos2(rect.min.x + 28.0, rect.center().y), rect.width() - 36.0, &text, regular(11.5), t.text);
                if w >= rect.width() - 40.0 {
                    ui.interact(rect, ui.scope_id().with(("server_problem", y as i64)), Sense::hover()).on_hover_text(text);
                }
                app.hit.add("serverProblem", 0, rect);
            }
            Line::Result(i) => result_row(app, ui, i, rect),
        }
    }
}

/// More results on the server, as a link; greyed with why when it can't be asked now.
fn link_line(app: &mut MailApp, ui: &mut Ui, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let link = app.server_link();
    let (text, live) = match link {
        Link::Ask => (tl!("More results on the server").to_string(), true),
        Link::NotConnected { retry } => (format!("{} · {}", tl!("More results on the server"), tl!("Not connected")), retry),
        Link::Searching => (tl!("Searching the server…").to_string(), false),
        Link::End | Link::None => return,
    };
    let font = regular(12.5);
    let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), t.accent_text);
    let at = Rect::from_min_size(pos2(rect.min.x + TEXT_LEFT - 18.0, rect.center().y - galley.size().y / 2.0), galley.size());
    let resp = ui.interact(at.expand(4.0), ui.scope_id().with("more_on_server"), if live { Sense::click() } else { Sense::hover() });
    let resp = if live { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    let resp = match link {
        Link::NotConnected { retry: true } => resp.on_hover_text(tl!("The server couldn't be reached. Click to try again.")),
        Link::NotConnected { retry: false } => resp.on_hover_text(tl!("Send/Receive signs in; then the server can be searched.")),
        _ => resp,
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, live, &text));
    app.hit.add("moreOnServer", 0, at);
    let color = match link {
        Link::Ask => t.accent_text,
        _ => t.text_disabled,
    };
    ui.painter().text(at.left_center(), Align2::LEFT_CENTER, &text, font, color);
    if live && resp.hovered() {
        ui.painter().line_segment([pos2(at.min.x, at.max.y + 1.0), pos2(at.max.x, at.max.y + 1.0)], Stroke::new(1.0, color));
    }
    if resp.clicked() {
        let _ = app.run("ui.moreResultsOnServer", serde_json::json!({}));
    }
}

/// One server result, as a row of the list.
fn result_row(app: &mut MailApp, ui: &mut Ui, index: usize, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let Some(r) = app.session.server_search.results.get(index).cloned() else { return };
    // Opened: the message as it is now (a flag or a read mark may have changed). Otherwise the
    // hit, which is all there is until it is fetched.
    let m = match r.opened.and_then(|id| app.session.mailbox.message(id).cloned()) {
        Some(stored) => stored,
        None => mailcraft_engine::cmd::search_server::as_message(r.account, r.folder, &r.found),
    };
    let resp = ui.interact(rect, ui.scope_id().with(("server_result", index)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    app.hit.add("serverResult", index as u64, rect);
    let p = ui.painter();
    let one = app.session.prefs.one_line;
    let body = Rect::from_min_max(pos2(rect.min.x + ROW_INSET, rect.min.y), pos2(rect.max.x, rect.max.y - 1.0));
    let selected = r.opened.is_some_and(|id| app.session.sel.messages.contains(&id));
    // The same card, unread dot and account stripe as a message already on this computer.
    let card = body.shrink2(vec2(4.0, 1.0));
    if selected {
        p.rect_filled(card, 6.0, t.selection);
        p.rect_stroke(card, 6.0, Stroke::new(1.0, t.selection_edge), egui::StrokeKind::Inside);
    } else if resp.hovered() {
        p.rect_filled(card, 6.0, t.hover_subtle);
    }
    let unread = !m.read;
    let line1 = if one { rect.center().y } else { rect.min.y + SENDER_Y };
    let line2 = if one { rect.center().y } else { rect.min.y + SUBJECT_Y };
    if unread {
        p.circle_filled(pos2(body.min.x + 13.0, line1), 4.0, t.accent);
    }
    if !selected {
        p.line_segment([pos2(body.min.x + 12.0, rect.max.y - 0.5), pos2(rect.max.x - 10.0, rect.max.y - 0.5)], Stroke::new(1.0, t.border_faint));
    }
    if let Some(color) = super::account_stripe(&app.session, &m) {
        let stripe = Rect::from_min_max(pos2(rect.min.x + 1.0, rect.min.y + 3.0), pos2(rect.min.x + 1.0 + super::ACCOUNT_STRIPE, rect.max.y - 3.0));
        p.rect_filled(stripe, 1.5, crate::theme::c32(color));
        if t.high_contrast {
            p.rect_stroke(stripe, 1.5, Stroke::new(1.0, t.text), egui::StrokeKind::Inside);
        }
    }
    let outgoing = app.session.mailbox.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox));
    let person = if outgoing { m.to.first().unwrap_or(&m.from) } else { &m.from };
    let pictured = app.ui.sender_images && !one;
    if pictured {
        let center = pos2(rect.min.x + TEXT_LEFT + super::AVATAR_RADIUS, rect.min.y + super::AVATAR_Y);
        crate::sender_images::avatar(app, ui, center, super::AVATAR_RADIUS, &m, person);
    }
    let left = rect.min.x + TEXT_LEFT + if pictured { 2.0 * super::AVATAR_RADIUS + super::AVATAR_GAP } else { 0.0 };
    let (time_end, text_end) = (rect.max.x - TIME_RIGHT, rect.max.x - TEXT_RIGHT);
    // The marks at the right of the sender's line: the flag in its column, the rest before it.
    // In one line the date sits there first, and the marks stay to its left.
    let flag = Rect::from_center_size(pos2(rect.max.x - 14.0, line1), vec2(12.0, 12.0));
    let sender_line_end = if m.flag == FlagStatus::None { time_end } else { flag.min.x - 2.0 };
    let mut x = sender_line_end;
    match m.flag {
        FlagStatus::Flagged => icons::paint(p, flag, "flag", t.icon, t.accent),
        FlagStatus::Complete => icons::paint(p, flag, "complete", t.icon, t.accent),
        FlagStatus::None => {}
    }
    let date =
        crate::dates::list_label(mailcraft_engine::list::when(&m), app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
    let (font, color) = if unread { (semibold(LIST_TEXT), t.accent_text) } else { (regular(LIST_TEXT), t.text) };
    let mut one_date_w = 0.0;
    if one {
        one_date_w = p.text(pos2(sender_line_end, line1), Align2::RIGHT_CENTER, &date, font.clone(), color).width();
        x = sender_line_end - one_date_w - 10.0;
    }
    let mut marker = |icon: &str, color| {
        icons::paint(p, Rect::from_center_size(pos2(x - 6.0, line1), vec2(12.0, 12.0)), icon, color, t.accent);
        x -= 15.0;
    };
    if m.has_attachments() {
        marker("attach", t.text_dim);
    }
    match m.importance {
        Importance::High => marker("highImportance", t.icon),
        Importance::Low => marker("lowImportance", t.icon),
        Importance::Normal => {}
    }
    let who = if outgoing {
        let names: Vec<&str> = m.to.iter().map(|a| a.display()).collect();
        if names.is_empty() { tl!("(no recipients)").to_string() } else { names.join("; ") }
    } else {
        m.from.display().to_string()
    };
    let who_font = if unread { semibold(SENDER_TEXT) } else { regular(SENDER_TEXT) };
    // A single line puts the sender in the same column a local row uses. The same y and the
    // same left edge would draw the sender on top of the subject.
    let who_end = if one { (left + super::one_line_sender_width(time_end - left)).min(x - 4.0) } else { text_end.min(x - 4.0) };
    paint_clipped(p, pos2(left, line1), (who_end - left).max(0.0), &who, who_font, t.text);
    let date_w = if one { one_date_w } else { p.text(pos2(time_end, line2), Align2::RIGHT_CENTER, &date, font.clone(), color).width() };
    // In a search of every mailbox the folder it is in, under the time (or before it, with no
    // preview), then its categories; its preview before them.
    let lines = app.session.prefs.preview_lines;
    let preview_y = rect.min.y + PREVIEW_TOP + 7.0;
    let (mut sx, tail_y) = if lines > 0 { (time_end, preview_y) } else { (time_end - date_w - 8.0, line2) };
    if let Some(folder) = super::shows_folder(&app.session).then(|| app.session.mailbox.folder(r.folder)).flatten() {
        let label = crate::nav::result_folder_label(&app.session.mailbox, folder);
        sx = super::paint_result_folder(p, pos2(sx, tail_y), &label, &t) - 6.0;
        app.hit.add("serverResultFolder", index as u64, Rect::from_min_max(pos2(sx + 6.0, tail_y - 7.0), pos2(time_end, tail_y + 7.0)));
    }
    for c in m.categories.iter().take(4) {
        let color = crate::theme::c32(app.session.mailbox.category_color_of(m.account, c));
        p.rect_filled(Rect::from_center_size(pos2(sx - 5.0, tail_y), vec2(10.0, 10.0)), 2.0, color);
        sx -= 13.0;
    }
    let subject_end = if lines > 0 { text_end.min(time_end - date_w - 8.0) } else { text_end.min(sx - 6.0) };
    let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { m.subject.as_str() };
    let subject_x = if one { who_end + 10.0 } else { left };
    paint_clipped(p, pos2(subject_x, line2), (subject_end - subject_x).max(0.0), subject, font, color);
    if lines > 0 {
        let end = text_end.min(sx - 6.0);
        let preview: String = m.body.text.split_whitespace().collect::<Vec<_>>().join(" ");
        paint_clipped(p, pos2(left, preview_y), (end - left).max(0.0), &preview, regular(LIST_TEXT), t.text_dim);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{who}, {subject}")));
    if resp.clicked() {
        app.open_server_result(index);
    }
    if resp.double_clicked()
        && let Some(id) = app.session.server_search.results.get(index).and_then(|r| r.opened)
    {
        let _ = app.run("ui.openWindow", serde_json::json!({"message": id}));
    }
}
