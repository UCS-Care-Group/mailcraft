// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypt in the window (#30 part 7): what a message being written will be at Send, shown before it
//! (R16a), and the stop when Encrypt is on and a recipient has no Public key (R18): Send
//! unencrypted, Remove, or Cancel; and the same stop when several different keys arrived with mail
//! for a recipient and none is the owner's (which one is theirs to say).
//!
//! The window only asks and shows: the engine decides ([`mailcraft_engine::cmd::encrypting`]) and
//! the host encrypts. Sending in the clear is the owner's choice, made here with a click:
//! Send unencrypted turns Encrypt off for that message (`compose.encrypt`, which an agent can't do)
//! and sends it. Nothing here is ever a command's params that an agent could send, and input the
//! control channel injected (`MailApp::driven`) is never the owner's word for it, as it isn't for
//! marking a key Verified (`security.rs`): the buttons that send in the clear or choose a key do
//! nothing for it, and neither does the ribbon's Encrypt turning Encrypt off.

use std::hash::{Hash, Hasher};

use egui::Ui;
use mailcraft_engine::cmd::encrypting::{self, KeyConflict, Will};
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
        // The account's setting leaves it as it is: said, and why, in the warning colour.
        Will::ClearBecause { addresses } => {
            Some((crate::i18n::fmt(tl!("Goes unencrypted: no single Public key for {names}"), &[("names", &named(addresses))]), true))
        }
        Will::Encrypted { to, unverified, blind } => {
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
            if !blind.is_empty() {
                text.push_str(" · ");
                text.push_str(tl!("each Bcc recipient gets a message of their own"));
            }
            // (The lines of the header go as they are: nothing of them is hidden by encrypting.)
            text.push_str(" · ");
            text.push_str(tl!("the subject and the addresses aren't encrypted"));
            Some((text, false))
        }
        Will::Missing { addresses, .. } => {
            Some((crate::i18n::fmt(tl!("Can't be Encrypted: no Public key for {names}"), &[("names", &named(addresses))]), true))
        }
        Will::Conflict { conflicts, .. } => {
            let addresses: Vec<String> = conflicts.iter().map(|c| c.address.clone()).collect();
            Some((
                crate::i18n::fmt(
                    tl!("Can't be Encrypted yet: several keys came for {names}, and you say which is theirs"),
                    &[("names", &named(&addresses))],
                ),
                true,
            ))
        }
        Will::Refused(why) => Some((crate::i18n::t(why).to_string(), true)),
    }
}

