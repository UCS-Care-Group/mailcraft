//! Ways of organising mail the owner sets up: search folders (saved searches the folder pane
//! shows), Quick Steps (named runs of actions on messages) and contact groups (a name standing
//! for several people).

use serde::{Deserialize, Serialize};

use crate::{Address, Id, clean_name};

/// Most search folders, Quick Steps and contact groups a mailbox keeps, each.
pub const MAX_SAVED: usize = 500;
/// Most actions one Quick Step runs.
pub const MAX_STEP_ACTIONS: usize = 20;
/// Most members one contact group has.
pub const MAX_MEMBERS: usize = 2_000;
/// Longest query a search folder keeps.
pub const MAX_SEARCH_QUERY: usize = 2_000;

/// A saved search the folder pane shows under Search Folders: it holds no mail of its own and is
/// never a folder on a server. Its query is in the search box's language
/// ([`crate::search::Query`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchFolder {
    /// Shares the numbering of folders, so that the folder in view can be either.
    pub id: Id,
    pub name: String,
    pub query: String,
    /// Only this account's mail; every account's when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Id>,
}

impl SearchFolder {
    /// The query as it may be kept: one line, without control characters, bounded.
    pub fn clean_query(query: &str) -> String {
        query.chars().map(|c| if c.is_control() { ' ' } else { c }).take(MAX_SEARCH_QUERY).collect::<String>().trim().to_string()
    }
}

/// One thing a Quick Step does to the messages it runs on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type", content = "value")]
pub enum StepAction {
    /// Move to the folder with this id.
    MoveTo(Id),
    /// Copy to the folder with this id.
    CopyTo(Id),
    MarkRead,
    MarkUnread,
    Flag,
    ClearFlag,
    MarkComplete,
    Categorize(String),
    ClearCategories,
    /// Move to Deleted Items.
    Delete,
    Archive,
    /// Start a reply to the message (the last action that writes: the owner finishes it).
    Reply,
    ReplyAll,
    /// Start forwarding the message to these addresses (`a@example.org; b@example.org`).
    Forward(String),
}

impl StepAction {
    /// The action in words, for the list of Quick Steps.
    pub fn describe(&self) -> String {
        match self {
            StepAction::MoveTo(_) => "move to a folder".into(),
            StepAction::CopyTo(_) => "copy to a folder".into(),
            StepAction::MarkRead => "mark as read".into(),
            StepAction::MarkUnread => "mark as unread".into(),
            StepAction::Flag => "flag".into(),
            StepAction::ClearFlag => "clear the flag".into(),
            StepAction::MarkComplete => "mark complete".into(),
            StepAction::Categorize(c) => format!("categorize as \"{c}\""),
            StepAction::ClearCategories => "clear categories".into(),
            StepAction::Delete => "delete".into(),
            StepAction::Archive => "archive".into(),
            StepAction::Reply => "reply".into(),
            StepAction::ReplyAll => "reply all".into(),
            StepAction::Forward(to) if to.trim().is_empty() => "forward".into(),
            StepAction::Forward(to) => format!("forward to {}", to.trim()),
        }
    }

    /// Whether it starts a message for the owner to write.
    pub fn writes(&self) -> bool {
        matches!(self, StepAction::Reply | StepAction::ReplyAll | StepAction::Forward(_))
    }
}

/// A named run of actions on the selected messages, from the Home tab: one undo step.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct QuickStep {
    pub id: Id,
    pub name: String,
    pub actions: Vec<StepAction>,
}

impl QuickStep {
    /// `Move to a folder, mark as read`.
    pub fn describe(&self) -> String {
        let text = self.actions.iter().map(StepAction::describe).collect::<Vec<_>>().join(", ");
        let mut chars = text.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().chain(chars).collect(),
            None => String::new(),
        }
    }
}

/// One person in a contact group: an address, and the contact it was taken from when it was.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GroupMember {
    pub address: Address,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<Id>,
}

/// A contact group (a distribution list kept here): its name on the To line stands for its
/// members when the message is sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContactGroup {
    pub id: Id,
    pub name: String,
    pub members: Vec<GroupMember>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    /// vCard `UID`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub uid: String,
}

impl ContactGroup {
    /// Add a member unless the address is in the group already; answers whether it was added.
    pub fn add(&mut self, member: GroupMember) -> bool {
        if self.members.len() >= MAX_MEMBERS || !member.address.is_valid() || self.members.iter().any(|m| m.address.same_mailbox(&member.address)) {
            return false;
        }
        self.members.push(member);
        true
    }

    /// Whether `text` (what was typed on a recipient line) names this group.
    pub fn named(&self, text: &str) -> bool {
        clean_name(text).is_some_and(|t| t.eq_ignore_ascii_case(&self.name))
    }
}

/// What has been done about a message's request for a read receipt.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReceiptAnswer {
    /// Nothing yet (or the message asks for none).
    #[default]
    Pending,
    /// A receipt was queued in the Outbox.
    Sent,
    /// The owner chose not to send one.
    Declined,
}

impl ReceiptAnswer {
    pub fn is_pending(&self) -> bool {
        *self == ReceiptAnswer::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_steps_say_what_they_do() {
        let step = QuickStep {
            actions: vec![StepAction::MoveTo(4), StepAction::MarkRead, StepAction::Forward("a@example.org".into())],
            ..Default::default()
        };
        assert_eq!(step.describe(), "Move to a folder, mark as read, forward to a@example.org");
        assert_eq!(QuickStep::default().describe(), "");
        let json = serde_json::to_string(&step.actions).unwrap();
        assert!(json.contains(r#"{"type":"moveTo","value":4}"#) && json.contains(r#"{"type":"markRead"}"#), "{json}");
        assert_eq!(serde_json::from_str::<Vec<StepAction>>(&json).unwrap(), step.actions);
        assert!(StepAction::Reply.writes() && !StepAction::Delete.writes());
    }

    #[test]
    fn groups_keep_each_address_once() {
        let mut g = ContactGroup { name: "Book club".into(), ..Default::default() };
        assert!(g.add(GroupMember { address: Address::new("Ada", "ada@example.org"), contact: None }));
        assert!(!g.add(GroupMember { address: Address::new("", "ADA@example.org"), contact: None }));
        assert!(!g.add(GroupMember { address: Address::new("", "not an address"), contact: None }));
        assert_eq!(g.members.len(), 1);
        assert!(g.named(" book CLUB ") && !g.named("book"));
    }

    #[test]
    fn queries_are_kept_on_one_line() {
        assert_eq!(SearchFolder::clean_query(" is:unread\r\nfrom:ada "), "is:unread  from:ada");
        assert_eq!(SearchFolder::clean_query(&"x".repeat(10_000)).len(), MAX_SEARCH_QUERY);
    }
}
