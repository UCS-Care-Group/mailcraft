// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What changes mail on the stand-in: `Email/set` (RFC 8620 §5.3, RFC 8621 §4.6).
//!
//! - **`Email/set`** updates and destroys; it makes nothing. An update is a patch (RFC 8620 §5.3):
//!   `keywords/<keyword>` and `mailboxIds/<id>` set to `true` or to `null`, or the whole of `keywords` or
//!   `mailboxIds`. An email lies in one mailbox at least, and in mailboxes the server has
//!   (`invalidProperties`); on a server that keeps an email in one mailbox, in one (`tooManyMailboxes`), and
//!   there a move gives it a new id, which the answer says (`updated`). An email it does not have is
//!   `notFound`. No more than `maxObjectsInSet` at a time (`requestTooLarge`).
//! - **Trouble on demand.** [`Data::refused_changes`](super::Data::refused_changes) makes `Email/set` refuse
//!   an email with the error given. With [`Data::passes_over_unknown`](super::Data::passes_over_unknown),
//!   `Email/set` says nothing of an email it does not have instead of `notFound`.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::{ACCOUNT, Data, Email, Outcome, account, allow_only, invalid, refuse};
use crate::Id;

/// A `SetError` (RFC 8620 §5.3).
pub(super) fn set_error(kind: &str, description: &str) -> Value {
    json!({"type": kind, "description": description})
}

/// A map of an answer, or `null` when it holds nothing (RFC 8620 §5.3).
pub(super) fn or_null(map: Map<String, Value>) -> Value {
    if map.is_empty() { Value::Null } else { Value::Object(map) }
}

/// An argument that is an object, or nothing: `None` when it is something else.
pub(super) fn object<'a>(args: &'a Map<String, Value>, name: &str) -> Option<Option<&'a Map<String, Value>>> {
    match args.get(name) {
        None | Some(Value::Null) => Some(None),
        Some(Value::Object(map)) => Some(Some(map)),
        Some(_) => None,
    }
}

/// A set of names written as an object of `true`s (`keywords`, `mailboxIds`).
pub(super) fn truths(value: &Value) -> Option<BTreeSet<String>> {
    let map = value.as_object()?;
    map.iter().map(|(name, on)| (on == &Value::Bool(true)).then(|| name.clone())).collect()
}

/// Whether `text` may be a keyword (RFC 8621 §4.1.1): printable ASCII, and none of the characters IMAP keeps
/// for itself.
pub(super) fn is_keyword(text: &str) -> bool {
    !text.is_empty() && text.len() <= 255 && text.bytes().all(|b| (0x21..=0x7E).contains(&b) && !b"(){]%*\"\\".contains(&b))
}

impl Data {
    /// The mailboxes and the keywords `email` has once `patch` is applied, or the error it is refused with.
    fn patched(&self, email: &Email, patch: &Map<String, Value>) -> Result<(BTreeSet<String>, BTreeSet<String>), Value> {
        let (mut mailboxes, mut keywords) = (email.mailboxes.clone(), email.keywords.clone());
        let wrong = |path: &str| json!({"type": "invalidProperties", "description": "a property that can't be changed so", "properties": [path]});
        for (path, value) in patch {
            let unescaped = |token: &str| token.replace("~1", "/").replace("~0", "~");
            match path.split_once('/') {
                None if path == "keywords" => keywords = truths(value).filter(|all| all.iter().all(|k| is_keyword(k))).ok_or_else(|| wrong(path))?,
                None if path == "mailboxIds" => mailboxes = truths(value).ok_or_else(|| wrong(path))?,
                Some((set @ ("keywords" | "mailboxIds"), name)) if !name.contains('/') => {
                    let name = unescaped(name);
                    let into = if set == "keywords" { &mut keywords } else { &mut mailboxes };
                    match value {
                        Value::Bool(true) if set != "keywords" || is_keyword(&name) => {
                            into.insert(name);
                        }
                        Value::Null => {
                            into.remove(&name);
                        }
                        _ => return Err(json!({"type": "invalidPatch", "description": "a keyword or a mailbox is set to true, or to null"})),
                    }
                }
                _ => return Err(wrong(path)),
            }
        }
        if mailboxes.is_empty() || mailboxes.iter().any(|id| !self.mailboxes.iter().any(|m| &m.id == id)) {
            return Err(
                json!({"type": "invalidProperties", "description": "an email lies in a mailbox the server has", "properties": ["mailboxIds"]}),
            );
        }
        if self.profile.one_mailbox && mailboxes.len() > 1 {
            return Err(set_error("tooManyMailboxes", "an email lies in one mailbox on this server"));
        }
        Ok((mailboxes, keywords))
    }

