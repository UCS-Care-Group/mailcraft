//! Title bar (Quick Access Toolbar, search, account) and status bar.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::{Module, Pane, Scope};
use serde_json::json;

use crate::theme::{Tokens, TypeRung, paint_text, regular, semibold, text_width};
use crate::{MailApp, icons};

fn qat_button(ui: &mut Ui, app: &mut MailApp, icon: &str, tip: &str, id: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let enabled = app.enabled(id);
    let (r, resp) = ui.allocate_exact_size(vec2(QAT_BUTTON, QAT_BUTTON.min(ui.available_height())), Sense::click());
    if resp.hovered() && enabled {
        ui.painter().rect_filled(r, 4.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let c = if enabled { t.icon } else { t.text_disabled };
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), icon, c, if enabled { t.accent } else { c });
    let sc = crate::widgets::shortcut_text(app, if id == "ui.sync" { "sync.request" } else { id });
    let resp = resp.on_hover_text(if sc.is_empty() { tip.to_string() } else { format!("{tip} ({sc})") });
    if resp.clicked() && enabled {
        let _ = app.run(id, json!({}));
    }
    resp
}

/// The title row: its height, the height of the search box in it, and the side of a quick button
/// (the buttons stand 30 apart, their icons 16 across).
pub const TITLE_BAR: f32 = 48.0;
const SEARCH_BOX: f32 = 34.0;
const QAT_BUTTON: f32 = 28.0;

/// Height of the macOS integrated title bar: AppKit draws the traffic lights itself, centred in
/// the standard 28 pt band at the top of the window, so the title bar is that band.
pub const MAC_TITLE_BAR: f32 = 28.0;
const MAC_LIGHT: f32 = 12.0;
const MAC_LIGHT_PITCH: f32 = 20.0;
const MAC_LIGHTS_LEFT: f32 = 10.0;
/// Where title-bar content starts to the right of the traffic lights.
pub const MAC_CONTENT_LEFT: f32 = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT + (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;

/// The search box: typing filters the message list as you go; Escape clears it.
fn search_box(app: &mut MailApp, ui: &mut Ui, rect: Rect) {
    let t = Tokens::get(ui.ctx());
    let id = ui.id().with("search_box");
    let focused = ui.memory(|m| m.has_focus(id));
    // A box in the ribbon's colour on the window's, with an edge that shows on both: a bright one
    // on the near-black of the dark window, a quiet one on the light.
    let edge = if t.dark { t.group_label } else { t.border_strong };
    ui.painter().rect(
        rect,
        4.0,
        if focused { t.input } else { t.ribbon },
        Stroke::new(1.0, if focused { t.accent } else { edge }),
        egui::StrokeKind::Inside,
    );
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 15.0, rect.center().y), vec2(15.0, 15.0)), "search", t.text_dim, t.accent);
    // The engine's search is the truth; the box follows it when it changes elsewhere (a command).
    if !focused && app.search_text != app.session.view.search {
        app.search_text = app.session.view.search.clone();
    }
    // The scope menu is about mail folders, so People (which searches contacts) goes without it.
    let mail_search = app.session.view.module != Module::People;
    let scope_w = if mail_search { 104.0 } else { 0.0 };
    let edit_rect = Rect::from_min_max(pos2(rect.min.x + 28.0, rect.min.y + 1.0), pos2(rect.max.x - scope_w - 4.0, rect.max.y - 1.0));
    let hint = match app.session.view.module {
        Module::People => tl!("Search people"),
        // Calendar and Tasks have no search of their own yet: what is typed looks in the mail.
        Module::Calendar | Module::Tasks => tl!("Search mail"),
        Module::Mail => tl!("Search mail (try from:, has:attachment, is:unread)"),
    };
    let edit = egui::TextEdit::singleline(&mut app.search_text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(hint)
        // The tabs' size in the tall box of the title row; the small one in the macOS title bar.
        .font(if rect.height() >= 30.0 { regular(13.0) } else { TypeRung::Control.regular() })
        .vertical_align(egui::Align::Center)
        .desired_width(edit_rect.width());
    let resp = ui.put(edit_rect, edit);
    if std::mem::take(&mut app.search_focus) {
        resp.request_focus();
    }
    if resp.changed() {
        let text = app.search_text.clone();
        let _ = match app.session.view.module {
            Module::People => app.execute("people.search", json!({"query": text})),
            _ => app.execute("view.search", json!({"query": text})),
        };
    }
    if focused && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.search_text.clear();
        let _ = app.execute("view.clearSearch", json!({}));
        let _ = app.execute("people.search", json!({"query": ""}));
        resp.surrender_focus();
    }
    if !mail_search {
        return;
    }
    // Scope: this folder, or every mailbox.
    let scope_rect = Rect::from_min_max(pos2(rect.max.x - scope_w, rect.min.y + 2.0), pos2(rect.max.x - 2.0, rect.max.y - 2.0));
    let everywhere = app.session.view.scope == Scope::Everywhere;
    let label = if everywhere { tl!("All Mailboxes") } else { tl!("Current Folder") };
    let sresp = ui.interact(scope_rect, ui.id().with("search_scope"), Sense::click());
    if sresp.hovered() {
        ui.painter().rect_filled(scope_rect, 4.0, t.hover);
    }
    ui.painter().text(scope_rect.center(), Align2::CENTER_CENTER, format!("{label} ▾"), TypeRung::Caption.regular(), t.text_dim);
    egui::Popup::menu(&sresp).show(|ui| {
        for (name, code, on) in [(tn!("Current Folder"), "folder", !everywhere), (tn!("All Mailboxes"), "everywhere", everywhere)] {
            if ui.selectable_label(on, tl!(name)).clicked() {
                let query = app.session.view.search.clone();
                let _ = app.execute("view.search", json!({"query": query, "scope": code}));
                ui.close();
            }
        }
    });
}

