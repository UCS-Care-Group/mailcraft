//! Reading a message: its header, attachments and body, in the reading pane or in a window of
//! its own ([`show_message`] is both).
//!
//! HTML mail is shown through `mailcraft-html`'s display blocks, never executed: scripts and
//! style sheets don't exist here. What a message may decide is on a short whitelist (colours,
//! emphasis, alignment, sizes, tables; see `mailcraft-html`), and its colours are checked against
//! the window's before they are used ([`colors`]). Pictures attached to the message are decoded
//! and shown where they belong ([`images`]).
//!
//! Pictures a message refers to on the web are placeholders, so that opening a message tells its
//! sender nothing, unless the owner asks for them: for the message, for its sender or its
//! sender's domain, or for all mail ([`remote`]). Then the host downloads them (or has them
//! from the last time); nothing in this crate goes anywhere, and nothing in it keeps a file.
//!
//! The body is laid out once per message, place, width and theme ([`layout`]) and painted from
//! that every frame ([`view`]).
//!
//! The text of a message can be selected and copied, in its body and in its header ([`select`]).
//!
//! A message that carries a meeting (an invitation, an update, a cancellation, somebody's
//! answer) says so in a bar between its header and its body, with the ways to answer it
//! ([`invitation`]).

pub(crate) mod colors;
mod hold;
pub(crate) mod images;
mod invitation;
mod layout;
pub(crate) mod remote;
mod security;
mod select;
#[cfg(test)]
mod tests;
mod view;

use std::sync::Arc;

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_html::Block;
use mailcraft_model::{FlagStatus, Importance, Message, format_list};
use serde_json::json;

pub(crate) use invitation::{list_marks, options as invitation_options, question as invitation_question};
pub use remote::{Access, RemotePictures};
pub use select::Selections;
pub(crate) use select::{inspect as selection_inspect, select_all_params};

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// The widest a message's body is laid out, in points: lines longer than this are hard to read.
pub const MAX_BODY_WIDTH: f32 = 860.0;

/// Where a message is shown. One that is on screen in two places is laid out for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// The reading pane of the main window.
    Pane,
    /// A window of its own, by the window's number.
    Window(u64),
}

/// The reading pane: the selected message, or a word on why there is none to show.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let picked = app.session.sel.messages.len();
    // What showing it needs (its body, the pictures in it) is brought into memory first, when
    // the mailbox keeps it on disk; the message then shows whole in this very frame.
    if let Some(id) = app.session.sel.focus()
        && app.session.needs_load_for_display(id)
    {
        let _ = app.session.load_for_display(id);
    }
    let Some(m) = app.session.sel.focus().and_then(|id| app.session.mailbox.messages.get(&id).cloned()) else {
        let r = ui.available_rect_before_wrap();
        icons::paint(
            ui.painter(),
            Rect::from_center_size(pos2(r.center().x, r.min.y + r.height() * 0.38), vec2(56.0, 56.0)),
            "mail",
            t.border_strong,
            t.border_strong,
        );
        ui.painter().text(
            pos2(r.center().x, r.min.y + r.height() * 0.38 + 50.0),
            Align2::CENTER_CENTER,
            tl!("Select an item to read"),
            regular(13.0),
            t.text_dim,
        );
        return;
    };
    if picked > 1 {
        let r = ui.available_rect_before_wrap();
        let text = crate::i18n::fmt(tl!("{n} items selected"), &[("n", &picked.to_string())]);
        ui.painter().text(pos2(r.center().x, r.min.y + r.height() * 0.4), Align2::CENTER_CENTER, text, semibold(15.0), t.text_dim);
        return;
    }
    show_message(app, ui, &m, Place::Pane);
}

