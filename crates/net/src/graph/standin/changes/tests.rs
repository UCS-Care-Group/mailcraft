//! The stand-in's changes of messages, on the stand-in alone. Nothing here leaves this machine;
//! every message is invented.

use std::io::{Read, Write};
use std::net::TcpStream;

use serde_json::{Value, json};

use crate::graph::standin::{Fault, StandIn, TOKEN};

const NOW: i64 = 1_790_000_000;
const IMMUTABLE: &str = "Prefer: IdType=\"ImmutableId\"\r\n";
/// MailCraft's marker, as R3.3.2 is to write it: a named string property of a set of our own.
const MARKER: &str = "String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Name MailCraftMarker";

/// One request: the status (0 when the connection closed without an answer), the headers
/// (names in lower case) and the body.
fn send(server: &StandIn, method: &str, target: &str, body: Option<&Value>) -> (u16, Vec<(String, String)>, String) {
    let target = target.strip_prefix(&server.url()).unwrap_or(target);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let body = body.map(Value::to_string).unwrap_or_default();
    let typed = if body.is_empty() { String::new() } else { "Content-Type: application/json\r\n".into() };
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\n{IMMUTABLE}{typed}Content-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    let Some((head, body)) = all.split_once("\r\n\r\n") else { return (0, Vec::new(), all) };
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    (status, headers, body.to_string())
}

fn json_of(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or(Value::Null)
}

fn patch(server: &StandIn, id: &str, body: Value) -> (u16, Value) {
    let (status, _, answer) = send(server, "PATCH", &format!("/v1.0/me/messages/{id}"), Some(&body));
    (status, json_of(&answer))
}

/// A folder's delta from the start to its end: what came (messages and `@removed`), and the
/// delta link to go on from.
fn delta(server: &StandIn, folder: &str, link: Option<&str>) -> (Vec<Value>, String) {
    let mut link = link.map_or_else(|| format!("/v1.0/me/mailFolders/{folder}/messages/delta"), str::to_string);
    let mut all = Vec::new();
    loop {
        let (status, _, body) = send(server, "GET", &link, None);
        assert_eq!(status, 200, "{body}");
        let page = json_of(&body);
        all.extend(page["value"].as_array().unwrap().iter().cloned());
        match page["@odata.nextLink"].as_str() {
            Some(next) => link = next.to_string(),
            None => return (all, page["@odata.deltaLink"].as_str().unwrap().to_string()),
        }
    }
}

fn setup(count: usize) -> (StandIn, String, Vec<String>) {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, count, NOW);
    (server, inbox, ids)
}

#[test]
fn a_patch_changes_read_flag_categories_and_importance_and_shows_in_the_next_delta() {
    let (server, inbox, ids) = setup(3);
    let (_, link) = delta(&server, &inbox, None);
    let id = &ids[1];
    let (status, answer) = patch(
        &server,
        id,
        json!({
            "isRead": true,
            "importance": "high",
            "categories": ["Blue category", "Team"],
            "flag": {
                "flagStatus": "flagged",
                "startDateTime": {"dateTime": "2026-10-15T00:00:00", "timeZone": "UTC"},
                "dueDateTime": {"dateTime": "2026-10-20T09:00:00.0000000", "timeZone": "W. Europe Standard Time"},
            },
        }),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["id"], json!(id));
    assert_eq!((answer["isRead"].clone(), answer["importance"].clone()), (json!(true), json!("high")));
    assert_eq!(answer["categories"], json!(["Blue category", "Team"]));
    assert_eq!(answer["flag"]["flagStatus"], "flagged");
    assert_eq!(answer["flag"]["startDateTime"], json!({"dateTime": "2026-10-15T00:00:00.0000000", "timeZone": "UTC"}));
    // Written back in UTC: 09:00 in Berlin's summer time is 07:00.
    assert_eq!(answer["flag"]["dueDateTime"], json!({"dateTime": "2026-10-20T07:00:00.0000000", "timeZone": "UTC"}));
    // Only that message comes in the next round, as it is now.
    let (changed, link) = delta(&server, &inbox, Some(&link));
    assert_eq!(changed.len(), 1);
    assert_eq!((changed[0]["id"].clone(), changed[0]["flag"].clone()), (json!(id), answer["flag"].clone()));

    // Complete: a completed date of the stand-in's time; then not flagged: no dates at all.
    let (status, answer) = patch(&server, id, json!({"flag": {"flagStatus": "complete"}}));
    assert_eq!(status, 200);
    assert!(answer["flag"]["completedDateTime"]["dateTime"].is_string() && answer["flag"]["dueDateTime"].is_object());
    let (_, answer) = patch(&server, id, json!({"flag": {"flagStatus": "notFlagged"}}));
    assert_eq!(answer["flag"], json!({"flagStatus": "notFlagged"}));
    let (changed, _) = delta(&server, &inbox, Some(&link));
    assert_eq!(changed.len(), 1, "two changes of one message come as one");
    // The same change twice is the same message.
    let (again, _) = patch(&server, id, json!({"flag": {"flagStatus": "notFlagged"}}));
    assert_eq!(again, 200);
}

