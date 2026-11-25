// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contact-based conversation aggregation (#1907): non-destructive virtual grouping of incoming
//! and outgoing emails per correspondent across folders and optionally across accounts.

use std::collections::{HashMap, HashSet};

use mailcraft_model::correspondents::normalize_address;
use mailcraft_model::search::Query;
use mailcraft_model::{FlagStatus, FolderKind, Id, Mailbox, Message, contains_ci};
use mailcraft_time::Timestamp;
use serde::Serialize;

use crate::list::{preview, when};
use crate::{Filter, Session};

/// Most contacts sharing a single email address before it is treated as a shared/generic address
/// (matches `cmd::people_merge::MAX_LINK`) so a shared alias doesn't merge unrelated contacts.
const MAX_CONTACT_LINK: usize = 5;

/// Default page size when loading chronological chat history incrementally.
pub const DEFAULT_CHAT_PAGE: usize = 50;
/// Maximum page size allowed for a single `mail.correspondentMessages` response.
pub const MAX_CHAT_PAGE: usize = 500;

/// Summary of one contact-based conversation, displayed as a single row in the conversation list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrespondentSummary {
    /// Canonical normalized email key (deterministic across runs; lexicographically smallest
    /// address in the merged identity set).
    pub key: String,
    /// Display name for this correspondent (custom group name > Contact name > latest message name > email).
    pub name: String,
    /// Primary email address shown in the header/subtitle.
    pub primary_email: String,
    /// All normalized email addresses aggregated into this conversation.
    pub addresses: Vec<String>,
    /// Matching Contact id if present in the mailbox.
    pub contact_id: Option<Id>,
    /// Whether this person's addresses are joined by a manual `CorrespondentGroup`.
    pub manually_merged: bool,
    /// Newest message id in this conversation (within the active scope).
    pub latest_id: Id,
    /// Timestamp of the newest message (`list::when`).
    pub latest_when: Timestamp,
    /// Whether the newest message was sent by the owner.
    pub latest_outgoing: bool,
    /// Subject of the newest message.
    pub latest_subject: String,
    /// Single-line preview of the newest message.
    pub latest_preview: String,
    /// Total messages in this conversation (within the active scope).
    pub message_count: usize,
    /// Unread messages in this conversation (within the active scope).
    pub unread_count: usize,
    /// Whether any message in the conversation is flagged.
    pub has_flagged: bool,
    /// Whether any message in the conversation has listed attachments.
    pub has_attachments: bool,
}

/// All normalized email addresses owned by the user across every configured account.
pub fn own_addresses(mb: &Mailbox) -> HashSet<String> {
    let mut own = HashSet::new();
    for account in &mb.accounts {
        if let Some(addr) = normalize_address(&account.email) {
            own.insert(addr);
        }
        for sender in account.sender_addresses() {
            if let Some(addr) = normalize_address(&sender.email) {
                own.insert(addr);
            }
        }
    }
    own
}

/// Whether `m` is an outgoing message (in Sent/Outbox/Drafts or sent from one of the owner's own
/// addresses).
pub fn is_outgoing(mb: &Mailbox, m: &Message, own: &HashSet<String>) -> bool {
    if mb.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox | FolderKind::Drafts)) {
        return true;
    }
    normalize_address(&m.from.email).is_some_and(|addr| own.contains(&addr))
}

/// Folders eligible for contact-based aggregation: excludes Deleted Items, Junk E-mail, and their
/// subfolders. When `all_accounts` is false (and Unified All Accounts is not active), restricts to
/// the account of `s.view.folder`.
pub fn eligible_folders(s: &Session, all_accounts: bool) -> HashSet<Id> {
    let mb = &s.mailbox;
    let multi_account = all_accounts || crate::unified::of(s).is_some();
    let active_account = (!multi_account).then(|| mb.folder(s.view.folder).map(|f| f.account)).flatten();

    let excluded: HashSet<Id> = mb
        .folders
        .iter()
        .filter(|f| active_account.is_none_or(|acc| f.account == acc) && matches!(f.kind, FolderKind::Deleted | FolderKind::Junk))
        .flat_map(|f| mb.subtree(f.id))
        .collect();

    mb.folders.iter().filter(|f| active_account.is_none_or(|acc| f.account == acc) && !excluded.contains(&f.id)).map(|f| f.id).collect()
}

