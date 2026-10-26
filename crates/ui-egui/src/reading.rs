// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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
pub(crate) mod images;
#[cfg(test)]
mod images_tests;
mod invitation;
mod layout;
pub(crate) mod remote;
mod select;
mod sharing;
#[cfg(test)]
mod sharing_tests;
#[cfg(test)]
mod tests;
mod view;

use std::sync::Arc;

use egui::{Align2, Rect, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_html::Block;
use mailcraft_model::{FlagStatus, Importance, Message, format_list_capped};
use serde_json::json;

pub(crate) use invitation::{list_marks, options as invitation_options, question as invitation_question};
pub use remote::{Access, RemotePictures};
pub use select::Selections;
pub(crate) use select::{inspect as selection_inspect, select_all_params};

use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// Most recipient names the header of the reading pane lists, and most category chips it paints.
const SHOWN_NAMES: usize = 50;
const SHOWN_CATEGORIES: usize = 8;

/// The widest a picture in a message's body is shown, in points. Its text fills the pane, as
/// Outlook's does, and as the body of a reply being written does above the original (#47).
pub const MAX_BODY_WIDTH: f32 = 860.0;
/// The widest a message's text is laid out, in points: wider than any screen's pane, a bound on
/// the work of laying it out, not a measure.
pub const MAX_TEXT_WIDTH: f32 = 4_000.0;

/// Where a message is shown. One that is on screen in two places is laid out for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    /// The reading pane of the main window.
    Pane,
    /// A window of its own, by the window's number.
    Window(u64),
    /// Under the owner's text in a reply or forward being written: the original it carries.
    Quote,
    /// File › Signatures: a sample reply with the signature, as a recipient sees it.
    Preview,
}

/// The reading pane: the selected message, or a word on why there is none to show.
pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let picked = app.session.sel.messages.len();
    // What showing it needs (its body, the pictures in it, a calendar part) is brought into
    // memory first, when the mailbox keeps it on disk; the message then shows whole in this
    // very frame. (One that can't be read says so in its text, once.)
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
        let text = tln!(picked, "{n} item selected", "{n} items selected");
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
    if place != Place::Quote && app.session.needs_load_for_display(m.id) {
        let _ = app.session.load_for_display(m.id);
    }
    remote::tick(app, &ctx);
    select::begin(app, ui, m, place);
    let doc = view::doc_where(&ctx, m, place, remote::available(app));
    let access = if doc.remote.is_empty() { Access::Blocked } else { remote::access(app, m) };
    // Asked for: what has not come yet is sent for, while the message is on screen.
    let progress = if access == Access::Allowed { remote::want(app, &ctx, m, &doc) } else { remote::Progress::default() };
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    header(app, ui, m, place, |app, ui, t| {
        remote::line(app, ui, m, &doc, access, progress, place, t);
        unreadable_line(ui, m, t);
    });
    // A meeting that came with the message: what it is, and the ways to answer it.
    invitation::bar(app, ui, m, place);
    // A calendar that was shared with the owner, and the way to open it (#70).
    sharing::bar(app, ui, m);
    // Only its headers and a preview are here yet (R3.2): say so, and why.
    if m.unfetched {
        let t = Tokens::get(ui.ctx());
        let said = unfetched_said(app, m);
        egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin { left: 20, right: 20, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(said).font(crate::theme::regular(12.0)).color(t.text_dim));
        });
    }
    // Held in the Outbox (R3.4.2): what happened, and the owner's say-so to send it again.
    if !m.held.is_empty() {
        let t = Tokens::get(ui.ctx());
        let id = m.id;
        egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin { left: 20, right: 20, top: 6, bottom: 6 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                // The pane lays its header out without gaps; the words and the button need them.
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(egui::RichText::new(tl!("Held:")).font(crate::theme::regular(12.0)).strong().color(t.text));
                ui.label(egui::RichText::new(m.held.as_str()).font(crate::theme::regular(12.0)).color(t.text_dim));
                if ui.button(tl!("Send Again")).clicked() {
                    let _ = app.run("mail.sendAgain", serde_json::json!({"id": id}));
                }
            });
        });
    }
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

