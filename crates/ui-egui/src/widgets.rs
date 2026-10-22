// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The classic ribbon's pieces (large and small buttons, groups), menu items, and the small
//! pieces the panes share (icon buttons, avatars, category chips).

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use serde_json::Value;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// Height of the ribbon content area, and of the groups' names under it (with the room between them).
pub const CONTENT_H: f32 = 66.0;
pub const LABEL_H: f32 = 22.0;

/// Shortcut text for a command (`⌘R` on macOS, `Ctrl+R` elsewhere): its first key.
pub fn shortcut_text(app: &MailApp, id: &str) -> String {
    let key = app.session.registry.get(id).and_then(|s| s.shortcut.split(" / ").next()).filter(|k| !k.is_empty());
    // The keys the body of a message being written has for its formatting.
    key.or_else(|| crate::compose::editor::key_of(id)).map(key_text).unwrap_or_default()
}

/// A key as the platform writes it (`⌘⇧K` on macOS, `Ctrl+Shift+K` elsewhere).
pub fn key_text(sc: &str) -> String {
    if sc.is_empty() {
        return String::new();
    }
    let sc = sc.replace("Arrow", "");
    if cfg!(target_os = "macos") {
        sc.replace("Mod+", "⌘").replace("Shift+", "⇧").replace("Alt+", "⌥").replace("Ctrl+", "⌃")
    } else {
        sc.replace("Mod+", "Ctrl+")
    }
}

/// The tooltip of a control that runs command `id`: its name and key, where it is in the ribbon,
/// and why it can't run when it can't. `label` is already in the interface language.
pub(crate) fn command_tooltip(app: &MailApp, resp: Response, label: &str, id: &str) -> Response {
    let sc = shortcut_text(app, id);
    let spec = app.session.registry.get(id);
    let desc = spec.map(|s| crate::i18n::location(s.location)).unwrap_or_default();
    let reason = spec.and_then(|s| (s.enabled)(&app.session));
    resp.on_hover_ui(|ui| {
        ui.set_max_width(260.0);
        if sc.is_empty() {
            ui.label(egui::RichText::new(label).font(semibold(12.0)));
        } else {
            ui.label(egui::RichText::new(format!("{label} ({sc})")).font(semibold(12.0)));
        }
        if !desc.is_empty() {
            ui.label(egui::RichText::new(desc).small().weak());
        }
        if let Some(r) = reason {
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Not available: {reason}"), &[("reason", tl!(r))])).small().weak());
        }
    })
}

pub(crate) fn bg(ui: &Ui, r: Rect, resp: &Response, checked: bool, t: &Tokens) {
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if checked {
        t.checked
    } else if resp.hovered() {
        t.hover
    } else {
        return;
    };
    ui.painter().rect_filled(r, CornerRadius::same(4), fill);
    if checked {
        ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, t.accent.linear_multiply(0.5)), egui::StrokeKind::Inside);
    }
}

fn text_w(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), regular(size), Color32::WHITE).size().x)
}

/// Lay out and paint a large button of the classic ribbon: the icon over its label (of one line or
/// two), greyed when it isn't `on`, pressed-looking when `checked`, with a `▾` when it opens a menu.
/// Gives back where its label was painted too.
pub(crate) fn big_frame(ui: &mut Ui, icon: &str, label: &str, on: bool, checked: bool, menu: bool) -> (Response, Rect) {
    let t = Tokens::get(ui.ctx());
    let galley_w = label.split('\n').map(|l| text_w(ui, l, 11.5)).fold(0.0, f32::max);
    let w = (galley_w + if menu { 20.0 } else { 12.0 }).max(46.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, CONTENT_H), Sense::click());
    if on {
        bg(ui, r, &resp, checked, &t);
    }
    let ic = Rect::from_center_size(pos2(r.center().x, r.min.y + 20.0), vec2(32.0, 32.0));
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    icons::paint(ui.painter(), ic, icon, c, a);
    let mut y = r.min.y + 41.0;
    let lines: Vec<&str> = label.split('\n').collect();
    let mut painted = Rect::NOTHING;
    for (i, l) in lines.iter().enumerate() {
        let txt = if menu && i + 1 == lines.len() { format!("{l} ▾") } else { (*l).to_string() };
        let colour = if on { t.text } else { t.text_disabled };
        painted = painted.union(ui.painter().text(pos2(r.center().x, y), Align2::CENTER_TOP, txt, regular(11.5), colour));
        y += 13.0;
    }
    (resp, painted)
}