/// Union-find identity map linking email addresses that belong to the same person (via manual
/// `correspondent_aliases` and multi-email `Contact` cards).
pub struct IdentityMap {
    /// Canonical key (lexicographically smallest address in the component) for every address in a
    /// multi-address component.
    canonical: HashMap<String, String>,
    /// All addresses in each canonical component, sorted.
    members: HashMap<String, Vec<String>>,
    /// Custom display name from manual `correspondent_aliases`, keyed by canonical key.
    custom_names: HashMap<String, String>,
    /// Matching contact (id, non-empty name, primary email), keyed by normalized address.
    contacts: HashMap<String, (Id, String, String)>,
    /// Addresses belonging to at least one manual `CorrespondentGroup`.
    manual: HashSet<String>,
}

impl IdentityMap {
    /// Build an identity map from `mb.correspondent_aliases` and `mb.contacts`, ignoring `own`
    /// addresses so the owner's own address never bridges two distinct correspondents.
    pub fn build(mb: &Mailbox, own: &HashSet<String>) -> Self {
        let mut nodes: Vec<String> = Vec::new();
        let mut idx: HashMap<String, usize> = HashMap::new();
        let mut intern = |addr: &str| -> usize {
            if let Some(&i) = idx.get(addr) {
                return i;
            }
            let i = nodes.len();
            nodes.push(addr.to_string());
            idx.insert(addr.to_string(), i);
            i
        };

        let mut raw_edges: Vec<(usize, usize)> = Vec::new();
        let mut manual = HashSet::new();
        let mut manual_names: Vec<(String, String)> = Vec::new();

        for group in &mb.correspondent_aliases {
            let valid: Vec<String> = group.addresses.iter().filter_map(|a| normalize_address(a)).filter(|a| !own.contains(a)).collect();
            if valid.len() >= 2 {
                let first = intern(&valid[0]);
                for addr in &valid {
                    manual.insert(addr.clone());
                }
                for addr in valid.iter().skip(1) {
                    let other = intern(addr);
                    raw_edges.push((first, other));
                }
                if !group.name.trim().is_empty() {
                    manual_names.push((valid[0].clone(), group.name.trim().to_string()));
                }
            }
        }

        // Count how many contacts share each address so generic shared addresses don't over-link.
        let mut addr_counts: HashMap<String, usize> = HashMap::new();
        let mut contacts_map: HashMap<String, (Id, String, String)> = HashMap::new();
        for c in mb.contacts.values() {
            let valid: Vec<String> = c.emails.iter().filter_map(|l| normalize_address(&l.value)).filter(|a| !own.contains(a)).collect();
            let mut seen_here = HashSet::new();
            for addr in &valid {
                if seen_here.insert(addr.clone()) {
                    *addr_counts.entry(addr.clone()).or_default() += 1;
                }
            }
        }

        for c in mb.contacts.values() {
            let cname = c.name();
            let primary = c.primary_email().and_then(normalize_address).unwrap_or_default();
            let valid: Vec<String> = c.emails.iter().filter_map(|l| normalize_address(&l.value)).filter(|a| !own.contains(a)).collect();
            for addr in &valid {
                contacts_map.entry(addr.clone()).or_insert_with(|| (c.id, cname.clone(), primary.clone()));
            }
            let linkable: Vec<String> = valid.into_iter().filter(|a| addr_counts.get(a).copied().unwrap_or(0) <= MAX_CONTACT_LINK).collect();
            if linkable.len() >= 2 {
                let first = intern(&linkable[0]);
                for addr in linkable.iter().skip(1) {
                    let other = intern(addr);
                    raw_edges.push((first, other));
                }
            }
        }

        let mut parent: Vec<usize> = (0..nodes.len()).collect();
        for (a, b) in raw_edges {
            uf_join(&mut parent, a, b);
        }

        let mut comp: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, addr) in nodes.iter().enumerate() {
            let r = uf_find(&mut parent, i);
            comp.entry(r).or_default().push(addr.clone());
        }

