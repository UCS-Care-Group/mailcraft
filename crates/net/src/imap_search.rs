// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A search of an IMAP account on its own server, asked when the owner searches.
//!
//! One connection per account. The account's folders are read one after another on that
//! connection, and each folder's hits are handed back as soon as they arrive (`partial`), so a
//! window can show them while another account, on its own connection, is still being searched.
//! Gmail's whole mailbox is one search of All Mail (`X-GM-RAW`), which sees mail that was never
//! downloaded. Nothing of the query or of a password is written to a log line.

use crate::NetError;
use crate::sync::{internal_date, open_imap};
use mailcraft_i18n::said;
use mailcraft_model::search::{Query, ServerTerm};
use mailcraft_model::sync::{GraphMessage, GraphSearchResult, ServerMessage};
use mailcraft_model::{Account, FlagStatus, Message};

/// Most folders of one account asked in one search, and most hits brought back from it.
const MAX_FOLDERS: usize = 24;
const MAX_HITS: usize = 40;
/// Most words of the query sent. The rest is checked here against what comes back.
const MAX_WORDS: usize = 6;

/// Search `folders` of `account` (names as the server knows them) for `query`. `gmail_all`: the
/// account is Gmail and the search is the whole mailbox, so All Mail is asked once. Each call of
/// `on_page` is one folder's hits, or the failure; the last call has `partial` false.
pub fn search(
    account: &Account,
    secret: &str,
    ticket: u64,
    folders: &[String],
    query: &str,
    gmail_all: bool,
    mut on_page: impl FnMut(GraphSearchResult),
) {
    let mut base = GraphSearchResult { account: account.id, email: account.email.clone(), ticket, ..Default::default() };
    let words = needles(query);
    if words.is_empty() {
        base.errors.push("Nothing of this search can be asked of the server.".into());
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
        let uids = if gmail_all && is_all_mail(folder) {
            match gmail_query(&words) {
                Some(query) => imap.uid_search_gmail("1:*", &query),
                None => imap.uid_search_words(&words.iter().map(String::as_str).collect::<Vec<_>>()),
            }
        } else {
            imap.uid_search_words(&words.iter().map(String::as_str).collect::<Vec<_>>())
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
        for uid in uids {
            if hits >= MAX_HITS {
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
        let more_folders = at != last && hits < MAX_HITS;
        page.partial = more_folders;
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

/// The words the server is asked for: what of the query a server's search understands, bare.
fn needles(query: &str) -> Vec<String> {
    let mut words = Vec::new();
    for term in Query::parse(query).for_server().terms {
        let ServerTerm::Words { words: raw, .. } = term else { continue };
        for word in raw {
            let word: String = word.chars().filter(|c| !matches!(c, '\r' | '\n' | '\0')).take(80).collect();
            if word.is_empty() {
                continue;
            }
            words.push(word);
            if words.len() >= MAX_WORDS {
                return words;
            }
        }
    }
    words
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

fn gmail_query(words: &[String]) -> Option<String> {
    let query = words.join(" ");
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
    let fetched = match imap.uid_fetch_message(uid) {
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
    if fetched.body.as_ref().is_none_or(Vec::is_empty) {
        return Err(NetError::Config(said!("the message is no longer on the server")));
    }
    Ok(read_fetched(fetched))
}

fn read_fetched(fetched: mailcraft_proto::imap::Fetched) -> Message {
    let mut message = mailcraft_mime_model::parse_fetched(fetched.body.as_deref().unwrap_or_default());
    message.uid = fetched.uid;
    message.read = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"));
    message.flag = if fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged")) { FlagStatus::Flagged } else { FlagStatus::None };
    message.answered = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Answered"));
    message.draft = fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Draft"));
    message.forwarded |= fetched.flags.iter().any(|f| f.eq_ignore_ascii_case("$Forwarded"));
    message.received = fetched.internal_date.as_deref().and_then(internal_date).unwrap_or(message.date);
    message
}

#[cfg(test)]
#[path = "imap_search_tests.rs"]
mod imap_search_tests;
