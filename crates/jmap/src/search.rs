// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox searched on its server (#1725: More results on the server), a page at a time, only when
//! the owner asks for it.
//!
//! - **What is asked.** `Email/query` (RFC 8621 §4.4) with a filter made of the part of the owner's query a
//!   server can be asked ([`mailcraft_model::search::Query::for_server`]): a word or a phrase is `text`,
//!   `from:`, `to:`, `cc:`, `subject:` and `body:` are the conditions of those names, `has:attachment` is
//!   `hasAttachment`, and `importance:high` or `low` the `Importance` header field. Several terms are joined
//!   by `AND`. The standard has no condition for an attachment's name, so a search with `attachment:` is not
//!   one a JMAP server is asked. A search of one mailbox says `inMailbox`; a search of the account says which
//!   mailboxes' mail stays out of it (`inMailboxOtherThan`: Deleted Items and Junk E-mail).
//! - **A page** is [`PAGE`] emails, newest first by arrival, from the position the last answer ended at
//!   ([`GraphSearchResult::next`]); then `Email/get` for what a message list shows of them, as a run asks it.
//! - **Where a found email lies** is the first of its mailboxes this computer has, by the order a run lists
//!   an email in ([`crate::mail::message::first_place`]), so that a result opened lies where the next run
//!   expects it. One that lies in none of them is passed over.
//! - **Every word goes as text**: a JSON string, with a phrase in double quotes and no quote or backslash of
//!   the owner's inside it.
//! - **Nothing a server says is trusted**: ids are held to the JMAP alphabet, the lists are capped, an entry
//!   that isn't the shape of the standard is passed over, and a position that doesn't move ends the search.
//!   The query, the addresses and what was found stay out of errors, log lines and `Debug` output.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_jmap_types::{Comparator, EmailFilterCondition, EmailQueryRequest, Filter, FilterOperator, Operator, QueryRequest};
use mailcraft_model::Importance;
use mailcraft_model::search::{Query, ServerField, ServerTerm};
use mailcraft_model::sync::{GraphSearchResult, JmapSearchRequest, MAX_SEARCH_FOLDERS};
use serde_json::Value;

use crate::connection::busy_seconds;
use crate::mail::message::{Marks, first_place, whole};
use crate::mail::{DETAILS_BYTES, EMAIL_PROPERTIES, WINDOW_BYTES, arguments, get, rank, said, secrets_of};
use crate::{ApiRequest, CAPABILITY_MAIL, Connection, Error, Failure, HttpExchange, Id, Redactor, TransportError};

/// Emails asked for to a page.
pub const PAGE: usize = 50;
/// The longest text of one condition, in characters.
const MAX_TEXT: usize = 256;
/// The furthest position a search is followed to.
const MAX_POSITION: u64 = 100_000;

/// The words of a term as the text of a condition: one word as it is, several as a phrase in double quotes.
/// Quotes, backslashes and control characters are taken out of each word.
fn text_of(words: &[String]) -> Option<String> {
    let clean = |word: &String| word.chars().filter(|c| !c.is_control() && !matches!(c, '"' | '\\')).collect::<String>();
    let words: Vec<String> = words.iter().map(clean).filter(|word| !word.trim().is_empty()).collect();
    let text: String = words.join(" ").chars().take(MAX_TEXT).collect();
    match words.len() {
        0 => None,
        1 => Some(text),
        _ => Some(format!("\"{text}\"")),
    }
}

/// One term as a condition; `None` when the standard has none for it.
fn condition(term: &ServerTerm) -> Option<EmailFilterCondition> {
    let mut condition = EmailFilterCondition::default();
    match term {
        ServerTerm::Words { field, words } => {
            let text = Some(text_of(words)?);
            match field {
                None => condition.text = text,
                Some(ServerField::From) => condition.from = text,
                Some(ServerField::To) => condition.to = text,
                Some(ServerField::Cc) => condition.cc = text,
                Some(ServerField::Subject) => condition.subject = text,
                Some(ServerField::Body) => condition.body = text,
                Some(ServerField::Attachment) => return None,
            }
        }
        ServerTerm::HasAttachment => condition.has_attachment = Some(true),
        ServerTerm::Importance(Importance::High) => condition.header = Some(vec!["Importance".into(), "high".into()]),
        ServerTerm::Importance(Importance::Low) => condition.header = Some(vec!["Importance".into(), "low".into()]),
        ServerTerm::Importance(Importance::Normal) => return None,
    }
    Some(condition)
}

