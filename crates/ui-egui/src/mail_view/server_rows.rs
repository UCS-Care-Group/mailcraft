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
    Problem(String),
    /// The server result at this index of the session's.
    Result(usize),
}

impl Line {
    fn height(&self, row_h: f32) -> f32 {
        match self {
            Line::Problem(_) => NOTE_H,
            Line::Result(_) => row_h,
        }
    }
}

/// The messages the servers found, as ordinary rows of the list. `local` is what the list above
/// already shows, so a message is not drawn twice. Nothing here says where a message was found.
fn lines(app: &MailApp, local: &HashSet<Id>) -> Vec<Line> {
    if app.server_link() == Link::None {
        return Vec::new();
    }
    let search = &app.session.server_search;
    if !mailcraft_engine::cmd::search_server::is_current(&app.session) || !search.asked() {
        return Vec::new();
    }
    let mut out = Vec::new();
    out.extend(search.errors.iter().take(MAX_SAID).map(|e| Line::Problem(e.clone())));
    for (i, r) in search.results.iter().enumerate() {
        let already = r.opened.is_some_and(|id| local.contains(&id));
        if !already {
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

/// One server result, as a row of the list.
fn result_row(app: &mut MailApp, ui: &mut Ui, index: usize, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let Some(r) = app.session.server_search.results.get(index).cloned() else { return };
    let m = mailcraft_engine::cmd::search_server::as_message(r.account, r.folder, &r.found);
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
    let mut x = time_end;
    let flag = Rect::from_center_size(pos2(rect.max.x - 14.0, line1), vec2(12.0, 12.0));
    match m.flag {
        FlagStatus::Flagged => icons::paint(p, flag, "flag", t.icon, t.accent),
        FlagStatus::Complete => icons::paint(p, flag, "complete", t.icon, t.accent),
        FlagStatus::None => {}
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
    paint_clipped(p, pos2(left, line1), (text_end.min(x - 4.0) - left).max(0.0), &who, who_font, t.text);
    let date =
        crate::dates::list_label(mailcraft_engine::list::when(&m), app.session.now(), app.session.offset_at(m.date), app.session.prefs.twelve_hour);
    let (font, color) = if unread { (semibold(LIST_TEXT), t.accent_text) } else { (regular(LIST_TEXT), t.text) };
    let date_w = p.text(pos2(time_end, line2), Align2::RIGHT_CENTER, &date, font.clone(), color).width();
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
    paint_clipped(p, pos2(left, line2), (subject_end - left).max(0.0), subject, font, color);
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
