// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A search of an IMAP account on its own server, asked when the owner searches.
//!
//! One connection per account. The account's folders are read one after another on that
//! connection, and each folder's hits are handed back as soon as they arrive (`partial`), so a
//! window can show them while another account, on its own connection, is still being searched.
//! Gmail is asked folder by folder like any other account: All Mail is not a folder kept here, so
//! a search of it cannot be placed. Nothing of the query or of a password is written to a log line.

use std::collections::{HashMap, HashSet};

use crate::NetError;
use crate::sync::{MAX_MESSAGE, internal_date, message_from, open_imap, too_large};
use mailcraft_i18n::said;
use mailcraft_model::Importance;
use mailcraft_model::search::{Query, ServerField, ServerTerm};
use mailcraft_model::sync::{GraphMessage, GraphSearchResult, ServerMessage};
use mailcraft_model::{Account, FlagStatus, Message};
use mailcraft_proto::imap::encode_utf7;

/// Most folders of one account asked in one search, and most hits brought back from it.
const MAX_FOLDERS: usize = 24;
const MAX_HITS: usize = 40;
/// Most words of the query sent. The rest is checked here against what comes back.
const MAX_WORDS: usize = 6;

/// Search `folders` of `account` (names as the server knows them) for `query`. `known` is the
/// UIDs already on this computer, by folder name: they are not counted toward the hit limit.
/// `gmail_all`: when the folder list holds All Mail, that folder is asked once. Each call of
/// `on_page` is one folder's hits, or the failure; the last call has `partial` false.
pub fn search(
    account: &Account,
    secret: &str,
    ticket: u64,
    folders: &[String],
    query: &str,
    known: &HashMap<String, HashSet<u32>>,
    gmail_all: bool,
    mut on_page: impl FnMut(GraphSearchResult),
) {
    let mut base = GraphSearchResult { account: account.id, email: account.email.clone(), ticket, ..Default::default() };
    let (asked, left) = clauses(query);
    // Nothing the server can be asked: not an error. The list stays what is already here.
    if asked.is_empty() {
        on_page(base);
        return;
    }
    let mut imap = match open_imap(account, secret) {
        Ok(imap) => imap,
        Err(e) => {
            base.offline = e.is_unreachable();
            base.errors.push(format!("Couldn't search this mailbox: {}", e.without(secret)));
            on_page(base);
            return;
        }
    };
    let folders = chosen(folders, gmail_all);
    let mut hits = 0usize;
    let last = folders.len().saturating_sub(1);
    for (at, folder) in folders.iter().enumerate() {
        if hits >= MAX_HITS {
            break;
        }
        let mut page = base.clone();
        page.errors.clear();
        page.partial = at != last && hits < MAX_HITS;
        // `folder` is the decoded name kept here. The server is asked in modified UTF-7.
        let selected = match imap.select(&encode_utf7(folder), true) {
            Ok(selected) => selected,
            Err(e) => {
                let e = NetError::from(e);
                if e.is_fatal() {
                    page.partial = false;
                    page.offline = e.is_unreachable();
                    page.errors.push(format!("Couldn't search this mailbox: {}", e.without(secret)));
                    on_page(page);
                    let _ = imap.logout();
                    return;
                }
                // A folder that won't open is skipped. The others are still asked.
                if at == last {
                    page.partial = false;
                    on_page(page);
                }
                continue;
            }
        };
        let clause_refs: Vec<(&str, Option<&str>, &str)> = asked.iter().map(|c| (c.key, c.header, c.text.as_str())).collect();
        let gmail = gmail_all && is_all_mail(folder);
        let uids = if gmail {
            match gmail_query(&asked) {
                Some(query) => imap.uid_search_gmail("1:*", &query).map(|uids| (uids, false)),
                None => imap.uid_search_clauses_newest(&clause_refs),
            }
        } else {
            imap.uid_search_clauses_newest(&clause_refs)
        };
        let (uids, truncated) = match uids {
            Ok(uids) => uids,
            Err(e) => {
                let e = NetError::from(e);
                page.partial = false;
                page.offline = e.is_unreachable();
                page.errors.push(format!("Couldn't search this mailbox: {}", e.without(secret)));
                on_page(page);
                let _ = imap.logout();
                return;
            }
        };
        let mut uids = uids;
        // A higher UID is the newer message. The limit then keeps the newest, not the oldest.
        uids.sort_unstable_by(|a, b| b.cmp(a));
        let held = known.get(folder.as_str());
        let mut stopped = false;
        for uid in uids {
            if held.is_some_and(|uids| uids.contains(&uid)) {
                continue;
            }
            if hits >= MAX_HITS {
                stopped = true;
                break;
            }
            // Words past the six the server was asked, and a Gmail search that cannot say
            // `importance:low`, are checked on what came back. A word that can live only in the
            // body needs the whole message: the header's preview would hide a real match.
            let mut check = left.clone();
            if gmail {
                for clause in &asked {
                    if clause.header == Some("Importance") && clause.text == "low" {
                        check.push(Left::Importance(Importance::Low));
                    }
                }
            }
            let whole = check.iter().any(Left::needs_body);
            let fetched = match if whole { imap.uid_fetch_message(uid) } else { imap.uid_fetch_header(uid) } {
                Ok(Some(fetched)) => fetched,
                Ok(None) => continue,
                Err(e) => {
                    let e = NetError::from(e);
                    if e.is_fatal() {
                        page.partial = false;
                        page.offline = e.is_unreachable();
                        page.errors.push(format!("Couldn't search this mailbox: {}", e.without(secret)));
                        on_page(page);
                        let _ = imap.logout();
                        return;
                    }
                    continue;
                }
            };
            let parsed = mailcraft_mime_model::parse_fetched(fetched.body.as_deref().unwrap_or_default());
            if !holds(&parsed, &check) {
                continue;
            }
            if let Some(message) = message_of(folder, &fetched, &parsed, selected.uid_validity) {
                page.messages.push(message);
                hits += 1;
            }
        }
        let more_folders = at != last && hits < MAX_HITS && !stopped;
        page.partial = more_folders;
        page.capped = stopped || truncated || (hits >= MAX_HITS && at != last);
        on_page(page);
        if !more_folders {
            let _ = imap.logout();
            return;
        }
    }
    if folders.is_empty() {
        on_page(base);
    }
    let _ = imap.logout();
}