/// One message in the room `ui` has: its header, its attachments, and its body, which scrolls.
/// The reading pane shows the selected message with it, and a message window its own.
pub fn show_message(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, place: Place) {
    let ctx = ui.ctx().clone();
    // A window shows its own copy of the message: what the mailbox keeps on disk is brought in
    // here, and the window takes the whole message up at its next frame.
    if app.session.needs_load_for_display(m.id) {
        let _ = app.session.load_for_display(m.id);
    }
    // A signed or encrypted message: what its signature comes to, and what it says once opened
    // (kept in memory only).
    let protection = app.session.security_view(m.id);
    let opened;
    let m = match protection.as_ref().filter(|v| v.opened.is_some()) {
        Some(view) => {
            opened = Arc::new(view.apply(m));
            &opened
        }
        None => m,
    };
    remote::tick(app, &ctx);
    select::begin(app, ui, m, place);
    let doc = view::doc_where(&ctx, m, place, remote::available(app));
    let access = if doc.remote.is_empty() { Access::Blocked } else { remote::access(app, m) };
    // Asked for: what has not come yet is sent for, while the message is on screen.
    let progress = if access == Access::Allowed { remote::want(app, &ctx, m, &doc) } else { remote::Progress::default() };
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    header(app, ui, m, place, |app, ui, t| remote::line(app, ui, m, &doc, access, progress, place, t));
    if let Some(view) = &protection {
        security::bar(app, ui, m, view);
    }
    // A message the server wouldn't send as the account: why, and the way to send it again.
    hold::bar(app, ui, m);
    // A meeting that came with the message: what it is, and the ways to answer it.
    invitation::bar(app, ui, m, place);
    // The body gets the part of the pane that can be seen, whatever was laid out above it: a
    // header too wide for a narrow pane must not make the text run out of sight with it.
    let room = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    ui.scope_builder(egui::UiBuilder::new().max_rect(room), |ui| {
        // Sideways too: a data table is never squeezed below its words, so a wide one scrolls.
        egui::ScrollArea::both().auto_shrink([false, false]).id_salt(("body", m.id)).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 12, bottom: 24 }).show(ui, |ui| {
                view::show(app, ui, m, place, access);
            });
        });
    });
}

/// Pictures the host has brought are taken in, whatever is on screen ([`remote::tick`]).
pub(crate) fn tick(app: &mut MailApp, ctx: &egui::Context) {
    remote::tick(app, ctx);
}

/// The message's display blocks, from its HTML when it has some, else from its text.
pub fn blocks_of(m: &Message) -> Vec<Block> {
    view::Doc::of(m).blocks
}

/// The actions on a message that is being read, as the pane's header and a message window's
/// toolbar offer them: icon, label, command. The window adds its own to these.
pub(crate) const ACTIONS: [(&str, &str, &str); 4] = [
    ("reply", tn!("Reply"), "mail.reply"),
    ("replyAll", tn!("Reply All"), "mail.replyAll"),
    ("forward", tn!("Forward"), "mail.forward"),
    ("delete", tn!("Delete"), "mail.delete"),
];

