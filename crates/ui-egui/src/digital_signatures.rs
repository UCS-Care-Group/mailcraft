// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital Signatures in the window (#30 part 5): which of the owner's certificates or OpenPGP
//! keys an account signs with, and whether it always signs; and the PIN of a smartcard that
//! signs, when its reader has no PIN pad of its own.
//!
//! The keys are the host's to find ([`SigningHost::keys`]: the system's certificate store, the
//! keychain, PKCS#11 tokens, gpg-agent), on a thread of its own; the dialog shows them and runs
//! `account.setSigning` with the one chosen, by its fingerprint. No private key, PIN or
//! passphrase ever passes through here but a PIN typed into [`Dialog::SigningPin`], which goes
//! straight to the host, is held there in memory for the session and is never a command's
//! params, a log line or a file. The browser build has no host for this: nothing is offered.

use std::sync::mpsc::Receiver;

use egui::Ui;
use mailcraft_model::Id;
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use serde_json::json;

use crate::MailApp;
use crate::own_keys::KeyPlace;
use crate::theme::Tokens;

/// How wide the dialog is.
pub(crate) const WIDTH: f32 = 520.0;
/// The most keys shown.
const MAX_SHOWN: usize = 64;

/// One of the owner's keys that can sign, as the host found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigningChoice {
    pub key: SigningKey,
    /// The addresses it is for.
    pub addresses: Vec<String>,
    /// Where it is, as File › Options › Security says it.
    pub place: KeyPlace,
    /// Until when it is valid (seconds since 1970); 0 when it doesn't say.
    pub expires: i64,
}

/// The host's side of signing ([`crate::Services::signing`]): finding the owner's keys, and the
/// PIN of a smartcard that wants one.
pub trait SigningHost: Send + Sync {
    /// The owner's keys that can sign mail now, found on a thread of the host's own; why there
    /// are none when they can't be looked for.
    fn keys(&self) -> Receiver<Result<Vec<SigningChoice>, String>>;
    /// The smartcard (its name) a signature was refused for because it needs its PIN, when one
    /// did since the owner was last asked.
    fn pin_wanted(&self) -> Option<String>;
    /// The PIN the owner typed for that smartcard: held by the host in memory for the session.
    fn give_pin(&self, pin: String);
    /// The owner closed the question without a PIN.
    fn no_pin(&self);
}

/// The Digital Signatures dialog of one account.
pub struct DigitalSignatures {
    pub account: Id,
    keys: Option<Result<Vec<SigningChoice>, String>>,
    waiting: Option<Receiver<Result<Vec<SigningChoice>, String>>>,
    chosen: Option<SigningKey>,
    always: bool,
    error: String,
}

impl DigitalSignatures {
    /// The dialog for `account`, as it is set now; its keys asked of the host.
    pub(crate) fn open(app: &MailApp, account: Id) -> Option<DigitalSignatures> {
        let a = app.session.mailbox.account(account)?;
        if !mailcraft_engine::cmd::signing::can_sign(a) {
            return None;
        }
        let waiting = app.services.signing.as_ref().map(|host| host.keys());
        let keys = if waiting.is_some() { None } else { Some(Err(tl!("Digital signatures are made by the desktop program.").to_string())) };
        Some(DigitalSignatures { account, keys, waiting, chosen: a.signing.key.clone(), always: a.signing.always, error: String::new() })
    }

