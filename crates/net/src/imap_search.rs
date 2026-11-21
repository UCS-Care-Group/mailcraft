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
/// Most messages fetched whole so a restriction the server was not asked can be checked. Past
/// this the page says the search was cut short.
const MAX_CHECKED: usize = 200;

/// Search `folders` of `account` (names as the server knows them) for `query`. `known` is the
/// UIDs already on this computer whose body is here, by folder name, with that folder's
/// UIDVALIDITY (0 when it was not known). They are not counted toward the hit limit, and only
/// while that validity is still the server's: after a folder is renumbered the old UIDs name
/// other messages. A message whose body is not here is not among them, so a match that lives
/// only in that body is not missed. Each call of `on_page` is one folder's hits, or the
/// failure; the last call has `partial` false.
pub fn search(
    account: &Account,
    secret: &str,
    ticket: u64,
    folders: &[String],
    query: &str,
    known: &HashMap<String, (u32, HashSet<u32>)>,
    mut on_page: impl FnMut(GraphSearchResult),
) {
    let mut base = GraphSearchResult { account: account.id, email: account.email.clone(), ticket, ..Default::default() };
    let (asked, left) = clauses(query);
    // Nothing to ask and nothing to check here: not an error. The list stays what is already here.
    if asked.is_empty() && left.is_empty() {
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
    let (folders, folders_left) = chosen(folders);
    let mut hits = 0usize;
    let mut checked = 0usize;
    let last = folders.len().saturating_sub(1);
    for (at, folder) in folders.iter().enumerate() {
        if hits >= MAX_HITS {
            break;
        }
        let mut page = base.clone();
        page.errors.clear();
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
        // `has:attachment` is not sent: some servers' `TEXT` never sees a nested part's headers,
        // and a message that only quotes the phrase would be a false hit. It is checked on what
        // comes back. With nothing else to ask, every message that is not deleted is a candidate.
        let uids = if asked.is_empty() { imap.uid_search_undeleted() } else { imap.uid_search_clauses_newest(&clause_refs) };
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
        // A UID names a message only while the folder's UIDVALIDITY is the one it was read under.
        // Validity 0 was not known here, so those UIDs stay skipped.
        let held = known.get(folder.as_str()).filter(|(validity, _)| *validity == 0 || *validity == selected.uid_validity).map(|(_, uids)| uids);
        let mut stopped = false;
        // A message larger than a synchronisation will fetch cannot be checked for a word that
        // lives in its body. It is left out, and the page says the search was cut short.
        let mut oversized = false;
        for uid in uids {
            if held.is_some_and(|uids| uids.contains(&uid)) {
                continue;
            }
            if hits >= MAX_HITS {
                stopped = true;
                break;
            }
            // Words past the six the server was asked, and `has:attachment`, are checked on what
            // came back. A word that can live only in the body needs the whole message: the
            // header's preview would hide a real match. Each such message is one of [`MAX_CHECKED`].
            let check = left.clone();
            if !check.is_empty() {
                if checked >= MAX_CHECKED {
                    page.capped = true;
                    page.partial = false;
                    on_page(page);
                    let _ = imap.logout();
                    return;
                }
                checked += 1;
            }
            let whole = check.iter().any(Left::needs_body);
            if whole {
                let size = match imap.uid_fetch_sizes(&uid.to_string()) {
                    Ok(sizes) => sizes.into_iter().find(|(found, _)| *found == uid).map(|(_, size)| size).unwrap_or(0),
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
                // The same limit a synchronisation uses. Past it, the body is not fetched: a
                // header cannot answer a word that may sit only in the body, and one huge
                // message must not end the search.
                if size > MAX_MESSAGE {
                    oversized = true;
                    continue;
                }
            }
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
        page.capped = stopped || truncated || oversized || folders_left || (hits >= MAX_HITS && at != last);
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
                // A bare word is `TEXT`. `attachment:` is not sent: that key matches the body
                // too, and a file's name is checked on the message that comes back.
                let sent = match field {
                    Some(ServerField::From) => Some(("FROM", None)),
                    Some(ServerField::To) => Some(("TO", None)),
                    Some(ServerField::Cc) => Some(("CC", None)),
                    Some(ServerField::Subject) => Some(("SUBJECT", None)),
                    Some(ServerField::Body) => Some(("BODY", None)),
                    None => Some(("TEXT", None)),
                    Some(ServerField::Attachment) => None,
                };
                for word in words {
                    let word: String = word.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).take(80).collect();
                    if word.is_empty() {
                        continue;
                    }
                    // `attachment:` names a file. Sending it as `TEXT` would count a body that
                    // merely says the word. It is checked on the names that come back.
                    if sent.is_none() || out.len() >= MAX_WORDS {
                        keep(&mut left, field, word);
                    } else if let Some((key, header)) = sent {
                        out.push(Clause { key, header, text: word });
                    }
                }
            }
            // Not sent. A server's `TEXT` may not see the headers of a nested part, so the search
            // would find nothing and say nothing, and a message that quotes the phrase would be a
            // hit. The messages the other clauses find (or every message, when this is the whole
            // search) are fetched and kept only when they have an attachment.
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
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// Inbox first, then at most [`MAX_FOLDERS`]. Putting Inbox first before the limit means a long
/// folder list cannot leave it out. `true` when eligible folders were left out.
fn chosen(folders: &[String]) -> (Vec<String>, bool) {
    let mut names: Vec<String> = folders.iter().filter(|name| !name.is_empty()).cloned().collect();
    let eligible = names.len();
    names.sort_by_key(|name| !name.eq_ignore_ascii_case("INBOX"));
    names.truncate(MAX_FOLDERS);
    (names, eligible > MAX_FOLDERS)
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