/// The filter for `request`'s search: where to look, and every term of the query a server can be asked.
/// `None` when a term can't be said, or there is none.
pub(crate) fn filter_of(request: &JmapSearchRequest) -> Option<Filter<EmailFilterCondition>> {
    let terms = Query::parse(&request.query).for_server().terms;
    let mut conditions: Vec<EmailFilterCondition> = terms.iter().map(condition).collect::<Option<_>>()?;
    if conditions.is_empty() {
        return None;
    }
    let valid = |id: &&String| Id::new(id.as_str()).is_ok();
    if !request.folder.is_empty() {
        let mailbox = Some(&request.folder).filter(valid)?;
        conditions.insert(0, EmailFilterCondition { in_mailbox: Some(mailbox.clone()), ..Default::default() });
    } else {
        let outside: Vec<String> = request.outside.iter().filter(valid).take(MAX_SEARCH_FOLDERS).cloned().collect();
        if !outside.is_empty() {
            conditions.insert(0, EmailFilterCondition { in_mailbox_other_than: Some(outside), ..Default::default() });
        }
    }
    Some(match conditions.len() {
        1 => Filter::Condition(conditions.pop()?),
        _ => Filter::Operator(FilterOperator { operator: Operator::And, conditions: conditions.into_iter().map(Filter::Condition).collect() }),
    })
}

/// The answer a search begins from.
fn base(request: &JmapSearchRequest) -> GraphSearchResult {
    GraphSearchResult { account: request.account, email: request.email.clone(), ticket: request.ticket, ..Default::default() }
}

