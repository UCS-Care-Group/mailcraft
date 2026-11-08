// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's other sender addresses (#797): the aliases and role addresses delivered to the
//! same mailbox (`support@`, an address at another domain of the owner's) that its outgoing
//! server lets it send from over the same sign-in. A message is from one of them when the owner
//! picks it as From, and a reply or forward is from the one the original was sent to.
//!
//! Only an account kept in step by IMAP and sending through SMTP has them
//! ([`Account::takes_sender_addresses`]): a Microsoft account sends as another address only with
//! a permission its server gives, which this does not ask for, and a JMAP account sends nothing
//! yet. Another account's list is kept in its file as it is and never used.

use serde::{Deserialize, Serialize};

use crate::{Account, AccountKind, Address, Message, clean_name, mailbox_address};

/// Most sender addresses one account keeps besides its own.
pub const MAX_SENDER_ADDRESSES: usize = 50;

/// Headers that say which of the owner's addresses a message was delivered to, read after its
/// To and Cc lines, in this order: the address it was sent to before the server took it to the
/// mailbox, then the mailbox's own.
const DELIVERED_TO: [&str; 2] = ["x-original-to", "delivered-to"];

/// Most values of those headers read of one message: a server adds one a hop, and a message is
/// anybody's to write.
const MAX_DELIVERED_TO: usize = 20;

/// One of an account's other sender addresses: the address, the name recipients see beside it
/// (the account's own when empty), and the signature its messages start with (the account's
/// own when empty).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SenderAddress {
    pub email: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    /// A [`crate::Signature`] name, for new messages, replies and forwards alike.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub signature: String,
}

/// Why a list of sender addresses can't be kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SenderProblem {
    /// This isn't an address mail can be sent from.
    NotAnAddress(String),
    /// More than [`MAX_SENDER_ADDRESSES`].
    TooMany,
}

/// `list` made fit to keep: each address checked as a mailbox's own address is
/// ([`mailbox_address`]), once whatever its letter case, never the account's own `main`; the names
/// on one line. An address that isn't one is refused, not dropped: the owner typed it.
pub fn clean_sender_addresses(main: &str, list: &[SenderAddress]) -> Result<Vec<SenderAddress>, SenderProblem> {
    let mut kept: Vec<SenderAddress> = Vec::new();
    for entry in list {
        let typed = entry.email.trim();
        let email =
            mailbox_address(typed).ok_or_else(|| SenderProblem::NotAnAddress(typed.chars().filter(|c| !c.is_control()).take(80).collect()))?;
        if email.eq_ignore_ascii_case(main.trim()) || kept.iter().any(|k| k.email.eq_ignore_ascii_case(&email)) {
            continue;
        }
        if kept.len() >= MAX_SENDER_ADDRESSES {
            return Err(SenderProblem::TooMany);
        }
        let display_name = clean_name(&entry.display_name).unwrap_or_default();
        let signature = clean_name(&entry.signature).unwrap_or_default();
        kept.push(SenderAddress { email, display_name, signature });
    }
    Ok(kept)
}

impl Account {
    /// Whether the account sends from other addresses of its own: one kept in step by IMAP that
    /// sends through SMTP.
    pub fn takes_sender_addresses(&self) -> bool {
        self.kind == AccountKind::Imap && !self.is_shared()
    }

    /// Its other sender addresses that are used: none for an account that doesn't take them, and
    /// of a list read from a file only those that are still addresses, at most
    /// [`MAX_SENDER_ADDRESSES`] (a file is anybody's to write).
    pub fn sender_addresses(&self) -> impl Iterator<Item = &SenderAddress> {
        let main = self.email.trim();
        self.sender_addresses
            .iter()
            .filter(move |s| self.takes_sender_addresses() && mailbox_address(&s.email).is_some() && !s.email.eq_ignore_ascii_case(main))
            .take(MAX_SENDER_ADDRESSES)
    }

    /// The other sender address `email` is (any letter case), if it is one that is used.
    pub fn sender_address(&self, email: &str) -> Option<&SenderAddress> {
        let email = email.trim();
        if email.is_empty() {
            return None;
        }
        self.sender_addresses().find(|s| s.email.eq_ignore_ascii_case(email))
    }

    /// Who a message from `email` is from: that sender address with its name (the account's
    /// name when it has none), or the account's own address for `""`, its own address, or one
    /// that isn't among its sender addresses.
    pub fn from_address(&self, email: &str) -> Address {
        match self.sender_address(email) {
            Some(s) => Address::new(if s.display_name.is_empty() { &self.display_name } else { &s.display_name }, s.email.clone()),
            None => self.address(),
        }
    }

    /// Whether `email` is the account's own address or one of its sender addresses that is used,
    /// whatever its letter case.
    pub fn sends_as(&self, email: &str) -> bool {
        let email = email.trim();
        !email.is_empty() && (email.eq_ignore_ascii_case(self.email.trim()) || self.sender_address(email).is_some())
    }

    /// Which of its sender addresses a reply to, or forward of, `m` is from (#797): the first of
    /// the account's addresses found among the original's From (a message the owner sent from
    /// one), To and Cc; else the first of its sender addresses in the original's `X-Original-To`
    /// and `Delivered-To` headers (mail to an alias in Bcc; the mailbox's own address there,
    /// which a server writes whichever address the mail was for, says nothing). The address as
    /// the account keeps it, or `""` for its own address, also when none is found. Only whole
    /// addresses match, in any letter case: `ada+news@` is not `ada@`.
    pub fn reply_from(&self, m: &Message) -> String {
        if self.sender_addresses().next().is_none() {
            return String::new();
        }
        for a in std::iter::once(&m.from).chain(m.to.iter()).chain(m.cc.iter()) {
            if a.email.trim().eq_ignore_ascii_case(self.email.trim()) {
                return String::new();
            }
            if let Some(s) = self.sender_address(&a.email) {
                return s.email.clone();
            }
        }
        DELIVERED_TO
            .iter()
            .flat_map(|name| m.headers.iter().filter(move |(n, _)| n.trim().eq_ignore_ascii_case(name)))
            .take(MAX_DELIVERED_TO)
            .filter_map(|(_, value)| Address::parse(value))
            .find_map(|a| self.sender_address(&a.email).map(|s| s.email.clone()))
            .unwrap_or_default()
    }
}
