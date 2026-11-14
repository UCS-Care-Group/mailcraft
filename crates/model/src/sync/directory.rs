// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Names looked up in an Exchange server account's address list (#1262): what a host is asked to
//! look up and what it brings back.
//!
//! The engine builds an [`EwsResolveRequest`] for the names Check Names couldn't match among the
//! owner's contacts, a host asks the account's server for each (`ResolveNames`, the organisation's
//! address list only) and hands back an [`EwsResolveResult`], which is given to
//! `compose.checkNames` as its `found`. People's Address Book asks the same for what the owner
//! searches for, and shows who was found. Nothing of it is kept in the mailbox or written
//! anywhere: who was found lives in the window while the owner chooses or looks, and as the
//! addresses chosen (or a contact the owner makes of somebody, on their say-so).

use serde::{Deserialize, Serialize};

use crate::{Address, Id, MAX_ADDRESS};

/// The most names one request looks up.
pub const MAX_RESOLVE_TEXTS: usize = 10;
/// The most people kept for one name.
pub const MAX_RESOLVE_PEOPLE: usize = 100;
/// The longest text looked up, in characters.
pub const MAX_RESOLVE_TEXT: usize = 256;
/// The longest name, company, department, job title or office kept, in characters.
const MAX_FIELD: usize = 256;
/// The longest phone number kept, in characters.
const MAX_PHONE: usize = 40;

/// What kind of recipient somebody in the address list is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DirectoryKind {
    /// A person's mailbox.
    #[default]
    Mailbox,
    /// A distribution list: the server expands it when mail is sent.
    List,
    /// Somebody outside the organisation, in its address list.
    Contact,
    /// A Microsoft 365 group.
    Group,
    /// A public folder that takes mail.
    PublicFolder,
    /// Any other kind.
    Other,
}

/// Somebody the address list has: a name, an address, and what tells two of one name apart.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DirectoryPerson {
    pub name: String,
    pub email: String,
    pub kind: DirectoryKind,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub company: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub department: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The business phone number.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phone: String,
    /// The mobile phone number.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mobile: String,
    /// Where their office is.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub office: String,
}

impl DirectoryPerson {
    /// The person as hostile data is to be taken: `None` when the address isn't one mail can go to;
    /// else every field one line and bounded.
    pub fn checked(self) -> Option<Self> {
        let email = one_line(&self.email, MAX_ADDRESS.saturating_add(1));
        let address = Address::new(one_line(&self.name, MAX_FIELD), email.as_str());
        let odd = email.contains([',', ';', '<', '>', '"', '(', ')', ' ']) || email.matches('@').count() != 1;
        if !address.is_valid() || odd || email.chars().count() > MAX_ADDRESS {
            return None;
        }
        Some(Self {
            name: address.name,
            email: address.email,
            kind: self.kind,
            company: one_line(&self.company, MAX_FIELD),
            department: one_line(&self.department, MAX_FIELD),
            title: one_line(&self.title, MAX_FIELD),
            phone: phone_number(&self.phone),
            mobile: phone_number(&self.mobile),
            office: one_line(&self.office, MAX_FIELD),
        })
    }

    /// The person as a recipient.
    pub fn address(&self) -> Address {
        Address::new(&self.name, &self.email)
    }
}

/// Who was found is the owner's business: a log line says what kind, never who.
impl std::fmt::Debug for DirectoryPerson {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryPerson").field("kind", &self.kind).finish_non_exhaustive()
    }
}

/// The names to look up in the address list of an Exchange server account, with where its
/// server is, as an [`EwsFetchRequest`](super::EwsFetchRequest) says it. The password goes beside
/// the request and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsResolveRequest {
    pub account: Id,
    /// The account's address.
    pub email: String,
    /// The address of the Exchange Web Services endpoint. Never logged.
    pub url: String,
    /// The name the password is sent with; empty means `email`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The fingerprint of the one certificate the owner trusts for this account; empty when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// The names as typed, each looked up as it is: at most [`MAX_RESOLVE_TEXTS`].
    pub texts: Vec<String>,
}

/// Counts only: no address, and no name looked up.
impl std::fmt::Debug for EwsResolveRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsResolveRequest")
            .field("account", &self.account)
            .field("trusted", &!self.trusted.is_empty())
            .field("texts", &self.texts.len())
            .finish()
    }
}

/// What the address list has for one name looked up.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsResolved {
    /// The name as it was looked up (one of the request's `texts`).
    pub text: String,
    /// The people found, in the server's order, at most [`MAX_RESOLVE_PEOPLE`].
    pub people: Vec<DirectoryPerson>,
    /// The server has more than it listed: the name is to be typed longer.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub more: bool,
}

impl std::fmt::Debug for EwsResolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsResolved").field("people", &self.people).field("more", &self.more).finish_non_exhaustive()
    }
}

/// What a look-up brought back: each name the host got an answer for, and what went wrong, said in
/// words, when something did (a name not answered then is simply missing).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsResolveResult {
    pub account: Id,
    pub found: Vec<EwsResolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The server turned the password down: the window asks for it again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub auth_refused: bool,
}

impl std::fmt::Debug for EwsResolveResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsResolveResult")
            .field("account", &self.account)
            .field("found", &self.found)
            .field("error", &self.error.is_some())
            .field("auth_refused", &self.auth_refused)
            .finish()
    }
}

/// `text` as one line: runs of white space and control characters become one space, the ends are
/// trimmed, and at most `most` characters are kept.
pub fn one_line(text: &str, most: usize) -> String {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    words.join(" ").chars().take(most).collect::<String>().trim_end().to_owned()
}

/// `text` when it is written as a phone number is (digits, at least three, with spaces, `+`, `-`,
/// `.`, `/`, brackets and an extension's `x`, at most [`MAX_PHONE`] characters); else nothing.
pub fn phone_number(text: &str) -> String {
    let number = one_line(text, MAX_PHONE.saturating_add(1));
    let digits = number.chars().filter(char::is_ascii_digit).count();
    let shaped = number.chars().all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '-' | '.' | '/' | '(' | ')' | 'x' | 'X'));
    if shaped && digits >= 3 && number.chars().count() <= MAX_PHONE { number } else { String::new() }
}