        let mut canonical = HashMap::new();
        let mut members = HashMap::new();
        for mut addrs in comp.into_values() {
            addrs.sort();
            addrs.dedup();
            if let Some(canon) = addrs.first().cloned() {
                for a in &addrs {
                    canonical.insert(a.clone(), canon.clone());
                }
                members.insert(canon, addrs);
            }
        }

        let mut custom_names = HashMap::new();
        for (addr, name) in manual_names {
            let canon = canonical.get(&addr).cloned().unwrap_or(addr);
            custom_names.entry(canon).or_insert(name);
        }

        IdentityMap { canonical, members, custom_names, contacts: contacts_map, manual }
    }

    /// Resolve a normalized email address to its canonical key.
    pub fn resolve(&self, norm_email: &str) -> String {
        self.canonical.get(norm_email).cloned().unwrap_or_else(|| norm_email.to_string())
    }

    /// All normalized email addresses belonging to `canonical_key`.
    pub fn addresses_of(&self, canonical_key: &str) -> Vec<String> {
        self.members.get(canonical_key).cloned().unwrap_or_else(|| vec![canonical_key.to_string()])
    }

    /// Whether `canonical_key` is covered by a manual `CorrespondentGroup`.
    pub fn is_manually_merged(&self, canonical_key: &str) -> bool {
        self.addresses_of(canonical_key).iter().any(|a| self.manual.contains(a))
    }

    /// Custom name set by a manual merge, if any.
    pub fn custom_name(&self, canonical_key: &str) -> Option<&str> {
        self.custom_names.get(canonical_key).map(String::as_str).filter(|s| !s.is_empty())
    }

    /// Matching Contact metadata across any address in `canonical_key`'s component.
    pub fn contact_of(&self, canonical_key: &str) -> Option<(Id, &str, &str)> {
        self.addresses_of(canonical_key)
            .into_iter()
            .find_map(|addr| self.contacts.get(&addr).map(|(id, name, primary)| (*id, name.as_str(), primary.as_str())))
    }
}

fn uf_find(parent: &mut [usize], mut i: usize) -> usize {
    for _ in 0..parent.len() {
        let up = parent.get(i).copied().unwrap_or(i);
        if up == i {
            return i;
        }
        let grand = parent.get(up).copied().unwrap_or(up);
        if let Some(slot) = parent.get_mut(i) {
            *slot = grand;
        }
        i = grand;
    }
    i
}

fn uf_join(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (uf_find(parent, a), uf_find(parent, b));
    if ra != rb
        && let Some(slot) = parent.get_mut(rb)
    {
        *slot = ra;
    }
}

/// Extract `(canonical_key, display_name_hint, email_used)` pairs associated with message `m`.
/// An incoming message maps to its sender; an outgoing message maps to each distinct external
/// correspondent in `to`, `cc`, `bcc` (deduped by canonical key so merged addresses don't double-count).
fn message_correspondents(mb: &Mailbox, m: &Message, own: &HashSet<String>, id_map: &IdentityMap) -> (bool, Vec<(String, String, String)>) {
    let outgoing = is_outgoing(mb, m, own);
    let mut out: Vec<(String, String, String)> = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();

    if outgoing {
        for addr in m.to.iter().chain(&m.cc).chain(&m.bcc) {
            let Some(norm) = normalize_address(&addr.email) else { continue };
            if own.contains(&norm) {
                continue;
            }
            let key = id_map.resolve(&norm);
            if seen_keys.insert(key.clone()) {
                out.push((key, addr.name.trim().to_string(), norm));
            }
        }
    } else if let Some(norm) = normalize_address(&m.from.email) {
        let key = id_map.resolve(&norm);
        out.push((key, m.from.name.trim().to_string(), norm));
    }
    (outgoing, out)
}

/// Whether `m` matches the active list `Filter`.
fn matches_filter(m: &Message, filter: Filter, own: &HashSet<String>) -> bool {
    match filter {
        Filter::All => true,
        Filter::Unread => !m.read,
        Filter::Flagged => m.flag == FlagStatus::Flagged,
        Filter::Attachments => m.has_attachments(),
        Filter::ToMe => m.to.iter().any(|a| normalize_address(&a.email).is_some_and(|e| own.contains(&e))),
    }
}

