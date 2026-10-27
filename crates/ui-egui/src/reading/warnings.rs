// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Sender warnings** where a message is read (#508): one banner above it with each reason,
//! red when any is a **warning** and amber when they are only **cautions**; the quiet
//! **External** tag beside the sender's name; a mark on a link whose text names one place and
//! which goes to another; and a mark on the message's row in the list for a failed check or a
//! name that hides another address.
//!
//! What the warnings are, and what they rest on, is the engine's
//! ([`mailcraft_engine::Session::sender_warnings`]): this only shows them. Each says what it is
//! about in plain words; the sender's own words are shown as data, cut short, never as ours.

use egui::{Rect, Sense, Ui, vec2};
use mailcraft_model::Message;
use mailcraft_model::auth_results::Failed;
use mailcraft_model::sender_warnings::{Level, NameReason, Warning, banner_level};
use serde_json::json;

use super::Place;
use super::remote::notice;
use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The most characters of a name, an address or a host a line of the banner shows.
const SHOWN: usize = 60;

/// `text` cut to [`SHOWN`] characters, with an ellipsis when it is longer.
/// Whether `c` turns the direction of text round (or marks it), which would let a name or a
/// domain show its letters in another order than they are written in (`moc.example`).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

pub(super) fn short(text: &str) -> String {
    let text: String = text.chars().filter(|c| !is_bidi_control(*c)).collect();
    if text.chars().count() > SHOWN { text.chars().take(SHOWN - 1).chain(['…']).collect() } else { text }
}

/// What a reason is called, and what it means for this message.
fn said(w: &Warning, m: &Message, links: usize) -> (String, String) {
    let real = short(m.from.email.trim());
    let fill = |template: &str, pairs: &[(&str, &str)]| crate::i18n::fmt(template, pairs);
    match w {
        Warning::FailedCheck(Failed::DmarcPolicyNone) => (
            tl!("Failed check.").to_string(),
            tl!("Your mail server checked where this message came from, and the check failed, though the sender's domain asks for nothing to be done about it: the sender's address may be forged.").to_string(),
        ),
        Warning::FailedCheck(_) => (
            tl!("Failed check.").to_string(),
            tl!("Your mail server checked where this message came from, and the check failed: the sender's address may be forged.").to_string(),
        ),
        Warning::NameHidesAddress { reason: NameReason::AddressInName(other), .. } => (
            tl!("The name hides another address.").to_string(),
            fill(tl!("The sender's name shows {shown}, but the message came from {real}."), &[("shown", &short(other)), ("real", &real)]),
        ),
        Warning::NameHidesAddress { reason: NameReason::KnownName, name } => (
            tl!("The name of someone you know.").to_string(),
            fill(
                tl!("{name} is the name of someone you know, but this message came from {real}, which isn't one of their addresses."),
                &[("name", &short(name)), ("real", &real)],
            ),
        ),
        Warning::NameHidesAddress { reason: NameReason::HiddenCharacters, .. } => (
            tl!("The name hides characters.").to_string(),
            fill(tl!("Most of the sender's name is characters that aren't shown, and the message came from {real}."), &[("real", &real)]),
        ),
        Warning::NameHidesAddress { reason: NameReason::OwnName, .. } => (
            tl!("Your own name.").to_string(),
            fill(tl!("The sender uses your name, but this message came from {real}, which isn't one of your addresses."), &[("real", &real)]),
        ),
        Warning::LookalikeSender { domain, like } => (
            tl!("Lookalike address.").to_string(),
            fill(
                tl!("The sender's domain, {domain}, looks like {like}, which you correspond with, but it isn't the same."),
                &[("domain", &short(domain)), ("like", &short(like))],
            ),
        ),
        Warning::LinkMismatch { text_host, host, .. } => {
            let first = if text_host.is_empty() {
                fill(tl!("A link whose text is mostly characters that aren't shown goes to {host}."), &[("host", &short(host))])
            } else {
                fill(tl!("A link that reads {named} goes to {host}."), &[("named", &short(text_host)), ("host", &short(host))])
            };
            let more = links.saturating_sub(1);
            let rest = if more > 0 { format!(" {}", tln!(more, "And {n} more link like it.", "And {n} more links like it.")) } else { String::new() };
            (tl!("A link goes somewhere else.").to_string(), format!("{first}{rest} {}", tl!("Point at a link to see where it goes.")))
        }
        Warning::External { domain } => {
            (tl!("From outside your organisation.").to_string(), fill(tl!("The sender's address is at {domain}."), &[("domain", &short(domain))]))
        }
    }
}

/// A small close button (a cross) at the right of the banner, in the banner's hue: a click is
/// `true`. Named for a screen reader as `label`; `tip` says what it does.
fn close_button(ui: &mut Ui, label: &str, tip: &str, t: &Tokens) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 3.0, if resp.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(12.0, 12.0)), "close", t.icon, t.accent);
    let label = label.to_string();
    resp.widget_info(move || egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    resp.on_hover_text(tip).clicked()
}

/// Whether the owner has opened the slim bar of a dismissed warning for `id`'s message again.
fn reopened(ui: &Ui, id: u64) -> bool {
    ui.data(|d| d.get_temp::<bool>(egui::Id::unique(("senderWarningsOpen", id)))).unwrap_or(false)
}

fn reopen(ui: &Ui, id: u64, open: bool) {
    ui.data_mut(|d| d.insert_temp(egui::Id::unique(("senderWarningsOpen", id)), open));
}

