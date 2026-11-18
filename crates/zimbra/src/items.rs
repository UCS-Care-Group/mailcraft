// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages as headers and a preview (`SearchRequest` for messages): a page of one folder's, newest
//! first, or the messages a delta named, by id.
//!
//! What a message is said to be comes from its summary: `id`, `l` (its folder), `f` (its flags:
//! `u` unread, `f` flagged, `r` replied, `w` forwarded, `d` draft, `a` with attachments, `!` and
//! `?` high and low importance), `tn` (its tags, which are categories here), `d` (when it arrived,
//! in milliseconds), `s` (its size), `su`, `fr` (the preview), `mid` (its `Message-ID`) and `e`
//! (the people: `f` from, `t` to, `c` cc).

use mailcraft_model::sync::{GraphMessage, ServerMessage};
use mailcraft_model::{Address, FlagStatus, Importance};
use serde_json::{Value, json};

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::json;
use crate::soap;

/// Messages one page of a folder holds.
pub const PAGE: usize = 250;
/// Most messages asked for by id at once.
pub const BY_ID: usize = 100;
/// Most people taken of a message, and categories.
const MAX_PEOPLE: usize = 200;
const MAX_CATEGORIES: usize = 50;

/// One page of messages, with what the next page's cursor is made of.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    /// The messages, as headers and a preview.
    pub messages: Vec<GraphMessage>,
    /// More messages wait.
    pub more: bool,
    /// The last item's id and sort value (`d`, in milliseconds): asked for as the next page's
    /// `cursor`, so that a page never depends on how many messages lie before it (an offset moves
    /// when another program deletes or delivers while the folder is read, and a message between
    /// the pages would be missed for good).
    pub cursor: Option<(String, u64)>,
}

/// The page of folder `folder`'s messages that lies behind `cursor` (its last item's id and sort
/// value), or from the newest when there is none.
pub fn folder_page(base: &str, token: &Secret, folder: &str, cursor: Option<(&str, u64)>) -> Result<HttpRequest, Error> {
    let mut body = json!({"query": format!("inid:\"{folder}\""), "types": "message", "sortBy": "dateDesc", "limit": PAGE, "recip": "2"});
    if let (Some((id, sort)), Value::Object(map)) = (cursor, &mut body) {
        map.insert("cursor".into(), json!({"id": id, "sortVal": sort.to_string()}));
    }
    soap::request(base, "SearchRequest", soap::MAIL, body, Some(token))
}

/// The messages `ids` (own ids, at most [`BY_ID`]), wherever they are.
pub fn by_id(base: &str, token: &Secret, ids: &[String]) -> Result<HttpRequest, Error> {
    by_id_in(base, token, ids, true)
}

/// One message `id` asks for again, this time by its own name: `in:anywhere` makes the server walk
/// every folder to scope the search, and a folder it walks that is already gone faults the request
/// no matter which id it names — while `item:{id}` alone resolves the id itself.
pub fn by_id_alone(base: &str, token: &Secret, id: &str) -> Result<HttpRequest, Error> {
    by_id_in(base, token, &[id.to_string()], false)
}

/// The messages `ids` (own ids, at most [`BY_ID`]), scoped `anywhere` or asked for by name alone.
fn by_id_in(base: &str, token: &Secret, ids: &[String], anywhere: bool) -> Result<HttpRequest, Error> {
    let ids: Vec<&str> = ids.iter().map(String::as_str).filter(|id| id.bytes().all(|b| b.is_ascii_digit())).take(BY_ID).collect();
    // `in:anywhere`: a search without it leaves Trash and Junk out, and an id a delta named that
    // doesn't come back would be dropped while the run's token moves on.
    let query = format!("{}item:{{{}}}", if anywhere { "in:anywhere " } else { "" }, ids.join(","));
    let body = json!({"query": query, "types": "message", "limit": BY_ID, "recip": "2"});
    soap::request(base, "SearchRequest", soap::MAIL, body, Some(token))
}

