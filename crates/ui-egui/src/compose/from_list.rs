// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the From line of a message offers (#797): each account's own address, in the order of
//! the folder pane (#702), each followed by its other sender addresses.

use mailcraft_model::{Id, Mailbox};

/// One choice of From: the account, the sender address (`""` for its own) and how it is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FromChoice {
    pub(crate) account: Id,
    pub(crate) from: String,
    pub(crate) label: String,
}

/// Every address a message can be from, in order.
pub(crate) fn choices(mb: &Mailbox) -> Vec<FromChoice> {
    let mut out = Vec::new();
    for a in mb.accounts_in_order() {
        out.push(FromChoice { account: a.id, from: String::new(), label: a.address().full() });
        for s in a.sender_addresses() {
            out.push(FromChoice { account: a.id, from: s.email.clone(), label: a.from_address(&s.email).full() });
        }
    }
    out
}

/// Whether there is a choice to offer: more than one account, or an account with sender
/// addresses of its own.
pub(crate) fn has_choice(mb: &Mailbox) -> bool {
    mb.accounts.len() > 1 || mb.accounts.iter().any(|a| a.sender_addresses().next().is_some())
}

/// The From line as it is shown for a message from `account`'s sender address `from`.
pub(crate) fn current(mb: &Mailbox, account: Id, from: &str) -> String {
    mb.account(account).map(|a| a.from_address(from).full()).unwrap_or_default()
}

/// Whether `choice` is what a message from `account`'s `from` is from.
pub(crate) fn is_current(choice: &FromChoice, account: Id, from: &str) -> bool {
    choice.account == account && choice.from.eq_ignore_ascii_case(from)
}
