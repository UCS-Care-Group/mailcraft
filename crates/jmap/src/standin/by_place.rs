// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The quirks of a server that names a message by where it is.
//!
//! Some JMAP servers are a thin layer in front of a mailbox that is not a JMAP store at all (an IMAP
//! server, say). What they serve is what the layer can make of that: an id says where its message lies,
//! an email lies in one mailbox only, the folders are the mailbox's own to make or not, and the header
//! fields come raw (RFC 8621 §4.1.2.2) instead of parsed into `subject`, `from` and the rest. A client
//! that must work against such a server as well as against a store of the standard kind is tried
//! against both by setting [`Data::profile`](super::Data::profile).
//!
//! A move gives a message a new id and blob here ([`Data::move_to`](super::Data::move_to)). What is not here
//! yet: the rules of sending.

use serde_json::{Value, json};

use super::{Outcome, account, allow_only, refuse};

/// Most changes of one kind that a `Mailbox/set` is answered for.
const MAX_KEYS: usize = 10_000;

/// What a server of one kind serves and refuses, beyond what every stand-in does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// `Email/get` serves the fields a store parses out of the header: `subject`, `from`, `to`, `cc`,
    /// `messageId`, `sentAt`, and `threadId`. A server that does not leaves a client the raw `headers`
    /// to read them from, and refuses (here: `invalidArguments`) a property it does not have.
    pub parsed_fields: bool,
    /// `Mailbox/set` is a method that refuses every change it is asked for (`forbidden`). Without it
    /// the method does not exist, as in every stand-in so far.
    pub fixed_mailboxes: bool,
    /// An email lies in one mailbox only (`maxMailboxesPerEmail` is 1), so [`Data::add_to_mailbox`](super::Data::add_to_mailbox)
    /// has nothing to do, and its id says where it lies, so that [`Data::move_to`](super::Data::move_to) gives
    /// it (and its blob) a new one.
    pub one_mailbox: bool,
}

impl Profile {
    /// A store of the standard kind: parsed fields, an email in as many mailboxes as it is put in.
    pub fn standard() -> Profile {
        Profile { parsed_fields: true, fixed_mailboxes: false, one_mailbox: false }
    }

    /// A server that names a message by where it is: raw headers only, one mailbox for an email, and
    /// mailboxes that are not to be made, renamed or removed.
    pub fn by_place() -> Profile {
        Profile { parsed_fields: false, fixed_mailboxes: true, one_mailbox: true }
    }
}

impl Default for Profile {
    fn default() -> Profile {
        Profile::standard()
    }
}

/// `Mailbox/set` of a server whose folders are fixed: it answers (RFC 8620 §5.3) and refuses all.
pub(super) fn mailbox_set(args: &serde_json::Map<String, Value>, state: &str) -> Outcome<Value> {
    allow_only(args, &["accountId", "ifInState", "create", "update", "destroy", "onDestroyRemoveEmails"])?;
    account(args)?;
    let refused = |what: &str| -> Outcome<Value> {
        let keys: Vec<String> = match args.get(what) {
            Some(Value::Object(members)) => members.keys().take(MAX_KEYS).cloned().collect(),
            Some(Value::Array(ids)) => ids.iter().filter_map(Value::as_str).take(MAX_KEYS).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        let reason = json!({"type": "forbidden", "description": "the mailboxes of this server are not changed through JMAP"});
        Ok(Value::Object(keys.into_iter().map(|k| (k, reason.clone())).collect()))
    };
    if !matches!(args.get("create"), None | Some(Value::Null) | Some(Value::Object(_)))
        || !matches!(args.get("update"), None | Some(Value::Null) | Some(Value::Object(_)))
        || !matches!(args.get("destroy"), None | Some(Value::Null) | Some(Value::Array(_)))
    {
        return Err(refuse("invalidArguments", "create and update are objects, destroy is a list"));
    }
    Ok(json!({
        "accountId": super::ACCOUNT,
        "oldState": state,
        "newState": state,
        "notCreated": refused("create")?,
        "notUpdated": refused("update")?,
        "notDestroyed": refused("destroy")?,
    }))
}