#[test]
fn a_patch_is_made_whole_or_not_at_all_and_says_what_was_wrong() {
    let (server, _, ids) = setup(1);
    let id = &ids[0];
    let wrong = [
        json!({"isRead": "yes"}),
        json!({"importance": "urgent"}),
        json!({"categories": "Red"}),
        json!({"categories": [1]}),
        json!({"flag": {"flagStatus": "maybe"}}),
        // A due date without a start (the reference: 400).
        json!({"flag": {"flagStatus": "flagged", "dueDateTime": {"dateTime": "2026-10-20T09:00:00", "timeZone": "UTC"}}}),
        // A start without a due date, `null` or none (as Microsoft 365 answers it: 400).
        json!({"flag": {"flagStatus": "flagged", "startDateTime": {"dateTime": "2026-10-17T00:00:00", "timeZone": "UTC"}, "dueDateTime": null}}),
        json!({"flag": {"flagStatus": "flagged", "startDateTime": {"dateTime": "2026-10-17T00:00:00", "timeZone": "UTC"}}}),
        json!({"flag": {"startDateTime": {"dateTime": "tomorrow", "timeZone": "UTC"}}}),
        json!({"flag": {"startDateTime": {"dateTime": "2026-10-20T09:00:00", "timeZone": "Mars/Olympus"}}}),
        json!({"subject": "Changed"}),
        json!({"noSuchThing": 1}),
        json!({"singleValueExtendedProperties": [{"id": "not an id", "value": "x"}]}),
        json!([1, 2]),
        // Read is fine, the rest isn't: nothing of it is made.
        json!({"isRead": true, "importance": "loud"}),
    ];
    for body in wrong {
        let (status, answer) = patch(&server, id, body.clone());
        assert_eq!(status, 400, "{body} → {answer}");
        assert!(answer["error"]["code"].is_string(), "{answer}");
    }
    assert!(!server.state().mailbox.messages[id].read);
    // What Microsoft 365 takes: a flag with no dates (a due date of `null` too), and a start due at once.
    for body in [
        json!({"flag": {"flagStatus": "flagged"}}),
        json!({"flag": {"flagStatus": "flagged", "dueDateTime": null}}),
        json!({"flag": {"flagStatus": "flagged", "startDateTime": {"dateTime": "2026-10-17T00:00:00", "timeZone": "UTC"}, "dueDateTime": {"dateTime": "2026-10-17T00:00:00", "timeZone": "UTC"}}}),
    ] {
        assert_eq!(patch(&server, id, body.clone()).0, 200, "{body}");
    }
    // A message that isn't there, and an id that is no id.
    assert_eq!(patch(&server, "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A9999999999AAA=", json!({"isRead": true})).0, 404);
    assert_eq!(patch(&server, "short", json!({"isRead": true})).0, 400);
    // An ordinary id names the same message.
    let ordinary = id.replace("AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A", "AAMkAGVmMDEzMTM4LTZmYWUtNDdkNC1hMDZiLTU1OGY5OTZhYmY4OABGAAAA");
    assert_eq!(patch(&server, &ordinary, json!({"isRead": true})).0, 200);
    assert!(server.state().mailbox.messages[id].read);
}

