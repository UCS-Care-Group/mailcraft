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
    let asked = clauses(query);
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
        match imap.select(folder, true) {
            Ok(_) => {}
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
        }
        let clause_refs: Vec<(&str, Option<&str>, &str)> = asked.iter().map(|c| (c.key, c.header, c.text.as_str())).collect();
        let uids = if gmail_all && is_all_mail(folder) {
            match gmail_query(&asked) {
                Some(query) => imap.uid_search_gmail("1:*", &query),
                None => imap.uid_search_clauses(&clause_refs),
            }
        } else {
            imap.uid_search_clauses(&clause_refs)
        };
        let uids = match uids {
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
            let fetched = match imap.uid_fetch_header(uid) {
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
            if let Some(message) = message_of(folder, fetched) {
                page.messages.push(message);
                hits += 1;
            }
        }
        let more_folders = at != last && hits < MAX_HITS && !stopped;
        page.partial = more_folders;
        page.capped = stopped || (hits >= MAX_HITS && at != last);
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

/// What of the query the server is asked, with each word kept in the part the owner named.
fn clauses(query: &str) -> Vec<Clause> {
    let mut out = Vec::new();
    for term in Query::parse(query).for_server().terms {
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
                    out.push(Clause { key, header, text: word });
                    if out.len() >= MAX_WORDS {
                        return out;
                    }
                }
            }
            ServerTerm::HasAttachment => out.push(Clause { key: "HEADER", header: Some("Content-Disposition"), text: "attachment".into() }),
            ServerTerm::Importance(Importance::High) => out.push(Clause { key: "HEADER", header: Some("Importance"), text: "high".into() }),
            ServerTerm::Importance(Importance::Low) => out.push(Clause { key: "HEADER", header: Some("Importance"), text: "low".into() }),
            ServerTerm::Importance(_) => {}
        }
        if out.len() >= MAX_WORDS {
            break;
        }
    }
    out
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
        let piece = match (clause.key, clause.header) {
            ("FROM", _) => format!("from:{}", clause.text),
            ("TO", _) => format!("to:{}", clause.text),
            ("CC", _) => format!("cc:{}", clause.text),
            ("SUBJECT", _) => format!("subject:{}", clause.text),
            ("HEADER", Some("Content-Disposition")) => "has:attachment".to_string(),
            ("HEADER", Some("Importance")) if clause.text == "high" => "is:important".to_string(),
            ("BODY", _) | ("TEXT", _) => clause.text.clone(),
            _ => clause.text.clone(),
        };
        parts.push(piece);
    }
    let query = parts.join(" ");
    mailcraft_proto::imap::searchable(&query).then_some(query)
}

fn message_of(remote: &str, fetched: mailcraft_proto::imap::Fetched) -> Option<GraphMessage> {
    let uid = fetched.uid;
    if uid == 0 {
        return None;
    }
    let parsed = mailcraft_mime_model::parse_fetched(fetched.body.as_deref().unwrap_or_default());
    let flagged = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged"));
    let read = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"));
    let draft = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"));
    let preview: String = parsed.body.text.chars().take(180).collect();
    Some(GraphMessage {
        server: ServerMessage {
            id: format!("imap:{uid}"),
            folder: remote.to_string(),
            read,
            flag: if flagged { FlagStatus::Flagged } else { FlagStatus::None },
            ..Default::default()
        },
        message_id: parsed.message_id,
        subject: parsed.subject,
        from: parsed.from,
        to: parsed.to,
        cc: parsed.cc,
        sent: parsed.date,
        received: fetched.internal_date.as_deref().and_then(internal_date).unwrap_or(parsed.date),
        preview,
        has_attachments: !parsed.attachments.is_empty(),
        draft: Some(draft),
    })
}

/// The whole of one message, asked when its row in a search is opened. `folder` is the name the
/// server uses. The secret is never written into the error.
pub fn fetch_message(account: &Account, secret: &str, folder: &str, uid: u32) -> Result<Message, String> {
    fetch_message_at(account, secret, folder, uid).map_err(|e| e.without(secret).to_string())
}

fn fetch_message_at(account: &Account, secret: &str, folder: &str, uid: u32) -> Result<Message, NetError> {
    if folder.is_empty() || uid == 0 {
        return Err(NetError::Config(said!("the message is no longer on the server")));
    }
    let mut imap = open_imap(account, secret)?;
    if let Err(e) = imap.select(folder, true) {
        let _ = imap.logout();
        return Err(NetError::from(e));
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
