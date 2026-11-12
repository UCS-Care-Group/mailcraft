// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local filing suggestions from a bounded sample of already filed message headers.

use std::collections::{HashMap, HashSet};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

use mailcraft_model::{FolderKind, Id, Mailbox, Message, normalize_subject, words};
use serde::{Deserialize, Serialize};

const SCAN_LIMIT: usize = 16_000;
const SUGGESTION_LIMIT: usize = 3;
const HEADER_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    Sender,
    Subject,
    SenderAndSubject,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Suggestion {
    pub folder: Id,
    pub reason: Reason,
}

#[derive(Default)]
struct Evidence {
    sender: usize,
    subject: usize,
    both: usize,
}

/// Prepare the read-only command's work, also used by the Move menus to run it off the UI thread.
pub fn prepare(session: &mut crate::Session, params: &serde_json::Value) -> Result<Option<Scan>, crate::CmdError> {
    let ids = crate::cmd::targets_whole(session, params)?;
    Ok(match ids.as_slice() {
        [id] if session.prefs.suggest_folders => Scan::new(&session.mailbox, *id),
        _ => None,
    })
}

/// The message map shares its chunks with the mailbox. Capture it once, then read headers
/// from that snapshot in a worker (or in short slices on a host without threads).
pub struct Scan {
    messages: mailcraft_model::messages::Messages,
    message: Arc<Message>,
    folders: HashMap<Id, (FolderKind, Option<Id>)>,
    destinations: Option<HashSet<Id>>,
    topic: Vec<String>,
    seen: HashSet<(Id, String)>,
    evidence: HashMap<Id, Evidence>,
    total_sender: usize,
    remaining: usize,
    before: Option<Id>,
}

impl Scan {
    pub fn new(mailbox: &Mailbox, id: Id) -> Option<Self> {
        let message = mailbox.messages.get(&id)?;
        if message.draft || mailbox.stays_on_server(id) {
            return None;
        }
        let folders: HashMap<_, _> = mailbox.folders.iter().filter(|f| f.account == message.account).map(|f| (f.id, (f.kind, f.parent))).collect();
        if !under(&folders, message.folder, |kind| kind != FolderKind::Junk) {
            return None;
        }
        Some(Self {
            messages: mailbox.messages.clone(),
            message: message.clone(),
            folders,
            destinations: None,
            topic: topic_words(message),
            seen: HashSet::new(),
            evidence: HashMap::new(),
            total_sender: 0,
            remaining: SCAN_LIMIT,
            before: None,
        })
    }

    pub fn advance(&mut self, batch: usize) -> bool {
        let destinations = self.destinations.get_or_insert_with(|| {
            self.folders
                .iter()
                .filter(|(id, (kind, _))| {
                    *kind == FolderKind::Custom
                        && under(&self.folders, **id, |kind| matches!(kind, FolderKind::Custom | FolderKind::Inbox | FolderKind::Archive))
                })
                .map(|(&id, _)| id)
                .collect()
        });
        if destinations.is_empty() {
            return true;
        }
        let references: HashSet<_> = self.topic.iter().map(String::as_str).filter(|word| reference(word)).collect();
        let meaningful = self.topic.len() >= 2 && self.topic.iter().map(String::len).sum::<usize>() >= 10 || !references.is_empty();
        let bounds = (Unbounded, self.before.map_or(Unbounded, Excluded));
        let mut visited = 0;
        let batch = batch.min(self.remaining);
        for (&id, filed) in self.messages.range(bounds).rev().take(batch) {
            self.before = Some(id);
            visited += 1;
            if id == self.message.id || filed.draft || filed.account != self.message.account || !destinations.contains(&filed.folder) {
                continue;
            }
            let sender = !self.message.from.email.is_empty() && self.message.from.email.eq_ignore_ascii_case(&filed.from.email);
            let subject = meaningful && {
                let filed_topic = topic_words(filed);
                self.topic == filed_topic || filed_topic.iter().any(|word| references.contains(word.as_str()))
            };
            let message_id = &filed.message_id;
            if !(sender || subject)
                || !message_id.is_empty() && !self.seen.insert((filed.folder, message_id[..message_id.floor_char_boundary(HEADER_BYTES)].to_owned()))
            {
                continue;
            }
            self.total_sender += usize::from(sender);
            let votes = self.evidence.entry(filed.folder).or_default();
            votes.sender += usize::from(sender);
            votes.subject += usize::from(subject);
            votes.both += usize::from(sender && subject);
        }
        self.remaining -= visited;
        visited < batch || self.remaining == 0
    }

    pub fn finish(mut self) -> Vec<Suggestion> {
        self.advance(SCAN_LIMIT);
        self.suggestions()
    }

    pub fn suggestions(self) -> Vec<Suggestion> {
        let mut ranked: Vec<_> = self
            .evidence
            .into_iter()
            .filter(|(folder, _)| *folder != self.message.folder)
            .filter_map(|(folder, votes)| {
                let reason = if votes.both > 0 {
                    Reason::SenderAndSubject
                } else if votes.subject >= 2 {
                    Reason::Subject
                } else if votes.sender >= 2 && votes.sender * 2 > self.total_sender {
                    Reason::Sender
                } else {
                    return None;
                };
                Some((reason, votes.both, votes.subject, votes.sender, std::cmp::Reverse(folder)))
            })
            .collect();
        ranked.sort_unstable_by(|a, b| b.cmp(a));
        ranked.into_iter().take(SUGGESTION_LIMIT).map(|(reason, _, _, _, folder)| Suggestion { folder: folder.0, reason }).collect()
    }
}

fn topic_words(message: &Message) -> Vec<String> {
    let end = message.subject.floor_char_boundary(HEADER_BYTES);
    let mut words = words::words(&normalize_subject(&message.subject[..end]));
    words.truncate(48);
    words
}

fn reference(word: &str) -> bool {
    word.len() >= 5 && word.bytes().any(|b| b.is_ascii_digit()) && word.bytes().any(|b| b.is_ascii_alphabetic())
}

fn under(folders: &HashMap<Id, (FolderKind, Option<Id>)>, mut folder: Id, allowed: impl Fn(FolderKind) -> bool) -> bool {
    for _ in 0..32 {
        let Some(&(kind, parent)) = folders.get(&folder) else { return false };
        if !allowed(kind) {
            return false;
        }
        let Some(parent) = parent else { return true };
        folder = parent;
    }
    false
}

#[cfg(test)]
#[path = "filing_suggestions_tests.rs"]
mod tests;