fn header(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, place: Place, pictures: impl FnOnce(&mut MailApp, &mut Ui, &Tokens)) {
    let t = Tokens::get(ui.ctx());
    // The texts that can be selected are laid out here and painted once the pointer has had its
    // say ([`select::header`]).
    let mut fields = select::Fields::default();
    let around = egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 12, top: 12, bottom: 10 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        // Subject, with the actions on the right (a window has them in its toolbar).
        ui.horizontal(|ui| {
            let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { m.subject.as_str() };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
                if place == Place::Pane {
                    for (icon, tip, id) in ACTIONS.iter().rev() {
                        let sc = crate::widgets::shortcut_text(app, id);
                        if crate::widgets::icon_button(ui, icon, &format!("{} ({sc})", tl!(tip)), 17.0, true).clicked() {
                            let _ = app.run(id, json!({}));
                        }
                    }
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    fields.add(ui, select::Field::Subject, egui::Label::new(egui::RichText::new(subject).font(semibold(17.0)).color(t.text)).wrap());
                });
            });
        });
        ui.add_space(4.0);
        // Who and when.
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            crate::widgets::avatar(ui, r.center(), 17.0, &m.from);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                let when = crate::i18n::header_label(m.date, app.session.offset_at(m.date), app.session.prefs.twelve_hour);
                // The date first, at the right; the name and address get what is left, cut short
                // when the pane is narrow rather than run under the date.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        fields.add(ui, select::Field::Date, egui::Label::new(egui::RichText::new(when).font(regular(11.5)).color(t.text_dim)));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let name = egui::RichText::new(m.from.display()).font(semibold(13.0)).color(t.text);
                            fields.add(ui, select::Field::Sender, egui::Label::new(name).truncate());
                            if !m.from.name.is_empty() {
                                let address = egui::RichText::new(format!("<{}>", m.from.email)).font(regular(11.5)).color(t.text_dim);
                                fields.add(ui, select::Field::Address, egui::Label::new(address).truncate());
                            }
                        });
                    });
                });
                let mut line = |ui: &mut Ui, field: select::Field, label: &str, list: &[mailcraft_model::Address]| {
                    if !list.is_empty() {
                        let text = egui::RichText::new(format!("{} {}", tl!(label), format_list(list))).font(regular(11.5)).color(t.text_dim);
                        fields.add(ui, field, egui::Label::new(text).truncate());
                    }
                };
                line(ui, select::Field::To, tn!("To:"), &m.to);
                line(ui, select::Field::Cc, tn!("Cc:"), &m.cc);
                line(ui, select::Field::Bcc, tn!("Bcc:"), &m.bcc);
            });
        });
        notes(app, ui, m, &t, pictures);
        attachments(app, ui, m, &t);
    });
    select::header(app, ui, place, fields, around.response.rect);
    let r = ui.available_rect_before_wrap();
    ui.painter().line_segment([pos2(r.min.x, r.min.y), pos2(r.max.x, r.min.y)], Stroke::new(1.0, t.border));
}

/// Info lines under the header: the flag, importance, what was done with the message, its
/// pictures on the web (`pictures` lays that line out), categories.
fn notes(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, t: &Tokens, pictures: impl FnOnce(&mut MailApp, &mut Ui, &Tokens)) {
    let mut lines: Vec<(&str, String)> = Vec::new();
    match m.flag {
        FlagStatus::Flagged => {
            let due = m.flag_due.map(|d| format!(" {} {}", tl!("Due"), crate::i18n::date_medium(app.session.local(d).date))).unwrap_or_default();
            lines.push(("flag", format!("{}{due}", tl!("Follow up."))));
        }
        FlagStatus::Complete => lines.push(("complete", tl!("Completed.").to_string())),
        FlagStatus::None => {}
    }
    match m.importance {
        Importance::High => lines.push(("highImportance", tl!("This message was sent with High importance.").to_string())),
        Importance::Low => lines.push(("lowImportance", tl!("This message was sent with Low importance.").to_string())),
        Importance::Normal => {}
    }
    // (An invitation that was answered says so, and what, in its own bar.)
    let meeting = invitation::info(app, ui.ctx(), m);
    if m.answered && !meeting.as_ref().is_some_and(|meeting| meeting.says_answer()) {
        lines.push(("reply", tl!("You replied to this message.").to_string()));
    }
    if m.forwarded {
        lines.push(("forward", tl!("You forwarded this message.").to_string()));
    }
    for (icon, text) in lines {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), icon, t.text_dim, t.accent);
            ui.add(egui::Label::new(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim)).wrap());
        });
    }
    pictures(app, ui, t);
    // A message waiting in the Outbox for its time (Delay Delivery).
    if let Some(at) = m.deliver_after.filter(|_| m.is_deferred(app.session.now())) {
        let when = app.session.local(at);
        let text = crate::i18n::fmt(
            tl!("This message waits in the Outbox until {day} {time}."),
            &[("day", &crate::i18n::date_medium(when.date)), ("time", &when.time_short(app.session.prefs.twelve_hour))],
        );
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), "clock", t.text_dim, t.accent);
            ui.add(egui::Label::new(egui::RichText::new(text).font(regular(11.5)).color(t.text_dim)).wrap());
        });
    }
    // The sender asked for a read receipt, and the owner chose to be asked (File › Options).
    if mailcraft_engine::cmd::receipts::ask_in_reading_pane(&app.session, m) {
        ui.horizontal_wrapped(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), "info", t.text_dim, t.accent);
            ui.label(egui::RichText::new(tl!("The sender asked to be told when you read this message.")).font(regular(11.5)).color(t.text_dim));
            let id = m.id;
            let send = ui.add(egui::Button::new(egui::RichText::new(tl!("Send Receipt")).font(regular(11.0))).small());
            app.hit.add("readReceipt.send", id, send.rect);
            if send.clicked() {
                let _ = app.run("mail.readReceipt", json!({"id": id, "send": true}));
            }
            let skip = ui.add(egui::Button::new(egui::RichText::new(tl!("Don't Send")).font(regular(11.0))).small());
            app.hit.add("readReceipt.decline", id, skip.rect);
            if skip.clicked() {
                let _ = app.run("mail.readReceipt", json!({"id": id, "send": false}));
            }
        });
    }
    if !m.categories.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let mut x = r.min.x;
        for c in &m.categories {
            x += crate::widgets::category_chip(ui, pos2(x, r.center().y), c, crate::theme::c32(app.session.mailbox.category_color(c))) + 6.0;
        }
    }
    // The rest of the conversation, to jump to.
    let thread = app.session.mailbox.conversation(m.id);
    if thread.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(crate::i18n::fmt(tl!("{n} messages in this conversation:"), &[("n", &thread.len().to_string())]))
                    .font(regular(11.5))
                    .color(t.text_dim),
            );
            for id in thread.iter().rev().take(12) {
                let Some(other) = app.session.mailbox.message(*id) else { continue };
                let label = format!(
                    "{} · {}",
                    other.from.display(),
                    crate::i18n::list_label(other.date, app.session.now(), app.session.offset_at(other.date), app.session.prefs.twelve_hour)
                );
                if ui.add_enabled(*id != m.id, egui::Button::new(egui::RichText::new(label).font(regular(11.0))).small()).clicked() {
                    let _ = app.run("mail.select", json!({"id": id}));
                }
            }
        });
    }
}

