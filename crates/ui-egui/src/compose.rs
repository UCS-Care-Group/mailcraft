// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing a message: recipients, subject, attachments and the body, in place of the reading pane.

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_engine::ComposeMode;
use serde_json::json;

use crate::theme::{Tokens, paint_clipped, regular};
use crate::{MailApp, icons};

pub(crate) mod editor;
#[cfg(test)]
mod tests;

const LABEL_W: f32 = 62.0;

/// One labelled single-line field. Returns the new text when it was edited.
fn field(ui: &mut Ui, label: &str, text: &str, hint: &str, focus: bool) -> Option<String> {
    field_at(ui, label, text, hint, focus, 0.0).0
}

/// [`field`], also answering where the text is and how it responded.
/// `right` is room kept free at the line's right end for something drawn there (the Bcc link).
fn field_at(ui: &mut Ui, label: &str, text: &str, hint: &str, focus: bool, right: f32) -> (Option<String>, Rect, egui::Response) {
    let t = Tokens::get(ui.ctx());
    let mut buf = text.to_string();
    let mut changed = None;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, label, regular(12.0), t.text_dim);
    let edit_rect = Rect::from_min_max(pos2(r.min.x + 16.0 + LABEL_W, r.min.y + 3.0), pos2(r.max.x - 12.0 - right, r.max.y - 3.0));
    let edit = egui::TextEdit::singleline(&mut buf)
        .id(ui.scope_id().with(("compose", label)))
        .frame(egui::Frame::NONE)
        .hint_text(hint)
        .font(regular(13.0))
        .align(egui::Align2::LEFT_CENTER)
        .desired_width(edit_rect.width());
    let resp = ui.put(edit_rect, edit);
    if focus {
        resp.request_focus();
    }
    if resp.changed() {
        changed = Some(buf);
    }
    ui.painter().line_segment([pos2(r.min.x + 16.0, r.max.y), pos2(r.max.x - 12.0, r.max.y)], Stroke::new(1.0, t.border));
    (changed, edit_rect, resp)
}

/// The suggestions under a recipient line, from one frame to the next.
#[derive(Clone, Default)]
struct Suggesting {
    /// Which line they are for (`to`, `cc`, `bcc`).
    field: String,
    items: Vec<mailcraft_engine::cmd::compose::Suggestion>,
    selected: usize,
    /// Where they were drawn, so a click on one isn't taken for a click away from the line.
    rect: Option<Rect>,
    /// What `items` were asked for: what was being typed, and the mailbox as it was
    /// ([`Session::rev`], [`Session::openings`]). They are asked for again only when one of these
    /// changes: a line with the keyboard is drawn at least twice a second (its cursor blinks),
    /// and asking reads the whole mailbox.
    ///
    /// [`Session::rev`]: mailcraft_engine::Session::rev
    /// [`Session::openings`]: mailcraft_engine::Session::openings
    asked: Option<(String, u64, u64)>,
}

/// What is being typed at the end of a recipient line: the part after the last `;` or `,`.
pub(crate) fn typing(text: &str) -> (&str, &str) {
    let at = text.rfind([';', ',']).map_or(0, |i| i + 1);
    let (before, last) = text.split_at(at);
    (before, last.trim_start())
}

/// A recipient line with `who` in place of what was being typed, ready for the next name.
pub(crate) fn completed(text: &str, who: &mailcraft_engine::cmd::compose::Suggestion) -> String {
    let (before, _) = typing(text);
    let name: String = who.name.chars().filter(|c| !c.is_control() && !"\"<>".contains(*c)).collect();
    let name = name.trim();
    let written = if name.is_empty() || name.eq_ignore_ascii_case(&who.email) {
        who.email.clone()
    } else if name.contains([',', ';']) {
        format!("\"{name}\" <{}>", who.email)
    } else {
        format!("{name} <{}>", who.email)
    };
    let gap = if before.is_empty() || before.ends_with(' ') { "" } else { " " };
    format!("{before}{gap}{written}; ")
}

/// Whether the quiet Bcc link is shown (no Bcc line yet).
fn bcc_link(app: &MailApp) -> bool {
    app.session.compose.as_ref().is_some_and(|c| c.bcc.is_empty()) && !app.compose_bcc
}