/// One page of `request`'s search. `cancel` is read before each of its two requests. The connection is
/// the caller's: it connected with the address and the secret the request goes with.
pub async fn search_messages<H: HttpExchange>(connection: &Connection<H>, request: &JmapSearchRequest, cancel: &AtomicBool) -> GraphSearchResult {
    let mut out = base(request);
    let client = connection.client();
    let secrets = secrets_of(&request.email, &request.session, client.redactor(), client.account_id().as_str());
    let Some(filter) = filter_of(request) else {
        out.errors.push("Nothing of this search can be asked of the server.".into());
        return out;
    };
    // Where the last page ended; anything that is no position begins at the start.
    let position = request.next.trim().parse::<u64>().ok().filter(|at| *at <= MAX_POSITION).unwrap_or(0);
    if cancel.load(Ordering::Relaxed) {
        return out;
    }
    let found = match query(connection, client.account_id().as_str(), filter, position, &secrets).await {
        Ok(found) => found,
        Err(e) => {
            stop(&mut out, &e, &secrets, position);
            return out;
        }
    };
    // The ids of the page, each once, as many as were asked for.
    let mut seen = HashSet::new();
    let listed = found.get("ids").and_then(Value::as_array).map_or(0, |ids| ids.len().min(PAGE));
    let ids: Vec<&str> = found
        .get("ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(PAGE)
        .filter_map(Value::as_str)
        .filter(|id| Id::new(id).is_ok() && seen.insert(*id))
        .collect();
    // More to come when the server says it has more than this page reaches, or says nothing and gave a
    // full page; never when the page listed nothing.
    let reached = position.saturating_add(u64::try_from(listed).unwrap_or(u64::MAX));
    let more = match found.get("total").and_then(Value::as_u64) {
        Some(total) => total > reached,
        None => listed >= PAGE,
    };
    if more && listed > 0 && reached <= MAX_POSITION {
        out.next = reached.to_string();
    }
    if ids.is_empty() || cancel.load(Ordering::Relaxed) {
        if !ids.is_empty() {
            // Called off between the two requests: this page is asked for again.
            out.next = position.to_string();
        }
        return out;
    }
    let args = match arguments(&get(client.account_id().as_str(), Some(&ids), &EMAIL_PROPERTIES)) {
        Ok(args) => args,
        Err(e) => {
            stop(&mut out, &Failure::Request(e.into()), &secrets, position);
            return out;
        }
    };
    let got = match one_call(connection, "Email/get", args, DETAILS_BYTES, &secrets).await {
        Ok(got) => got,
        Err(e) => {
            stop(&mut out, &e, &secrets, position);
            return out;
        }
    };
    // The mailboxes an email may be said to lie in, with their rank: the first of them lists it.
    let mut claimable: HashMap<String, usize> = HashMap::new();
    for place in request.places.iter().filter(|place| Id::new(&place.id).is_ok()).take(MAX_SEARCH_FOLDERS) {
        claimable.entry(place.id.clone()).or_insert_with(|| rank(place.kind));
    }
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for email in got.get("list").and_then(Value::as_array).into_iter().flatten().take(ids.len()) {
        if let Some(id) = email.get("id").and_then(Value::as_str) {
            by_id.entry(id).or_insert(email);
        }
    }
    for id in &ids {
        let Some(email) = by_id.get(id) else { continue };
        let Some(mailbox) = first_place(email.get("mailboxIds"), Some(&claimable)) else { continue };
        out.messages.push(whole(id, mailbox, email, Marks::of(email.get("keywords"))));
    }
    out.messages.sort_by_key(|m| std::cmp::Reverse(m.received));
    log::info!("jmap search: {} found on a page of {listed}, more to come: {}", out.messages.len(), !out.next.is_empty());
    out
}

/// `Email/query` for the page at `position`.
async fn query<H: HttpExchange>(
    connection: &Connection<H>,
    account: &str,
    filter: Filter<EmailFilterCondition>,
    position: u64,
    secrets: &Redactor,
) -> Result<Value, Failure> {
    let args = arguments(&EmailQueryRequest {
        query: QueryRequest {
            account_id: account.to_owned(),
            filter: Some(filter),
            sort: Some(vec![Comparator { property: "receivedAt".to_owned(), is_ascending: Some(false), ..Default::default() }]),
            position: Some(i64::try_from(position).unwrap_or(i64::MAX)),
            anchor: None,
            anchor_offset: None,
            limit: Some(u64::try_from(PAGE).unwrap_or(u64::MAX)),
            calculate_total: Some(true),
        },
        collapse_threads: None,
    })
    .map_err(|e| Failure::Request(e.into()))?;
    one_call(connection, "Email/query", args, WINDOW_BYTES, secrets).await
}

/// A request of the one call `method` with `args`, and its answer, held to `most` bytes.
async fn one_call<H: HttpExchange>(
    connection: &Connection<H>,
    method: &str,
    args: serde_json::Map<String, Value>,
    most: usize,
    secrets: &Redactor,
) -> Result<Value, Failure> {
    let mut request = ApiRequest::new();
    request.using(CAPABILITY_MAIL).map_err(|e| Failure::Request(e.into()))?;
    request.set_response_limit(most);
    let call = request.add_call(method, args).map_err(|e| Failure::Request(e.into()))?;
    let mut answers = connection.call(&request, secrets).await?;
    Ok(Value::Object(answers.take_named(&call, method)?))
}

/// The answer of a search that could not begin: signing in to the server ([`Connection::connect`]) failed
/// with `error`.
pub fn failed_search(request: &JmapSearchRequest, error: &Error) -> GraphSearchResult {
    let mut out = base(request);
    let position = request.next.trim().parse::<u64>().ok().unwrap_or(0);
    stop(&mut out, &Failure::Request(error.clone()), &secrets_of(&request.email, &request.session, &Redactor::new(), ""), position);
    out
}

/// Say that the search stopped, and why, with `secrets` hidden and no word that holds an address; and
/// where it stopped, so that asking again asks for the same page.
fn stop(out: &mut GraphSearchResult, e: &Failure, secrets: &Redactor, position: u64) {
    match e {
        Failure::Request(Error::Transport(TransportError::Aborted)) => return,
        Failure::Request(error @ Error::Busy { .. }) => out.busy_for = Some(busy_seconds(error).unwrap_or(0)),
        Failure::Request(Error::Transport(failure)) => out.offline = !matches!(failure, TransportError::Tls | TransportError::RedirectRefused),
        _ => {}
    }
    out.next = if position > 0 { position.to_string() } else { String::new() };
    out.errors.push(format!("Couldn't search this mailbox: {}", said(secrets, e)));
}

#[cfg(test)]
mod tests;
