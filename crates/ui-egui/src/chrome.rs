// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Title bar (Quick Access Toolbar, search, account) and status bar.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::{Module, Pane, Scope};
use serde_json::json;

use crate::theme::{Tokens, TypeRung, medium, regular, semibold, text_width};
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

/// "Copy Diagnostics", beside the window's own buttons, so a tester finds it (D1): the same command
/// as Help › Copy Diagnostics.
fn diagnostics_pill(ui: &mut Ui, app: &mut MailApp, h: f32) {
    let tip = tl!(
        "Copy the version, the platform and the end of MailCraft's own record, to paste into a problem report. No message, address, password or token is in it, and nothing is sent."
    );
    pill(ui, app, h, "copy", tl!("Copy Diagnostics"), tip, "ui.copyDiagnostics");
}

/// "Report a Problem", beside Copy Diagnostics: the same command as Help › Report a Problem,
/// which opens its window ([`crate::report_problem`]).
fn report_pill(ui: &mut Ui, app: &mut MailApp, h: f32) {
    let tip = tl!("Tell MailCraft's makers what went wrong, on GitHub or by email. You check the report and send it yourself.");
    pill(ui, app, h, "report", tl!("Report a Problem"), tip, "ui.reportProblem");
}

/// A rounded button of the title bar, with its icon and its label, that runs `id` by hand.
fn pill(ui: &mut Ui, app: &mut MailApp, h: f32, icon: &str, label: &str, tip: &str, id: &str) {
    let t = Tokens::get(ui.ctx());
    let font = medium(11.5);
    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), t.text).size().x);
    let (r, resp) = ui.allocate_exact_size(vec2(w + 34.0, h), Sense::click());
    let fill = if resp.is_pointer_button_down_on() {
        t.pressed
    } else if resp.hovered() {
        t.hover
    } else {
        t.title_bar
    };
    ui.painter().rect(r, h / 2.0, fill, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 15.0, r.center().y), vec2(16.0, 16.0)), icon, t.icon, t.accent);
    ui.painter().text(pos2(r.min.x + 26.0, r.center().y), Align2::LEFT_CENTER, label, font, t.text);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run(id, json!({}));
    }
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
    icons::paint(ui.painter(), Rect::from_center_size(pos2(rect.min.x + 15.0, rect.center().y), vec2(16.0, 16.0)), "search", t.text_dim, t.accent);
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
    // The tabs' size in the tall box of the title row; the small one in the macOS title bar.
    let font = if rect.height() >= 30.0 { regular(13.0) } else { TypeRung::Control.regular() };
    // Keywords as pills (#196), drawn under the text in a place kept for them.
    let cursor = if focused { egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range()).map(|r| r.primary.index.0) } else { None };
    let pill_slot = ui.painter().add(egui::Shape::Noop);
    let mut drawn: Vec<crate::search_pills::Pill> = Vec::new();
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, _wrap: f32| {
        drawn = if mail_search { crate::search_pills::pills(text.as_str(), cursor) } else { Vec::new() };
        ui.fonts_mut(|f| f.layout_job(crate::search_pills::layout(text.as_str(), &drawn, font.clone(), &t)))
    };
    let edit = egui::TextEdit::singleline(&mut app.search_text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(hint)
        .font(font.clone())
        .vertical_align(egui::Align::Center)
        .desired_width(edit_rect.width())
        .layouter(&mut layouter);
    let output = ui
        .scope_builder(egui::UiBuilder::new().max_rect(edit_rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)), |ui| {
            edit.show(ui)
        })
        .inner;
    let shapes = crate::search_pills::shapes(&output.galley, output.galley_pos, &drawn, &t);
    ui.painter().with_clip_rect(output.text_clip_rect).set(pill_slot, egui::Shape::Vec(shapes));
    let resp = output.response.response;
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
        for (name, code, on) in [("Current Folder", "folder", !everywhere), ("All Mailboxes", "everywhere", everywhere)] {
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
                    ui.add_space(8.0);
                    diagnostics_pill(ui, app, avatar_h.min(26.0));
                    ui.add_space(4.0);
                    report_pill(ui, app, avatar_h.min(26.0));
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

/// How tall the status bar is.
pub const STATUS_BAR_H: f32 = 30.0;
/// The size of the status bar's text.
pub(crate) const STATUS_TEXT: f32 = 12.0;
/// The bar a Send/Receive under way fills, in the status bar.
const SYNC_BAR: egui::Vec2 = egui::Vec2::new(120.0, 6.0);
/// The most of the bar's width a message at its left takes, so the right side keeps room.
const STATUS_MSG_SHARE: f32 = 0.45;

/// What the status bar says of Send/Receive.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SyncShown {
    /// Nothing.
    Nothing,
    /// A run under way: a steady label and how far it has got (0 to 1), never the folder it is at.
    Bar { label: String, fraction: f32 },
    /// What it did, a problem, or what it waits for (a sign-in in the browser), in words.
    Text(String),
}