/// The banner above `m` when it has a warning or a caution: one line for each reason, the first
/// link that goes somewhere else standing for the rest, and External folded in as a line of its
/// own. Nothing for a message that has none, or only the External tag.
///
/// The owner can dismiss it (the cross at its right; `mail.dismissWarnings`, remembered with the
/// message). A caution then goes; a warning is kept as a slim bar, one line that opens the
/// banner again with a click. A kind of warning that was not there when it was dismissed brings
/// the whole banner back.
pub(super) fn banner(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, t: &Tokens) {
    let warnings = app.session.sender_warnings(m, true);
    let Some(level) = banner_level(&warnings) else { return };
    let (hue, lead) = match level {
        Level::Warning => (t.red, tl!("This message may not be what it seems.")),
        _ => (t.orange, tl!("Be careful with this message.")),
    };
    let dismissed = mailcraft_model::sender_warnings::dismissed(&warnings, &m.warnings_dismissed);
    let open = reopened(ui, m.id);
    if dismissed && level != Level::Warning {
        return;
    }
    if dismissed && !open {
        slim(app, ui, m, place, hue, lead, t);
        return;
    }
    let links = warnings.iter().filter(|w| matches!(w, Warning::LinkMismatch { .. })).count();
    let mut link_said = false;
    let mut close = false;
    let shown = notice(hue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r.shrink(1.0), "warning", hue, hue);
            ui.add(egui::Label::new(egui::RichText::new(lead).font(semibold(11.5)).color(t.text)).wrap());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (label, tip) = if dismissed {
                    (tl!("Hide"), tl!("Hide this message's warning again"))
                } else {
                    (tl!("Dismiss"), tl!("Dismiss this warning for this message. It comes back if a new kind of warning appears."))
                };
                close = close_button(ui, label, tip, t);
            });
        });
        for w in warnings.iter() {
            if matches!(w, Warning::LinkMismatch { .. }) {
                if link_said {
                    continue;
                }
                link_said = true;
            }
            let (title, why) = said(w, m, links);
            // Under the lead, every line of it.
            egui::Frame::NONE.inner_margin(egui::Margin { left: 22, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
                    ui.add(egui::Label::new(egui::RichText::new(title).font(semibold(11.5)).color(t.text)).wrap());
                    ui.add(egui::Label::new(egui::RichText::new(why).font(regular(11.5)).color(t.text_dim)).wrap());
                });
            });
        }
    });
    if place == Place::Pane {
        app.hit.add("senderWarnings", m.id, shown.response.rect);
    }
    if close {
        if dismissed {
            reopen(ui, m.id, false);
        } else {
            let _ = app.run("mail.dismissWarnings", json!({"ids": [m.id]}));
        }
    }
}

/// The slim bar a dismissed warning leaves: its mark and its lead on one line, and a click opens
/// the whole banner again.
fn slim(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, hue: egui::Color32, lead: &str, t: &Tokens) {
    let shown = notice(hue, t).inner_margin(egui::Margin { left: 8, right: 8, top: 2, bottom: 2 }).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            icons::paint(ui.painter(), r, "warning", hue, hue);
            ui.add(egui::Label::new(egui::RichText::new(lead).font(semibold(11.5)).color(t.text)).selectable(false).truncate());
        });
    });
    let tip = tl!("Show this message's warning");
    let resp = shown.response.interact(Sense::click()).on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tip));
    if place == Place::Pane {
        app.hit.add("senderWarningsCollapsed", m.id, shown.response.rect);
    }
    if resp.clicked() {
        reopen(ui, m.id, true);
    }
}

/// Whether `m` is from outside the owner's organisation.
pub(super) fn external(app: &mut MailApp, m: &Message) -> bool {
    app.session.sender_warnings(m, true).iter().any(|w| matches!(w, Warning::External { .. }))
}

/// The External tag beside the sender's name: small and quiet, a word in a thin frame.
/// The width the External tag takes, its frame and the room before it with it, in the language
/// the window speaks.
pub(super) fn external_tag_width(ui: &Ui, t: &Tokens) -> f32 {
    ui.painter().layout_no_wrap(tl!("External").to_string(), regular(10.5), t.text_dim).size().x + 8.0 + 2.0 + 6.0 + ui.spacing().item_spacing.x
}

pub(super) fn external_tag(ui: &mut Ui, t: &Tokens) -> egui::Response {
    egui::Frame::NONE
        .stroke(egui::Stroke::new(1.0, t.border_strong))
        .corner_radius(3.0)
        .inner_margin(egui::Margin { left: 4, right: 4, top: 0, bottom: 0 })
        .show(ui, |ui| ui.add(egui::Label::new(egui::RichText::new(tl!("External")).font(regular(10.5)).color(t.text_dim)).selectable(false)))
        .response
        .on_hover_text(tl!("This message came from outside your organisation."))
}

/// Whether `m`'s row in the list wears the warning mark: a warning, not a caution (the server
/// says its checks failed, or its name hides another address). Worked out for the rows on screen, without reading bodies.
pub(crate) fn row_mark(app: &mut MailApp, m: &Message) -> bool {
    app.session.sender_warnings(m, false).iter().any(|w| w.level() == Level::Warning)
}

/// The addresses of `m`'s links that go somewhere other than their text says, as the links of
/// the laid-out body have them, for their mark and their tip.
pub(super) fn mismatched(app: &mut MailApp, m: &Message) -> Vec<String> {
    app.session
        .sender_warnings(m, true)
        .iter()
        .filter_map(|w| match w {
            Warning::LinkMismatch { url, .. } => Some(url.clone()),
            _ => None,
        })
        .collect()
}

/// Whether the link to `url` (as a message's link has it) is one of `mismatched`.
pub(super) fn is_mismatched(url: &str, mismatched: &[String]) -> bool {
    !mismatched.is_empty() && mailcraft_model::sender_warnings::link_targets(url).iter().any(|link| mismatched.iter().any(|m| m == link.as_str()))
}