    /// Apply `patch` to the email `id`: what the answer's `updated` holds for it (`null`, or its new id on a
    /// server that names an email by where it lies), or the error.
    pub(super) fn update_email(&mut self, id: &str, patch: &Value) -> Result<Value, Value> {
        if let Some((kind, description)) = self.refused_changes.get(id) {
            return Err(set_error(kind, description));
        }
        let Some(email) = self.email(id) else { return Err(set_error("notFound", "no such email")) };
        let Some(patch) = patch.as_object() else { return Err(set_error("invalidPatch", "a patch is an object")) };
        let (mailboxes, keywords) = self.patched(email, patch)?;
        let moved = mailboxes != email.mailboxes;
        let renamed = (moved && self.profile.one_mailbox).then(|| (self.id("B"), self.id("E")));
        let Some(email) = self.emails.iter_mut().find(|e| e.id == id) else { return Err(set_error("notFound", "no such email")) };
        (email.mailboxes, email.keywords) = (mailboxes, keywords);
        let changed = match renamed {
            Some((blob, new)) => {
                (email.blob, email.id) = (blob.clone(), new.clone());
                json!({"id": new, "blobId": blob})
            }
            None => Value::Null,
        };
        self.changed();
        Ok(changed)
    }

    /// `Email/set` (RFC 8621 §4.6): updates and destroys.
    pub(super) fn email_set(&mut self, args: &Map<String, Value>) -> Outcome<Value> {
        allow_only(args, &["accountId", "ifInState", "create", "update", "destroy"])?;
        account(args)?;
        let old = self.state();
        self.in_state(args)?;
        let (Some(create), Some(update)) = (object(args, "create"), object(args, "update")) else {
            return Err(invalid("create and update are objects, or null"));
        };
        let destroy = id_list(args, "destroy")?;
        let count = create.map_or(0, Map::len).saturating_add(update.map_or(0, Map::len)).saturating_add(destroy.len());
        if count > self.max_set {
            return Err(refuse("requestTooLarge", format!("{count} changes, and {} are taken at a time", self.max_set)));
        }
        let not_created: Map<String, Value> = create
            .into_iter()
            .flatten()
            .map(|(creation, _)| (creation.clone(), set_error("forbidden", "an email is not made with Email/set here")))
            .collect();
        let (mut updated, mut not_updated) = (Map::new(), Map::new());
        // (A server that passes over what it does not know: no word of those in the answer.)
        let passed_over = |data: &Data, id: &str| data.passes_over_unknown && data.email(id).is_none() && !data.refused_changes.contains_key(id);
        for (id, patch) in update.into_iter().flatten() {
            if passed_over(self, id) {
                continue;
            }
            match self.update_email(id, patch) {
                Ok(changed) => updated.insert(id.clone(), changed),
                Err(error) => not_updated.insert(id.clone(), error),
            };
        }
        let (mut destroyed, mut not_destroyed) = (Vec::new(), Map::new());
        for id in destroy {
            if passed_over(self, &id) {
                continue;
            }
            match self.refused_changes.get(&id).cloned() {
                Some((kind, description)) => {
                    not_destroyed.insert(id, set_error(&kind, &description));
                }
                None if self.remove(&id) => destroyed.push(id),
                None => {
                    not_destroyed.insert(id, set_error("notFound", "no such email"));
                }
            }
        }
        Ok(json!({
            "accountId": ACCOUNT,
            "oldState": old,
            "newState": self.state(),
            "created": null,
            "updated": or_null(updated),
            "destroyed": if destroyed.is_empty() { Value::Null } else { json!(destroyed) },
            "notCreated": or_null(not_created),
            "notUpdated": or_null(not_updated),
            "notDestroyed": or_null(not_destroyed),
        }))
    }

    /// `ifInState` (RFC 8620 §5.3): a state that is not the server's is `stateMismatch`.
    pub(super) fn in_state(&self, args: &Map<String, Value>) -> Outcome<()> {
        match args.get("ifInState") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(state)) if *state == self.state() => Ok(()),
            Some(Value::String(_)) => Err(refuse("stateMismatch", "the state is another now")),
            Some(_) => Err(invalid("ifInState is a string, or null")),
        }
    }
}

/// The argument `name` as a list of ids (`destroy`): none for `null`.
pub(super) fn id_list(args: &Map<String, Value>, name: &str) -> Outcome<Vec<String>> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().filter(|id| Id::new(id).is_ok()).map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| invalid("an id is 1 to 255 characters of A-Z a-z 0-9 - _")),
        Some(_) => Err(invalid(format!("{name} is a list of ids, or null"))),
    }
}
