// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail rules: conditions on an arriving message and what to do with it.
//!
//! **Whose rule (#566).** A rule is for the arriving mail of one account ([`Rule::account`]). A
//! rule from before rules had accounts has none: it is for every account's mail, and is kept on
//! this computer until the owner moves it to an account's server.
//!
//! **Where it is kept.** A rule of a Microsoft account can be kept on its server ([`Keep::Server`]),
//! which runs it whether MailCraft is open or not, and shows it in Outlook on the web and on the
//! phone. Once the server has it ([`Rule::server`]) it never runs here on arriving mail: the
//! server has run it already. Until then it runs here, as every rule kept on this computer does.
//! A rule made in Outlook or on the web is the server's too; one with parts this version doesn't
//! know ([`ServerRule::unknown`]) is switched on and off, moved and deleted, never rewritten and
//! never run here.

use serde::{Deserialize, Serialize};

use crate::{Id, Importance, Message, contains_ci};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Condition {
    /// The sender's name or address contains the text (a Microsoft account's rule: the address
    /// alone, as its server reads it).
    FromContains(String),
    /// Any To/Cc recipient's name or address contains the text (a Microsoft account's rule: the
    /// addresses alone).
    ToContains(String),
    SubjectContains(String),
    BodyContains(String),
    HasAttachment,
    Importance(Importance),
}