/// Lay out and paint a small button of the classic ribbon: a 16 pt icon with its label beside it.
pub(crate) fn small_frame(ui: &mut Ui, icon: &str, label: Option<&str>, on: bool, checked: bool, menu: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let text_width = label.map(|l| text_w(ui, l, 11.5) + 6.0).unwrap_or(0.0);
    let (r, resp) = ui.allocate_exact_size(vec2(24.0 + text_width + if menu { 10.0 } else { 0.0 }, 22.0), Sense::click());
    if on {
        bg(ui, r, &resp, checked, &t);
    }
    let ic = Rect::from_center_size(pos2(r.min.x + 12.0, r.center().y), vec2(16.0, 16.0));
    let (c, a) = if on { (t.icon, t.accent) } else { (t.text_disabled, t.text_disabled) };
    icons::paint(ui.painter(), ic, icon, c, a);
    if let Some(l) = label {
        ui.painter().text(pos2(r.min.x + 24.0, r.center().y), Align2::LEFT_CENTER, l, regular(11.5), if on { t.text } else { t.text_disabled });
    }
    if menu {
        icons::paint(ui.painter(), Rect::from_center_size(pos2(r.max.x - 7.0, r.center().y), vec2(8.0, 8.0)), "dropdown", c, a);
    }
    resp
}

/// A column of small buttons (up to three fit the ribbon's height).
pub fn stack(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = vec2(1.0, 0.0);
        add(ui);
    });
}

/// A ribbon group: content, a centred label below and a divider on the right.
pub fn group(ui: &mut Ui, title: &str, app: &mut MailApp, add: impl FnOnce(&mut Ui, &mut MailApp)) {
    let t = Tokens::get(ui.ctx());
    let key = crate::ribbon::hit_id(title);
    let title = tl!(title);
    let label_w = text_w(ui, title, 11.0);
    let start = ui.cursor().min;
    let inner = ui.vertical(|ui| {
        ui.set_min_width(label_w + 10.0);
        ui.allocate_ui_with_layout(vec2(ui.available_width(), CONTENT_H), egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing = vec2(1.0, 1.0);
            add(ui, app);
        });
    });
    let r = inner.response.rect;
    let r = Rect::from_min_max(start, pos2(r.max.x, start.y + CONTENT_H + LABEL_H));
    // The group's name, with room above it under the buttons' labels.
    let at = ui.painter().text(pos2(r.center().x, r.max.y - 8.0), Align2::CENTER_CENTER, title, regular(11.0), t.group_label);
    app.hit.add("ribbonGroup", key, at);
    ui.add_space(5.0);
    let x = ui.cursor().min.x;
    ui.painter().line_segment([pos2(x, start.y + 4.0), pos2(x, start.y + CONTENT_H + LABEL_H - 4.0)], Stroke::new(1.0, t.border));
    ui.add_space(6.0);
}

/// A menu item that runs a command; shows its shortcut and greys out when it can't run.
pub fn mi(ui: &mut Ui, app: &mut MailApp, label: &str, id: &str, params: Value) {
    let sc = shortcut_text(app, id);
    let enabled = app.enabled(id);
    let resp = ui.add_enabled(enabled, egui::Button::new(tl!(label)).shortcut_text(sc).min_size(vec2(190.0, 0.0)));
    if resp.clicked() {
        let _ = app.run(id, params);
        ui.close();
    }
}