/// Whether `m` matches an in-conversation or list search string (via word index / `Query` plus
/// direct substring match on subject, sender name/address, and attachment filenames).
fn matches_query(s: &Session, m: &Message, query: &Query, raw_needle: &str, decided: Option<bool>) -> bool {
    if query.is_empty() && raw_needle.is_empty() {
        return true;
    }
    if decided.unwrap_or_else(|| !query.is_empty() && query.matches_in(m, |at| s.offset_at(at))) {
        return true;
    }
    !raw_needle.is_empty()
        && (contains_ci(&m.subject, raw_needle)
            || contains_ci(&m.from.name, raw_needle)
            || contains_ci(&m.from.email, raw_needle)
            || m.attachments.iter().any(|a| contains_ci(&a.name, raw_needle)))
}

struct Accum<'a> {
    latest: &'a Message,
    latest_when: Timestamp,
    latest_outgoing: bool,
    latest_email: String,
    best_name: String,
    best_name_when: Timestamp,
    message_count: usize,
    unread_count: usize,
    has_flagged: bool,
    has_attachments: bool,
    has_to_me: bool,
    query_hit: bool,
}

fn finalize_summary(key: String, acc: Accum<'_>, id_map: &IdentityMap) -> CorrespondentSummary {
    let contact = id_map.contact_of(&key);
    let custom_name = id_map.custom_name(&key);
    let primary_email = contact.map(|(_, _, p)| p).filter(|p| !p.is_empty()).map(str::to_string).unwrap_or_else(|| acc.latest_email.clone());
    let name = custom_name
        .map(str::to_string)
        .or_else(|| contact.map(|(_, n, _)| n).filter(|n| !n.is_empty()).map(str::to_string))
        .or_else(|| (!acc.best_name.is_empty()).then_some(acc.best_name))
        .unwrap_or_else(|| primary_email.clone());
    CorrespondentSummary {
        addresses: id_map.addresses_of(&key),
        contact_id: contact.map(|(id, _, _)| id),
        manually_merged: id_map.is_manually_merged(&key),
        key,
        name,
        primary_email,
        latest_id: acc.latest.id,
        latest_when: acc.latest_when,
        latest_outgoing: acc.latest_outgoing,
        latest_subject: acc.latest.subject.clone(),
        latest_preview: preview(acc.latest, 140),
        message_count: acc.message_count,
        unread_count: acc.unread_count,
        has_flagged: acc.has_flagged,
        has_attachments: acc.has_attachments,
    }
}

/// Folders and direction rule for the correspondent list (`summaries`) based on `s.view.folder`:
/// - Inbox: only received (`!outgoing`) messages in Inbox.
/// - Sent / Outbox: only sent (`outgoing`) messages in Sent / Outbox.
/// - Other folders: messages in the active folder.
enum ListScope {
    Incoming(HashSet<Id>),
    Outgoing(HashSet<Id>),
    Any(HashSet<Id>),
}

impl ListScope {
    fn contains_folder(&self, folder: Id) -> bool {
        match self {
            ListScope::Incoming(folders) | ListScope::Outgoing(folders) | ListScope::Any(folders) => folders.contains(&folder),
        }
    }

    fn matches(&self, m: &Message, outgoing: bool) -> bool {
        match self {
            ListScope::Incoming(folders) => !outgoing && folders.contains(&m.folder),
            ListScope::Outgoing(folders) => outgoing && folders.contains(&m.folder),
            ListScope::Any(folders) => folders.contains(&m.folder),
        }
    }
}