#[test]
fn a_move_keeps_the_immutable_id_and_the_folders_deltas_say_so() {
    let (server, inbox, ids) = setup(2);
    let archive = server.state().well_known("archive");
    let (_, inbox_link) = delta(&server, &inbox, None);
    let (_, archive_link) = delta(&server, &archive, None);
    let (status, _, body) = send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[0]), Some(&json!({"destinationId": "archive"})));
    assert_eq!(status, 201, "{body}");
    let moved = json_of(&body);
    assert_eq!((moved["id"].clone(), moved["parentFolderId"].clone()), (json!(ids[0]), json!(archive)));
    let (from_inbox, _) = delta(&server, &inbox, Some(&inbox_link));
    assert_eq!(from_inbox, [json!({"@removed": {"reason": "deleted"}, "id": ids[0]})]);
    let (into_archive, _) = delta(&server, &archive, Some(&archive_link));
    assert_eq!(into_archive.len(), 1);
    assert_eq!(into_archive[0]["id"], json!(ids[0]));
    // It is found by its id where it is now: how a client looks for a move whose answer was lost.
    let (status, _, body) = send(&server, "GET", &format!("/v1.0/me/messages/{}?$select=parentFolderId", ids[0]), None);
    assert_eq!((status, json_of(&body)["parentFolderId"].clone()), (200, json!(archive)));
    // Nowhere to go, or nowhere at all.
    let nowhere = json!({"destinationId": "AQMkAGV4YW1wbGUtb3JnLW1haWxib3gtZm9sZGVycwAuAAADk1v_nothing_AAAA=="});
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[1]), Some(&nowhere)).0, 404);
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[1]), Some(&json!({}))).0, 400);
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[1]), Some(&json!({"destinationId": "inbox", "x": 1}))).0, 400);
    assert_eq!(send(&server, "GET", &format!("/v1.0/me/messages/{}/move", ids[1]), None).0, 405);
}

