// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Account Settings › Sender addresses (#797): the other addresses an IMAP account sends from
//! over the same sign-in, each with the name recipients see and the signature it starts with.
//! Saved by `account.setSenderAddresses`, which checks them.

use egui::{RichText, TextEdit, Ui};
use mailcraft_model::{Account, Id, MAX_ADDRESS, MAX_NAME, MAX_SENDER_ADDRESSES, SenderAddress, SenderProblem, clean_sender_addresses};
use serde_json::{Value, json};

use super::{FIELD_MARGIN, caption, field_id};
use crate::forms::kit::shown;
use crate::i18n::fmt;
use crate::theme::Tokens;

/// One address as the page has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) email: String,
    pub(crate) name: String,
    /// A signature's name; empty for the account's.
    pub(crate) signature: String,
}

/// The addresses as they were, and as the page has them now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Senders {
    pub(crate) rows: Vec<Row>,
    was: Vec<Row>,
}

impl Senders {
    /// For an account that sends from other addresses; `None` for any other.
    pub(crate) fn of(a: &Account) -> Option<Senders> {
        if !a.takes_sender_addresses() {
            return None;
        }
        let rows: Vec<Row> =
            a.sender_addresses.iter().map(|s| Row { email: s.email.clone(), name: s.display_name.clone(), signature: s.signature.clone() }).collect();
        Some(Senders { was: rows.clone(), rows })
    }

    /// The rows that say something: an empty one the owner added and left is no address (one with
    /// only a signature chosen says something, and is refused for its missing address).
    fn filled(&self) -> Vec<&Row> {
        self.rows.iter().filter(|r| !(r.email.trim().is_empty() && r.name.trim().is_empty() && r.signature.is_empty())).collect()
    }

    /// `account.setSenderAddresses` for account `id`, when the page changed them.
    pub(crate) fn command(&self, id: Id) -> Option<(&'static str, Value)> {
        let now = self.filled();
        if now.len() == self.was.len() && now.iter().zip(&self.was).all(|(a, b)| *a == b) {
            return None;
        }
        let list: Vec<Value> = now.iter().map(|r| json!({"email": r.email.trim(), "displayName": r.name.trim(), "signature": r.signature})).collect();
        Some(("account.setSenderAddresses", json!({"id": id, "addresses": list})))
    }

    /// Whether the page changed them since they were opened or last saved.
    pub(crate) fn changed(&self) -> bool {
        self.command(0).is_some()
    }

    /// Why `account.setSenderAddresses` would refuse what the page has, checked before Save
    /// changes anything else, so that a refusal leaves the account as it was. `main` is the
    /// account's own address; `signatures` the names of the signatures there are.
    pub(crate) fn problem(&self, main: &str, signatures: &[String]) -> Option<String> {
        if !self.changed() {
            return None;
        }
        let typed: Vec<SenderAddress> = self
            .filled()
            .iter()
            .map(|r| SenderAddress { email: r.email.clone(), display_name: r.name.clone(), signature: r.signature.clone() })
            .collect();
        let kept = match clean_sender_addresses(main, &typed) {
            Ok(kept) => kept,
            Err(SenderProblem::NotAnAddress(address)) if address.trim().is_empty() => {
                return Some(tl!("a sender address has no e-mail address").to_string());
            }
            Err(SenderProblem::NotAnAddress(address)) => {
                return Some(fmt(tl!("\"{address}\" isn't an e-mail address"), &[("address", &shown(&address, 80))]));
            }
            Err(SenderProblem::TooMany) => {
                return Some(fmt(tl!("an account has {max} sender addresses at most"), &[("max", &MAX_SENDER_ADDRESSES.to_string())]));
            }
        };
        // (A signature removed since the page was opened.)
        kept.iter()
            .map(|x| x.signature.trim())
            .find(|name| !name.is_empty() && !signatures.iter().any(|s| s.eq_ignore_ascii_case(name)))
            .map(|name| fmt(tl!("no signature named \"{name}\""), &[("name", &shown(name, 80))]))
    }

    /// Saved: what the page has is what the account has.
    pub(crate) fn saved(&mut self) {
        self.rows.retain(|r| !(r.email.trim().is_empty() && r.name.trim().is_empty() && r.signature.is_empty()));
        self.was = self.rows.clone();
    }

    /// The section: a row a line for each, a way to take one out, and one to add another.
    pub(crate) fn show(&mut self, ui: &mut Ui, t: &Tokens, signatures: &[String]) {
        caption(ui, tl!("Other addresses you send from"), t);
        ui.label(
            RichText::new(tl!(
                "Addresses your mail server lets this account send from with the same sign-in, such as aliases. Replies are sent from the address a message came to."
            ))
            .color(t.text_dim),
        );
        let mut gone = None;
        for (at, row) in self.rows.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                let gap = ui.spacing().item_spacing.x;
                let room = (ui.available_width() - 150.0 - 28.0 - 3.0 * gap).max(160.0);
                let email = TextEdit::singleline(&mut row.email)
                    .id(field_id(("account", "sender", at, "email")))
                    .desired_width(room * 0.6)
                    .margin(FIELD_MARGIN)
                    .char_limit(MAX_ADDRESS)
                    .hint_text(tl!("Address"));
                crate::text_menu::field(ui.add(email)).on_hover_text(tl!("An address this account may send from"));
                let name = TextEdit::singleline(&mut row.name)
                    .id(field_id(("account", "sender", at, "name")))
                    .desired_width(room * 0.4)
                    .margin(FIELD_MARGIN)
                    .char_limit(MAX_NAME)
                    .hint_text(tl!("Name (optional)"));
                crate::text_menu::field(ui.add(name)).on_hover_text(tl!("The name recipients see; your own when empty"));
                let shown = if row.signature.is_empty() { tl!("Account's signature").to_string() } else { row.signature.clone() };
                egui::ComboBox::from_id_salt(("account", "sender", at, "signature")).width(150.0).selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut row.signature, String::new(), tl!("Account's signature"));
                    for name in signatures {
                        ui.selectable_value(&mut row.signature, name.clone(), name.as_str());
                    }
                });
                if ui.small_button("✕").on_hover_text(tl!("Remove this address")).clicked() {
                    gone = Some(at);
                }
            });
        }
        if let Some(at) = gone {
            self.rows.remove(at);
        }
        if self.rows.len() < MAX_SENDER_ADDRESSES && ui.button(tl!("Add address")).clicked() {
            self.rows.push(Row::default());
        }
    }
}
