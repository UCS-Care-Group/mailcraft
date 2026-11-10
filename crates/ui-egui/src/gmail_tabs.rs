// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's categories as a row of tabs above a Gmail account's Inbox (#911), when View › Gmail
//! Categories says tabs (`view.gmailCategories`): Primary, Social, Promotions, Updates and Forums,
//! each with its unread count, in place of the five rows under the Inbox in the folder pane.
//!
//! A tab does what its row in the folder pane does (`folder.select` with `gmail`). No tab is
//! chosen while the list shows the whole Inbox, as the Inbox opens; a click on the chosen tab
//! goes back to the whole Inbox. Tab reaches a tab, the arrow keys go from one to the next, and
//! Enter or Space chooses it. Messages let go over a tab move to its category, where moves between
//! Gmail's categories are on ([`crate::gmail_move`]).

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::GmailCategory;
use serde_json::json;

use crate::MailApp;
use crate::nav::gmail_name;
use crate::theme::{Tokens, bold, paint_clipped, regular, semibold};

/// A line of tabs' height.
const ROW_H: f32 = 32.0;
/// Where the first tab starts, as the list's own tabs do.
const LEFT: f32 = 22.0;
/// Room either side of a tab's text.
const PAD: f32 = 8.0;
/// The size of a tab's name.
const NAME_SIZE: f32 = 13.0;
/// The size of the unread count in its pill.
const COUNT_SIZE: f32 = 11.0;

/// The tab's place in the row, which `ui.inspect` knows it by (`gmailTab`, with that number).
fn hit_id(category: GmailCategory) -> u64 {
    GmailCategory::ALL.iter().position(|c| *c == category).map_or(0, |i| i as u64)
}

/// What a tab is read out as: its name and how many of its messages are unread.
pub(crate) fn said(category: GmailCategory, unread: usize) -> String {
    let name = gmail_name(category);
    if unread > 0 { format!("{name}, {}", tln!(unread, "{n} unread", "{n} unread")) } else { name.to_string() }
}

/// The row of tabs, above the list of a Gmail Inbox whose categories are shown as tabs; nothing
/// for any other list.
pub(crate) fn row(app: &mut MailApp, ui: &mut Ui) {
    if !mailcraft_engine::gmail::tabs_shown(&mut app.session) {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let inbox = app.session.view.folder;
    let now = mailcraft_engine::gmail::in_view(&app.session);
    let counts = app.session.folder_counts();

    // Each tab: its name, and the pill of its unread count when it has any.
    let ctx = ui.ctx().clone();
    let measure = |text: String, font| ctx.fonts_mut(|f| f.layout_no_wrap(text, font, t.text).size().x);
    let tabs: Vec<(GmailCategory, usize, f32, f32)> = GmailCategory::ALL
        .into_iter()
        .map(|category| {
            let unread = counts.gmail(inbox, category).unread;
            let name_w = measure(gmail_name(category).to_string(), semibold(NAME_SIZE));
            let pill_w = if unread > 0 { (measure(unread.to_string(), bold(COUNT_SIZE)) + 10.0).max(18.0) + 6.0 } else { 0.0 };
            (category, unread, name_w, pill_w)
        })
        .collect();
    // Laid out from the left; a tab that doesn't fit after another starts a line of its own,
    // so that every name is read whole in a narrow list. One wider than the list is cut short.
    let width = ui.available_width();
    let (start, end) = (LEFT - PAD, (width - 8.0).max(LEFT));
    let mut placed: Vec<(f32, usize, f32)> = Vec::with_capacity(tabs.len());
    let (mut x, mut line) = (start, 0usize);
    for (_, _, name_w, pill_w) in &tabs {
        let w = name_w + pill_w + 2.0 * PAD;
        if x > start && x + w > end {
            (x, line) = (start, line + 1);
        }
        let w = w.min(end - x).max(0.0);
        placed.push((x, line, w));
        x += w;
    }
    let lines = placed.last().map_or(1, |(_, line, _)| line + 1);
    let (r, _) = ui.allocate_exact_size(vec2(width, ROW_H * lines as f32), Sense::hover());
    ui.painter().hline(r.x_range(), r.max.y - 0.5, Stroke::new(1.0, t.border));

    for ((category, unread, name_w, pill_w), (x, line, w)) in tabs.into_iter().zip(placed) {
        let name_w = (w - pill_w - 2.0 * PAD).clamp(0.0, name_w);
        let top = r.min.y + ROW_H * line as f32;
        let tab = Rect::from_min_max(pos2(r.min.x + x, top + 2.0), pos2(r.min.x + x + w, top + ROW_H));
        let resp = ui.interact(tab, ui.scope_id().with(("gmail_tab", inbox, category.code())), Sense::click());
        app.hit.add("gmailTab", hit_id(category), tab);
        let on = now == Some(category);
        let under = Rect::from_min_max(pos2(tab.min.x + 2.0, tab.max.y - 2.5), pos2(tab.max.x - 2.0, tab.max.y));
        if on {
            ui.painter().rect_filled(under, 1.0, t.accent_text);
        } else if resp.hovered() {
            ui.painter().rect_filled(tab.shrink2(vec2(1.0, 3.0)), 4.0, t.hover_subtle);
            ui.painter().rect_filled(under, 1.0, t.border_strong);
        }
        // Reached with Tab or the arrow keys: where the keyboard is.
        if resp.has_focus() {
            ui.painter().rect_stroke(tab.shrink2(vec2(1.0, 3.0)), 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
        let mid = tab.center().y - 1.0;
        let font = if on { semibold(NAME_SIZE) } else { regular(NAME_SIZE) };
        let ink = if on { t.accent_text } else { t.text };
        let name = gmail_name(category);
        paint_clipped(ui.painter(), pos2(tab.min.x + PAD, mid), name_w + 1.0, name, font, ink);
        if unread > 0 {
            let galley = ui.painter().layout_no_wrap(unread.to_string(), bold(COUNT_SIZE), t.accent_soft_text);
            let w = pill_w - 6.0;
            let pill = Rect::from_min_size(pos2(tab.min.x + PAD + name_w + 6.0, mid - 8.5), vec2(w, 17.0));
            ui.painter().rect_filled(pill, 8.5, t.accent_soft);
            ui.painter().galley(pill.center() - galley.size() / 2.0, galley, t.accent_soft_text);
        }
        let said = said(category, unread);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Tab, true, &said));
        // A tab is chosen (AccessKit's `selected`), not switched on.
        ui.ctx().accesskit_node_builder(resp.id, |node| node.set_selected(on));
        let resp = if name_w + 0.5 < measure(name.to_string(), semibold(NAME_SIZE)) { resp.on_hover_text(name) } else { resp };
        if resp.clicked() {
            // The chosen tab again: the whole Inbox.
            let params = if on { json!({"id": inbox}) } else { json!({"id": inbox, "gmail": category.code()}) };
            let _ = app.run("folder.select", params);
        }
        // Messages dragged out of the list and let go over it move to it (#911).
        crate::gmail_move::drop_target(app, ui, &resp, tab, category);
    }
}