/// A recipient line: as it is typed in, the people it could be are offered underneath (contacts,
/// then whoever mail was exchanged with); arrow keys choose, Enter or Tab or a click takes one.
fn recipient_field(app: &mut MailApp, ui: &mut Ui, key: &str, label: &str, text: &str, hint: &str, focus: bool) {
    let t = Tokens::get(ui.ctx());
    let id = ui.scope_id().with(("compose", label));
    let store = egui::Id::unique("compose_suggesting");
    let mut st: Suggesting = ui.data(|d| d.get_temp(store)).unwrap_or_default();
    let mine = st.field == key && !st.items.is_empty();
    let focused = ui.memory(|m| m.has_focus(id));
    let over = st.rect.is_some_and(|r| ui.rect_contains_pointer(r));
    let open = mine && (focused || over);
    let mut take: Option<usize> = None;
    if open && focused {
        // Before the line itself sees the keys: they are for the list while it is showing.
        let n = st.items.len();
        ui.input_mut(|i| {
            // With Shift, Ctrl or Alt held the keys are not the list's: Shift+Tab goes back, as
            // without the list (`consume_key` takes Shift+Tab for Tab).
            if !i.modifiers.is_none() {
                return;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                st.selected = (st.selected + 1) % n;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                st.selected = (st.selected + n - 1) % n;
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) || i.consume_key(egui::Modifiers::NONE, egui::Key::Tab) {
                take = Some(st.selected.min(n - 1));
            }
        });
    }
    // The Bcc link sits at the right end of the To line: a click there is for it, so the line's
    // text stops short of it (a long To line was otherwise impossible to click at its end).
    let right = if key == "to" && bcc_link(app) { 44.0 } else { 0.0 };
    let (changed, at, resp) = field_at(ui, label, text, hint, focus, right);
    let mut now = changed.clone().unwrap_or_else(|| text.to_string());
    if open {
        let shown = egui::Area::new(store.with(key)).order(egui::Order::Foreground).fixed_pos(pos2(at.min.x, at.max.y + 2.0)).show(ui.ctx(), |ui| {
            egui::Frame::NONE.fill(t.menu).stroke(Stroke::new(1.0, t.border_strong)).corner_radius(6.0).inner_margin(egui::Margin::same(4)).show(
                ui,
                |ui| {
                    ui.set_width(at.width().clamp(240.0, 440.0));
                    for (i, who) in st.items.iter().enumerate() {
                        let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                        if i == st.selected || row.hovered() {
                            ui.painter().rect_filled(r, 4.0, if i == st.selected { t.selection } else { t.hover });
                        }
                        let name = if who.name.is_empty() { who.email.as_str() } else { who.name.as_str() };
                        let w = paint_clipped(ui.painter(), pos2(r.min.x + 8.0, r.center().y), r.width() * 0.5, name, regular(12.5), t.text);
                        if !who.name.is_empty() {
                            paint_clipped(
                                ui.painter(),
                                pos2(r.min.x + 16.0 + w, r.center().y),
                                (r.width() - w - 24.0).max(0.0),
                                &who.email,
                                regular(11.5),
                                t.text_dim,
                            );
                        }
                        if row.clicked() {
                            take = Some(i);
                        }
                    }
                },
            );
        });
        st.rect = Some(shown.response.rect);
    } else {
        st.rect = None;
    }
    if let Some(who) = take.and_then(|i| st.items.get(i)) {
        now = completed(&now, who);
        let _ = app.execute("compose.set", json!({ key: now }));
        // Carry on typing after it: the keyboard stays in the line, at its end.
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let end = egui::text::CCursor::new(now.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ui.ctx(), id);
        ui.memory_mut(|m| m.request_focus(id));
        st.items.clear();
    } else if let Some(new) = changed {
        let _ = app.execute("compose.set", json!({ key: new }));
    }
    // What to offer next frame, for what is being typed now.
    if resp.has_focus() || ui.memory(|m| m.has_focus(id)) {
        let (_, last) = typing(&now);
        let asking = (last.to_string(), app.session.rev(), app.session.openings());
        if st.field != key || st.asked.as_ref() != Some(&asking) {
            let items = if last.trim().is_empty() { Vec::new() } else { mailcraft_engine::cmd::compose::suggest(&app.session, last, 6) };
            st.selected = if st.field == key { st.selected.min(items.len().saturating_sub(1)) } else { 0 };
            st.field = key.to_string();
            st.items = items;
            st.asked = Some(asking);
        }
    } else if st.field == key && !over {
        st.items.clear();
        st.asked = None;
    }
    ui.data_mut(|d| d.insert_temp(store, st));
}