pub fn title_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let mac = app.integrated_titlebar;
    // The first button stands over the module bar's icons.
    let left = if mac { MAC_CONTENT_LEFT as i8 } else { 10 };
    let (search_h, avatar_h) = if mac { (22.0, 22.0) } else { (SEARCH_BOX, 30.0) };
    egui::Panel::top("title_bar")
        .exact_size(if mac { MAC_TITLE_BAR } else { TITLE_BAR })
        // One colour with the tabs under it, and no line between the two.
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.title_bar).inner_margin(egui::Margin { left, right: 10, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            let full = ui.max_rect();
            // Drag the window by the title bar (integrated title bar on macOS).
            let drag = ui.interact(full, ui.id().with("title_drag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            let mut qat_end = full.min.x;
            let mut right_start = full.max.x;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                // Quick Access Toolbar: Send/Receive, Undo, Redo.
                let sync_tip = if app.syncing() { tl!("Sending and receiving…") } else { tl!("Send/Receive All Folders") };
                qat_button(ui, app, "sync", sync_tip, "ui.sync");
                let undo = crate::i18n::fmt(tl!("Undo {what}"), &[("what", tl!(app.session.undo_label().unwrap_or("")))]);
                qat_button(ui, app, "undo", undo.trim(), "edit.undo");
                let redo = crate::i18n::fmt(tl!("Redo {what}"), &[("what", tl!(app.session.redo_label().unwrap_or("")))]);
                qat_button(ui, app, "redo", redo.trim(), "edit.redo");
                qat_end = ui.min_rect().max.x;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // The account in use.
                    let me = app
                        .session
                        .mailbox
                        .folder(app.session.view.folder)
                        .and_then(|f| app.session.mailbox.account(f.account))
                        .or(app.session.mailbox.default_account());
                    let address = me.map(|a| a.address()).unwrap_or_default();
                    let (r, resp) = ui.allocate_exact_size(vec2(avatar_h, avatar_h), Sense::click());
                    crate::widgets::avatar(ui, r.center(), avatar_h / 2.0 - 1.0, &address);
                    let tip = if address.email.is_empty() { tl!("No account yet: File › Add Account").to_string() } else { address.full() };
                    if resp.on_hover_text(tip).clicked() {
                        let _ = app.run("ui.backstage", json!({"value": true, "page": "info"}));
                    }
                    right_start = ui.min_rect().min.x;
                });
            });
            // The search box, centred, as wide as the space allows.
            let room = (right_start - qat_end - 40.0).max(0.0);
            let w = room.min(520.0);
            if w >= 220.0 {
                let cx = full.center().x.clamp(qat_end + 20.0 + w / 2.0, (right_start - 20.0 - w / 2.0).max(qat_end + 20.0 + w / 2.0));
                search_box(app, ui, Rect::from_center_size(pos2(cx, full.center().y), vec2(w, search_h)));
            }
        });
}