/// A menu item with a check mark when `checked` (the current choice of several).
pub fn mi_check(ui: &mut Ui, app: &mut MailApp, label: &str, checked: bool, id: &str, params: Value) {
    // An invisible mark keeps the unchecked labels aligned with the checked one.
    let mark = egui::RichText::new("✓");
    let mark = if checked { mark } else { mark.color(Color32::TRANSPARENT) };
    let enabled = app.enabled(id);
    let resp = ui.add_enabled(enabled, egui::Button::new((mark, tl!(label))).selected(checked).min_size(vec2(190.0, 0.0)));
    if resp.clicked() {
        let _ = app.run(id, params);
        ui.close();
    }
}

/// A bare icon button (pane toolbars, the title bar). `true` on the frame it is clicked.
pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str, size: f32, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(vec2(size + 8.0, size + 6.0), Sense::click());
    if resp.hovered() && enabled {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = if enabled { t.icon } else { t.text_disabled };
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(size, size)), icon, c, if enabled { t.accent } else { c });
    resp.on_hover_text(tip)
}

/// An icon with a label beside it, as a flat button.
pub fn icon_text_button(ui: &mut Ui, icon: &str, label: &str, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let w = text_w(ui, label, 12.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w + 34.0, 24.0), Sense::click());
    if resp.hovered() && enabled {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = if enabled { t.icon } else { t.text_disabled };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(pos2(r.min.x + 13.0, r.center().y), vec2(16.0, 16.0)),
        icon,
        c,
        if enabled { t.accent } else { c },
    );
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, regular(12.0), if enabled { t.text } else { t.text_disabled });
    resp
}

/// A filled button in the app colour (Send, Save, the default answer of a dialog).
pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let w = text_w(ui, label, 12.5).max(44.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w + 26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let fill = if !enabled {
        t.pressed
    } else if resp.hovered() {
        t.accent_text
    } else {
        t.accent
    };
    ui.painter().rect_filled(r, 4.0, fill);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, label, crate::theme::medium(12.5), if enabled { t.on_accent } else { t.text_disabled });
    // (Named for screen readers, as a button.)
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    resp
}

/// A round avatar with initials.
pub fn avatar(ui: &Ui, center: egui::Pos2, radius: f32, address: &mailcraft_model::Address) {
    let t = Tokens::get(ui.ctx());
    let key = if address.email.is_empty() { &address.name } else { &address.email };
    ui.painter().circle_filled(center, radius, crate::theme::avatar_color(key, &t));
    let initials = address.initials();
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        if initials.is_empty() { "?".to_string() } else { initials },
        semibold(radius * 0.78),
        Color32::WHITE,
    );
}

/// A category as a small coloured chip with its name. Returns the width used.
pub fn category_chip(ui: &Ui, left_center: egui::Pos2, name: &str, color: Color32) -> f32 {
    let t = Tokens::get(ui.ctx());
    let w = text_w(ui, name, 10.5) + 12.0;
    let r = Rect::from_min_size(pos2(left_center.x, left_center.y - 8.0), vec2(w, 16.0));
    ui.painter().rect_filled(r, 8.0, color.linear_multiply(if t.dark { 0.45 } else { 0.22 }));
    ui.painter().rect_stroke(r, 8.0, Stroke::new(1.0, color.linear_multiply(0.8)), egui::StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, name, regular(10.5), t.text);
    w
}

/// A section heading inside a side pane (`My Calendars`, `My Tasks`), in the size and weight of
/// the folder pane's own headings.
pub fn pane_heading(ui: &mut Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 10.0, r.center().y), r.width() - 16.0, text, regular(14.667), t.text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_read_like_the_platform() {
        if cfg!(target_os = "macos") {
            assert_eq!(key_text("Mod+Shift+R"), "⌘⇧R");
        } else {
            assert_eq!(key_text("Mod+Shift+R"), "Ctrl+Shift+R");
            assert_eq!(key_text("ArrowDown"), "Down");
        }
        assert_eq!(key_text(""), "");
    }
}