/// What the window shows of the message being written besides its body (copied out of the
/// session once a frame, so the commands it runs can change the message meanwhile).
struct Shown {
    mode: ComposeMode,
    dirty: bool,
    draft: bool,
    account: u64,
    to: String,
    cc: String,
    bcc: String,
    subject: String,
    /// Each attachment's name, as it is written to a file (a forward carries the sender's), and size.
    attachments: Vec<(String, String)>,
    quote: bool,
}

impl Shown {
    fn of(app: &MailApp) -> Option<Shown> {
        let c = app.session.compose.as_ref()?;
        Some(Shown {
            mode: c.mode,
            dirty: c.dirty,
            draft: c.draft.is_some(),
            account: c.account,
            to: c.to.clone(),
            cc: c.cc.clone(),
            bcc: c.bcc.clone(),
            subject: c.subject.clone(),
            attachments: c
                .attachments
                .iter()
                .map(|a| (mailcraft_engine::opening::file_name(&a.name), mailcraft_model::size_label_in(crate::i18n::current(), a.size() as u64)))
                .collect(),
            quote: c.quote.is_some(),
        })
    }
}

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let Some(c) = Shown::of(app) else { return };
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    // What this is, and the actions.
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let title = match c.mode {
        ComposeMode::New => tl!("New message"),
        ComposeMode::Reply => tl!("Reply"),
        ComposeMode::ReplyAll => tl!("Reply all"),
        ComposeMode::Forward => tl!("Forward"),
        ComposeMode::Draft => tl!("Draft"),
    };
    let mut actions =
        ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(12.0, 8.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    {
        let ui = &mut actions;
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);
        let sc = crate::widgets::shortcut_text(app, "compose.send");
        if crate::widgets::primary_button(ui, tl!("Send"), true).on_hover_text(format!("{} ({sc})", tl!("Send"))).clicked() {
            let _ = app.run("compose.send", json!({}));
        }
        if crate::widgets::icon_text_button(ui, "attach", tl!("Attach"), true).clicked() {
            let _ = app.run("ui.attach", json!({}));
        }
        if crate::widgets::icon_text_button(ui, "saveDraft", tl!("Save draft"), c.dirty || !c.draft).clicked() {
            let _ = app.run("compose.saveDraft", json!({}));
        }
        if crate::widgets::icon_text_button(ui, "discard", tl!("Discard"), true).clicked() {
            let _ = app.run("compose.discard", json!({}));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if crate::widgets::icon_button(ui, "close", tl!("Close"), 14.0, true).clicked() {
                let _ = app.run("compose.close", json!({}));
            }
            let state = if c.dirty {
                tl!("Not saved")
            } else if c.draft {
                tl!("Saved to Drafts")
            } else {
                ""
            };
            ui.label(egui::RichText::new(format!("{title}  {state}")).font(regular(11.5)).color(t.text_dim));
        });
    }
    ui.painter().line_segment([pos2(bar.min.x, bar.max.y), pos2(bar.max.x, bar.max.y)], Stroke::new(1.0, t.border));
    // A message that just opened has the keyboard: To for a new one, the body for a reply.
    let fresh = !c.dirty && ui.memory(|m| m.focused().is_none());
    let focus_to = fresh && c.to.is_empty();

    // From, when there is a choice.
    if app.session.mailbox.accounts.len() > 1 {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, tl!("From"), regular(12.0), t.text_dim);
        let current = app.session.mailbox.account(c.account).map(|a| a.address().full()).unwrap_or_default();
        let pick = Rect::from_min_max(pos2(r.min.x + 16.0 + LABEL_W, r.min.y + 4.0), pos2(r.max.x - 12.0, r.max.y - 4.0));
        let resp = ui.interact(pick, ui.scope_id().with("compose_from"), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(pick.expand2(vec2(4.0, 0.0)), 4.0, t.hover);
        }
        paint_clipped(ui.painter(), pos2(pick.min.x, pick.center().y), pick.width(), &format!("{current} ▾"), regular(13.0), t.text);
        let accounts: Vec<(u64, String)> = app.session.mailbox.accounts.iter().map(|a| (a.id, a.address().full())).collect();
        egui::Popup::menu(&resp).show(|ui| {
            for (id, label) in accounts {
                if ui.selectable_label(id == c.account, label).clicked() {
                    let _ = app.execute("compose.set", json!({"account": id}));
                    ui.close();
                }
            }
        });
        ui.painter().line_segment([pos2(r.min.x + 16.0, r.max.y), pos2(r.max.x - 12.0, r.max.y)], Stroke::new(1.0, t.border));
    }
    let hint = tl!("Names or addresses, separated by ;");
    for (key, label, text, show) in
        [("to", "To", &c.to, true), ("cc", "Cc", &c.cc, true), ("bcc", "Bcc", &c.bcc, !c.bcc.is_empty() || app.compose_bcc)]
    {
        if !show {
            continue;
        }
        recipient_field(app, ui, key, tl!(label), text, hint, key == "to" && focus_to);
    }
    if let Some(new) = field(ui, tl!("Subject"), &c.subject, "", false) {
        let _ = app.execute("compose.set", json!({"subject": new}));
    }
    if bcc_link(app) {
        // A quiet way to get the Bcc line.
        let last = ui.min_rect();
        let br = Rect::from_min_size(
            pos2(last.max.x - 52.0, bar.max.y + if app.session.mailbox.accounts.len() > 1 { 34.0 } else { 4.0 }),
            vec2(40.0, 22.0),
        );
        let resp = ui.interact(br, ui.scope_id().with("show_bcc"), Sense::click());
        ui.painter().text(br.center(), Align2::CENTER_CENTER, tl!("Bcc"), regular(11.5), if resp.hovered() { t.accent_text } else { t.text_dim });
        if resp.clicked() {
            app.compose_bcc = true;
        }
    }

    // Attachments, each with a way to take it off again.
    if !c.attachments.is_empty() {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 12, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (i, (name, size)) in c.attachments.iter().enumerate() {
                    let label = format!("{name}  ({size})");
                    let w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(label.clone(), regular(12.0), t.text).size().x).min(260.0);
                    let (r, _) = ui.allocate_exact_size(vec2(w + 52.0, 26.0), Sense::hover());
                    ui.painter().rect(r, 5.0, t.panel, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    icons::paint(
                        ui.painter(),
                        Rect::from_center_size(pos2(r.min.x + 13.0, r.center().y), vec2(16.0, 16.0)),
                        "attach",
                        t.icon,
                        t.accent,
                    );
                    paint_clipped(ui.painter(), pos2(r.min.x + 26.0, r.center().y), w, &label, regular(12.0), t.text);
                    let x = Rect::from_center_size(pos2(r.max.x - 12.0, r.center().y), vec2(18.0, 18.0));
                    let xr = ui.interact(x, ui.scope_id().with(("unattach", i)), Sense::click());
                    if xr.hovered() {
                        ui.painter().rect_filled(x, 3.0, t.hover);
                    }
                    icons::paint(ui.painter(), x.shrink(4.0), "close", t.text_dim, t.accent);
                    if xr.on_hover_text(tl!("Remove")).clicked() {
                        let _ = app.execute("compose.removeAttachment", json!({"index": i}));
                    }
                }
            });
        });
        let r = ui.available_rect_before_wrap();
        ui.painter().line_segment([pos2(r.min.x + 16.0, r.min.y), pos2(r.max.x - 12.0, r.min.y)], Stroke::new(1.0, t.border));
    }

    // The body takes what is left: the owner's text, and under it the original of a reply or
    // forward in HTML, as the reading pane shows it.
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("compose_body").show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 16, top: 10, bottom: 16 }).show(ui, |ui| {
            let room = if c.quote { 8.0 * 19.0 } else { (ui.available_height() - 26.0).max(8.0 * 19.0) };
            editor::show(app, ui, room, fresh && !focus_to);
            if c.quote {
                ui.add_space(8.0);
                // The original senses the pointer over all of the view it is drawn in: kept to
                // below where it begins, so that it doesn't lie over the owner's text and take a
                // click meant for it (the body then never got the keyboard back, #158).
                let below = Rect::everything_below(ui.cursor().min.y);
                ui.scope(|ui| {
                    ui.set_clip_rect(ui.clip_rect().intersect(below));
                    crate::reading::show_quote(app, ui);
                });
            }
        });
    });
}

#[cfg(test)]
#[path = "recipient_suggest_tests.rs"]
mod recipient_suggest_tests;
