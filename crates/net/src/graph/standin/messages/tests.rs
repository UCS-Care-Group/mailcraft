//! The stand-in's message delta, read the plain way: follow every link, keep what comes by id.
//! Nothing here leaves this machine; every message is invented.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::Value;

use crate::graph::standin::{StandIn, TOKEN};

const NOW: i64 = 1_790_000_000;
const IMMUTABLE: &str = "Prefer: IdType=\"ImmutableId\", odata.maxpagesize=100\r\n";

/// One GET: the status, the headers (names in lower case) and the body.
fn get(server: &StandIn, target: &str, headers: &str) -> (u16, Vec<(String, String)>, String) {
    let target = target.strip_prefix(&server.url()).unwrap_or(target);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    write!(stream, "GET {target} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n{headers}\r\n").unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    let (head, body) = all.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    (status, headers, body.to_string())
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap()
}

/// What a client keeps of one folder: the messages by id, and the link to go on from.
#[derive(Default)]
struct Kept {
    messages: HashMap<String, Value>,
    link: String,
    /// Every id that came, as often as it came.
    arrivals: Vec<String>,
}

/// Follow `kept`'s link (or start the folder afresh) to the end of the round: how many pages it took.
fn round(server: &StandIn, folder: &str, kept: &mut Kept) -> usize {
    if kept.link.is_empty() {
        kept.link = format!("/v1.0/me/mailFolders/{folder}/messages/delta?$select=subject,isRead,parentFolderId&$orderby=receivedDateTime+desc");
    }
    for pages in 1.. {
        let (status, _, body) = get(server, &kept.link.clone(), IMMUTABLE);
        assert_eq!(status, 200, "{body}");
        let page = json(&body);
        for entry in page["value"].as_array().unwrap() {
            let id = entry["id"].as_str().unwrap().to_string();
            kept.arrivals.push(id.clone());
            if entry.get("@removed").is_some() {
                kept.messages.remove(&id);
            } else {
                kept.messages.insert(id, entry.clone());
            }
        }
        if let Some(next) = page["@odata.nextLink"].as_str() {
            kept.link = next.to_string();
        } else {
            kept.link = page["@odata.deltaLink"].as_str().unwrap().to_string();
            return pages;
        }
    }
    0
}

fn inbox(server: &StandIn) -> String {
    server.state().well_known("inbox")
}

#[test]
fn a_first_round_gives_every_message_once_newest_first_and_the_next_only_what_changed() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    let ids = server.state().mailbox.generate(&inbox, 250, NOW);
    let mut kept = Kept::default();
    assert_eq!(round(&server, &inbox, &mut kept), 3);
    assert_eq!(kept.arrivals, ids, "every message once, newest first");
    assert!(kept.link.contains("$deltatoken="));
    // Only what was asked for (and the id).
    let first = kept.messages.get(&ids[0]).unwrap().as_object().unwrap();
    let mut names: Vec<&str> = first.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(names, ["id", "isRead", "parentFolderId", "subject"]);

    // Nothing changed: nothing comes.
    kept.arrivals.clear();
    round(&server, &inbox, &mut kept);
    assert!(kept.arrivals.is_empty());

    // One read, one deleted, one new: those three.
    let new = {
        let mut state = server.state();
        state.mailbox.update(&ids[5], |m| m.read = true);
        state.mailbox.delete(&ids[6]);
        state.mailbox.add(&inbox, "Late", NOW + 60)
    };
    round(&server, &inbox, &mut kept);
    assert_eq!(kept.arrivals, [ids[5].clone(), ids[6].clone(), new.clone()]);
    assert_eq!(kept.messages[&ids[5]]["isRead"], true);
    assert!(!kept.messages.contains_key(&ids[6]));
    assert_eq!(kept.messages.len(), 250);
}

#[test]
fn a_moved_message_is_removed_from_one_folder_and_new_in_the_other_by_the_same_immutable_id() {
    let server = StandIn::start().unwrap();
    let (inbox, archive) = (inbox(&server), server.state().well_known("archive"));
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    let (mut a, mut b) = (Kept::default(), Kept::default());
    round(&server, &inbox, &mut a);
    round(&server, &archive, &mut b);
    server.state().mailbox.move_to(&ids[1], &archive);
    let removed = {
        a.arrivals.clear();
        round(&server, &inbox, &mut a);
        a.arrivals.clone()
    };
    round(&server, &archive, &mut b);
    assert_eq!(removed, [ids[1].clone()]);
    assert!(!a.messages.contains_key(&ids[1]));
    assert_eq!(b.messages[&ids[1]]["parentFolderId"], archive.as_str());
}

#[test]
fn what_changes_during_a_round_comes_in_the_next_and_nothing_is_missed() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    let ids = server.state().mailbox.generate(&inbox, 300, NOW);
    // The first page, and then everything changes under the round.
    let mut kept = Kept { link: format!("/v1.0/me/mailFolders/{inbox}/messages/delta?$orderby=receivedDateTime+desc"), ..Default::default() };
    let (_, _, body) = get(&server, &kept.link.clone(), IMMUTABLE);
    let page = json(&body);
    for entry in page["value"].as_array().unwrap() {
        kept.messages.insert(entry["id"].as_str().unwrap().into(), entry.clone());
    }
    kept.link = page["@odata.nextLink"].as_str().unwrap().into();
    let late = {
        let mut state = server.state();
        state.mailbox.delete(&ids[250]);
        state.mailbox.update(&ids[10], |m| m.subject = "Changed".into());
        state.mailbox.update(&ids[200], |m| m.read = true);
        state.mailbox.add(&inbox, "Old but new", NOW - 1_000_000)
    };
    round(&server, &inbox, &mut kept);
    round(&server, &inbox, &mut kept);
    assert_eq!(kept.messages.len(), 300);
    assert!(!kept.messages.contains_key(&ids[250]) && kept.messages.contains_key(&late));
    assert_eq!(kept.messages[&ids[10]]["subject"], "Changed");
    assert_eq!(kept.messages[&ids[200]]["isRead"], true);
}