    /// Draw it; whether it is done.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        if let Some(rx) = &self.waiting {
            match rx.try_recv() {
                Ok(found) => {
                    self.keys = Some(found);
                    self.waiting = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.keys = Some(Err(tl!("The certificates and keys couldn't be looked for.").to_string()));
                    self.waiting = None;
                }
            }
        }
        let email = app.session.mailbox.account(self.account).map(|a| a.email.clone()).unwrap_or_default();
        ui.label(crate::i18n::fmt(
            tl!("A Digital signature shows that a message from {account} came from you and wasn't changed on the way."),
            &[("account", &email)],
        ));
        ui.checkbox(&mut self.always, tl!("Always add a Digital signature to messages from this account"));
        ui.label(egui::RichText::new(tl!("Sign with")).strong());
        match &self.keys {
            None => {
                ui.label(egui::RichText::new(tl!("Looking for your certificates and keys…")).color(t.text_dim));
            }
            Some(Err(why)) => {
                ui.label(egui::RichText::new(why.as_str()).color(t.red));
            }
            Some(Ok(keys)) if keys.is_empty() => {
                ui.label(
                    egui::RichText::new(tl!(
                        "No certificate or OpenPGP key that can sign was found on this computer: an S/MIME certificate with its private key in the system's certificate store (or on a smartcard), or an OpenPGP key in GnuPG."
                    ))
                    .color(t.text_dim),
                );
            }
            Some(Ok(keys)) => {
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    for choice in keys.iter().take(MAX_SHOWN) {
                        let picked = self.chosen.as_ref().is_some_and(|k| k.format == choice.key.format && k.fingerprint == choice.key.fingerprint);
                        let kind = match choice.key.format {
                            SignatureFormat::Smime => tl!("S/MIME certificate"),
                            SignatureFormat::OpenPgp => tl!("OpenPGP key"),
                        };
                        let name = if choice.key.label.is_empty() { choice.addresses.join(", ") } else { choice.key.label.clone() };
                        if ui.radio(picked, format!("{name} — {kind}")).clicked() {
                            self.chosen = Some(choice.key.clone());
                        }
                        let mut details = vec![crate::security::place_name(&choice.place)];
                        if !choice.addresses.is_empty() {
                            details.push(choice.addresses.join(", "));
                        }
                        if choice.expires > 0 {
                            details.push(crate::i18n::fmt(
                                tl!("valid until {date}"),
                                &[("date", &crate::dates::medium(mailcraft_model::time::Date::from_days(choice.expires.div_euclid(86_400))))],
                            ));
                        }
                        let fingerprint: String = choice.key.fingerprint.chars().take(40).collect();
                        details.push(fingerprint);
                        ui.label(egui::RichText::new(details.join(" · ")).color(t.text_dim).size(11.0));
                        if !email.is_empty() && !choice.addresses.iter().any(|a| a.eq_ignore_ascii_case(&email)) {
                            ui.label(
                                egui::RichText::new(tl!("It isn't for this account's address: recipients will see the signer as not verified."))
                                    .color(t.red)
                                    .size(11.0),
                            );
                        }
                    }
                });
                // The one chosen, said again under the list, where it can't be scrolled out of
                // sight: a key for another of the owner's addresses signs, but the recipients'
                // programs say the signer isn't who the message is from.
                let other = self
                    .chosen
                    .as_ref()
                    .and_then(|k| keys.iter().find(|c| c.key.format == k.format && c.key.fingerprint == k.fingerprint))
                    .filter(|c| !email.is_empty() && !c.addresses.iter().any(|a| a.eq_ignore_ascii_case(&email)));
                if let Some(choice) = other {
                    let text = if choice.addresses.is_empty() {
                        crate::i18n::fmt(
                            tl!("The chosen key names no e-mail address, so it isn't for {account}: recipients will see the signer as not verified."),
                            &[("account", &email)],
                        )
                    } else {
                        crate::i18n::fmt(
                            tl!("The chosen key is for {addresses}, not {account}: recipients will see the signer as not verified."),
                            &[("addresses", &choice.addresses.join(", ")), ("account", &email)],
                        )
                    };
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(text).color(t.red).strong());
                }
            }
        }
        if let Some(chosen) =
            self.chosen.as_ref().filter(|k| !self.keys.as_ref().is_some_and(|r| r.as_ref().is_ok_and(|ks| ks.iter().any(|c| c.key == **k))))
        {
            let name = if chosen.label.is_empty() { chosen.fingerprint.chars().take(16).collect() } else { chosen.label.clone() };
            ui.label(egui::RichText::new(crate::i18n::fmt(tl!("Chosen now: {key}"), &[("key", &name)])).color(t.text_dim).size(11.0));
        }
        if !self.error.is_empty() {
            ui.label(egui::RichText::new(self.error.as_str()).color(t.red));
        }
        let (ok, cancel) = crate::dialogs::buttons(ui, tl!("OK"), !self.always || self.chosen.is_some());
        if ok || (enter && (!self.always || self.chosen.is_some())) {
            let key = self.chosen.as_ref().map(|k| json!({"format": k.format.as_str(), "fingerprint": k.fingerprint, "label": k.label}));
            match app.execute("account.setSigning", json!({"account": self.account, "key": key, "always": self.always})) {
                Ok(_) => return true,
                Err(e) => self.error = e,
            }
        }
        cancel
    }
}

/// The question for a smartcard's PIN.
pub struct SigningPin {
    pub token: String,
    /// What is typed: overwritten when it is let go of.
    pin: zeroize::Zeroizing<String>,
}

impl SigningPin {
    pub(crate) fn new(token: String) -> SigningPin {
        SigningPin { token, pin: zeroize::Zeroizing::new(String::new()) }
    }

    /// Draw it; whether it is done.
    pub(crate) fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        ui.label(crate::i18n::fmt(tl!("The smartcard {token} needs its PIN to add a Digital signature."), &[("token", &self.token)]));
        let resp = crate::text_menu::secret(ui.add(egui::TextEdit::singleline(&mut *self.pin).password(true).desired_width(f32::INFINITY)));
        crate::text_menu::hold(&resp);
        ui.label(
            egui::RichText::new(tl!("It is held in memory until MailCraft closes, and never saved. Send Again sends the message that waits for it."))
                .color(t.text_dim)
                .size(11.0),
        );
        let (ok, cancel) = crate::dialogs::buttons(ui, tl!("OK"), !self.pin.is_empty());
        if (ok || enter) && !self.pin.is_empty() {
            if let Some(host) = &app.services.signing {
                // Moved, not copied: the host keeps it wiped on drop.
                host.give_pin(std::mem::take(&mut *self.pin));
            }
            return true;
        }
        if cancel {
            self.close(app);
        }
        cancel
    }

    /// Closed without a PIN: the host is told, and what was typed is let go of.
    pub(crate) fn close(&mut self, app: &MailApp) {
        self.pin.clear();
        if let Some(host) = &app.services.signing {
            host.no_pin();
        }
    }
}

impl MailApp {
    /// Ask for a smartcard's PIN when a signature wanted one and nothing else is open.
    pub(crate) fn ask_for_signing_pin(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        if let Some(token) = self.services.signing.as_ref().and_then(|host| host.pin_wanted()) {
            self.dialog = Some(crate::dialogs::Dialog::SigningPin(Box::new(SigningPin::new(token))));
        }
    }
}