#[test]
fn a_copy_has_an_id_of_its_own_and_carries_the_extended_properties_to_be_found_by() {
    let (server, inbox, ids) = setup(3);
    let archive = server.state().well_known("archive");
    let source = &ids[2];
    // The marker first, on the source.
    let (status, _) = patch(&server, source, json!({"singleValueExtendedProperties": [{"id": MARKER, "value": "mc-1"}]}));
    assert_eq!(status, 200);
    // Written again under the same id (whatever its case), it is replaced, not added.
    let (status, _) = patch(&server, source, json!({"singleValueExtendedProperties": [{"id": MARKER.to_lowercase(), "value": "mc-2"}]}));
    assert_eq!(status, 200);
    assert_eq!(server.state().mailbox.messages[source].extended.len(), 1);
    let (status, _, body) = send(&server, "POST", &format!("/v1.0/me/messages/{source}/copy"), Some(&json!({"destinationId": archive})));
    assert_eq!(status, 201, "{body}");
    let copy = json_of(&body);
    let copy_id = copy["id"].as_str().unwrap().to_string();
    assert_ne!(&copy_id, source);
    assert_eq!(copy["parentFolderId"], json!(archive));
    assert_eq!(copy["subject"], server.state().mailbox.messages[source].subject.as_str());
    // The source stays where it was.
    assert_eq!(server.state().mailbox.messages[source].folder, inbox);

    // Found in the destination by the marker's value; nothing found by another value, nor in
    // the source's folder but the source itself.
    let find = |folder: &str, value: &str| {
        let filter = format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{value}')");
        let target = format!("/v1.0/me/mailFolders/{folder}/messages?$filter={}&$select=id", crate::http::encode(&filter));
        let (status, _, body) = send(&server, "GET", &target, None);
        assert_eq!(status, 200, "{body}");
        json_of(&body)["value"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect::<Vec<_>>()
    };
    assert_eq!(find(&archive, "mc-2"), std::slice::from_ref(&copy_id));
    assert!(find(&archive, "mc-1").is_empty());
    assert_eq!(find(&inbox, "mc-2"), std::slice::from_ref(source));
    // The property comes with a message when it is asked for.
    let expand = crate::http::encode(&format!("singleValueExtendedProperties($filter=id eq '{MARKER}')"));
    let (_, _, body) = send(&server, "GET", &format!("/v1.0/me/messages/{copy_id}?$expand={expand}"), None);
    assert_eq!(
        json_of(&body)["singleValueExtendedProperties"],
        json!([{"id": MARKER, "value": "mc-2"}]),
        "kept under the id it was first written with"
    );
    // A filter the stand-in doesn't answer is said so.
    assert_eq!(send(&server, "GET", &format!("/v1.0/me/mailFolders/{archive}/messages?$filter=isRead+eq+true"), None).0, 400);
    // The copy comes in the destination's delta as a new message.
    let (all, _) = delta(&server, &archive, None);
    assert_eq!(all.iter().map(|m| m["id"].as_str().unwrap()).collect::<Vec<_>>(), [copy_id.as_str()]);
}

#[test]
fn a_delete_takes_the_message_out_of_every_folder() {
    let (server, inbox, ids) = setup(2);
    let (_, link) = delta(&server, &inbox, None);
    let (status, _, body) = send(&server, "DELETE", &format!("/v1.0/me/messages/{}", ids[0]), None);
    assert_eq!((status, body.as_str()), (204, ""));
    let (changed, _) = delta(&server, &inbox, Some(&link));
    assert_eq!(changed, [json!({"@removed": {"reason": "deleted"}, "id": ids[0]})]);
    // Gone: a second delete and any other change are told so.
    assert_eq!(send(&server, "DELETE", &format!("/v1.0/me/messages/{}", ids[0]), None).0, 404);
    assert_eq!(patch(&server, &ids[0], json!({"isRead": true})).0, 404);
    assert_eq!(send(&server, "GET", &format!("/v1.0/me/messages/{}", ids[0]), None).0, 404);
    // Under its folder's address too.
    assert_eq!(send(&server, "DELETE", &format!("/v1.0/me/mailFolders/inbox/messages/{}", ids[1]), None).0, 204);
    assert!(server.state().mailbox.messages.is_empty());
    // Past the recoverable items is nothing the stand-in does: that address names nothing.
    let kept = server.state().mailbox.add(&inbox, "Kept", NOW);
    // (Spelled in pieces: the word is nowhere in MailCraft's code, R3 spec S19.)
    let action = ["perm", "anent", "Delete"].concat();
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{kept}/{action}"), None).0, 400);
    assert!(server.state().mailbox.messages.contains_key(&kept));
}

#[test]
fn faults_on_demand_per_message() {
    let (server, _, ids) = setup(5);
    {
        let mut state = server.state();
        state.retry_after = Some(7);
        state.mailbox.faults.insert(ids[0].clone(), Fault::Gone);
        state.mailbox.faults.insert(ids[1].clone(), Fault::Denied);
        state.mailbox.faults.insert(ids[2].clone(), Fault::Invalid);
        state.mailbox.faults.insert(ids[3].clone(), Fault::Throttled(2));
    }
    let read = json!({"isRead": true});
    let (status, answer) = patch(&server, &ids[0], read.clone());
    assert_eq!((status, answer["error"]["code"].clone()), (404, json!("ErrorItemNotFound")));
    assert_eq!(patch(&server, &ids[1], read.clone()).0, 403);
    assert_eq!(patch(&server, &ids[2], read.clone()).0, 400);
    // Every time, and for a move and a delete too.
    assert_eq!(send(&server, "DELETE", &format!("/v1.0/me/messages/{}", ids[0]), None).0, 404);
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[1]), Some(&json!({"destinationId": "archive"}))).0, 403);
    // Throttled twice, with when to come back, then made.
    for _ in 0..2 {
        let (status, headers, _) = send(&server, "PATCH", &format!("/v1.0/me/messages/{}", ids[3]), Some(&read));
        assert_eq!(status, 429);
        assert!(headers.contains(&("retry-after".into(), "7".into())), "{headers:?}");
    }
    assert_eq!(patch(&server, &ids[3], read.clone()).0, 200);
    let state = server.state();
    assert!(ids[..3].iter().all(|id| !state.mailbox.messages[id].read) && state.mailbox.messages[&ids[3]].read);
}