fn attachments(app: &mut MailApp, ui: &mut Ui, m: &Message, t: &Tokens) {
    // The calendar part of a meeting is what its bar shows: it is not one of the files.
    let meeting = invitation::info(app, ui.ctx(), m);
    let files: Vec<(usize, &mailcraft_model::Attachment)> =
        m.listed_attachments().filter(|(index, _)| !invitation::stands_for(meeting.as_deref(), *index)).collect();
    if files.is_empty() {
        return;
    }
    ui.add_space(2.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (index, a) in files {
            let name_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(a.name.clone(), regular(12.0), t.text).size().x).min(220.0);
            let (r, resp) = ui.allocate_exact_size(vec2(name_w + 44.0, 38.0), Sense::click());
            ui.painter().rect(r, 5.0, if resp.hovered() { t.hover } else { t.panel }, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 17.0, r.center().y), vec2(18.0, 18.0)), "attach", t.icon, t.accent);
            paint_clipped(ui.painter(), pos2(r.min.x + 34.0, r.min.y + 13.0), name_w + 4.0, &a.name, regular(12.0), t.text);
            ui.painter().text(pos2(r.min.x + 34.0, r.min.y + 27.0), Align2::LEFT_CENTER, a.size_label(), regular(10.5), t.text_dim);
            let resp = resp.on_hover_text(crate::i18n::fmt(tl!("Save {name}"), &[("name", &a.name)]));
            if resp.clicked() {
                let _ = app.run("mail.saveAttachment", json!({"id": m.id, "index": index}));
            }
            resp.context_menu(|ui| {
                crate::widgets::mi(ui, app, tn!("Save As…"), "mail.saveAttachment", json!({"id": m.id, "index": index}));
                crate::widgets::mi(ui, app, tn!("Remove Attachment"), "mail.removeAttachment", json!({"id": m.id, "index": index}));
            });
        }
    });
}
