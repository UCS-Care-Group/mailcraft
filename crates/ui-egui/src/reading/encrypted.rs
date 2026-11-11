// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An **Encrypted message** where it is read (#30 part 6): it is opened with the owner's keys when
//! it is shown, and one bar above it says so: opened (held in memory only, never saved), being
//! opened, or why it wasn't, with Try Again. What was Digitally signed inside it has its own bar
//! ([`super::signature`]), so a message that is both says both.

use std::sync::Arc;

use egui::Ui;
use mailcraft_engine::decrypt::{Decryption, Failure};
use mailcraft_model::Message;
use serde_json::json;

use super::Place;
use super::remote::notice;
use crate::MailApp;
use crate::theme::{Tokens, regular, semibold};

/// The message as it opens, when it is an Encrypted one that has been (see [`MailApp::opened_view`]).
pub(super) fn view(app: &mut MailApp, ctx: &egui::Context, id: u64) -> Option<Arc<Message>> {
    app.opened_view(ctx, id)
}

pub(super) fn tick(app: &mut MailApp, ctx: &egui::Context) {
    app.tick_decrypting(ctx);
}

/// What a message that wasn't opened is told as, in plain words.
fn why(failure: &Failure) -> &'static str {
    match failure {
        Failure::NoKey => tl!("None of your keys on this computer is one this message was encrypted to."),
        Failure::Unsupported => tl!("This message is encrypted in a way MailCraft can't open yet."),
        Failure::Unavailable => tl!("The place your keys are kept (GnuPG, the system's certificates or a smartcard) can't be reached."),
        Failure::PinNeeded(_) => tl!("Your smartcard needs its PIN to open this message."),
        Failure::Cancelled => tl!("You closed the question for your PIN or passphrase."),
        Failure::Failed => tl!("It couldn't be opened: the key doesn't open it, or it was damaged."),
    }
}

/// The bar above `m` when it is an Encrypted message.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place) {
    if m.id == 0 || place == Place::Quote {
        return;
    }
    let state = app.session.decryption(m.id);
    let t = Tokens::get(ui.ctx());
    let (lead, hue, retry, check) = match &state {
        Decryption::NotEncrypted | Decryption::Waiting(_) => return,
        Decryption::Opening(_) => (tl!("Opening this message…").to_string(), t.text_dim, false, false),
        Decryption::Opened(_) => {
            (tl!("Opened with your key. What it says is held in memory only and never saved.").to_string(), t.green, false, false)
        }
        Decryption::NoOriginal => (
            tl!("It was downloaded before MailCraft kept the bytes it came as: Check Again downloads them.").to_string(),
            t.text_dim,
            false,
            app.session.enabled("mail.checkAgain"),
        ),
        Decryption::Failed(_, failure) => (why(failure).to_string(), t.orange, true, false),
    };
    let mut again = false;
    let mut check_again = false;
    let shown = notice(hue, &t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
            ui.label(egui::RichText::new(tl!("Encrypted")).font(semibold(11.5)).color(t.text));
            ui.add(egui::Label::new(egui::RichText::new(&lead).font(regular(11.5)).color(t.text)).wrap());
            if retry && app.session.enabled("mail.decryptAgain") {
                again = ui.small_button(tl!("Try Again")).on_hover_text(tl!("Try to open this message again with your keys")).clicked();
            }
            if check {
                check_again = ui.small_button(tl!("Check Again")).on_hover_text(tl!("Download the bytes this message came as again")).clicked();
            }
        });
    });
    if place == Place::Pane {
        app.hit.add("encryption", m.id, shown.response.rect);
    }
    if again {
        let _ = app.run("mail.decryptAgain", json!({"id": m.id}));
    }
    if check_again {
        let _ = app.run("mail.checkAgain", json!({"id": m.id}));
    }
}