fn list_scope(s: &Session, all_accounts: bool) -> ListScope {
    let mb = &s.mailbox;
    let multi_account = all_accounts || crate::unified::of(s).is_some();
    let active_kind = crate::unified::of(s).or_else(|| mb.folder(s.view.folder).map(|f| f.kind));
    let active_account = (!multi_account).then(|| mb.folder(s.view.folder).map(|f| f.account)).flatten();

    let mut folders: HashSet<Id> = match active_kind {
        Some(FolderKind::Inbox) => {
            mb.folders.iter().filter(|f| f.kind == FolderKind::Inbox && active_account.is_none_or(|acc| f.account == acc)).map(|f| f.id).collect()
        }
        Some(FolderKind::Sent | FolderKind::Outbox) => mb
            .folders
            .iter()
            .filter(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox) && active_account.is_none_or(|acc| f.account == acc))
            .map(|f| f.id)
            .collect(),
        Some(FolderKind::Custom) => std::iter::once(s.view.folder).collect(),
        _ if multi_account => {
            if let Some(kind) = active_kind {
                mb.folders.iter().filter(|f| f.kind == kind).map(|f| f.id).collect()
            } else {
                eligible_folders(s, true)
            }
        }
        _ => std::iter::once(s.view.folder).collect(),
    };
    let eligible = eligible_folders(s, multi_account);
    folders.retain(|id| eligible.contains(id));

    match active_kind {
        Some(FolderKind::Inbox) => ListScope::Incoming(folders),
        Some(FolderKind::Sent | FolderKind::Outbox) => ListScope::Outgoing(folders),
        _ => ListScope::Any(folders),
    }
}

/// All correspondent conversations matching `filter` and `query_str`, sorted by latest
/// communication time (`s.view.sort_descending`).
pub fn summaries(s: &Session, all_accounts: bool, filter: Filter, query_str: &str) -> Vec<CorrespondentSummary> {
    let mb = &s.mailbox;
    let own = own_addresses(mb);
    let id_map = IdentityMap::build(mb, &own);
    let scope = list_scope(s, all_accounts);
    let query = Query::parse(query_str);
    let needle = query_str.trim();
    let decided = if query.is_empty() { HashMap::new() } else { s.search_words_for(&query, |m| scope.contains_folder(m.folder)).0 };

    let mut map: HashMap<String, Accum<'_>> = HashMap::new();
    for m in mb.messages.values() {
        let m = m.as_ref();
        if !scope.contains_folder(m.folder) {
            continue;
        }
        let (outgoing, corrs) = message_correspondents(mb, m, &own, &id_map);
        if corrs.is_empty() || !scope.matches(m, outgoing) {
            continue;
        }
        let ts = when(m);
        let msg_hit = matches_query(s, m, &query, needle, decided.get(&m.id).copied());
        let to_me = matches_filter(m, Filter::ToMe, &own);
        let flagged = m.flag == FlagStatus::Flagged;
        let has_att = m.has_attachments();
        let unread = usize::from(!m.read);

        for (key, name_hint, email_used) in corrs {
            match map.get_mut(&key) {
                Some(acc) => {
                    acc.message_count = acc.message_count.saturating_add(1);
                    acc.unread_count = acc.unread_count.saturating_add(unread);
                    acc.has_flagged |= flagged;
                    acc.has_attachments |= has_att;
                    acc.has_to_me |= to_me;
                    acc.query_hit |= msg_hit;
                    if (ts, m.id) > (acc.latest_when, acc.latest.id) {
                        acc.latest = m;
                        acc.latest_when = ts;
                        acc.latest_outgoing = outgoing;
                        acc.latest_email = email_used;
                    }
                    if !name_hint.is_empty() && (acc.best_name.is_empty() || ts >= acc.best_name_when) {
                        acc.best_name = name_hint;
                        acc.best_name_when = ts;
                    }
                }
                None => {
                    map.insert(
                        key,
                        Accum {
                            latest: m,
                            latest_when: ts,
                            latest_outgoing: outgoing,
                            latest_email: email_used,
                            best_name_when: if name_hint.is_empty() { i64::MIN } else { ts },
                            best_name: name_hint,
                            message_count: 1,
                            unread_count: unread,
                            has_flagged: flagged,
                            has_attachments: has_att,
                            has_to_me: to_me,
                            query_hit: msg_hit,
                        },
                    );
                }
            }
        }
    }

    let mut out: Vec<CorrespondentSummary> = map
        .into_iter()
        .filter_map(|(key, acc)| {
            let filter_ok = match filter {
                Filter::All => true,
                Filter::Unread => acc.unread_count > 0,
                Filter::Flagged => acc.has_flagged,
                Filter::Attachments => acc.has_attachments,
                Filter::ToMe => acc.has_to_me,
            };
            if !filter_ok {
                return None;
            }
            let query_hit = acc.query_hit;
            let summary = finalize_summary(key, acc, &id_map);
            let search_ok =
                needle.is_empty() || query_hit || contains_ci(&summary.name, needle) || summary.addresses.iter().any(|a| contains_ci(a, needle));
            search_ok.then_some(summary)
        })
        .collect();

    let desc = s.view.sort_descending;
    out.sort_by(|a, b| {
        let ord = a.latest_when.cmp(&b.latest_when).then(a.latest_id.cmp(&b.latest_id)).then(b.key.cmp(&a.key));
        if desc { ord.reverse() } else { ord }
    });
    out
}

