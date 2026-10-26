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
    let at = line(ui, label, 0.0);
    let mut buf = text.to_string();
    let resp = ui.put(at, line_edit(&mut buf, ui.scope_id().with(("compose", label)), hint, at.width()));
    if focus {
        resp.request_focus();
    }
    resp.changed().then_some(buf)
}

/// A labelled line, with the rule under it; where its text goes. `right` is room kept free at
/// the line's right end for something drawn there (the Bcc link).
fn line(ui: &mut Ui, label: &str, right: f32) -> Rect {
    let t = Tokens::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter().text(pos2(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, label, regular(12.0), t.text_dim);
    ui.painter().line_segment([pos2(r.min.x + 16.0, r.max.y), pos2(r.max.x - 12.0, r.max.y)], Stroke::new(1.0, t.border));
    Rect::from_min_max(pos2(r.min.x + 16.0 + LABEL_W, r.min.y + 3.0), pos2(r.max.x - 12.0 - right, r.max.y - 3.0))
}

/// The text of a labelled line, `width` wide.
fn line_edit<'t>(text: &'t mut String, id: egui::Id, hint: &str, width: f32) -> egui::TextEdit<'t> {
    egui::TextEdit::singleline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .hint_text(hint)
        .font(regular(13.0))
        .align(egui::Align2::LEFT_CENTER)
        .desired_width(width)
}

/// What takes the place of the name being typed on a line of recipients when the suggestion
/// `who` is taken: the name and the address, then `; ` for the next one; after a space, unless it
/// is the line's `first`.
pub(crate) fn completed(first: bool, who: &mailcraft_engine::cmd::compose::Suggestion) -> String {
    let name: String = who.name.chars().filter(|c| !c.is_control() && !"\"<>".contains(*c)).collect();
    let name = name.trim();
    let written = if name.is_empty() || name.eq_ignore_ascii_case(&who.email) {
        who.email.clone()
    } else if name.contains([',', ';']) {
        format!("\"{name}\" <{}>", who.email)
    } else {
        format!("{name} <{}>", who.email)
    };
    let gap = if first { "" } else { " " };
    format!("{gap}{written}; ")
}

/// A line of recipients (a message's To, Cc and Bcc, a meeting's attendees), `edit`, given the
/// id `id`: as a name is typed in it, egui's list under it suggests who it could be (contacts,
/// then whoever mail was exchanged with). The arrow keys choose; Enter, Tab or a click takes one,
/// and the keyboard stays in the line, after it; Escape closes the list until something else is
/// typed. `offer`: whether anything is suggested now.
pub(crate) fn recipient_line(
    ui: &mut Ui,
    s: &mailcraft_engine::Session,
    id: egui::Id,
    edit: egui::TextEdit<'_>,
    offer: bool,
) -> egui::CompletionOutput {
    let ctx = ui.ctx().clone();
    let shown = id.with("suggestions_shown");
    // The line keeps Tab while the list shows, for the list (below); Shift+Tab still goes back.
    if ctx.data(|d| d.get_temp::<bool>(shown)).unwrap_or(false)
        && ctx.memory(|m| m.has_focus(id))
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab))
    {
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::Previous));
    }
    let filter = edit.get_event_filter();
    let out = egui::CompletionPopup::new(id).word_boundary(|c| c == ';' || c == ',').align(egui::RectAlign::BOTTOM_START).show(ui, edit, |q| {
        let typed = q.word.trim_start();
        if !offer || typed.trim().is_empty() {
            return Vec::new();
        }
        suggestions(&ctx, s, id, typed)
            .into_iter()
            .map(|who| {
                let row = egui::Suggestion::new(completed(q.is_at_start(), &who));
                if who.name.is_empty() { row.content(who.email) } else { row.content(who.name).description(who.email) }
            })
            .collect()
    });
    // The line keeps the keyboard on Escape and Tab while the list is open, for the list, and only
    // then. egui's list asks for it as the list was in the pass before, a pass late both ways: an
    // Escape right after the list came up would go to the window as well, and a Tab right after it
    // closed would be nobody's. (egui changes the filter only of a line that had the keyboard in
    // the pass before.)
    let open = out.is_open;
    ctx.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: filter.tab || open, escape: filter.escape || open, ..filter }));
    ctx.data_mut(|d| d.insert_temp(shown, out.is_open));
    out
}

/// The recipients suggested for `typed` on the line `id`. They are asked for again only when what is
/// typed or the mailbox changes ([`Session::rev`], [`Session::openings`]): a line with the
/// keyboard is drawn at least twice a second (its cursor blinks), and asking reads the whole
/// mailbox.
///
/// [`Session::rev`]: mailcraft_engine::Session::rev
/// [`Session::openings`]: mailcraft_engine::Session::openings
fn suggestions(ctx: &egui::Context, s: &mailcraft_engine::Session, id: egui::Id, typed: &str) -> Vec<mailcraft_engine::cmd::compose::Suggestion> {
    type Asked = ((String, u64, u64), Vec<mailcraft_engine::cmd::compose::Suggestion>);
    let kept = id.with("suggestions");
    let asking = (typed.to_string(), s.rev(), s.openings());
    if let Some((at, found)) = ctx.data(|d| d.get_temp::<Asked>(kept))
        && at == asking
    {
        return found;
    }
    let found = mailcraft_engine::cmd::compose::suggest(s, typed, 6);
    ctx.data_mut(|d| d.insert_temp::<Asked>(kept, (asking, found.clone())));
    found
}

/// Whether the quiet Bcc link is shown (no Bcc line yet).
fn bcc_link(app: &MailApp) -> bool {
    app.session.compose.as_ref().is_some_and(|c| c.bcc.is_empty()) && !app.compose_bcc
}

/// A recipient line, with the recipient suggestions under it ([`recipient_line`]).
fn recipient_field(app: &mut MailApp, ui: &mut Ui, key: &str, label: &str, text: &str, hint: &str, focus: bool) {
    let id = ui.scope_id().with(("compose", label));
    // The Bcc link sits at the right end of the To line: a click there is for it, so the line's
    // text stops short of it (a long To line was otherwise impossible to click at its end).
    let right = if key == "to" && bcc_link(app) { 44.0 } else { 0.0 };
    let at = line(ui, label, right);
    let mut buf = text.to_string();
    let edit = line_edit(&mut buf, id, hint, at.width());
    let layout = egui::Layout::centered_and_justified(egui::Direction::TopDown);
    let out = ui.scope_builder(egui::UiBuilder::new().max_rect(at).layout(layout), |ui| recipient_line(ui, &app.session, id, edit, true)).inner;
    if focus {
        out.text_edit.response.request_focus();
    }
    // (A suggestion taken changes the text after the line has said whether it changed.)
    if out.accepted.is_some() || out.text_edit.response.changed() {
        let _ = app.execute("compose.set", json!({ key: buf }));
    }
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
    // A message that just opened has the keyboard: To for a new one, the body for a reply. Not
    // while a dialog is over it: what is typed would go under the dialog, and the keyboard goes
    // back to what had it when the dialog closes (#406).
    let fresh = !c.dirty && ui.memory(|m| m.focused().is_none() && m.allows_interaction(ui.layer_id()));
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