/// The messages of a `SearchResponse`, whether there are more after them, and the last item's
/// place in the sort order (the next page's cursor; `None` when the page holds nothing that can
/// carry one).
pub fn answer(response: &HttpResponse) -> Result<Page, Error> {
    let answer = soap::answer("SearchRequest", response)?;
    if json::bad_list(answer.get("m")) {
        return Err(Error::Unreadable);
    }
    let listed = json::list(answer.get("m"));
    let mut messages = Vec::new();
    for m in listed.iter().take(PAGE) {
        if json::own_id(m.get("id")).is_none() {
            continue;
        }
        match m.get("l") {
            // An item that names no folder at all can't be told moved or removed: an empty
            // folder would have the run count it deleted while the token moves past the change.
            None => return Err(Error::Unreadable),
            // A folder of another mailbox: the item is passed over like a foreign id.
            Some(l) if json::own_id(Some(l)).is_none() => continue,
            _ => {}
        }
        if json::bad_list(m.get("e")) {
            return Err(Error::Unreadable);
        }
        if let Some(message) = message(m) {
            messages.push(message);
        }
    }
    let cursor = listed.iter().take(PAGE).next_back().and_then(|m| {
        let id = json::line(m.get("id"));
        // The cursor's sort value is the hit's `sf` (what the server sorted the page by, what a
        // `sortVal` continues from), its `d` when it has none: a last item with neither is no
        // cursor to go on from — a sort value made up as 0 could end the folder early or read it
        // again from its start.
        let sort = json::number(m.get("sf")).or_else(|| json::number(m.get("d")))?;
        (!id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)).then_some((id, sort))
    });
    Ok(Page { messages, more: json::on(answer.get("more")), cursor })
}

/// A message as the model has one, from its summary; `None` for one that has no own id.
pub fn message(m: &Value) -> Option<GraphMessage> {
    let id = json::own_id(m.get("id"))?;
    let flags = json::line(m.get("f"));
    let has = |c: char| flags.contains(c);
    let people = |kind: &str| -> Vec<Address> {
        json::list(m.get("e"))
            .iter()
            .filter(|e| json::line(e.get("t")) == kind)
            .take(MAX_PEOPLE)
            .map(|e| Address::new(json::line(e.get("p")), json::line(e.get("a"))))
            .filter(|a| !a.email.is_empty())
            .collect()
    };
    let categories: Vec<String> =
        json::line(m.get("tn")).split(',').map(str::trim).filter(|t| !t.is_empty()).take(MAX_CATEGORIES).map(String::from).collect();
    let arrived = json::number(m.get("d")).map_or(0, |ms| i64::try_from(ms / 1000).unwrap_or(0));
    let sent = json::number(m.get("sd")).map_or(arrived, |ms| i64::try_from(ms / 1000).unwrap_or(arrived));
    let server = ServerMessage {
        id,
        folder: json::own_id(m.get("l")).unwrap_or_default(),
        read: !has('u'),
        answered: has('r'),
        forwarded: has('w'),
        flag: if has('f') { FlagStatus::Flagged } else { FlagStatus::None },
        categories,
        importance: if has('!') {
            Importance::High
        } else if has('?') {
            Importance::Low
        } else {
            Importance::Normal
        },
        size: json::number(m.get("s")).unwrap_or(0),
        thread: json::own_id(m.get("cid")).unwrap_or_default(),
        ..ServerMessage::default()
    };
    Some(GraphMessage {
        server,
        message_id: json::line(m.get("mid")).trim_matches(['<', '>']).to_string(),
        subject: json::line(m.get("su")),
        from: people("f").into_iter().next().unwrap_or_default(),
        to: people("t"),
        cc: people("c"),
        sent,
        received: arrived,
        preview: json::line(m.get("fr")),
        has_attachments: has('a'),
        draft: Some(has('d')),
    })
}