/// What the status bar says of Send/Receive now: while a run goes on, a bar under a label that
/// changes only from one account to the next; else the words the run left.
pub(crate) fn sync_shown(app: &MailApp) -> SyncShown {
    let progress = &app.session.sync_progress;
    let waiting_for_owner = app.signing_in() || app.approving();
    if let Some(fraction) = progress.fraction().filter(|_| app.syncing() && !waiting_for_owner) {
        let label = if app.services.sync_cancel.load(std::sync::atomic::Ordering::Relaxed) {
            tl!("Stopping…").to_string()
        } else {
            match progress.current().and_then(|r| app.session.mailbox.account(r.account)) {
                Some(account) => {
                    let name: String = account.name.chars().filter(|c| !c.is_control()).take(32).collect();
                    crate::i18n::fmt(tl!("Updating {account}"), &[("account", name.trim())])
                }
                None => tl!("Send/Receive").to_string(),
            }
        };
        return SyncShown::Bar { label, fraction };
    }
    let sync = &app.session.sync_status;
    if sync.is_empty() { SyncShown::Nothing } else { SyncShown::Text(sync.clone()) }
}

/// The status bar, across the foot of the window.
pub fn status_bar(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status")
        .exact_size(STATUS_BAR_H)
        // No line around it: the bar is the window's own colour, and the panes above it are not.
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.status_bar).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            let full_w = ui.available_width();
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(14.0, 0.0);
                // One line of text, no wider than `most` or the room there is: what doesn't fit
                // ends in an ellipsis, and the whole of it is shown under the pointer.
                let st_most = |ui: &mut Ui, s: &str, color: egui::Color32, most: f32| {
                    let font = regular(STATUS_TEXT);
                    let whole = text_width(ui.painter(), s, &font, 0.0, color);
                    let w = whole.min(most).min(ui.available_width() - 2.0).max(0.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(w + 1.0, STATUS_TEXT * 1.4), Sense::hover());
                    crate::theme::paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), w + 1.0, s, font, color);
                    if w + 0.5 < whole { resp.on_hover_text(s) } else { resp }
                };
                let st = |ui: &mut Ui, s: &str, color: egui::Color32| st_most(ui, s, color, f32::INFINITY);
                match app.session.view.module {
                    Module::Mail => {
                        let shown = app.session.rows().len();
                        let folder = app.session.view.folder;
                        st(ui, &tln!(shown, "{n} item", "{n} items"), t.text_dim);
                        let unread = app.session.mailbox.unread_count(folder);
                        if unread > 0 {
                            st(ui, &crate::i18n::fmt(tl!("Unread: {n}"), &[("n", &unread.to_string())]), t.text_dim);
                        }
                        let picked = app.session.sel.messages.len();
                        if picked > 1 {
                            st(ui, &tln!(picked, "{n} selected", "{n} selected"), t.text_dim);
                        }
                    }
                    Module::Calendar => {
                        let (from, to) = mailcraft_engine::cmd::view::calendar_range(&app.session);
                        let count = mailcraft_engine::cmd::calendar::occurrences(&app.session, from, to).len();
                        st(ui, &tln!(count, "{n} event in view", "{n} events in view"), t.text_dim);
                    }
                    Module::People => {
                        st(ui, &tln!(app.session.mailbox.contacts.len(), "{n} contact", "{n} contacts"), t.text_dim);
                    }
                    Module::Tasks => {
                        let open = app.session.mailbox.tasks.values().filter(|x| !x.is_complete()).count();
                        st(ui, &tln!(open, "{n} open task", "{n} open tasks"), t.text_dim);
                    }
                }
                if let Some((msg, at)) = &app.status_msg
                    && crate::now_ms() - at < 6000.0
                {
                    let shown = st_most(ui, msg, t.accent_text, full_w * STATUS_MSG_SHARE);
                    app.hit.add("statusMessage", 0, shown.rect);
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 0.0);
                    // Reading pane: right, bottom, off.
                    if app.session.view.module == Module::Mail {
                        let pane = app.session.view.reading_pane;
                        for (icon, value, on, tip) in [
                            ("list", "off", pane == Pane::Off, "Reading pane off"),
                            ("previewLines", "bottom", pane == Pane::Bottom, "Reading pane at the bottom"),
                            ("readingPane", "right", pane == Pane::Right, "Reading pane on the right"),
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
                    // A new version downloaded and checked: Restart to update.
                    crate::updates::status_item(app, ui, &t);
                    // The last Send/Receive had problems: one click shows them, in Send/Receive
                    // Progress under their accounts.
                    if app.has_sync_problems() {
                        let link = ui.link(egui::RichText::new(tl!("Send/Receive problems")).size(STATUS_TEXT).color(t.red));
                        app.hit.add("syncProblemsLink", 0, link.rect);
                        if link.clicked() {
                            let _ = app.run("ui.dialog", json!({"name": "syncProblems"}));
                        }
                    }
                    let shown_sync = sync_shown(app);
                    if shown_sync != SyncShown::Nothing {
                        // Waiting for the owner in the browser (or for an administrator on
                        // Microsoft's approval page) can be given up from here.
                        if (app.signing_in() || app.approving()) && ui.link(egui::RichText::new(tl!("Cancel")).size(STATUS_TEXT)).clicked() {
                            let _ = app.run("ui.cancelSignIn", json!({}));
                        }
                        // So can a Send/Receive that is taking long: what it brought stays.
                        if app.syncing()
                            && ui
                                .link(egui::RichText::new(tl!("Cancel")).size(STATUS_TEXT))
                                .on_hover_text(tl!("Stop this Send/Receive; what has arrived stays"))
                                .clicked()
                        {
                            let _ = app.run("ui.cancelSync", json!({}));
                        }
                        ui.add_space(6.0);
                        // What Send/Receive is doing, or did: a click shows it account by account.
                        let shown = match &shown_sync {
                            SyncShown::Bar { label, fraction } => sync_bar(app, ui, &t, label, *fraction),
                            SyncShown::Text(said) => st(ui, said, t.text_dim),
                            SyncShown::Nothing => ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover()).1,
                        };
                        let shown = shown.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                        app.hit.add("syncStatus", 0, shown.rect);
                        if shown.hovered() {
                            ui.painter().hline(shown.rect.x_range(), shown.rect.max.y - 1.0, egui::Stroke::new(1.0, t.text_dim));
                        }
                        // Under the pointer, the run's own words say where it is.
                        let tip = match &shown_sync {
                            SyncShown::Bar { .. } if !app.session.sync_status.is_empty() => {
                                format!("{}\n{}", app.session.sync_status, tl!("Show Send/Receive Progress"))
                            }
                            _ => tl!("Show Send/Receive Progress").to_string(),
                        };
                        if shown.on_hover_text(tip).clicked() {
                            if app.sync_panel.is_some() {
                                app.sync_panel = None;
                            } else {
                                let _ = app.run("ui.dialog", json!({"name": "syncProgress"}));
                            }
                        }
                    } else if app.session.mailbox.accounts.iter().all(|a| a.kind == mailcraft_model::AccountKind::Local) {
                        st(ui, tl!("This mailbox is on this device only"), t.text_dim)
                            .on_hover_text(tl!("Add a mail server account in File › Add Account to send and receive."));
                    }
                    // Messages being fetched whole in the background (R3.2).
                    let fetching = app.fetching.progress.clone();
                    if !fetching.is_empty() {
                        ui.add_space(10.0);
                        st(ui, &fetching, t.text_dim);
                    }
                    // A save that takes a while, in the background (R6.3).
                    if let Some(saving) = app.saving_text() {
                        ui.add_space(10.0);
                        st(ui, &saving, t.text_dim);
                    }
                    // The word index being built, in the background (R6.2).
                    indexing_bar(app, ui, &t);
                });
            });
        });
}