/// What the reading pane says of a message of which only the headers and a preview are here.
pub(crate) fn unfetched_said(app: &MailApp, m: &Message) -> &'static str {
    if app.fetching.fetching(m.id) {
        tl!("Downloading the whole message…")
    } else if app.jmap_refused.contains(&m.account) {
        // Its server turned the account's sign-in down: nothing downloads until it is given again.
        tl!("The server turned down the sign-in for this account. Use Send/Receive to sign in again.")
    } else if app.fetching.offline && !app.session.mailbox.within_window(m, app.session.now()) {
        // Older than its mail to keep offline (#143): only its headers and preview are kept here.
        tl!("Not connected: the rest of this message is on the server")
    } else if app.fetching.offline {
        tl!("This message isn't on this computer yet. It will download when you're online.")
    } else {
        tl!("Only the beginning of this message is here. The rest downloads with the next Send/Receive.")
    }
}

/// Whether `m`'s subject, sender or text holds U+FFFD, the mark a reader leaves for a character
/// it couldn't read (bytes that aren't in the character set the message names, or no name for
/// one that isn't UTF-8 or Windows-1252).
pub(crate) fn has_unreadable(m: &Message) -> bool {
    const MARK: char = char::REPLACEMENT_CHARACTER;
    [m.subject.as_str(), m.from.name.as_str(), m.from.email.as_str(), m.body.text.as_str(), m.body.html.as_deref().unwrap_or("")]
        .iter()
        .any(|text| text.contains(MARK))
}

/// The line above a message that holds a character that couldn't be read ([`has_unreadable`]):
/// why there is a � in it. One line for the whole message, so that no � needs a word of its own.
/// Looked for once per message and kept, not every frame: a body can be long.
fn unreadable_line(ui: &mut Ui, m: &Message, t: &Tokens) {
    let key = egui::Id::unique(("mailcraft-unreadable", m.id, m.subject.len(), m.body.text.len(), m.body.html.as_ref().map_or(0, String::len)));
    let unreadable = match ui.ctx().data(|d| d.get_temp::<bool>(key)) {
        Some(kept) => kept,
        None => {
            let found = has_unreadable(m);
            ui.ctx().data_mut(|d| d.insert_temp(key, found));
            found
        }
    };
    if !unreadable {
        return;
    }
    remote::notice(t.blue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), "info", t.text, t.blue);
            let said = tl!("Some characters in this message couldn't be read: the sender's program didn't say properly how they were written.");
            ui.add(egui::Label::new(egui::RichText::new(said).font(regular(11.5)).color(t.text)).wrap());
        });
    });
}

/// The original a reply or forward in HTML carries below the owner's text, read-only, through the
/// same display blocks as the reading pane: nothing in it runs, its own pictures (`cid:`) are
/// shown, and pictures on the web only as the original's would be (the owner asked for them for
/// that message, its sender or its sender's domain, or for all mail; never in Junk E-mail but
/// message by message), while it is on screen.
pub(crate) fn show_quote(app: &mut MailApp, ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    let Some(m) = quote_message(app, &ctx) else { return };
    remote::tick(app, &ctx);
    let doc = view::doc_where(&ctx, &m, Place::Quote, remote::available(app));
    // A quote whose original isn't known (a reopened draft, an original since deleted) has no
    // folder or sender to judge by: it never fetches anything.
    let known = app.session.compose.as_ref().and_then(|c| c.source).is_some_and(|id| app.session.mailbox.messages.contains_key(&id));
    let access = if doc.remote.is_empty() || !known { Access::Blocked } else { remote::access(app, &m) };
    if access == Access::Allowed {
        remote::want(app, &ctx, &m, &doc);
    }
    view::show(app, ui, &m, Place::Quote, access);
}

