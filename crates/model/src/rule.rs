// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail rules: conditions on an arriving message and what to do with it.

use serde::{Deserialize, Serialize};

use crate::{Id, Importance, Message, contains_ci};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum Condition {
    /// The sender's name or address contains the text.
    FromContains(String),
    /// Any To/Cc recipient's name or address contains the text.
    ToContains(String),
    SubjectContains(String),
    BodyContains(String),
    HasAttachment,
    Importance(Importance),
}

impl Condition {
    pub fn matches(&self, m: &Message) -> bool {
        match self {
            Condition::FromContains(t) => contains_ci(&m.from.name, t) || contains_ci(&m.from.email, t),
            Condition::ToContains(t) => m.to.iter().chain(&m.cc).any(|a| contains_ci(&a.name, t) || contains_ci(&a.email, t)),
            Condition::SubjectContains(t) => contains_ci(&m.subject, t),
            Condition::BodyContains(t) => contains_ci(&m.body.text, t) || contains_ci(&m.hidden_text, t),
            Condition::HasAttachment => m.has_attachments(),
            Condition::Importance(i) => m.importance == *i,
        }
    }

    /// The condition in words: `from contains "newsletter"`.
    pub fn describe(&self) -> String {
        match self {
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
}

impl Rule {
    pub fn matches(&self, m: &Message) -> bool {
        self.enabled && !self.conditions.is_empty() && self.conditions.iter().all(|c| c.matches(m))
    }

    /// `When from contains "x" and has an attachment: flag it, mark it as read`.
    pub fn describe(&self) -> String {
        let when = self.conditions.iter().map(Condition::describe).collect::<Vec<_>>().join(" and ");
        let then = self.actions.iter().map(Action::describe).collect::<Vec<_>>().join(", ");
        format!("When {when}: {then}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Address, Attachment};

    #[test]
    fn conditions_all_must_hold() {
        let m = Message {
            from: Address::new("Weekly Digest", "news@example.org"),
            to: vec![Address::new("", "me@example.org")],
            subject: "Your October digest".into(),
            attachments: vec![Attachment::new("a.pdf", "application/pdf", vec![1])],
            ..Default::default()
        };
        let mut rule = Rule {
            enabled: true,
            conditions: vec![Condition::FromContains("DIGEST".into()), Condition::HasAttachment],
            actions: vec![Action::MarkRead],
            ..Default::default()
        };
        assert!(rule.matches(&m));
        rule.conditions.push(Condition::SubjectContains("invoice".into()));
        assert!(!rule.matches(&m));
        rule.conditions.pop();
        rule.enabled = false;
        assert!(!rule.matches(&m));
        // No conditions: matches nothing, not everything.
        assert!(!Rule { enabled: true, ..Default::default() }.matches(&m));
        assert!(Condition::ToContains("me@".into()).matches(&m) && !Condition::Importance(Importance::High).matches(&m));
        assert_eq!(
            Rule { conditions: vec![Condition::HasAttachment], actions: vec![Action::Flag, Action::MarkRead], ..Default::default() }.describe(),
            "When has an attachment: flag it, mark it as read"
        );
    }

    #[test]
    fn rules_round_trip_as_json() {
        let rule = Rule {
            id: 4,
            name: "News".into(),
            enabled: true,
            conditions: vec![Condition::FromContains("news".into())],
            actions: vec![Action::MoveTo(9), Action::Categorize("Blue".into())],
            stop: true,
        };
        let json = serde_json::to_string(&rule).unwrap();
        assert!(json.contains(r#"{"type":"moveTo","value":9}"#), "{json}");
        assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
    }
}