impl Condition {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_as(m, false)
    }

    /// [`Condition::matches`], with `addresses_only` for a rule of an account whose server keeps
    /// rules: its server looks for the words of a sender or a recipient in their addresses
    /// alone (Exchange's "words in the sender's address"), and so does this computer for it.
    pub fn matches_as(&self, m: &Message, addresses_only: bool) -> bool {
        let person = |a: &crate::Address, t: &str| contains_ci(&a.email, t) || (!addresses_only && contains_ci(&a.name, t));
        match self {
            Condition::FromContains(t) => person(&m.from, t),
            Condition::ToContains(t) => m.to.iter().chain(&m.cc).any(|a| person(a, t)),
            Condition::SubjectContains(t) => contains_ci(&m.subject, t),
            Condition::BodyContains(t) => contains_ci(&m.body.text, t) || contains_ci(&m.hidden_text, t),
            Condition::HasAttachment => m.has_attachments(),
            Condition::Importance(i) => m.importance == *i,
        }
    }

    /// The condition in words: `from contains "newsletter"`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Condition::describe`] for a rule that reads addresses alone ([`Condition::matches_as`]).
    pub fn describe_as(&self, addresses_only: bool) -> String {
        match self {
            Condition::FromContains(t) if addresses_only => format!("sender's address contains \"{t}\""),
            Condition::ToContains(t) if addresses_only => format!("recipient's address contains \"{t}\""),
            Condition::FromContains(t) => format!("from contains \"{t}\""),
            Condition::ToContains(t) => format!("sent to \"{t}\""),
            Condition::SubjectContains(t) => format!("subject contains \"{t}\""),
            Condition::BodyContains(t) => format!("body contains \"{t}\""),
            Condition::HasAttachment => "has an attachment".into(),
            Condition::Importance(i) => format!("importance is {}", i.code()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Action {
    /// Move to the folder with this id.
    MoveTo(Id),
    Categorize(String),
    Flag,
    MarkRead,
    SetImportance(Importance),
    /// Move to Deleted Items.
    Delete,
}

impl Action {
    pub fn describe(&self) -> String {
        match self {
            Action::MoveTo(_) => "move it to a folder".into(),
            Action::Categorize(c) => format!("categorize it as \"{c}\""),
            Action::Flag => "flag it".into(),
            Action::MarkRead => "mark it as read".into(),
            Action::SetImportance(i) => format!("set importance to {}", i.code()),
            Action::Delete => "delete it".into(),
        }
    }
}

/// Where the owner wants a rule kept (#566).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Keep {
    /// On this computer: it runs while MailCraft is open.
    #[default]
    Here,
    /// On its account's server, which runs it whether MailCraft is open or not. Until the server
    /// has it ([`Rule::server`]), it runs here.
    Server,
}

impl Keep {
    pub fn is_here(&self) -> bool {
        *self == Keep::Here
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A rule as its account's server has it (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerRule {
    /// What the server calls it.
    pub id: String,
    /// Where it stands among the server's rules (Graph's `sequence`), as it is to be: a move here
    /// changes it before the server is told.
    pub sequence: i32,
    /// Whether it is switched on, and where it stood, as the server last had it. What was
    /// changed here since is the difference ([`Rule::changed_here`]), and only that is sent.
    pub synced_enabled: bool,
    pub synced_sequence: i32,
    /// The parts of it this version doesn't know, by the server's names (`forwardTo`,
    /// `exceptions`…). Such a rule is switched on and off, moved and deleted, and never
    /// rewritten or run here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown: Vec<String>,
    /// The server doesn't let it be changed or deleted (Graph's `isReadOnly`).
    #[serde(skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// The server says it is in error (Graph's `hasError`).
    #[serde(skip_serializing_if = "is_false")]
    pub error: bool,
}

/// A rule deleted here that its server is yet to be told about (#566).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleGone {
    pub account: Id,
    /// What the server calls it.
    pub id: String,
}

/// Most rules deleted here that wait for their server to be told.
pub const MAX_RULE_GONE: usize = 500;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Rule {
    pub id: Id,
    pub name: String,
    pub enabled: bool,
    /// All must hold. A rule without conditions matches nothing (it would act on every message).
    pub conditions: Vec<Condition>,
    pub actions: Vec<Action>,
    /// Later rules don't run on a message this one matched.
    pub stop: bool,
    /// The account whose arriving mail it is for; 0 for a rule from before rules had accounts,
    /// which is for every account's mail and kept on this computer.
    #[serde(skip_serializing_if = "crate::is_zero_id")]
    pub account: Id,
    /// Where the owner wants it kept.
    #[serde(skip_serializing_if = "Keep::is_here")]
    pub keep: Keep,
    /// The server's copy: for a rule its account's server has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerRule>,
    /// Why the server didn't take it, when it didn't: it is kept here, and said so.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub refused: String,
}

impl Rule {
    pub fn matches(&self, m: &Message) -> bool {
        self.matches_as(m, false)
    }

    /// [`Rule::matches`], reading addresses alone when `addresses_only` ([`Condition::matches_as`]).
    /// A rule with parts this version doesn't know matches nothing here: what it would do is not
    /// known.
    pub fn matches_as(&self, m: &Message, addresses_only: bool) -> bool {
        self.enabled && self.known() && !self.conditions.is_empty() && self.conditions.iter().all(|c| c.matches_as(m, addresses_only))
    }

    /// `When from contains "x" and has an attachment: flag it, mark it as read`.
    pub fn describe(&self) -> String {
        self.describe_as(false)
    }

    /// [`Rule::describe`] for a rule that reads addresses alone.
    pub fn describe_as(&self, addresses_only: bool) -> String {
        let when = self.conditions.iter().map(|c| c.describe_as(addresses_only)).collect::<Vec<_>>().join(" and ");
        let then = self.actions.iter().map(Action::describe).collect::<Vec<_>>().join(", ");
        format!("When {when}: {then}")
    }

    /// Whether it is for the arriving mail of `account`.
    pub fn is_for(&self, account: Id) -> bool {
        self.account == 0 || self.account == account
    }

    /// Whether the server has it.
    pub fn on_server(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty())
    }

    /// Whether it is to go to the server and hasn't yet.
    pub fn waiting(&self) -> bool {
        self.keep == Keep::Server && !self.on_server()
    }

    /// Whether every part of it is known here: it can be shown whole and run here.
    pub fn known(&self) -> bool {
        self.server.as_ref().is_none_or(|s| s.unknown.is_empty())
    }

    /// Whether the server doesn't let it be changed or deleted.
    pub fn read_only(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.read_only)
    }

    /// Whether it was switched on or off, or moved, here since the server last had it.
    pub fn changed_here(&self) -> bool {
        self.server.as_ref().is_some_and(|s| !s.id.is_empty() && (self.enabled != s.synced_enabled || s.sequence != s.synced_sequence))
    }
}