/// One restriction sent to the server: an IMAP search key and, for `HEADER`, the field name.
struct Clause {
    key: &'static str,
    header: Option<&'static str>,
    text: String,
}

/// One restriction the server was not asked, checked on the message that came back.
#[derive(Clone)]
enum Left {
    /// The text occurs somewhere in the message, the way IMAP's `TEXT` matches.
    Text(String),
    Field {
        field: ServerField,
        text: String,
    },
    HasAttachment,
    Importance(Importance),
}

impl Left {
    /// A header fetch cannot answer this: the words can sit in a part the header does not hold.
    fn needs_body(&self) -> bool {
        match self {
            Left::Text(_) | Left::Field { field: ServerField::Body | ServerField::Attachment, .. } | Left::HasAttachment => true,
            Left::Field { .. } | Left::Importance(_) => false,
        }
    }
}

/// The clauses for what the server is asked, at most [`MAX_WORDS`], and the restrictions past
/// that, which are checked on each message that comes back.
fn clauses(query: &str) -> (Vec<Clause>, Vec<Left>) {
    let mut out = Vec::new();
    let mut left = Vec::new();
    for term in Query::parse(query).for_server().terms {
        let full = out.len() >= MAX_WORDS;
        match term {
            ServerTerm::Words { field, words } => {
                let (key, header) = match field {
                    Some(ServerField::From) => ("FROM", None),
                    Some(ServerField::To) => ("TO", None),
                    Some(ServerField::Cc) => ("CC", None),
                    Some(ServerField::Subject) => ("SUBJECT", None),
                    Some(ServerField::Body) => ("BODY", None),
                    Some(ServerField::Attachment) | None => ("TEXT", None),
                };
                for word in words {
                    let word: String = word.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).take(80).collect();
                    if word.is_empty() {
                        continue;
                    }
                    if out.len() >= MAX_WORDS {
                        keep(&mut left, field, word);
                    } else {
                        out.push(Clause { key, header, text: word });
                    }
                }
            }
            ServerTerm::HasAttachment if !full => {
                // `TEXT` sees the phrase in a nested part; `HEADER` only sees the top-level header.
                out.push(Clause { key: "TEXT", header: None, text: "Content-Disposition: attachment".into() });
            }
            ServerTerm::HasAttachment => left.push(Left::HasAttachment),
            ServerTerm::Importance(importance @ (Importance::High | Importance::Low)) if !full => {
                let text = if importance == Importance::High { "high" } else { "low" };
                out.push(Clause { key: "HEADER", header: Some("Importance"), text: text.into() });
            }
            ServerTerm::Importance(importance @ (Importance::High | Importance::Low)) => left.push(Left::Importance(importance)),
            ServerTerm::Importance(_) => {}
        }
    }
    (out, left)
}

