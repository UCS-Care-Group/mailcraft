// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's search of messages (R6.2), as Graph v1.0 documents `$search` on messages
//! ("Use the $search query parameter in Microsoft Graph APIs", "List messages"):
//!
//! - `GET /v1.0/me/messages?$search="…"` searches every folder of the mailbox (Deleted Items and
//!   Junk Email too), `GET /v1.0/me/mailFolders/{id}/messages?$search="…"` that folder alone (not
//!   what lies under it); the same at `/v1.0/users/{address}/…` for another mailbox (#70).
//! - The value is one string in double quotes, `"budget AND from:ada"`: clauses `word`,
//!   `"a phrase"` (its quotes written `\"` inside the value), and `property:value` with the
//!   properties the reference lists (`from`, `to`, `cc`, `bcc`, `subject`, `body`, `attachment`,
//!   `participants`, `recipients`, `hasAttachments`, `importance`); clauses are joined by `AND`,
//!   or by nothing, which is the same. A clause with no property looks in the sender, the subject
//!   and the body, as the reference says. `OR`, `NOT`, parentheses and any other property are not
//!   answered here (400): the client is to send none of them.
//! - A word matches the start of a word of the property, whatever its case; a phrase, its words
//!   one after another. A property of people (`from`, `to`, `cc`, `bcc`, `participants`,
//!   `recipients`) given a whole address (`from:ada@example.org`, `from:"ada+news@example.org"`)
//!   matches that address, whatever its case, and no other (#301).
//! - Results are sorted by when each message was sent, newest first, and there are at most 1,000
//!   of them; they come in pages (`$top`, 1 to 1,000) with an `@odata.nextLink` that carries the
//!   rest of the request. `$select` names message properties only. `$filter` and `$orderby` are
//!   refused beside `$search` (400): the reference shows neither with it for messages, so a client
//!   counts on neither. No `ConsistencyLevel` header is needed (that is for directory objects).
//!
//! [`State::search_refused`] answers every search with a status and a code instead (a mailbox
//! that won't be searched just now, a 403 by Graph's code…).

use serde_json::Value;

use super::{Answer, Asked, Refusal, State, bad, refuse};

/// Most results one search gives, as the reference has it.
const MOST: usize = 1_000;