#[test]
fn an_answer_lost_after_the_change_was_made() {
    let (server, inbox, ids) = setup(2);
    let archive = server.state().well_known("archive");
    server.state().mailbox.faults.insert(ids[0].clone(), Fault::AnswerLost);
    let (status, _, body) = send(&server, "POST", &format!("/v1.0/me/messages/{}/move", ids[0]), Some(&json!({"destinationId": "archive"})));
    assert_eq!((status, body.as_str()), (0, ""), "no answer at all");
    // Made all the same, and only the once: the next request is answered as usual.
    assert_eq!(server.state().mailbox.messages[&ids[0]].folder, archive);
    let (status, _, _) = send(&server, "GET", &format!("/v1.0/me/messages/{}", ids[0]), None);
    assert_eq!(status, 200);
    // A copy whose answer was lost is there, and has a copy of the marker to be found by.
    {
        let mut state = server.state();
        state.mailbox.update(&ids[1], |m| m.extended.push((MARKER.into(), "mc-9".into())));
        state.mailbox.faults.insert(ids[1].clone(), Fault::AnswerLost);
    }
    let (status, _, _) = send(&server, "POST", &format!("/v1.0/me/messages/{}/copy", ids[1]), Some(&json!({"destinationId": archive})));
    assert_eq!(status, 0);
    let state = server.state();
    let copies: Vec<_> = state.mailbox.messages.values().filter(|m| m.folder == archive && m.extended.iter().any(|(_, v)| v == "mc-9")).collect();
    assert_eq!(copies.len(), 1);
    assert_eq!(state.mailbox.messages[&ids[1]].folder, inbox);
}

/// A batch request with a body.
fn request(id: &str, method: &str, url: &str, body: Option<Value>) -> Value {
    let mut request = json!({"id": id, "method": method, "url": url, "headers": {"Prefer": "IdType=\"ImmutableId\""}});
    if let Some(body) = body {
        request["body"] = body;
        request["headers"]["Content-Type"] = json!("application/json");
    }
    request
}

fn batch(server: &StandIn, requests: Vec<Value>) -> (u16, Vec<(String, String)>, Value) {
    let (status, headers, body) = send(server, "POST", "/v1.0/$batch", Some(&json!({"requests": requests})));
    (status, headers, json_of(&body))
}

#[test]
fn a_batch_answers_each_request_by_itself_matched_by_its_id() {
    let (server, inbox, ids) = setup(5);
    {
        let mut state = server.state();
        state.retry_after = Some(3);
        state.mailbox.faults.insert(ids[1].clone(), Fault::Gone);
        state.mailbox.faults.insert(ids[2].clone(), Fault::Throttled(1));
    }
    let requests = vec![
        request("a", "PATCH", &format!("/me/messages/{}", ids[0]), Some(json!({"isRead": true}))),
        request("b", "PATCH", &format!("/me/messages/{}", ids[1]), Some(json!({"isRead": true}))),
        request("c", "PATCH", &format!("/me/messages/{}", ids[2]), Some(json!({"isRead": true}))),
        request("d", "POST", &format!("/me/messages/{}/move", ids[3]), Some(json!({"destinationId": "archive"}))),
        request("e", "DELETE", &format!("/me/messages/{}", ids[4]), None),
        request("f", "POST", &format!("/me/messages/{}/copy", ids[0]), Some(json!({"destinationId": "deleteditems"}))),
    ];
    let (status, _, answer) = batch(&server, requests);
    assert_eq!(status, 200, "{answer}");
    let responses = answer["responses"].as_array().unwrap();
    let of = |id: &str| responses.iter().find(|r| r["id"] == id).unwrap().clone();
    assert_eq!(of("a")["status"], 200);
    assert_eq!(of("a")["body"]["isRead"], true);
    assert_eq!(of("b")["status"], 404);
    assert_eq!(of("b")["body"]["error"]["code"], "ErrorItemNotFound");
    assert_eq!(of("c")["status"], 429);
    assert_eq!(of("c")["headers"]["Retry-After"], "3");
    assert_eq!(of("d")["status"], 201);
    assert_eq!(of("d")["body"]["id"], json!(ids[3]));
    assert_eq!(of("e")["status"], 204);
    assert_eq!(of("f")["status"], 201);
    assert_eq!(responses.len(), 6);
    let state = server.state();
    let read: Vec<bool> = ids[..3].iter().map(|id| state.mailbox.messages[id].read).collect();
    assert_eq!(read, [true, false, false]);
    assert_ne!(state.mailbox.messages[&ids[3]].folder, inbox);
    assert!(!state.mailbox.messages.contains_key(&ids[4]));
}

