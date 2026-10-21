// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Below the local results of a search: More results on the server, and the section On the
//! server with what came back (R6.2, [`crate::server_search`]). Laid out as the list's rows are,
//! so that a server result reads as any other message: the sender, the subject and the time,
//! then its preview, and in a search of every mailbox the folder it is in under the time; read,
//! flag, importance, categories, paperclip.

use std::collections::HashSet;

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{FlagStatus, Id, Importance};

use super::{GROUP_H, GROUP_TEXT, LIST_TEXT, PREVIEW_TOP, ROW_INSET, SENDER_TEXT, SENDER_Y, SUBJECT_Y, TEXT_LEFT, TEXT_RIGHT, TIME_RIGHT};
use crate::server_search::Link;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// The link's line, and a line of words in the section.
const LINK_H: f32 = 32.0;
const NOTE_H: f32 = 22.0;
/// Most errors said at once.
const MAX_SAID: usize = 3;

/// One line of what is drawn below the local results.
enum Line {
    Link(Link),
    Heading,
    Note(String),
    Problem(String),
    /// The server result at this index of the session's.
    Result(usize),
}

impl Line {
    fn height(&self, row_h: f32) -> f32 {
        match self {
            Line::Link(_) => LINK_H,
            Line::Heading => GROUP_H,
            Line::Note(_) | Line::Problem(_) => NOTE_H,
            Line::Result(_) => row_h,
        }
    }
}

/// What is below the local results now, line by line. `local`: the messages the local results
/// show (a result opened, once there, shows there only).
fn lines(app: &MailApp, local: &HashSet<Id>) -> Vec<Line> {
    let link = app.server_link();
    if link == Link::None {
        return Vec::new();
    }
    let search = &app.session.server_search;
    let held = mailcraft_engine::cmd::search_server::is_current(&app.session);
    if !held || !search.asked() {
        return vec![Line::Link(link)];
    }
    let mut out = vec![Line::Heading];
    if search.checked_here {
        out.push(Line::Note(tl!("Some of the search was checked on this computer").to_string()));
    }
    out.extend(search.errors.iter().take(MAX_SAID).map(|e| Line::Problem(e.clone())));
    for (i, r) in search.results.iter().enumerate() {
        let shown_here = r.opened.is_some_and(|id| local.contains(&id) || app.session.mailbox.message(id).is_none());
        if !shown_here {
            out.push(Line::Result(i));
        }
    }
    match link {
        Link::End if search.results.is_empty() => out.push(Line::Note(tl!("Nothing more was found on the server").to_string())),
        Link::End => {}
        other => out.push(Line::Link(other)),
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
            Line::Link(link) => link_line(app, ui, rect, link),
            Line::Heading => {
                let gr = Rect::from_min_max(pos2(rect.min.x + ROW_INSET, rect.min.y), rect.max);
                ui.painter().rect_filled(gr, 0.0, t.surface.lerp_to_gamma(t.selection, if t.dark { 1.0 } else { 0.45 }));
                ui.painter().rect_stroke(gr, 0.0, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
                let said = if app.server_link() == Link::Searching { tl!("On the server · Searching…") } else { tl!("On the server") };
                paint_clipped(ui.painter(), pos2(gr.min.x + 10.0, gr.center().y), gr.width() - 16.0, said, semibold(GROUP_TEXT), t.text);
                app.hit.add("onTheServer", 0, gr);
            }
            Line::Note(text) => {
                paint_clipped(ui.painter(), pos2(rect.min.x + 10.0, rect.center().y), rect.width() - 20.0, &text, regular(11.5), t.text_dim);
                app.hit.add("serverNote", 0, rect);
            }
            Line::Problem(text) => {
                let r = Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(12.0, 12.0));
                icons::paint(ui.painter(), r, "highImportance", t.orange, t.accent);
                let w = paint_clipped(ui.painter(), pos2(rect.min.x + 28.0, rect.center().y), rect.width() - 36.0, &text, regular(11.5), t.text);
                if w >= rect.width() - 40.0 {
                    ui.interact(rect, ui.id().with(("server_problem", y as i64)), Sense::hover()).on_hover_text(text);
                }
                app.hit.add("serverProblem", 0, rect);
            }
            Line::Result(i) => result_row(app, ui, i, rect),
        }
    }
}

/// More results on the server, as a link; greyed with why when it can't be asked now.
fn link_line(app: &mut MailApp, ui: &mut Ui, rect: Rect, link: Link) {
    let t = Tokens::get(ui.ctx());
    let (text, live) = match link {
        Link::Ask => (tl!("More results on the server").to_string(), true),
        Link::NotConnected { retry } => (format!("{} · {}", tl!("More results on the server"), tl!("Not connected")), retry),
        Link::Searching => (tl!("Searching the server…").to_string(), false),
        Link::End | Link::None => return,
    };
    let font = regular(12.5);
    let galley = ui.painter().layout_no_wrap(text.clone(), font.clone(), t.accent_text);
    let at = Rect::from_min_size(pos2(rect.min.x + TEXT_LEFT - 18.0, rect.center().y - galley.size().y / 2.0), galley.size());
    let resp = ui.interact(at.expand(4.0), ui.id().with("more_on_server"), if live { Sense::click() } else { Sense::hover() });
    let resp = if live { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    let resp = match link {
        Link::NotConnected { retry: true } => resp.on_hover_text(tl!("Microsoft's server couldn't be reached. Click to try again.")),
        Link::NotConnected { retry: false } => resp.on_hover_text(tl!("Send/Receive signs in to Microsoft; then the server can be searched.")),
        _ => resp,
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, live, &text));
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
    let m = mailcraft_engine::cmd::search_server::as_message(r.account, r.folder, &r.found);
    let resp = ui.interact(rect, ui.id().with(("server_result", index)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    app.hit.add("serverResult", index as u64, rect);
    let p = ui.painter();
    let body = Rect::from_min_max(pos2(rect.min.x + ROW_INSET, rect.min.y), pos2(rect.max.x, rect.max.y - 1.0));
    let selected = r.opened.is_some_and(|id| app.session.sel.messages.contains(&id));
    if selected {
        p.rect_filled(body, 0.0, t.selection);
    } else if resp.hovered() {
        p.rect_filled(body, 0.0, t.hover.linear_multiply(0.6));
    }
    let unread = !m.read;
    if unread {
        p.rect_filled(Rect::from_min_max(body.min, pos2(body.min.x + 4.0, body.max.y)), 0.0, t.accent);
    }
    let rule = if t.dark { t.border_strong } else { t.border };
    p.line_segment([pos2(body.min.x, rect.max.y - 0.5), pos2(rect.max.x, rect.max.y - 0.5)], Stroke::new(1.0, rule));
    let left = rect.min.x + TEXT_LEFT;
    let (time_end, text_end) = (rect.max.x - TIME_RIGHT, rect.max.x - TEXT_RIGHT);
    let (line1, line2) = (rect.min.y + SENDER_Y, rect.min.y + SUBJECT_Y);
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
    let who = m.from.display().to_string();
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
        let color = crate::theme::c32(app.session.mailbox.category_color(c));
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
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{who}, {subject}")));
    if resp.clicked() {
        app.open_server_result(index);
    }
    if resp.double_clicked()
        && let Some(id) = app.session.server_search.results.get(index).and_then(|r| r.opened)
    {
        let _ = app.run("ui.openWindow", serde_json::json!({"message": id}));
    }
}