/// What a clause asks for.
#[derive(Debug, PartialEq, Eq)]
enum Clause {
    /// Words one after another, in the properties named.
    Words {
        properties: Vec<&'static str>,
        words: Vec<String>,
    },
    /// One whole address, in small letters, among the people of the properties named.
    Address {
        properties: Vec<&'static str>,
        address: String,
    },
    HasAttachments(bool),
    Importance(&'static str),
}

/// Whether `text` is one whole address, as MailCraft's search has it, so that a test of both
/// finds the same mail ([`mailcraft_model::search::is_address`]).
fn is_whole_address(text: &str) -> bool {
    mailcraft_model::search::is_address(text)
}

/// The words of `text` as a search reads them: runs of letters and digits, in lower case.
fn words_of(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Whether `wanted` (lower-case words) starts words of `text` one after another.
fn found_in(text: &str, wanted: &[String]) -> bool {
    let have = words_of(text);
    if wanted.is_empty() {
        return false;
    }
    (0..have.len()).any(|at| wanted.iter().enumerate().all(|(k, w)| have.get(at + k).is_some_and(|h| h.starts_with(w.as_str()))))
}

/// The pieces of a `$search` value: what lies inside its quotes, then each clause, a property
/// and its text (a phrase's quotes undone). `Err` for what isn't one this stand-in answers.
fn clauses(value: &str) -> Result<Vec<Clause>, Refusal> {
    let inner = match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(inner) if value.len() >= 2 => inner,
        _ => value,
    };
    // Split on spaces outside phrases; `\"` opens and closes a phrase, `\\` is a backslash.
    let mut pieces: Vec<(String, bool)> = Vec::new();
    let (mut current, mut phrase, mut quoted) = (String::new(), false, false);
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('"') => {
                    phrase = !phrase;
                    quoted = true;
                }
                Some('\\') => current.push('\\'),
                _ => return Err(bad("A backslash in a $search value comes before a double quote or a backslash.")),
            },
            '"' => return Err(bad("A double quote inside a $search value is written \\\".")),
            '(' | ')' if !phrase => return Err(bad("Parentheses are not answered by this stand-in.")),
            c if c.is_whitespace() && !phrase => {
                if !current.is_empty() || quoted {
                    pieces.push((std::mem::take(&mut current), quoted));
                }
                quoted = false;
            }
            c => current.push(c),
        }
    }
    if phrase {
        return Err(bad("A phrase in the $search value isn't closed."));
    }
    if !current.is_empty() || quoted {
        pieces.push((current, quoted));
    }
    let mut out = Vec::new();
    for (piece, quoted) in pieces {
        match piece.as_str() {
            "AND" if !quoted => continue,
            "OR" | "NOT" if !quoted => return Err(bad(format!("{piece} is not answered by this stand-in."))),
            _ => {}
        }
        let (property, text) = match piece.split_once(':') {
            Some((property, text)) if !property.is_empty() && property.chars().all(|c| c.is_ascii_alphabetic()) => {
                (Some(property.to_ascii_lowercase()), text.to_string())
            }
            _ => (None, piece.clone()),
        };
        let words = words_of(&text);
        let clause = match property.as_deref() {
            None => Clause::Words { properties: vec!["from", "subject", "body"], words },
            Some("hasattachments" | "hasattachment") => match text.to_ascii_lowercase().as_str() {
                "true" => Clause::HasAttachments(true),
                "false" => Clause::HasAttachments(false),
                _ => return Err(bad("hasAttachments is true or false.")),
            },
            Some("importance") => match text.to_ascii_lowercase().as_str() {
                "low" => Clause::Importance("low"),
                "medium" | "normal" => Clause::Importance("normal"),
                "high" => Clause::Importance("high"),
                _ => return Err(bad("importance is low, medium or high.")),
            },
            Some(named) => {
                let properties: Vec<&'static str> = match named {
                    "from" => vec!["from"],
                    "to" => vec!["to"],
                    "cc" => vec!["cc"],
                    "bcc" => vec!["bcc"],
                    "subject" => vec!["subject"],
                    "body" => vec!["body"],
                    "attachment" => vec!["attachment"],
                    "participants" => vec!["from", "to", "cc", "bcc"],
                    "recipients" => vec!["to", "cc", "bcc"],
                    _ => return Err(bad(format!("'{named}' is not a property this stand-in searches."))),
                };
                let people = properties.iter().all(|p| ["from", "to", "cc", "bcc"].contains(p));
                match people && is_whole_address(&text) {
                    true => Clause::Address { properties, address: mailcraft_model::search::address_key(&text) },
                    false => Clause::Words { properties, words },
                }
            }
        };
        if matches!(&clause, Clause::Words { words, .. } if words.is_empty()) {
            return Err(bad("A clause of the $search value has no words in it."));
        }
        out.push(clause);
    }
    if out.is_empty() {
        return Err(bad("The $search value has nothing to search for."));
    }
    Ok(out)
}

/// Whether `m` has what `clause` asks for.
fn matches(m: &super::Message, clause: &Clause) -> bool {
    match clause {
        Clause::HasAttachments(has) => m.attachments.iter().any(|a| !a.inline) == *has,
        Clause::Importance(importance) => m.importance == *importance,
        Clause::Address { properties, address } => properties.iter().any(|property| {
            let people = |list: &[String]| list.iter().any(|p| mailcraft_model::search::address_key(p) == *address);
            match *property {
                "from" => people(std::slice::from_ref(&m.from)),
                "to" => people(&m.to),
                "cc" => people(&m.cc),
                "bcc" => people(&m.bcc),
                _ => false,
            }
        }),
        Clause::Words { properties, words } => properties.iter().any(|property| {
            let people = |list: &[String]| list.iter().any(|p| found_in(p, words) || found_in(p.split('@').next().unwrap_or(""), words));
            match *property {
                "from" => people(std::slice::from_ref(&m.from)),
                "to" => people(&m.to),
                "cc" => people(&m.cc),
                "bcc" => people(&m.bcc),
                "subject" => found_in(&m.subject, words),
                "body" => found_in(&m.text(), words),
                "attachment" => m.attachments.iter().any(|a| found_in(&a.name, words)),
                _ => false,
            }
        }),
    }
}

