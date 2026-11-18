// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving mail between Gmail's categories in the window (#911): messages dragged out of the list
//! and let go over a category's row in the folder pane or its tab above the Inbox, and the
//! message menu's Move to Category with the five. Each runs `mail.setGmailCategory`.
//!
//! While moves are switched off (`mailcraft_engine::gmail::GMAIL_CATEGORY_MOVES`) none of it is
//! there: a row or a tab takes nothing, and the menu has no such item. Mail of an account that
//! signs in with an app password has the item greyed, with a note that it needs Google sign-in.

use egui::{Rect, Stroke, Ui, vec2};
use mailcraft_engine::gmail::{NEEDS_GOOGLE_SIGN_IN, why_not_movable};
use mailcraft_model::{GmailCategory, Id};
use serde_json::json;

use crate::MailApp;
use crate::nav::gmail_name;
use crate::theme::{Tokens, regular};

/// The command.
const SET: &str = "mail.setGmailCategory";

/// Whether this window offers moves between Gmail's categories at all: switched on, and not in
/// the browser. (Off, command search doesn't list it either.)
pub(crate) fn switched_on(app: &MailApp) -> bool {
    app.session.gmail_category_moves && !cfg!(target_arch = "wasm32")
}

/// Of the messages `dragged`, those that can go to `category`: in a Gmail Inbox Gmail sorted, of
/// an account that signs in with Google, and in another category now.
pub(crate) fn movable(app: &MailApp, dragged: &[Id], category: GmailCategory) -> Vec<Id> {
    if !switched_on(app) || !app.enabled(SET) {
        return Vec::new();
    }
    let s = &app.session;
    dragged
        .iter()
        .copied()
        .filter(|id| s.mailbox.message(*id).is_some_and(|m| m.gmail_category != Some(category) && why_not_movable(s, m).is_none()))
        .collect()
}

/// A category's row or tab (`resp`, drawn in `rect`) as a place to let messages dragged out of
/// the list go: it shows that it takes them while they are over it, and moves them when they are
/// let go. Nothing while moves are switched off.
pub(crate) fn drop_target(app: &mut MailApp, ui: &Ui, resp: &egui::Response, rect: Rect, category: GmailCategory) {
    if let Some(drag) = resp.dnd_hover_payload::<crate::mail_view::MessageDrag>() {
        let takes = !movable(app, &drag.0, category).is_empty();
        crate::drag_pointer::note_folder(ui.ctx(), takes);
        if takes {
            let t = Tokens::get(ui.ctx());
            ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
    }
    if let Some(drag) = resp.dnd_release_payload::<crate::mail_view::MessageDrag>() {
        let ids = movable(app, &drag.0, category);
        if !ids.is_empty() {
            // The selection dragged goes unnamed, so that in conversation view its whole
            // conversation in that Inbox goes along; anything else dragged, as named.
            let sorted = |mut ids: Vec<Id>| {
                ids.sort_unstable();
                ids
            };
            let selection = sorted(app.session.acting_on()) == sorted(drag.0.clone());
            let params = if selection { json!({"category": category.code()}) } else { json!({"ids": ids, "category": category.code()}) };
            let _ = app.run(SET, params);
        }
    }
}

/// What the message menu shows of Move to Category for the messages it acts on: nothing (moves
/// are off, or none of them is in a Gmail Inbox Gmail sorted), the five, or the item greyed and
/// why.
enum Offer {
    Hidden,
    Open,
    Greyed(&'static str),
}

fn offer(app: &mut MailApp) -> Offer {
    if !switched_on(app) {
        return Offer::Hidden;
    }
    let ids = app.session.acting_on();
    let s = &app.session;
    let sorted: Vec<_> = ids.iter().filter_map(|id| s.mailbox.message(*id)).filter(|m| m.gmail_category.is_some()).collect();
    if sorted.is_empty() {
        return Offer::Hidden;
    }
    let reasons: Vec<Option<&'static str>> = sorted.iter().map(|m| why_not_movable(s, m)).collect();
    if reasons.iter().any(Option::is_none) && app.enabled(SET) {
        return Offer::Open;
    }
    let why = reasons.iter().flatten().find(|why| **why == NEEDS_GOOGLE_SIGN_IN).or_else(|| reasons.iter().flatten().next()).copied();
    Offer::Greyed(why.unwrap_or(NEEDS_GOOGLE_SIGN_IN))
}

/// Move to Category in the message menu, with the five; greyed with its note for mail that can't
/// be moved; not there at all while moves are off or for mail of no Gmail Inbox.
pub(crate) fn menu(app: &mut MailApp, ui: &mut Ui) {
    match offer(app) {
        Offer::Hidden => {}
        Offer::Open => {
            ui.menu_button(tl!("Move to Category"), |ui| {
                for category in GmailCategory::ALL {
                    if ui.add(egui::Button::new(gmail_name(category)).min_size(vec2(160.0, 0.0))).clicked() {
                        // The selection, unnamed: in conversation view the command takes the
                        // whole conversation in that Inbox along.
                        let ids = app.session.acting_on();
                        if !movable(app, &ids, category).is_empty() {
                            let _ = app.run(SET, json!({"category": category.code()}));
                        }
                        ui.close();
                    }
                }
            });
        }
        Offer::Greyed(why) => {
            let note = if why == NEEDS_GOOGLE_SIGN_IN { tl!("Needs Google sign-in") } else { tl!(why) };
            ui.add_enabled(false, egui::Button::new(tl!("Move to Category")).min_size(vec2(190.0, 0.0))).on_disabled_hover_text(note);
            let t = Tokens::get(ui.ctx());
            ui.label(egui::RichText::new(note).font(regular(11.0)).color(t.text_dim));
        }
    }
}
