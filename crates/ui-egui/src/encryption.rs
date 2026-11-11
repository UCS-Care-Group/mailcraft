// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypt in the window (#30 part 7): what a message being written will be at Send, shown before it
//! (R16a), and the stop when Encrypt is on and a recipient has no Public key (R18): Send
//! unencrypted, Remove, or Cancel.
//!
//! The window only asks and shows: the engine decides ([`mailcraft_engine::cmd::encrypting`]) and
//! the host encrypts. Sending in the clear is the owner's choice, made here with a click:
//! Send unencrypted turns Encrypt off for that message (`compose.encrypt`, which an agent can't do)
//! and sends it. Nothing here is ever a command's params that an agent could send.

use std::hash::{Hash, Hasher};

use egui::Ui;
use mailcraft_engine::cmd::encrypting::{self, Will};
use mailcraft_model::digital_signature::SignatureFormat;
use serde_json::json;

use crate::MailApp;
use crate::dialogs::button_row;
use crate::theme::Tokens;

/// How wide the stop is.
pub(crate) const WIDTH: f32 = 460.0;
/// The most recipients the stop names.
const MAX_NAMED: usize = 12;

/// What Encrypt will do to the message being written at Send, as the window shows it.
pub(crate) fn shown(app: &mut MailApp) -> Option<Will> {
    let c = app.session.compose.as_ref()?;
    let account = app.session.mailbox.account(c.account).or(app.session.mailbox.default_account())?;
    // (Nothing to work out, and nothing shown, for a message that isn't asked to be Encrypted.)
    if c.encrypt.is_none() && !account.encryption.when_possible {
        app.encryption_shown = None;
        return None;
    }
    if c.encrypt == Some(false) {
        app.encryption_shown = None;
        return None;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (&c.to, &c.cc, &c.bcc, c.account, c.sign, c.encrypt, account.encryption.when_possible, &account.email).hash(&mut hasher);
    account.signing.key.as_ref().map(|k| (&k.fingerprint, k.format as u8)).hash(&mut hasher);
    app.session.mailbox.public_keys.revision().hash(&mut hasher);
    // (Keys that expire do so by the hour.)
    (app.session.now() / 3_600).hash(&mut hasher);
    let key = hasher.finish();
    if let Some((k, will)) = &app.encryption_shown
        && *k == key
    {
        return Some(will.clone());
    }
    let will = encrypting::will_encrypt(&app.session, c);
    app.encryption_shown = Some((key, will.clone()));
    Some(will)
}

/// The words for a [`Will`] and whether it is a problem: nothing for one that goes as it is.
pub(crate) fn words(will: &Will) -> Option<(String, bool)> {
    let form = |f: SignatureFormat| match f {
        SignatureFormat::Smime => "S/MIME",
        SignatureFormat::OpenPgp => "OpenPGP",
    };
    match will {
        Will::Clear => None,
        Will::Encrypted { to, unverified } => {
            let others = to.recipients.iter().filter(|r| !r.own).count();
            let mut text = format!(
                "{} ({})",
                tln!(others, "Encrypted at Send to {n} recipient and you", "Encrypted at Send to {n} recipients and you"),
                form(to.format)
            );
            if !unverified.is_empty() {
                text.push_str(" · ");
                text.push_str(&crate::i18n::fmt(
                    tl!("keys of {names} came with their mail and weren't checked by you"),
                    &[("names", &named(unverified))],
                ));
            }
            Some((text, false))
        }
        Will::Missing { addresses, .. } => {
            Some((crate::i18n::fmt(tl!("Can't be Encrypted: no Public key for {names}"), &[("names", &named(addresses))]), true))
        }
        Will::Refused(why) => Some((crate::i18n::t(why).to_string(), true)),
    }
}

/// Some addresses, as many as fit, with how many more.
fn named(addresses: &[String]) -> String {
    let mut shown: Vec<&str> = addresses.iter().take(MAX_NAMED).map(String::as_str).collect();
    let more = addresses.len().saturating_sub(shown.len());
    if shown.is_empty() {
        shown.push("");
    }
    let mut text = shown.join(", ");
    if more > 0 {
        text.push_str(&format!(" +{more}"));
    }
    text
}

/// The stop: Encrypt is on and these recipients have no Public key.
pub struct Stop {
    addresses: Vec<String>,
}

/// The stop for the message being written, when pressing Send would be stopped by a recipient with
/// no key (R18): `None` when it isn't (the engine would send it, or refuse for another reason).
pub(crate) fn before_send(app: &MailApp) -> Option<Stop> {
    let c = app.session.compose.as_ref()?;
    match encrypting::will_encrypt(&app.session, c) {
        Will::Missing { addresses, .. } => Some(Stop { addresses }),
        _ => None,
    }
}

impl Stop {
    /// Draw it; whether it is done.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui, _enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        ui.label(tl!("These recipients have no Public key, so the message can't be Encrypted for them:"));
        ui.add_space(4.0);
        for a in self.addresses.iter().take(MAX_NAMED) {
            ui.label(egui::RichText::new(a.as_str()).strong());
        }
        if self.addresses.len() > MAX_NAMED {
            ui.label(egui::RichText::new(format!("+{}", self.addresses.len() - MAX_NAMED)).color(t.text_dim));
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(tl!("Remove them, or send the message without encryption: anybody on the way can read it."))
                .color(t.text_dim)
                .size(11.5),
        );
        ui.add_space(6.0);
        let (mut done, mut unencrypted, mut remove) = (false, false, false);
        // Cancel is the one Enter and Escape reach: sending in the clear takes a deliberate click.
        button_row(ui, |ui| {
            if ui.add(egui::Button::new(tl!("Cancel")).min_size(egui::vec2(72.0, 26.0))).clicked() {
                done = true;
            }
            if ui.add(egui::Button::new(tl!("Remove")).min_size(egui::vec2(72.0, 26.0))).clicked() {
                remove = true;
            }
            if ui.add(egui::Button::new(tl!("Send unencrypted")).min_size(egui::vec2(72.0, 26.0))).clicked() {
                unencrypted = true;
            }
        });
        if remove {
            let _ = app.execute("compose.removeRecipients", json!({"addresses": self.addresses}));
            done = true;
        }
        if unencrypted {
            // The owner's own choice, made here: Encrypt is turned off for this message, and it goes.
            if app.execute("compose.encrypt", json!({"value": false})).is_ok() {
                let _ = app.run("compose.send", json!({}));
            }
            done = true;
        }
        done
    }
}
