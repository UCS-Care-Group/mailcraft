// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Sender warnings** in a session: the **known correspondents** gathered from the mailbox and
//! kept up to date, and each message's warnings worked out when it is read and kept until what
//! they were measured against changes ([`Session::sender_warnings`]). The rules themselves are
//! `mailcraft_model::sender_warnings`; nothing here is stored in the mailbox file.
//!
//! The known correspondents are the contacts' addresses and whoever mail in Sent Items or the
//! Outbox went to. The same set says whose pictures sender images may ask for
//! ([`Session::knows`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use mailcraft_html::{Block, BlockKind};
use mailcraft_model::sender_warnings::{self, Facts, FactsBuilder, Warning};
use mailcraft_model::{Contact, FolderKind, Id, Message};

use crate::Session;

/// Most messages whose warnings are kept; past that the oldest kept go.
const MAX_KEPT: usize = 4_000;

/// The addresses the owner knows: the contacts', and whoever mail in Sent Items or the Outbox
/// (on its way there) went to. Gathered once for a mailbox; after that a change of it reads only
/// the messages that are new, and the mail is gathered again only when a message counted here
/// is gone.
#[derive(Default)]
struct Known {
    /// The mailbox ([`Session::openings`]), the version of it an undo or redo put in place
    /// ([`Session::restored`]) and its revision, that they were last brought up to date at.
    at: Option<(u64, u64, u64)>,
    /// The contacts read, and their addresses with their names. Holding them makes any change to
    /// one a new one (`Arc::make_mut`), so comparing them with the mailbox's finds what changed.
    contacts: BTreeMap<Id, Arc<Contact>>,
    of_contacts: HashSet<String>,
    contact_names: Vec<(String, String)>,
    /// Whoever the messages in `sent` went to, and the names they went by there. Every message
    /// below `next` was looked at, and the mailbox held `messages` messages then.
    written_to: HashSet<String>,
    written_names: HashSet<(String, String)>,
    sent: HashSet<Id>,
    next: Id,
    messages: usize,
    /// Counts the changes to what is known (or to the owner's accounts): what was worked out
    /// against it is worked out again.
    generation: u64,
    /// The owner's accounts as they were last read (address, name, shared).
    accounts: Vec<(String, String, bool)>,
}

impl Known {
    fn contains(&self, address: &str) -> bool {
        self.of_contacts.contains(address) || self.written_to.contains(address)
    }

    /// Brought up to date with the mailbox of `session`.
    // shortcut: mail moved out of Sent Items counts until a message is deleted for good or the mailbox is opened again, and mail moved into it from anywhere but the Outbox from then on; look at Sent Items at every change if that matters.
    fn update(&mut self, session: &Session) {
        let at = (session.openings(), session.restored(), session.rev());
        if self.at == Some(at) {
            return;
        }
        // Another mailbox, or another version of it, which may hold others below `next`.
        let fresh = self.at.is_none_or(|(openings, restored, _)| (openings, restored) != (at.0, at.1));
        self.at = Some(at);
        let mailbox = &session.mailbox;
        let accounts: Vec<(String, String, bool)> =
            mailbox.accounts.iter().map(|a| (a.email.trim().to_lowercase(), a.display_name.clone(), a.is_shared())).collect();
        if accounts != self.accounts {
            self.accounts = accounts;
            self.changed();
        }
        let same = self.contacts.len() == mailbox.contacts.len()
            && self.contacts.iter().zip(&mailbox.contacts).all(|((was, kept), (id, now))| was == id && Arc::ptr_eq(kept, now));
        if fresh || !same {
            self.contacts = mailbox.contacts.clone();
            self.contact_names = self
                .contacts
                .values()
                .flat_map(|c| {
                    let name = c.name();
                    c.emails.iter().map(move |e| (e.value.trim().to_lowercase(), name.clone()))
                })
                .collect();
            self.of_contacts = self.contact_names.iter().map(|(address, _)| address.clone()).collect();
            self.changed();
        }
        let outgoing: HashSet<Id> =
            mailbox.folders.iter().filter(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox)).map(|f| f.id).collect();
        let (all, expected) = (mailbox.messages.len(), self.messages.saturating_add(mailbox.messages.range(self.next..).count()));
        // Fewer than that: some were deleted for good, which matters only when they were counted
        // here. More: some came back below `next` (an undo).
        let gone =
            all > expected || (all < expected && self.sent.iter().any(|id| mailbox.messages.get(id).is_none_or(|m| !outgoing.contains(&m.folder))));
        let from = if fresh || gone {
            if !self.written_to.is_empty() {
                self.changed();
            }
            self.written_to.clear();
            self.written_names.clear();
            self.sent.clear();
            0
        } else {
            self.next
        };
        let mut grew = false;
        for (id, m) in mailbox.messages.range(from..).filter(|(_, m)| outgoing.contains(&m.folder)) {
            self.sent.insert(*id);
            for a in m.to.iter().chain(&m.cc).chain(&m.bcc) {
                let address = a.email.trim().to_lowercase();
                if sender_warnings::name_key(&a.name).is_some() {
                    grew |= self.written_names.insert((address.clone(), a.name.trim().to_string()));
                }
                grew |= self.written_to.insert(address);
            }
        }
        if grew {
            self.changed();
        }
        self.next = mailbox.messages.last_key_value().map_or(from, |(id, _)| id.saturating_add(1));
        self.messages = all;
    }

    fn changed(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

/// What a message's warnings were worked out from: who sent it, where it is, what its server
/// said, and how much of its body there was.
fn fingerprint(m: &Message) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (m.id, m.account, m.folder, &m.from.name, &m.from.email, m.uid == 0 && m.server.is_none()).hash(&mut hasher);
    m.headers.iter().take(2_000).find(|(name, _)| name.eq_ignore_ascii_case("Authentication-Results")).map(|(_, v)| v).hash(&mut hasher);
    (m.body.html.as_ref().map(String::len), m.body_is_kept()).hash(&mut hasher);
    hasher.finish()
}