/// The status bar, across the foot of the window.
pub fn status_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status")
        .exact_size(26.0)
        // No line around it: the bar is the window's own colour, and the panes above it are not.
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.status_bar).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(14.0, 0.0);
                let st = |ui: &mut Ui, s: &str, color: egui::Color32| {
                    let font = TypeRung::Caption.regular();
                    let track = TypeRung::Caption.tracking();
                    let w = text_width(ui.painter(), s, &font, track, color);
                    let (r, resp) = ui.allocate_exact_size(vec2(w + 1.0, TypeRung::Caption.line_height()), Sense::hover());
                    paint_text(ui.painter(), pos2(r.min.x, r.min.y), Align2::LEFT_TOP, s, &font, track, color);
                    resp
                };
                let n = |count: usize, one: &str, many: &str| {
                    crate::i18n::fmt(if count == 1 { tl!(one) } else { tl!(many) }, &[("n", &count.to_string())])
                };
                match app.session.view.module {
                    Module::Mail => {
                        let shown = app.session.rows().len();
                        let folder = app.session.view.folder;
                        st(ui, &n(shown, tn!("{n} item"), tn!("{n} items")), t.text_dim);
                        let unread = app.session.mailbox.unread_count(folder);
                        if unread > 0 {
                            st(ui, &crate::i18n::fmt(tl!("Unread: {n}"), &[("n", &unread.to_string())]), t.text_dim);
                        }
                        let picked = app.session.sel.messages.len();
                        if picked > 1 {
                            st(ui, &n(picked, tn!("{n} selected"), tn!("{n} selected")), t.text_dim);
                        }
                    }
                    Module::Calendar => {
                        let (from, to) = mailcraft_engine::cmd::view::calendar_range(&app.session);
                        let count = mailcraft_engine::cmd::calendar::occurrences(&app.session, from, to).len();
                        st(ui, &n(count, tn!("{n} event in view"), tn!("{n} events in view")), t.text_dim);
                    }
                    Module::People => {
                        st(ui, &n(app.session.mailbox.contacts.len(), tn!("{n} contact"), tn!("{n} contacts")), t.text_dim);
                    }
                    Module::Tasks => {
                        let open = app.session.mailbox.tasks.values().filter(|x| !x.is_complete()).count();
                        st(ui, &n(open, tn!("{n} open task"), tn!("{n} open tasks")), t.text_dim);
                    }
                }
                if let Some((msg, at)) = &app.status_msg
                    && crate::now_ms() - at < 6000.0
                {
                    st(ui, msg, t.accent_text);
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    // Reading pane: right, bottom, off.
                    if app.session.view.module == Module::Mail {
                        let pane = app.session.view.reading_pane;
                        for (icon, value, on, tip) in [
                            ("list", "off", pane == Pane::Off, tn!("Reading pane off")),
                            ("previewLines", "bottom", pane == Pane::Bottom, tn!("Reading pane at the bottom")),
                            ("readingPane", "right", pane == Pane::Right, tn!("Reading pane on the right")),
                        ] {
                            let (r, resp) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::click());
                            if on {
                                ui.painter().rect_filled(r, 3.0, t.checked);
                            } else if resp.hovered() {
                                ui.painter().rect_filled(r, 3.0, t.hover);
                            }
                            icons::paint(ui.painter(), r.shrink(3.0), icon, t.icon, t.accent);
                            if resp.on_hover_text(tl!(tip)).clicked() {
                                let _ = app.run("view.readingPane", json!({"value": value}));
                            }
                        }
                        ui.add_space(10.0);
                    }
                    let sync = app.session.sync_status.clone();
                    // The last Send/Receive had problems: one click shows all of what it said.
                    if app.has_sync_problems() && ui.link(egui::RichText::new(tl!("Send/Receive problems")).color(t.red)).clicked() {
                        let _ = app.run("ui.dialog", json!({"name": "syncProblems"}));
                    }
                    if !sync.is_empty() {
                        // Waiting for the owner in the browser (or for an administrator on
                        // Microsoft's approval page) can be given up from here.
                        if (app.signing_in() || app.approving()) && ui.link(tl!("Cancel")).clicked() {
                            let _ = app.run("ui.cancelSignIn", json!({}));
                        }
                        // So can a Send/Receive that is taking long: what it brought stays.
                        if app.syncing() && ui.link(tl!("Cancel")).on_hover_text(tl!("Stop this Send/Receive; what has arrived stays")).clicked() {
                            let _ = app.run("ui.cancelSync", json!({}));
                        }
                        st(ui, &sync, t.text_dim);
                    } else if app.session.mailbox.accounts.iter().all(|a| a.kind == mailcraft_model::AccountKind::Local) {
                        st(ui, tl!("This mailbox is on this device only"), t.text_dim)
                            .on_hover_text(tl!("Add a mail server account in File › Add Account to send and receive."));
                    }
                });
            });
        });
}

/// A heading with a count beside it, for the tops of panes.
pub fn pane_title(ui: &mut Ui, title: &str, detail: &str) {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
    let w = crate::theme::paint_clipped(ui.painter(), pos2(r.min.x + 12.0, r.center().y), r.width() - 24.0, title, semibold(15.0), t.text);
    if !detail.is_empty() {
        crate::theme::paint_clipped(
            ui.painter(),
            pos2(r.min.x + 20.0 + w, r.center().y + 1.0),
            (r.width() - 32.0 - w).max(0.0),
            detail,
            regular(11.5),
            t.text_dim,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_title_bar_centres_and_clears_the_traffic_lights() {
        // The lights are centred in the bar, and content keeps the same gap after them.
        let margin = (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
        let lights_end = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT;
        assert_eq!(MAC_CONTENT_LEFT - lights_end, margin);
        // `egui::Margin` is in `i8`.
        assert!(MAC_CONTENT_LEFT <= f32::from(i8::MAX));
    }
}