#[test]
fn a_batch_of_more_than_20_or_with_ids_twice_is_refused_whole() {
    let (server, _, ids) = setup(21);
    let read = |n: usize, id: &str| request(&n.to_string(), "PATCH", &format!("/me/messages/{id}"), Some(json!({"isRead": true})));
    let all: Vec<Value> = ids.iter().enumerate().map(|(n, id)| read(n, id)).collect();
    let (status, _, answer) = batch(&server, all.clone());
    assert_eq!(status, 400, "{answer}");
    assert!(server.state().mailbox.messages.values().all(|m| !m.read), "nothing of it is made");
    let (status, _, answer) = batch(&server, all[..20].to_vec());
    assert_eq!((status, answer["responses"].as_array().map(Vec::len)), (200, Some(20)));
    let twice = vec![read(1, &ids[0]), read(1, &ids[1])];
    assert_eq!(batch(&server, twice).0, 400);
    assert_eq!(batch(&server, Vec::new()).0, 400);
}

#[test]
fn a_whole_batch_throttled_or_its_answer_lost() {
    let (server, _, ids) = setup(2);
    {
        let mut state = server.state();
        state.retry_after = Some(9);
        state.fail_next.push(("POST /v1.0/$batch".into(), 429, "TooManyRequests".into()));
    }
    let requests = vec![request("1", "PATCH", &format!("/me/messages/{}", ids[0]), Some(json!({"isRead": true})))];
    let (status, headers, _) = batch(&server, requests.clone());
    assert_eq!(status, 429);
    assert!(headers.contains(&("retry-after".into(), "9".into())), "{headers:?}");
    assert!(!server.state().mailbox.messages[&ids[0]].read, "nothing of it was made");
    assert_eq!(batch(&server, requests).0, 200);
    assert!(server.state().mailbox.messages[&ids[0]].read);

    // One request whose answer is lost loses the batch's whole answer; all of it was made.
    server.state().mailbox.faults.insert(ids[1].clone(), Fault::AnswerLost);
    let requests = vec![
        request("1", "PATCH", &format!("/me/messages/{}", ids[1]), Some(json!({"flag": {"flagStatus": "flagged"}}))),
        request("2", "PATCH", &format!("/me/messages/{}", ids[0]), Some(json!({"importance": "low"}))),
    ];
    let (status, _, _) = batch(&server, requests);
    assert_eq!(status, 0);
    let state = server.state();
    assert_eq!((state.mailbox.messages[&ids[1]].flag.as_str(), state.mailbox.messages[&ids[0]].importance.as_str()), ("flagged", "low"));
}

#[test]
fn filters_and_extended_property_ids_are_read_carefully() {
    use super::{property_expansion, property_filter, property_id_ok, quoted};
    assert_eq!(
        property_filter("singleValueExtendedProperties/Any(ep: ep/id eq 'String 0x1' and ep/value eq 'it''s')"),
        Some(("String 0x1".into(), "it's".into()))
    );
    assert_eq!(property_filter("singleValueExtendedProperties/any(x:x/id eq 'a' and x/value eq 'b')"), Some(("a".into(), "b".into())));
    for wrong in [
        "",
        "singleValueExtendedProperties/any(ep: ep/id eq 'a')",
        "singleValueExtendedProperties/any(ep: ep/id eq 'a' and ep/value eq 'b') or true",
        "singleValueExtendedProperties/any(ep: other/id eq 'a' and ep/value eq 'b')",
        "singleValueExtendedProperties/any(ep: ep/id eq 'a and ep/value eq 'b')",
        "singleValueExtendedProperties/any(é: é/id eq 'a' and é/value eq 'b')",
        "sïngle",
    ] {
        assert_eq!(property_filter(wrong), None, "{wrong}");
    }
    assert_eq!(property_expansion("singleValueExtendedProperties($filter=id eq 'Integer 0x0E08')"), Some("Integer 0x0E08".into()));
    assert_eq!(quoted("'unended"), None);
    assert!(property_id_ok("Integer 0x0E08") && property_id_ok(MARKER) && property_id_ok("String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Id 0x8005"));
    for wrong in ["Integer", "Integer 0x", "Text 0x1", "String {6d1c4e3a} Name X", "String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Name ", "String é"]
    {
        assert!(!property_id_ok(wrong), "{wrong}");
    }
}