fn keep(left: &mut Vec<Left>, field: Option<ServerField>, word: String) {
    match field {
        None => left.push(Left::Text(word)),
        Some(field) => left.push(Left::Field { field, text: word }),
    }
}

/// Every restriction the server was not asked holds in `parsed`. Matching is a case-insensitive
/// substring, as IMAP's own search keys are, so a hit the server would have returned is kept.
fn holds(parsed: &Message, left: &[Left]) -> bool {
    left.iter().all(|item| match item {
        Left::Text(text) => contains_text(parsed, text),
        Left::Field { field, text } => match field {
            ServerField::From => contains_address(&parsed.from, text),
            ServerField::To => parsed.to.iter().any(|a| contains_address(a, text)),
            ServerField::Cc => parsed.cc.iter().any(|a| contains_address(a, text)),
            ServerField::Subject => contains_str(&parsed.subject, text),
            ServerField::Body => contains_str(&parsed.body.text, text),
            ServerField::Attachment => parsed.attachments.iter().any(|a| contains_str(&a.name, text)),
        },
        Left::HasAttachment => !parsed.attachments.is_empty(),
        Left::Importance(importance) => parsed.importance == *importance,
    })
}

fn contains_text(parsed: &Message, text: &str) -> bool {
    contains_str(&parsed.subject, text)
        || contains_address(&parsed.from, text)
        || parsed.to.iter().any(|a| contains_address(a, text))
        || parsed.cc.iter().any(|a| contains_address(a, text))
        || contains_str(&parsed.body.text, text)
        || parsed.attachments.iter().any(|a| contains_str(&a.name, text))
}

fn contains_address(address: &mailcraft_model::Address, text: &str) -> bool {
    contains_str(&address.name, text) || contains_str(&address.email, text)
}