/// The carried original as a message to show: the original's sender, date and place (what its
/// pictures are allowed by), with the quote's HTML for a body and the pictures the quote takes
/// along. Kept from frame to frame while the quote stays the same.
fn quote_message(app: &MailApp, ctx: &egui::Context) -> Option<Arc<Message>> {
    let c = app.session.compose.as_ref()?;
    let quote = c.quote.as_ref()?;
    let key = egui::Id::unique("mailcraft-compose-quote");
    let kept: Option<Arc<Message>> = ctx.data(|d| d.get_temp(key));
    let source = c.source.and_then(|id| app.session.mailbox.messages.get(&id));
    let pictures = if quote.pictures.is_empty() { &c.attachments } else { &quote.pictures };
    let same = |m: &Arc<Message>| {
        m.body.html.as_deref() == Some(quote.html.as_str()) && m.id == source.map_or(0, |s| s.id) && m.attachments.len() == pictures.len()
    };
    if let Some(m) = kept.filter(same) {
        return Some(m);
    }
    let mut m = source.map(|s| (**s).clone()).unwrap_or_default();
    m.body = mailcraft_model::Body { text: quote.text.clone(), html: Some(quote.html.clone()) };
    // A forward carries the original's attachments itself, its pictures among them.
    m.attachments = pictures.clone();
    // Its body is the quote's, whole: nothing of it waits on disk.
    m.kept = None;
    let m = Arc::new(m);
    ctx.data_mut(|d| d.insert_temp(key, m.clone()));
    Some(m)
}