impl State {
    /// `GET …/messages?$search=…` of the mailbox, or of the folder `folder` (an id or a well-known
    /// name).
    pub(super) fn search_messages(&mut self, asked: &Asked, folder: Option<&str>) -> Result<Answer, Refusal> {
        if let Some((status, code)) = self.search_refused {
            return Err(refuse(status, code, "Refused, as asked."));
        }
        if let Some((other, _)) = asked.query.iter().find(|(name, _)| !["$search", "$select", "$top", "$skip"].contains(&name.as_str())) {
            return Err(bad(format!("The query parameter '{other}' is not answered beside $search by this stand-in.")));
        }
        let value = asked.param("$search").unwrap_or("");
        let clauses = clauses(value)?;
        let folder = match folder {
            Some(named) => Some(self.mail_folder(named)?.id.clone()),
            None => None,
        };
        let look = State::look_of(asked, true)?;
        let mut found: Vec<&super::Message> = self
            .mailbox
            .messages
            .values()
            .filter(|m| folder.as_ref().is_none_or(|f| m.folder == *f))
            .filter(|m| clauses.iter().all(|clause| matches(m, clause)))
            .collect();
        // Sent (30 seconds before it arrived, as the stand-in has it), newest first.
        found.sort_by(|a, b| b.received.cmp(&a.received).then_with(|| a.id.cmp(&b.id)));
        let ids: Vec<String> = found.into_iter().take(MOST).map(|m| m.id.clone()).collect();
        let list: Vec<Value> = ids.iter().map(|id| self.message_json(id, &look)).collect();
        self.searches = self.searches.saturating_add(1);
        self.page(asked, list)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use serde_json::Value;

    use super::{Clause, clauses, found_in, words_of};
    use crate::graph::standin::{StandIn, TOKEN};
    use crate::http;

    const NOW: i64 = 1_790_000_000;

    /// One GET with immutable ids: the status and the body as JSON.
    fn get(server: &StandIn, target: &str) -> (u16, Value) {
        let target = target.strip_prefix(&server.url()).unwrap_or(target);
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nPrefer: IdType=\"ImmutableId\"\r\n\r\n").unwrap();
        let mut all = String::new();
        stream.read_to_string(&mut all).unwrap();
        let (head, body) = all.split_once("\r\n\r\n").unwrap();
        let status = head.lines().next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    fn search(path: &str, value: &str, more: &[(&str, &str)]) -> String {
        let mut pairs = vec![("$search", value)];
        pairs.extend_from_slice(more);
        format!("{path}?{}", http::form(&pairs))
    }

    fn subjects(page: &Value) -> Vec<String> {
        page["value"].as_array().unwrap().iter().map(|m| m["subject"].as_str().unwrap_or("").to_string()).collect()
    }

    #[test]
    fn a_search_pages_through_its_results_newest_first_and_a_folder_is_searched_alone() {
        let server = StandIn::start().unwrap();
        let inbox = server.state().well_known("inbox");
        let archive = server.state().well_known("archive");
        {
            let mut state = server.state();
            for i in 0..120 {
                state.mailbox.add(&inbox, &format!("Budget {i}"), NOW - 60 * i);
            }
            state.mailbox.add(&archive, "Budget archived", NOW + 60);
            state.mailbox.add(&inbox, "Lunch", NOW + 120);
        }
        let (status, page) = get(&server, &search("/v1.0/me/messages", "\"budget\"", &[("$top", "50"), ("$select", "subject,parentFolderId")]));
        assert_eq!(status, 200, "{page}");
        let first = subjects(&page);
        assert_eq!(first.len(), 50);
        assert_eq!(first[0], "Budget archived", "the whole mailbox, newest first");
        assert!(page["value"][0].get("bodyPreview").is_none(), "only what was selected");
        let mut all = first;
        let mut next = page["@odata.nextLink"].as_str().unwrap().to_string();
        loop {
            let (status, page) = get(&server, &next);
            assert_eq!(status, 200);
            all.extend(subjects(&page));
            match page["@odata.nextLink"].as_str() {
                Some(link) => next = link.to_string(),
                None => break,
            }
        }
        assert_eq!(all.len(), 121);
        assert!(!all.contains(&"Lunch".to_string()));
        // A folder alone, and a well-known name for it.
        let (_, page) = get(&server, &search("/v1.0/me/mailFolders/archive/messages", "\"subject:budget\"", &[]));
        assert_eq!(subjects(&page), ["Budget archived"]);
        // A phrase, in order; a word starts a word.
        let (_, page) = get(&server, &search(&format!("/v1.0/me/mailFolders/{inbox}/messages"), r#""subject:\"budget 11\"""#, &[("$top", "5")]));
        assert_eq!(subjects(&page), ["Budget 11", "Budget 110", "Budget 111", "Budget 112", "Budget 113"]);
    }

    #[test]
    fn what_the_reference_doesnt_show_beside_a_search_is_refused() {
        let server = StandIn::start().unwrap();
        for (more, why) in [
            (vec![("$filter", "isRead eq false")], "a filter"),
            (vec![("$orderby", "receivedDateTime desc")], "an order"),
            (vec![("$top", "1001")], "too many to a page"),
            (vec![("$select", "nosuch")], "a property messages don't have"),
        ] {
            let (status, _) = get(&server, &search("/v1.0/me/messages", "\"budget\"", &more));
            assert_eq!(status, 400, "{why}");
        }
        let (status, _) = get(&server, &search("/v1.0/me/messages", "\"budget OR lunch\"", &[]));
        assert_eq!(status, 400);
        // Refused by Graph's code, when told to.
        server.state().search_refused = Some((403, "ErrorAccessDenied"));
        let (status, body) = get(&server, &search("/v1.0/me/messages", "\"budget\"", &[]));
        assert_eq!((status, body["error"]["code"].as_str()), (403, Some("ErrorAccessDenied")));
    }

    #[test]
    fn another_mailbox_is_searched_at_its_own_address_and_alone() {
        let server = StandIn::start().unwrap();
        server.state().add_other("support@example.org", true);
        let theirs = server.state().other("support@example.org").unwrap().folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        let mine = server.state().well_known("inbox");
        server.state().other("support@example.org").unwrap().mailbox.add(&theirs, "Their budget", NOW);
        server.state().mailbox.add(&mine, "My budget", NOW);
        let (status, page) = get(&server, &search("/v1.0/users/support@example.org/messages", "\"budget\"", &[]));
        assert_eq!(status, 200);
        assert_eq!(subjects(&page), ["Their budget"]);
        let (_, page) = get(&server, &search("/v1.0/me/messages", "\"budget\"", &[]));
        assert_eq!(subjects(&page), ["My budget"]);
        server.state().add_other("closed@example.org", false);
        let (status, body) = get(&server, &search("/v1.0/users/closed@example.org/messages", "\"budget\"", &[]));
        assert_eq!((status, body["error"]["code"].as_str()), (403, Some("ErrorAccessDenied")));
    }

    #[test]
    fn a_search_value_is_read_as_the_reference_writes_one() {
        let words = |p: &[&'static str], w: &[&str]| Clause::Words { properties: p.to_vec(), words: w.iter().map(|w| w.to_string()).collect() };
        assert_eq!(clauses("\"pizza\"").unwrap(), [words(&["from", "subject", "body"], &["pizza"])]);
        // A part of an address, or an address after a property that isn't people's, is words.
        assert_eq!(clauses("\"from:ada@\"").unwrap(), [words(&["from"], &["ada"])]);
        assert_eq!(clauses("\"subject:ada@example.org\"").unwrap(), [words(&["subject"], &["ada", "example", "org"])]);
        assert_eq!(
            clauses("\"participants:\\\"Ada@Example.org\\\"\"").unwrap(),
            [Clause::Address { properties: vec!["from", "to", "cc", "bcc"], address: "ada@example.org".into() }]
        );
        assert_eq!(
            clauses(r#""budget AND from:ada@example.org AND subject:\"q4 plan\" hasAttachments:true importance:high""#).unwrap(),
            [
                words(&["from", "subject", "body"], &["budget"]),
                // A whole address is that address (#301).
                Clause::Address { properties: vec!["from"], address: "ada@example.org".into() },
                words(&["subject"], &["q4", "plan"]),
                Clause::HasAttachments(true),
                Clause::Importance("high"),
            ]
        );
        for refused in ["\"a OR b\"", "\"NOT a\"", "\"(a)\"", "\"\"", "\"nosuch:x\"", "\"a \\\"b\"", "\"a\"b\"", "\"subject:-\""] {
            assert!(clauses(refused).is_err(), "{refused}");
        }
        assert!(found_in("Quarterly budget review", &words_of("budg rev")));
        assert!(!found_in("Quarterly budget review", &words_of("review budget")));
    }
}