fn contains_str(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

/// All Mail first when the whole Gmail mailbox is searched; otherwise Inbox first, and at most
/// [`MAX_FOLDERS`].
fn chosen(folders: &[String], gmail_all: bool) -> Vec<String> {
    if gmail_all && let Some(all) = folders.iter().find(|name| is_all_mail(name)) {
        return vec![all.clone()];
    }
    let mut names: Vec<String> = folders.iter().filter(|name| !name.is_empty()).take(MAX_FOLDERS).cloned().collect();
    names.sort_by_key(|name| !name.eq_ignore_ascii_case("INBOX"));
    names
}

fn is_all_mail(remote: &str) -> bool {
    let name = remote.rsplit('/').next().unwrap_or(remote);
    name.eq_ignore_ascii_case("All Mail") || name.eq_ignore_ascii_case("Alle Nachrichten")
}

fn gmail_query(clauses: &[Clause]) -> Option<String> {
    let mut parts = Vec::new();
    for clause in clauses {
        let Some(piece) = gmail_piece(clause) else { continue };
        parts.push(piece);
    }
    if parts.is_empty() {
        return None;
    }
    let query = parts.join(" ");
    mailcraft_proto::imap::searchable(&query).then_some(query)
}

/// Gmail's own search words for one clause. `importance:low` has no operator, so it is left out
/// and checked on the message; a bare word would match the letters instead of the importance.
fn gmail_piece(clause: &Clause) -> Option<String> {
    match (clause.key, clause.header) {
        ("FROM", _) => Some(format!("from:{}", clause.text)),
        ("TO", _) => Some(format!("to:{}", clause.text)),
        ("CC", _) => Some(format!("cc:{}", clause.text)),
        ("SUBJECT", _) => Some(format!("subject:{}", clause.text)),
        ("BODY", _) => Some(format!("body:{}", clause.text)),
        ("TEXT", _) if clause.text.eq_ignore_ascii_case("Content-Disposition: attachment") => Some("has:attachment".to_string()),
        ("TEXT", _) => Some(clause.text.clone()),
        ("HEADER", Some("Content-Disposition")) => Some("has:attachment".to_string()),
        ("HEADER", Some("Importance")) if clause.text == "high" => Some("is:important".to_string()),
        ("HEADER", Some("Importance")) => None,
        _ => Some(clause.text.clone()),
    }
}

fn message_of(remote: &str, fetched: &mailcraft_proto::imap::Fetched, parsed: &Message, validity: u32) -> Option<GraphMessage> {
    let uid = fetched.uid;
    if uid == 0 {
        return None;
    }
    let flagged = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged"));
    let read = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"));
    let draft = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"));
    let preview: String = parsed.body.text.chars().take(180).collect();
    let id = if validity == 0 { format!("imap:{uid}") } else { format!("imap:{validity}:{uid}") };
    Some(GraphMessage {
        server: ServerMessage {
            id,
            folder: remote.to_string(),
            read,
            flag: if flagged { FlagStatus::Flagged } else { FlagStatus::None },
            importance: parsed.importance,
            ..Default::default()
        },
        message_id: parsed.message_id.clone(),
        subject: parsed.subject.clone(),
        from: parsed.from.clone(),
        to: parsed.to.clone(),
        cc: parsed.cc.clone(),
        sent: parsed.date,
        received: fetched.internal_date.as_deref().and_then(internal_date).unwrap_or(parsed.date),
        preview,
        has_attachments: !parsed.attachments.is_empty(),
        draft: Some(draft),
    })
}

/// The whole of one message, asked when its row in a search is opened. `folder` is the name the
/// server uses. The secret is never written into the error.
pub fn fetch_message(account: &Account, secret: &str, folder: &str, uid: u32, validity: u32) -> Result<Message, String> {
    fetch_message_at(account, secret, folder, uid, validity).map_err(|e| e.without(secret).to_string())
}

fn fetch_message_at(account: &Account, secret: &str, folder: &str, uid: u32, validity: u32) -> Result<Message, NetError> {
    if folder.is_empty() || uid == 0 {
        return Err(NetError::Config(said!("the message is no longer on the server")));
    }
    let mut imap = open_imap(account, secret)?;
    let selected = match imap.select(&encode_utf7(folder), true) {
        Ok(selected) => selected,
        Err(e) => {
            let _ = imap.logout();
            return Err(NetError::from(e));
        }
    };
    // A UID names a message only for as long as the folder's UIDVALIDITY stays the same.
    if validity != 0 && selected.uid_validity != 0 && validity != selected.uid_validity {
        let _ = imap.logout();
        return Err(NetError::Config(said!("the message is no longer on the server")));
    }
    let size = match imap.uid_fetch_sizes(&uid.to_string()) {
        Ok(sizes) => sizes.into_iter().find(|(found, _)| *found == uid).map(|(_, size)| size).unwrap_or(0),
        Err(e) => {
            let _ = imap.logout();
            return Err(NetError::from(e));
        }
    };
    // The same limit a synchronisation uses: a larger message keeps its header, not its body.
    let large = size > MAX_MESSAGE;
    let fetched = match if large { imap.uid_fetch_header(uid) } else { imap.uid_fetch_message(uid) } {
        Ok(fetched) => fetched,
        Err(e) => {
            let _ = imap.logout();
            return Err(NetError::from(e));
        }
    };
    let _ = imap.logout();
    let Some(fetched) = fetched else {
        return Err(NetError::Config(said!("the message is no longer on the server")));
    };
    if !large && fetched.body.as_ref().is_none_or(Vec::is_empty) {
        return Err(NetError::Config(said!("the message is no longer on the server")));
    }
    Ok(if large { too_large(fetched, size, MAX_MESSAGE) } else { message_from(fetched) })
}

#[cfg(test)]
#[path = "imap_search_tests.rs"]
mod imap_search_tests;