/// `m` shown read-only through the display blocks, as at `place` ([`Place::Preview`]): its own
/// pictures only, nothing from the web.
pub(crate) fn show_plain_view(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, place: Place) {
    view::show(app, ui, m, place, Access::Blocked);
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
    ("reply", "Reply", "mail.reply"),
    ("replyAll", "Reply All", "mail.replyAll"),
    ("forward", "Forward", "mail.forward"),
    ("delete", "Delete", "mail.delete"),
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
            crate::sender_images::avatar(app, ui, r.center(), 17.0, m, &m.from);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 1.0);
                let when = crate::dates::header_label(m.date, app.session.offset_at(m.date), app.session.prefs.twelve_hour);
                // The date first, at the right; the name and address get what is left, cut short
                // when the pane is narrow rather than run under the date.
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        fields.add(ui, select::Field::Date, egui::Label::new(egui::RichText::new(when).font(regular(11.5)).color(t.text_dim)));
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            // The address is what the sender really is: it gets its room first, and a
                            // long name is cut short instead (and a name that looks like an address
                            // is marked, as it may be passing for one).
                            let address_text = format!("<{}>", m.from.email);
                            let room = ui.available_width();
                            let address_w = if m.from.name.is_empty() {
                                0.0
                            } else {
                                ui.painter().layout_no_wrap(address_text.clone(), regular(11.5), t.text_dim).size().x.min(room * 0.8)
                            };
                            let name_w = (room - address_w - ui.spacing().item_spacing.x).max(room * 0.2);
                            let looks_like_address = m.from.name.contains('@');
                            let name =
                                egui::RichText::new(m.from.display()).font(semibold(13.0)).color(if looks_like_address { t.orange } else { t.text });
                            ui.scope(|ui| {
                                ui.set_max_width(name_w);
                                fields.add(ui, select::Field::Sender, egui::Label::new(name).truncate());
                            });
                            if !m.from.name.is_empty() {
                                let address = egui::RichText::new(address_text).font(regular(11.5)).color(t.text_dim);
                                fields.add(ui, select::Field::Address, egui::Label::new(address).truncate());
                            }
                        });
                    });
                });
                // (`said` is the line in the interface language, with `{names}` still to fill.)
                let mut line = |ui: &mut Ui, field: select::Field, said: &str, list: &[mailcraft_model::Address]| {
                    if !list.is_empty() {
                        // (A sender can write thousands of names; the first ones are shown.)
                        let (mut names, more) = format_list_capped(list, SHOWN_NAMES);
                        if more > 0 {
                            names.push_str(&format!("; {}", tln!(more, "and {n} more", "and {n} more")));
                        }
                        let said = crate::i18n::fmt(said, &[("names", &names)]);
                        let text = egui::RichText::new(said).font(regular(11.5)).color(t.text_dim);
                        fields.add(ui, field, egui::Label::new(text).truncate());
                    }
                };
                line(ui, select::Field::To, tl!("To: {names}"), &m.to);
                line(ui, select::Field::Cc, tl!("Cc: {names}"), &m.cc);
                line(ui, select::Field::Bcc, tl!("Bcc: {names}"), &m.bcc);
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
            let said = match m.flag_due {
                Some(d) => crate::i18n::fmt(tl!("Follow up. Due {date}"), &[("date", &crate::dates::medium(app.session.local(d).date))]),
                None => tl!("Follow up.").to_string(),
            };
            lines.push(("flag", said));
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
    if !m.categories.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        let mut x = r.min.x;
        for c in m.categories.iter().take(SHOWN_CATEGORIES) {
            x += crate::widgets::category_chip(ui, pos2(x, r.center().y), c, crate::theme::c32(app.session.mailbox.category_color(c))) + 6.0;
        }
    }
    // The rest of the conversation, to jump to.
    let thread = app.session.mailbox.conversation(m.id);
    if thread.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(tln!(thread.len(), "{n} message in this conversation:", "{n} messages in this conversation:"))
                    .font(regular(11.5))
                    .color(t.text_dim),
            );
            for id in thread.iter().rev().take(12) {
                let Some(other) = app.session.mailbox.message(*id) else { continue };
                let label = format!(
                    "{} · {}",
                    other.from.display(),
                    crate::dates::list_label(other.date, app.session.now(), app.session.offset_at(other.date), app.session.prefs.twelve_hour)
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
            // The name it is opened and saved under, not the one the sender wrote: a right-to-left
            // override can make `fdp.exe` read as `exe.pdf`.
            let shown = mailcraft_engine::opening::file_name(&a.name);
            let name_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(shown.clone(), regular(12.0), t.text).size().x).min(220.0);
            let (r, resp) = ui.allocate_exact_size(vec2(name_w + 44.0, 38.0), Sense::click());
            ui.painter().rect(r, 5.0, if resp.hovered() { t.hover } else { t.panel }, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            icons::paint(ui.painter(), Rect::from_center_size(pos2(r.min.x + 17.0, r.center().y), vec2(20.0, 20.0)), "attach", t.icon, t.accent);
            paint_clipped(ui.painter(), pos2(r.min.x + 34.0, r.min.y + 13.0), name_w + 4.0, &shown, regular(12.0), t.text);
            ui.painter().text(
                pos2(r.min.x + 34.0, r.min.y + 27.0),
                Align2::LEFT_CENTER,
                mailcraft_model::size_label_in(crate::i18n::current(), a.size() as u64),
                regular(10.5),
                t.text_dim,
            );
            // Double-click opens it, as in Outlook; its menu opens or saves it.
            let resp = resp.on_hover_text(crate::i18n::fmt(tl!("Double-click to open {name}; right-click to save it"), &[("name", &shown)]));
            if resp.double_clicked() {
                let _ = app.run("mail.openAttachment", json!({"id": m.id, "index": index}));
            }
            resp.context_menu(|ui| {
                crate::widgets::mi(ui, app, "Open", "mail.openAttachment", json!({"id": m.id, "index": index}));
                crate::widgets::mi(ui, app, "Save As…", "mail.saveAttachment", json!({"id": m.id, "index": index}));
                crate::widgets::mi(ui, app, "Remove Attachment", "mail.removeAttachment", json!({"id": m.id, "index": index}));
            });
        }
    });
}