/// A message's warnings as they were worked out.
struct Kept {
    fingerprint: u64,
    generation: u64,
    /// The body's links were looked at.
    links: bool,
    warnings: Arc<Vec<Warning>>,
}

/// The sender warnings of a session ([`Session::sender_warnings`]). In memory only.
#[derive(Default)]
pub struct SenderChecks {
    known: Known,
    /// What the warnings are measured against, and the generation of `known` it was made at.
    facts: Option<(u64, Arc<Facts>)>,
    kept: HashMap<Id, Kept>,
}

impl SenderChecks {
    /// The facts for the generation `known` is at, made again when it moved on.
    fn facts(&mut self) -> Arc<Facts> {
        let generation = self.known.generation;
        if let Some((at, facts)) = &self.facts
            && *at == generation
        {
            return facts.clone();
        }
        let known = &self.known;
        let mut b =
            FactsBuilder::default().accounts_by(known.accounts.iter().map(|(address, name, shared)| (address.as_str(), name.as_str(), *shared)));
        for (address, name) in &known.contact_names {
            b.known(address, name);
        }
        for address in &known.written_to {
            b.known(address, "");
        }
        for (address, name) in &known.written_names {
            b.known(address, name);
        }
        let facts = Arc::new(b.build());
        self.facts = Some((generation, facts.clone()));
        facts
    }
}

/// Each link of `blocks` as its text and where it goes, adjacent pieces of one link's text put
/// together, inside tables too; at most [`sender_warnings::MAX_LINKS`] of them.
pub fn links(blocks: &[Block]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    gather(blocks, &mut out, 0);
    out
}

fn gather(blocks: &[Block], out: &mut Vec<(String, String)>, depth: usize) {
    if depth > 16 {
        return;
    }
    for block in blocks {
        if out.len() >= sender_warnings::MAX_LINKS {
            return;
        }
        if let BlockKind::Table(table) = &block.kind {
            for cell in table.rows.iter().flat_map(|r| &r.cells) {
                gather(&cell.blocks, out, depth + 1);
            }
            continue;
        }
        let mut current: Option<(String, String)> = None;
        for span in &block.spans {
            match (&span.link, &mut current) {
                (Some(url), Some((text, href))) if href == url => {
                    if text.len() < 4_096 {
                        text.push_str(&span.text);
                    }
                }
                (Some(url), _) => {
                    out.extend(current.take());
                    current = Some((span.text.clone(), url.clone()));
                }
                (None, _) => out.extend(current.take()),
            }
        }
        out.extend(current);
    }
}

impl Session {
    /// Whether the owner knows `address` (in lower case): it is one of the contacts', or mail in
    /// Sent Items went to it.
    pub fn knows(&mut self, address: &str) -> bool {
        let mut checks = std::mem::take(&mut self.sender_checks);
        checks.known.update(self);
        let known = checks.known.contains(address);
        self.sender_checks = checks;
        known
    }

    /// The sender warnings of `m` (`mailcraft_model::sender_warnings`): with its links looked at
    /// when `links` is set and its body is here. Worked out once and kept, until the message, or
    /// what they are measured against (the contacts, Sent Items, the owner's accounts), changes.
    pub fn sender_warnings(&mut self, m: &Message, links: bool) -> Arc<Vec<Warning>> {
        let mut checks = std::mem::take(&mut self.sender_checks);
        checks.known.update(self);
        let generation = checks.known.generation;
        let print = fingerprint(m);
        if let Some(kept) = checks.kept.get(&m.id)
            && kept.fingerprint == print
            && kept.generation == generation
            && (kept.links || !links)
        {
            let warnings = kept.warnings.clone();
            self.sender_checks = checks;
            return warnings;
        }
        let facts = checks.facts();
        let account = self.mailbox.account(m.account);
        let kind = self.mailbox.folder(m.folder).map(|f| f.kind);
        let body = links.then(|| m.body.html.as_deref().map(|html| self::links(&mailcraft_html::parse(html)))).flatten();
        let pairs = body.as_ref().map(|pairs| pairs.iter().map(|(text, href)| (text.as_str(), href.as_str())));
        let warnings = Arc::new(sender_warnings::check(m, account, kind, &facts, pairs));
        if checks.kept.len() >= MAX_KEPT {
            checks.kept.clear();
        }
        checks.kept.insert(m.id, Kept { fingerprint: print, generation, links, warnings: warnings.clone() });
        self.sender_checks = checks;
        warnings
    }
}