/// A Send/Receive under way, in the status bar: its label, then a bar that fills as the run goes
/// and never goes back while it lasts (an account that joins the run doesn't empty it).
fn sync_bar(app: &mut MailApp, ui: &mut Ui, t: &Tokens, label: &str, fraction: f32) -> egui::Response {
    let (resp, track) = progress_bar(ui, t, label, fraction, "status_sync_bar");
    app.hit.add("syncBar", 0, track);
    resp
}

/// A label and a bar that fills as `fraction` goes from 0 to 1, in the status bar: the bar never
/// goes back while it is shown (`key` remembers how far it got). Returns the whole and the bar.
fn progress_bar(ui: &mut Ui, t: &Tokens, label: &str, fraction: f32, key: &str) -> (egui::Response, Rect) {
    let font = regular(STATUS_TEXT);
    let gap = 8.0;
    let label_w = text_width(ui.painter(), label, &font, 0.0, t.text_dim).min(220.0);
    let room = (ui.available_width() - 2.0).max(0.0);
    let bar_w = SYNC_BAR.x.min(room);
    let label_w = label_w.min((room - bar_w - gap).max(0.0));
    let (r, resp) = ui.allocate_exact_size(vec2(label_w + gap + bar_w, STATUS_TEXT * 1.4), Sense::hover());
    let key = egui::Id::new(key);
    let before: f32 = ui.ctx().data(|d| d.get_temp(key)).unwrap_or(0.0);
    let shown = fraction.clamp(0.0, 1.0).max(before);
    ui.ctx().data_mut(|d| d.insert_temp(key, shown));
    if label_w > 4.0 {
        crate::theme::paint_clipped(ui.painter(), pos2(r.min.x, r.center().y), label_w + 1.0, label, font, t.text_dim);
    }
    let track = Rect::from_min_size(pos2(r.max.x - bar_w, r.center().y - SYNC_BAR.y / 2.0), vec2(bar_w, SYNC_BAR.y));
    let radius = SYNC_BAR.y / 2.0;
    ui.painter().rect_filled(track, radius, if t.dark { t.border } else { t.border_strong.linear_multiply(0.6) });
    if shown > 0.0 {
        let fill = Rect::from_min_max(track.min, pos2(track.min.x + (bar_w * shown).max(SYNC_BAR.y), track.max.y));
        ui.painter().rect_filled(fill, radius, t.accent);
    }
    (resp, track)
}

