// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An **Encrypted message** where it is read (#30 part 6): it is opened with the owner's keys when
//! it is shown, and one bar above it says so: opened (held in memory only, never saved), being
//! opened, or why it wasn't, with Try Again. What was Digitally signed inside it has its own bar
//! ([`super::signature`]), so a message that is both says both.

use std::sync::Arc;

use egui::Ui;
use mailcraft_engine::decrypt::{Decryption, Failure, Outside};
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
        Failure::WrongPassphrase => tl!("The passphrase or PIN you typed was wrong."),
        Failure::TooLarge => tl!("What it opens to is larger than MailCraft keeps open at once."),
        Failure::Failed => tl!("It couldn't be opened: the key doesn't open it, or it was damaged."),
    }
}

/// What an opened message had beside what was encrypted, in words.
fn outside_said(outside: Outside) -> Option<String> {
    if !outside.any() {
        return None;
    }
    Some(match (outside.text, outside.files) {
        (true, 0) => tl!("There was also text that wasn't encrypted, and nobody signed it. It isn't shown here.").to_string(),
        (false, n) => tln!(
            n,
            "There was also {n} file that wasn't encrypted. It isn't shown here.",
            "There were also {n} files that weren't encrypted. They aren't shown here."
        ),
        (true, _) => tl!("There was also text and files that weren't encrypted, and nobody signed them. They aren't shown here.").to_string(),
    })
}

/// The bar above `m` when it is an Encrypted message.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place) {
    if m.id == 0 || place == Place::Quote {
        return;
    }
    let state = app.session.decryption(m.id);
    let t = Tokens::get(ui.ctx());
    let mut open = false;
    let mut extra: Vec<String> = Vec::new();
    let (lead, hue, retry, check) = match &state {
        Decryption::NotEncrypted => return,
        // The owner didn't choose this message (an agent did, or it was there already): it is opened when the owner says so.
        Decryption::Waiting(_) if app.decryption_needs_owner(m.id) => {
            open = true;
            (tl!("It isn't opened until you choose it. Select it, or press Open.").to_string(), t.text_dim, false, false)
        }
        Decryption::Waiting(_) if !app.decrypting.is_empty() => {
            (tl!("Waiting for another message to be opened first…").to_string(), t.text_dim, false, false)
        }
        Decryption::Waiting(_) => return,
        Decryption::Opening(_) => (tl!("Opening this message…").to_string(), t.text_dim, false, false),
        Decryption::Opened(_) => {
            if app.session.decrypted_authenticated(m.id) == Some(false) {
                extra.push(
                    tl!("It was sent with an older encryption that can't tell whether it was changed on the way, so its pictures are never downloaded.")
                        .to_string(),
                );
            }
            extra.extend(outside_said(app.session.decrypted_outside(m.id)));
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
    let mut owner_opens = false;
    let shown = notice(hue, &t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = egui::vec2(6.0, 3.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(tl!("Encrypted")).font(semibold(11.5)).color(t.text));
            ui.add(egui::Label::new(egui::RichText::new(&lead).font(regular(11.5)).color(t.text)).wrap());
            if retry && app.session.enabled("mail.decryptAgain") {
                again = ui.small_button(tl!("Try Again")).on_hover_text(tl!("Try to open this message again with your keys")).clicked();
            }
            if check {
                check_again = ui.small_button(tl!("Check Again")).on_hover_text(tl!("Download the bytes this message came as again")).clicked();
            }
            if open {
                let resp = ui.small_button(tl!("Open")).on_hover_text(tl!("Open this message with your keys"));
                if place == Place::Pane {
                    app.hit.add("openEncrypted", m.id, resp.rect);
                }
                // (Not for a click the control channel injected: this is the owner's to say.)
                owner_opens = resp.clicked() && !app.driven;
            }
        });
        for line in &extra {
            ui.add(egui::Label::new(egui::RichText::new(line).font(regular(11.0)).color(t.text_dim)).wrap());
        }
    });
    if place == Place::Pane {
        app.hit.add("encryption", m.id, shown.response.rect);
    }
    if owner_opens {
        app.owner_opens(m.id);
    }
    if again {
        let _ = app.run("mail.decryptAgain", json!({"id": m.id}));
    }
    if check_again {
        let _ = app.run("mail.checkAgain", json!({"id": m.id}));
    }
}