/// Summary and chronological (oldest-first) message ids for correspondent `key_or_email`.
pub fn messages_for(s: &Session, key_or_email: &str, all_accounts: bool, filter: Filter, query_str: &str) -> (Option<CorrespondentSummary>, Vec<Id>) {
    let mb = &s.mailbox;
    let own = own_addresses(mb);
    let id_map = IdentityMap::build(mb, &own);
    let target_key = normalize_address(key_or_email).map(|n| id_map.resolve(&n)).unwrap_or_else(|| key_or_email.trim().to_lowercase());
    if target_key.is_empty() {
        return (None, Vec::new());
    }
    let folders = eligible_folders(s, all_accounts);
    let query = Query::parse(query_str);
    let needle = query_str.trim();
    let decided = if query.is_empty() { HashMap::new() } else { s.search_words_for(&query, |m| folders.contains(&m.folder)).0 };

    let mut acc: Option<Accum<'_>> = None;
    let mut matched: Vec<(Timestamp, Id)> = Vec::new();

    for m in mb.messages.values() {
        let m = m.as_ref();
        if !folders.contains(&m.folder) {
            continue;
        }
        let (outgoing, corrs) = message_correspondents(mb, m, &own, &id_map);
        let Some((_, name_hint, email_used)) = corrs.into_iter().find(|(k, _, _)| k == &target_key) else {
            continue;
        };
        let ts = when(m);
        let unread = usize::from(!m.read);
        let flagged = m.flag == FlagStatus::Flagged;
        let has_att = m.has_attachments();
        let to_me = matches_filter(m, Filter::ToMe, &own);

        match &mut acc {
            Some(a) => {
                a.message_count = a.message_count.saturating_add(1);
                a.unread_count = a.unread_count.saturating_add(unread);
                a.has_flagged |= flagged;
                a.has_attachments |= has_att;
                a.has_to_me |= to_me;
                if (ts, m.id) > (a.latest_when, a.latest.id) {
                    a.latest = m;
                    a.latest_when = ts;
                    a.latest_outgoing = outgoing;
                    a.latest_email = email_used;
                }
                if !name_hint.is_empty() && (a.best_name.is_empty() || ts >= a.best_name_when) {
                    a.best_name = name_hint;
                    a.best_name_when = ts;
                }
            }
            None => {
                acc = Some(Accum {
                    latest: m,
                    latest_when: ts,
                    latest_outgoing: outgoing,
                    latest_email: email_used,
                    best_name_when: if name_hint.is_empty() { i64::MIN } else { ts },
                    best_name: name_hint,
                    message_count: 1,
                    unread_count: unread,
                    has_flagged: flagged,
                    has_attachments: has_att,
                    has_to_me: to_me,
                    query_hit: false,
                });
            }
        }

        if matches_filter(m, filter, &own) && matches_query(s, m, &query, needle, decided.get(&m.id).copied()) {
            matched.push((ts, m.id));
        }
    }

    matched.sort_by_key(|&(ts, id)| (ts, id));
    let ids = matched.into_iter().map(|(_, id)| id).collect();
    let summary = acc.map(|a| finalize_summary(target_key, a, &id_map));
    (summary, ids)
}

/// Return the chronological window of up to `limit` newest messages ending `offset_from_end`
/// before the end of `ids`, along with how many older messages precede the returned slice.
pub fn paginate_latest(ids: &[Id], limit: usize, offset_from_end: usize) -> (&[Id], usize) {
    let end = ids.len().saturating_sub(offset_from_end);
    let start = end.saturating_sub(limit);
    (ids.get(start..end).unwrap_or_default(), start)
}