/// The word index being built (R6.2), in the status bar: "Indexing mail: 45%" and its bar, as
/// Send/Receive shows its progress. Nothing when it isn't being built; the bar's memory of how
/// far it got goes when it is done.
fn indexing_bar(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let key = "status_index_bar";
    let mailcraft_engine::WordIndex::Building { done, total } = app.session.word_index() else {
        ui.ctx().data_mut(|d| d.remove::<f32>(egui::Id::new(key)));
        return;
    };
    if total == 0 {
        // Its file being read, or a moment's look at what came since: nothing to count.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        return;
    }
    let fraction = (done.min(total) as f64 / total as f64) as f32;
    let percent = ((fraction * 100.0).floor() as u32).min(100);
    let label = crate::i18n::fmt(tl!("Indexing mail: {percent}%"), &[("percent", &percent.to_string())]);
    ui.add_space(10.0);
    let (resp, track) = progress_bar(ui, t, &label, fraction, key);
    app.hit.add("indexBar", 0, track);
    resp.on_hover_text(tl!("Building the index search answers from; until it is done, some older mail is searched more slowly"));
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
}

/// Forget how far the bar of the last run got, once no run is under way.
pub(crate) fn sync_bar_rest(app: &MailApp, ctx: &egui::Context) {
    if !matches!(sync_shown(app), SyncShown::Bar { .. }) {
        ctx.data_mut(|d| d.remove::<f32>(egui::Id::new("status_sync_bar")));
    }
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