#[test]
fn a_replayed_page_comes_again_and_changes_nothing_kept_by_id() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    let ids = server.state().mailbox.generate(&inbox, 250, NOW);
    server.state().mailbox.replays = 2;
    let mut kept = Kept::default();
    round(&server, &inbox, &mut kept);
    assert_eq!(kept.arrivals.len(), 450, "two pages of 100 came twice");
    assert_eq!(kept.messages.len(), 250);
    assert!(ids.iter().all(|id| kept.messages.contains_key(id)));
}

#[test]
fn a_forgotten_link_and_a_reset_say_so_and_the_folder_starts_afresh() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    server.state().mailbox.generate(&inbox, 150, NOW);
    let mut kept = Kept::default();
    round(&server, &inbox, &mut kept);

    // Forgotten: 400 syncStateNotFound, for a next link and for a delta link alike.
    let delta = kept.link.clone();
    server.state().mailbox.forget_links();
    let (status, _, body) = get(&server, &delta, IMMUTABLE);
    assert_eq!((status, json(&body)["error"]["code"].as_str()), (400, Some("syncStateNotFound")));

    // Reset: 410 Gone, and the Location starts the folder afresh.
    let mut kept = Kept::default();
    round(&server, &inbox, &mut kept);
    server.state().mailbox.resets = 1;
    let (status, headers, _) = get(&server, &kept.link.clone(), IMMUTABLE);
    assert_eq!(status, 410);
    let location = headers.iter().find(|(n, _)| n == "location").map(|(_, v)| v.clone()).unwrap();
    assert!(location.ends_with("$deltatoken="));
    let mut afresh = Kept { link: location, ..Default::default() };
    round(&server, &inbox, &mut afresh);
    assert_eq!(afresh.messages.len(), 150);
}

#[test]
fn ids_are_immutable_only_when_asked_for_and_the_mime_is_had_by_either() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    let id = server.state().mailbox.add(&inbox, "Hello", NOW);
    let (_, _, body) = get(&server, &format!("/v1.0/me/mailFolders/{inbox}/messages/delta"), "");
    let ordinary = json(&body)["value"][0]["id"].as_str().unwrap().to_string();
    assert_ne!(ordinary, id);
    for id in [&id, &ordinary] {
        let (status, headers, mime) = get(&server, &format!("/v1.0/me/messages/{id}/$value"), "");
        assert_eq!(status, 200);
        assert!(headers.iter().any(|(n, v)| n == "content-type" && v == "text/plain"));
        assert!(mime.contains("Subject: Hello\r\n") && mime.contains("Message-ID: <m1@mail.example.org>"));
    }
    let (status, _, _) = get(&server, "/v1.0/me/messages/AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A9999999999AAA=/$value", "");
    assert_eq!(status, 404);
}

#[test]
fn what_a_delta_doesnt_take_is_refused() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    for query in ["$orderby=subject", "$select=nonsense", "$top=0", "$filter=isRead+eq+false", "changeType=created"] {
        let (status, _, body) = get(&server, &format!("/v1.0/me/mailFolders/{inbox}/messages/delta?{query}"), "");
        assert_eq!(status, 400, "{query}: {body}");
    }
    let mut kept = Kept::default();
    round(&server, &inbox, &mut kept);
    let (status, _, _) = get(&server, &format!("{}&$select=subject", kept.link), "");
    assert_eq!(status, 400, "a token carries the rest");
    // A folder's counts.
    server.state().mailbox.generate(&inbox, 4, NOW);
    let (_, _, body) = get(&server, &format!("/v1.0/me/mailFolders/{inbox}"), "");
    assert_eq!((json(&body)["totalItemCount"].as_u64(), json(&body)["unreadItemCount"].as_u64()), (Some(4), Some(4)));
}

/// The done-when's mailbox: 200,000 messages, every one once.
#[test]
fn two_hundred_thousand_messages_come_once_each() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    let ids = server.state().mailbox.generate(&inbox, 200_000, NOW);
    let mut kept = Kept::default();
    assert_eq!(round(&server, &inbox, &mut kept), 2_000);
    assert_eq!(kept.arrivals.len(), 200_000);
    assert_eq!(kept.messages.len(), 200_000);
    assert_eq!(kept.arrivals.first(), ids.first());
    assert_eq!(kept.arrivals.last(), ids.last());
}

#[test]
fn a_delta_that_expands_the_size_is_refused_as_exchange_online_refuses_it() {
    let server = StandIn::start().unwrap();
    let inbox = inbox(&server);
    server.state().mailbox.generate(&inbox, 3, NOW);
    let target = format!(
        "/v1.0/me/mailFolders/{inbox}/messages/delta?$select=subject&$expand=singleValueExtendedProperties($filter=id%20eq%20'Integer%200x0E08')"
    );
    let (status, _, body) = get(&server, &target, IMMUTABLE);
    assert_eq!(status, 400);
    let error = &json(&body)["error"];
    assert_eq!(error["code"], "BadRequest");
    assert_eq!(error["message"], "Parsing OData Select and Expand failed: Value cannot be null. (Parameter 'initialState')");
}
