// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules that a Microsoft account's server keeps (#566, Graph's `messageRules`), as plain data
//! like [`crate::remote_tasks`]: a host with sockets brings a [`RulesDownload`] and takes a
//! [`RulesUpload`] to the server, and the engine merges the one and works out the other
//! (`sync.applyRules`, `sync.ruleChanges`, `sync.applyRuleChanges`).
//!
//! All of it is hostile: it comes from a server, or from a command's parameters.
//! [`RemoteRule::tidy`] cuts a rule to size before any of it is kept.

use serde::{Deserialize, Serialize};

use crate::remote::{key, line};
use crate::{Account, Action, Condition, Id, MAX_NAME, MAX_RULES, clean_name};

/// Most rules one download is taken to hold; what is beyond is left out and the download is not
/// complete.
pub const MAX_SERVER_RULES: usize = MAX_RULES;
/// Most changes one [`RulesUpload`] carries (the rest go with the next).
pub const MAX_RULE_OPS: usize = 100;
/// Most conditions and actions of one rule, longest text a condition looks for, most parts named
/// as unknown, and the longest name of one.
pub const MAX_PARTS: usize = 20;
pub const MAX_WORDS: usize = 200;
const MAX_UNKNOWN: usize = 20;
const MAX_PART_NAME: usize = 64;
/// Longest text kept of why a server didn't take a change.
const MAX_WHY: usize = 300;

/// A rule as a server has it. Its conditions and actions are those this version knows; a part it
/// doesn't is named in `unknown`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RemoteRule {
    /// What the server calls it; a rule without one is not merged.
    pub id: String,
    pub name: String,
    /// Where it stands among the server's rules.
    pub sequence: i32,
    pub enabled: bool,
    /// The server doesn't let it be changed or deleted.
    pub read_only: bool,
    /// The server says it is in error.
    pub error: bool,
    pub conditions: Vec<Condition>,
    /// What it does, but for a move ([`RemoteRule::move_to`]): never an [`Action::MoveTo`].
    pub actions: Vec<Action>,
    /// What the server calls the folder it moves mail to; empty for none.
    pub move_to: String,
    pub stop: bool,
    /// The parts this version doesn't know, by the server's names.
    pub unknown: Vec<String>,
}

impl RemoteRule {
    /// Cut everything to the size the mailbox keeps.
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.move_to = key(&self.move_to);
        self.name = clean_name(&line(&self.name, MAX_NAME)).unwrap_or_else(|| "(no name)".into());
        let words = |t: &str| line(t, MAX_WORDS);
        let mut conditions = Vec::new();
        for condition in std::mem::take(&mut self.conditions).into_iter().take(MAX_PARTS) {
            let tidied = match condition {
                Condition::FromContains(t) => Condition::FromContains(words(&t)),
                Condition::ToContains(t) => Condition::ToContains(words(&t)),
                Condition::SubjectContains(t) => Condition::SubjectContains(words(&t)),
                Condition::BodyContains(t) => Condition::BodyContains(words(&t)),
                other => other,
            };
            let empty = matches!(&tidied, Condition::FromContains(t) | Condition::ToContains(t) | Condition::SubjectContains(t) | Condition::BodyContains(t) if t.is_empty());
            if !empty {
                conditions.push(tidied);
            }
        }
        self.conditions = conditions;
        let mut actions = Vec::new();
        for action in std::mem::take(&mut self.actions).into_iter().take(MAX_PARTS) {
            match action {
                Action::MoveTo(_) => {}
                Action::Categorize(c) => {
                    if let Some(c) = clean_name(&c) {
                        actions.push(Action::Categorize(c));
                    }
                }
                other => actions.push(other),
            }
        }
        self.actions = actions;
        let mut unknown: Vec<String> = Vec::new();
        for part in std::mem::take(&mut self.unknown) {
            let part = line(&part, MAX_PART_NAME);
            if !part.is_empty() && !unknown.contains(&part) && unknown.len() < MAX_UNKNOWN {
                unknown.push(part);
            }
        }
        self.unknown = unknown;
    }
}

/// An account's rules, as its server has them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RulesDownload {
    /// The account whose rules they are ([`RulesDownload::for_account`]).
    pub account: Id,
    /// That account's address (a download is not taken in by another account that happens to
    /// have the id).
    pub email: String,
    /// Every rule the server has is in `rules`. When not, nothing is removed for not being here.
    pub complete: bool,
    pub rules: Vec<RemoteRule>,
}

impl RulesDownload {
    pub fn for_account(mut self, account: &Account) -> RulesDownload {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}

/// What is done to a rule on a server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleOpKind {
    /// Make it, whole ([`RuleOp::body`]).
    #[default]
    Create,
    /// Switch it on or off, or move it ([`RuleOp::enabled`], [`RuleOp::sequence`]).
    Update,
    Delete,
}

/// One change to tell a server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleOp {
    pub kind: RuleOpKind,
    /// The rule here; 0 for a deletion of one that is gone from here.
    pub rule: Id,
    /// What the server calls it (a creation has none).
    pub id: String,
    /// For a change: switched on or off, and where it is to stand.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i32>,
    /// For a creation: the rule, with what the server calls the folder it moves mail to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<RemoteRule>,
}

/// What a host is to tell an account's server.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RulesUpload {
    pub account: Id,
    pub email: String,
    pub ops: Vec<RuleOp>,
}

/// What a server did with one change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuleResult {
    /// Taken.
    Done,
    /// The server no longer has the rule.
    Gone,
    /// The account may not change its rules (the owner agreed to reading only, or not at all).
    Denied,
    /// Not now: try again later.
    #[default]
    Failed,
    /// Refused for good.
    Refused,
}

/// What a server said of one change.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RuleOutcome {
    pub kind: RuleOpKind,
    pub rule: Id,
    /// The id it has, as the server says (a creation).
    pub id: String,
    pub result: RuleResult,
    /// What the change said, as it was sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<i32>,
    /// The status of the answer; 0 when there was none.
    pub status: u16,
    /// Why not, in a few words (never what was sent).
    pub error: String,
}

impl RuleOutcome {
    pub fn tidy(&mut self) {
        self.id = key(&self.id);
        self.error = line(&self.error, MAX_WHY);
    }
}

/// What a server did with a [`RulesUpload`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RulesUploaded {
    pub account: Id,
    pub email: String,
    pub outcomes: Vec<RuleOutcome>,
    /// The server stopped taking the sign-in part-way.
    pub token_refused: bool,
}

impl RulesUploaded {
    pub fn for_account(mut self, account: &Account) -> RulesUploaded {
        self.account = account.id;
        self.email = account.email.trim().to_string();
        self
    }
}