/// What the last save of the message being written as a draft kept, when it was to be Encrypted
/// (#30 part 7): said in the line under its header.
pub(crate) fn draft_note(app: &MailApp) -> Option<String> {
    match app.session.compose.as_ref()?.draft_sealed? {
        true => Some(tl!("the draft is kept Encrypted on this computer, and can't be reopened to edit until MailCraft can decrypt").to_string()),
        false => Some(tl!("the draft is kept on this computer only, as you wrote it, not Encrypted").to_string()),
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

/// The stop: Encrypt is on and these recipients have no Public key, or several keys arrived for
/// them and none is the owner's.
pub struct Stop {
    addresses: Vec<String>,
    conflicts: Vec<KeyConflict>,
    /// The form the keys to choose from are of.
    format: SignatureFormat,
}

/// The stop for the message being written, when pressing Send would be stopped by a recipient with
/// no key (R18) or with keys to choose from: `None` when it isn't (the engine would send it, or
/// refuse for another reason).
pub(crate) fn before_send(app: &MailApp) -> Option<Stop> {
    let c = app.session.compose.as_ref()?;
    match encrypting::will_encrypt(&app.session, c) {
        Will::Missing { format, addresses } => Some(Stop { addresses, conflicts: Vec::new(), format }),
        Will::Conflict { format, conflicts } => Some(Stop { addresses: Vec::new(), conflicts, format }),
        _ => None,
    }
}

/// Whether `id` with `params`, from input the control channel injected, would turn Encrypt off for
/// the message being written: refused, for it is the owner's choice alone (as sending in the clear is).
pub(crate) fn refused_when_driven(app: &MailApp, id: &str, params: &serde_json::Value) -> Option<String> {
    if id != "compose.encrypt" || !app.driven {
        return None;
    }
    let c = app.session.compose.as_ref()?;
    let on = c.encrypt.unwrap_or_else(|| {
        app.session.mailbox.account(c.account).or(app.session.mailbox.default_account()).is_some_and(|a| a.encryption.when_possible)
    });
    let wanted = params.get("value").and_then(serde_json::Value::as_bool).unwrap_or(!on);
    (on && !wanted).then(|| tl!("Encrypt is turned off only by you, not by input sent to the window").to_string())
}

impl Stop {
    /// Send the message being written without encryption: the owner's own choice, made with a click
    /// here. Encrypt is turned off for it and it goes. Input the control channel injected is never
    /// their word for it: nothing happens.
    pub(crate) fn send_unencrypted(app: &mut MailApp) {
        if app.driven {
            return;
        }
        if app.execute("compose.encrypt", json!({"value": false})).is_ok() {
            let _ = app.run("compose.send", json!({}));
        }
    }

    /// Say which of the keys that came for `address` is theirs: the owner's word, in the window
    /// (never a command's, and never input the control channel injected). Whether it was taken.
    pub(crate) fn choose(&self, app: &mut MailApp, address: &str, fingerprint: &str) -> bool {
        !app.driven && app.session.set_key_chosen(address, self.format, fingerprint).is_ok_and(|changed| changed)
    }

    /// Draw it; whether it is done.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui, _enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        let owners = !app.driven;
        if !self.addresses.is_empty() {
            ui.label(tl!("These recipients have no Public key, so the message can't be Encrypted for them:"));
            ui.add_space(4.0);
            for a in self.addresses.iter().take(MAX_NAMED) {
                ui.label(egui::RichText::new(a.as_str()).strong());
            }
            if self.addresses.len() > MAX_NAMED {
                ui.label(egui::RichText::new(format!("+{}", self.addresses.len() - MAX_NAMED)).color(t.text_dim));
            }
            ui.add_space(6.0);
        }
        let mut chosen: Option<(String, String)> = None;
        if !self.conflicts.is_empty() {
            ui.label(tl!("Several different keys came with mail for these recipients, and nothing says which is theirs. Choose the one that is, or send without encryption:"));
            ui.add_space(4.0);
            for conflict in self.conflicts.iter().take(MAX_NAMED) {
                ui.label(egui::RichText::new(conflict.address.as_str()).strong());
                for candidate in conflict.candidates.iter().take(MAX_NAMED) {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(group(&candidate.fingerprint)).font(egui::FontId::monospace(11.0)).color(t.text_dim));
                        ui.label(
                            egui::RichText::new(crate::i18n::fmt(
                                tl!("arrived {date}"),
                                &[(
                                    "date",
                                    &crate::dates::medium(
                                        mailcraft_time::DateTime::from_timestamp(candidate.seen, app.session.offset_at(candidate.seen)).date,
                                    ),
                                )],
                            ))
                            .color(t.text_dim)
                            .size(11.5),
                        );
                        if ui.add_enabled(owners, egui::Button::new(tl!("Use this key"))).clicked() && owners {
                            chosen = Some((conflict.address.clone(), candidate.fingerprint.clone()));
                        }
                    });
                }
            }
            ui.add_space(6.0);
        }
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
        if let Some((address, fingerprint)) = chosen {
            self.choose(app, &address, &fingerprint);
            done = true;
        }
        if remove {
            let gone: Vec<String> = self.addresses.iter().cloned().chain(self.conflicts.iter().map(|c| c.address.clone())).collect();
            let _ = app.execute("compose.removeRecipients", json!({"addresses": gone}));
            done = true;
        }
        if unencrypted {
            Stop::send_unencrypted(app);
            done = true;
        }
        done
    }
}

/// A fingerprint in groups of four, as the owner compares it.
fn group(fingerprint: &str) -> String {
    let digits: Vec<char> = fingerprint.chars().filter(char::is_ascii_hexdigit).collect();
    digits.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}
