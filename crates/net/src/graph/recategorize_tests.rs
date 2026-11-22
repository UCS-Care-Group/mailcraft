// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A renamed category given to the mail and the events that only the server has (#851), through
//! the strict Graph stand-in. Nothing leaves this machine and everything is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::remote::{CategoryRename, CategoryRenamed, EventsUpload};
use mailcraft_model::sync::GraphMailRequest;

use super::mail::whole_run;
use super::pace::{Gate, Retry};
use super::recategorize::renamed_list;
use super::standin::{StandIn, TOKEN};
use super::{EventOptions, push_events_at};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|n| n.to_string()).collect()
}

fn rename(from: &str, to: &str) -> CategoryRename {
    CategoryRename { from: from.into(), to: to.into(), calendars: Vec::new() }
}

/// A message in `folder` (a well-known name) that carries `categories`: its id.
fn message(server: &StandIn, folder: &str, subject: &str, categories: &[&str]) -> String {
    let mut state = server.state();
    let folder = state.well_known(folder);
    let id = state.mailbox.add(&folder, subject, NOW);
    state.mailbox.messages.get_mut(&id).unwrap().categories = names(categories);
    id
}

fn categories_of(server: &StandIn, id: &str) -> Vec<String> {
    server.state().mailbox.messages.get(id).unwrap().categories.clone()
}

/// A whole mail run that carries `rename`: what the server made of it.
fn mail_run(server: &StandIn, rename: Option<CategoryRename>) -> Option<CategoryRenamed> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), rename, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().filter_map(|piece| piece.renamed).next_back()
}

#[test]
fn the_new_name_stands_where_the_old_one_stood_and_only_once() {
    assert_eq!(renamed_list(&names(&["Red", "Holiday", "Blue"]), "Holiday", "Holiday 2026"), names(&["Red", "Holiday 2026", "Blue"]));
    assert_eq!(renamed_list(&names(&["holiday 2026", "HOLIDAY"]), "Holiday", "Holiday 2026"), names(&["Holiday 2026"]));
    assert_eq!(renamed_list(&names(&["Red"]), "Holiday", "Holiday 2026"), names(&["Red"]));
}

#[test]
fn mail_in_every_folder_takes_the_new_name_and_other_mail_is_left_alone() {
    let server = StandIn::start().unwrap();
    let old = message(&server, "inbox", "Flights", &["Red", "Holiday"]);
    let deleted = message(&server, "deleteditems", "Old hotel", &["Holiday"]);
    let junk = message(&server, "junkemail", "Cheap trips", &["Holiday", "Holiday 2026"]);
    let other = message(&server, "inbox", "Invoice", &["Holidays", "Red"]);
    let came = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    assert!(came.done && !came.denied && !came.refused, "{came:?}");
    assert_eq!((came.changed, came.skipped), (3, 0));
    assert_eq!(categories_of(&server, &old), names(&["Red", "Holiday 2026"]));
    assert_eq!(categories_of(&server, &deleted), names(&["Holiday 2026"]));
    assert_eq!(categories_of(&server, &junk), names(&["Holiday 2026"]));
    assert_eq!(categories_of(&server, &other), names(&["Holidays", "Red"]), "a name that only begins the same is another category");
    // Nothing carries the old name any more: a second run changes nothing and is done.
    let again = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    assert!(again.done && again.changed == 0, "{again:?}");
}

#[test]
fn a_run_without_a_rename_asks_for_none_and_a_shared_mailbox_is_never_asked() {
    let server = StandIn::start().unwrap();
    let kept = message(&server, "inbox", "Flights", &["Holiday"]);
    assert_eq!(mail_run(&server, None), None);
    // A shared mailbox's run goes to its end, and asks for no rename: only the owner's own mailbox.
    server.state().add_other("team@example.org", true);
    let request = GraphMailRequest {
        account: 4,
        email: "team@example.org".into(),
        shared: true,
        rename: Some(rename("Holiday", "Holiday 2026")),
        ..Default::default()
    };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    let last = pieces.last().unwrap();
    assert!(last.whole, "{:?}", last.errors);
    assert!(pieces.iter().all(|piece| piece.renamed.is_none()));
    assert_eq!(categories_of(&server, &kept), names(&["Holiday"]));
}

#[test]
fn more_than_a_batch_goes_in_several_and_a_quote_in_the_name_is_found() {
    let server = StandIn::start().unwrap();
    let ids: Vec<String> = (0..45).map(|n| message(&server, "inbox", &format!("Note {n}"), &["Kids' stuff"])).collect();
    let came = mail_run(&server, Some(rename("Kids' stuff", "Family"))).unwrap();
    assert!(came.done, "{came:?}");
    assert_eq!(came.changed, 45);
    assert!(ids.iter().all(|id| categories_of(&server, id) == names(&["Family"])));
    // A name with what a web address reads as its own (%, #, &, +, a space) is asked for as it is.
    let odd = message(&server, "inbox", "Sale", &["50% off #1 & more + less"]);
    let came = mail_run(&server, Some(rename("50% off #1 & more + less", "Sales"))).unwrap();
    assert!(came.done && came.changed == 1, "{came:?}");
    assert_eq!(categories_of(&server, &odd), names(&["Sales"]));
    // How far it got went out as it went: found, then after each batch of twenty.
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), rename: Some(rename("Family", "Kin")), ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    let went: Vec<(u32, u32)> = pieces.iter().filter_map(|piece| piece.renaming.filter(|_| piece.partial)).map(|r| (r.done, r.total)).collect();
    assert_eq!(went, [(0, 45), (20, 45), (40, 45), (45, 45)]);
}

#[test]
fn a_server_that_takes_no_batch_is_asked_one_message_at_a_time() {
    let server = StandIn::start().unwrap();
    let ids: Vec<String> = (0..3).map(|n| message(&server, "inbox", &format!("Note {n}"), &["Holiday"])).collect();
    server.state().refuse_batches = true;
    let graph = super::Graph::for_mail(&server.url(), TOKEN, Arc::new(Gate::new()), super::pace::Priority::Owner, QUICK).unwrap();
    let items = super::recategorize::Items { lists: vec![graph.mail_path("/messages")], item: graph.batch_path("/messages/") };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {}).renamed;
    assert!(came.done, "{came:?}");
    assert_eq!(came.changed, 3);
    assert!(ids.iter().all(|id| categories_of(&server, id) == names(&["Holiday 2026"])));
}

#[test]
fn a_refused_sign_in_leaves_the_rename_for_a_later_run() {
    let server = StandIn::start().unwrap();
    let kept = message(&server, "inbox", "Flights", &["Holiday"]);
    let graph =
        super::Graph::for_mail(&server.url(), "not-the-token-0123456789", Arc::new(Gate::new()), super::pace::Priority::Owner, QUICK).unwrap();
    let items = super::recategorize::Items { lists: vec![graph.mail_path("/messages")], item: graph.batch_path("/messages/") };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {});
    assert!(came.token_refused && !came.renamed.done && !came.renamed.denied && !came.renamed.refused);
    assert_eq!(categories_of(&server, &kept), names(&["Holiday"]));
    // A mail run says so, as for any other part of it that the sign-in stops.
    let mut last = mailcraft_model::sync::GraphMailResult::default();
    let request =
        GraphMailRequest { account: 3, email: "owner@example.org".into(), rename: Some(rename("Holiday", "Holiday 2026")), ..Default::default() };
    super::mail::renamed_into(&graph, &request, &AtomicBool::new(false), &mut last, &mut |_| {});
    assert!(last.errors.iter().any(|e| e.contains("Couldn't rename the category on Microsoft's server")), "{:?}", last.errors);
}

/// An event of `calendar` that carries `categories`: its id.
fn event(server: &StandIn, calendar: &str, subject: &str, categories: &[&str]) -> String {
    let mut state = server.state();
    let id = state.add_event(calendar, subject, NOW);
    state.events.iter_mut().find(|e| e.id == id).unwrap().categories = names(categories);
    id
}

fn event_categories(server: &StandIn, id: &str) -> Vec<String> {
    server.state().events.iter().find(|e| e.id == id).unwrap().categories.clone()
}

fn events_run(server: &StandIn, rename: CategoryRename, shared: &str) -> Option<CategoryRenamed> {
    let upload = EventsUpload { account: 3, email: "owner@example.org".into(), rename: Some(rename), ..Default::default() };
    let uploaded = push_events_at(&server.url(), TOKEN, &upload, &EventOptions { shared: shared.into(), ..Default::default() });
    assert!(!uploaded.token_refused && uploaded.outcomes.is_empty());
    uploaded.renamed
}

#[test]
fn events_of_the_main_calendar_and_of_the_calendars_named_take_the_new_name() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let projects = server.state().add_calendar("Projects", true, false);
    let family = server.state().add_calendar("Family", true, false);
    let in_main = event(&server, &main, "Flight out", &["Holiday", "Red"]);
    let in_projects = event(&server, &projects, "Book hotel", &["Holiday"]);
    let not_named = event(&server, &family, "Pack", &["Holiday"]);
    let other = event(&server, &main, "Dentist", &["Red"]);
    let came = events_run(&server, CategoryRename { calendars: vec![projects.clone()], ..rename("Holiday", "Holiday 2026") }, "").unwrap();
    assert!(came.done, "{came:?}");
    assert_eq!((came.changed, came.skipped), (2, 0));
    assert_eq!(event_categories(&server, &in_main), names(&["Holiday 2026", "Red"]));
    assert_eq!(event_categories(&server, &in_projects), names(&["Holiday 2026"]));
    assert_eq!(event_categories(&server, &not_named), names(&["Holiday"]), "a calendar the request doesn't name is not asked");
    assert_eq!(event_categories(&server, &other), names(&["Red"]));
}

#[test]
fn an_event_the_server_wont_change_is_counted_and_holds_nothing_up() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let read_only = server.state().add_calendar("Rota", false, false);
    let mine = event(&server, &main, "Flight out", &["Holiday"]);
    let theirs = event(&server, &read_only, "Shift", &["Holiday"]);
    let came = events_run(&server, CategoryRename { calendars: vec![read_only.clone()], ..rename("Holiday", "Holiday 2026") }, "").unwrap();
    assert!(came.done, "{came:?}");
    assert_eq!((came.changed, came.skipped), (1, 1));
    assert_eq!(event_categories(&server, &mine), names(&["Holiday 2026"]));
    assert_eq!(event_categories(&server, &theirs), names(&["Holiday"]));
}

#[test]
fn a_shared_mailboxs_upload_renames_nothing() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let mine = event(&server, &main, "Flight out", &["Holiday"]);
    server.state().add_other("support@example.org", true);
    assert_eq!(events_run(&server, rename("Holiday", "Holiday 2026"), "support@example.org"), None);
    assert_eq!(event_categories(&server, &mine), names(&["Holiday"]));
}

#[test]
fn a_calendar_that_is_gone_doesnt_end_the_others() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let mine = event(&server, &main, "Flight out", &["Holiday"]);
    let came =
        events_run(&server, CategoryRename { calendars: vec!["AAMkAGone-calendar=".into()], ..rename("Holiday", "Holiday 2026") }, "").unwrap();
    assert!(came.done && !came.denied && !came.refused, "{came:?}");
    assert_eq!(came.changed, 1);
    assert_eq!(event_categories(&server, &mine), names(&["Holiday 2026"]));
}

// ---- what the server refuses or puts off, against a scripted server ----

/// A scripted Graph: `main` and `other` are two lists of events; what each answers is set by
/// the test.
#[derive(Default)]
struct Script {
    /// The status the first page of the main list (`/me/calendar/events`) is answered with (0: its items).
    main_status: u16,
    /// The same for the other list (`/me/calendars/other/events`).
    other_status: u16,
    /// The main list has a second page, answered with this status.
    second_page: Option<u16>,
    /// The status each PATCH of an item is answered with, by its id (none: 200).
    item_status: Vec<(&'static str, u16)>,
    /// The status the whole `$batch` is answered with (0: an answer for each item).
    batch_status: u16,
    /// The `$batch` is answered 200 with nothing about any item.
    batch_empty: bool,
    /// The `$batch` answers for its first item alone.
    batch_first_only: bool,
    /// The main list is this many pages of a hundred items each (0: the one item).
    pages: usize,
    /// The status every PATCH is answered with (0: by `item_status`).
    every_item: u16,
    /// The status the read of an item right before its change is answered with, by its id (none: 200).
    read_status: Vec<(&'static str, u16)>,
    /// What that read answers with for an item, by its id (none: the list it was listed with).
    read_body: Vec<(&'static str, serde_json::Value)>,
    /// The batch of reads answers for its first item alone.
    reads_first_only: bool,
    /// The status a `$batch` of changes is answered with, the reads being answered (0: an answer for each).
    writes_status: u16,
    /// A `$batch` of changes also answers 200 for this place in the chunk, which it wasn't asked.
    answers_unasked: Option<&'static str>,
    /// Every page of the main list names the same hundred items.
    same_ids: bool,
    /// What an item answered "busy" inside a `$batch` gives as its own `Retry-After`.
    item_wait: Option<&'static str>,
    /// Set once the first item was changed by a request of its own: the owner stops the run there.
    stop: Option<Arc<AtomicBool>>,
    /// Each request made for one item by itself: the method and the id.
    singly: Vec<(String, String)>,
    /// The ids that were changed.
    changed: Vec<String>,
    /// Each change that was asked for: the id and the body.
    written: Vec<(String, serde_json::Value)>,
}

fn status_line(status: u16) -> Vec<u8> {
    format!("HTTP/1.1 {status} Other\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

fn scripted(script: &Arc<std::sync::Mutex<Script>>) -> crate::fake::FakeHttp {
    use serde_json::{Value, json};
    let script = script.clone();
    crate::fake::FakeHttp::start(move |_, request| {
        let mut script = script.lock().unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        let mut line = head.lines().next().unwrap_or("").split(' ');
        let (method, target) = (line.next().unwrap_or(""), line.next().unwrap_or(""));
        let host = head.lines().find_map(|l| l.strip_prefix("Host: ")).unwrap_or("");
        let item = |id: &str| json!({"id": id, "categories": ["Holiday"]});
        let answer =
            |status: u16, value: Value| if status == 0 { crate::fake::json_answer("200 OK", &value.to_string()) } else { status_line(status) };
        match (method, target.split('?').next().unwrap_or("")) {
            ("GET", "/v1.0/me/calendar/events") if script.pages > 0 => {
                let n = target.split("page=").nth(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
                let named = if script.same_ids { 1 } else { n };
                let mut page = json!({"value": (0..100).map(|i| item(&format!("p{named}i{i}"))).collect::<Vec<_>>()});
                if n < script.pages {
                    page["@odata.nextLink"] = json!(format!("http://{host}/v1.0/me/calendar/events?page={}", n + 1));
                }
                answer(0, page)
            }
            ("GET", "/v1.0/me/calendar/events") if target.contains("page=2") => {
                answer(script.second_page.unwrap_or(0), json!({"value": [item("m2")]}))
            }
            ("GET", "/v1.0/me/calendar/events") => {
                let mut page = json!({"value": [item("m1")]});
                if script.second_page.is_some() {
                    page["@odata.nextLink"] = json!(format!("http://{host}/v1.0/me/calendar/events?page=2"));
                }
                answer(script.main_status, page)
            }
            ("GET", "/v1.0/me/calendars/other/events") => answer(script.other_status, json!({"value": [item("o1")]})),
            ("POST", "/v1.0/$batch") if script.batch_status != 0 => status_line(script.batch_status),
            ("POST", "/v1.0/$batch") if script.batch_empty => crate::fake::json_answer("200 OK", "{}"),
            ("POST", "/v1.0/$batch") if script.writes_status != 0 && body.contains("\"PATCH\"") => status_line(script.writes_status),
            ("POST", "/v1.0/$batch") => {
                let asked = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
                let mut responses = Vec::new();
                for one in asked["requests"].as_array().cloned().unwrap_or_default() {
                    let url = one["url"].as_str().unwrap_or("");
                    let id = url.split('?').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
                    // The read of an item right before its change.
                    if one["method"] == "GET" {
                        assert!(url.ends_with("?%24select=categories") || url.ends_with("?$select=categories"), "{url}");
                        assert!(one.get("body").is_none());
                        if script.reads_first_only && !responses.is_empty() {
                            continue;
                        }
                        let status = script.read_status.iter().find(|(have, _)| *have == id).map_or(200, |(_, status)| *status);
                        let body = script.read_body.iter().find(|(have, _)| *have == id).map_or_else(|| item(&id), |(_, body)| body.clone());
                        let mut response = json!({"id": one["id"], "status": status, "body": if status == 200 { body } else { json!({}) }});
                        if let Some(wait) = script.item_wait.filter(|_| status == 429) {
                            response["headers"] = json!({"retry-after": wait});
                        }
                        responses.push(response);
                        continue;
                    }
                    assert_eq!(one["method"], "PATCH");
                    if script.read_body.is_empty() {
                        assert_eq!(one["body"], json!({"categories": ["Holiday 2026"]}));
                    }
                    script.written.push((id.clone(), one["body"].clone()));
                    let status = script.item_status.iter().find(|(have, _)| *have == id).map_or(200, |(_, status)| *status);
                    let status = if script.every_item != 0 { script.every_item } else { status };
                    if script.batch_first_only && !responses.is_empty() {
                        continue;
                    }
                    if status == 200 {
                        script.changed.push(id);
                    }
                    let mut response = json!({"id": one["id"], "status": status, "body": {}});
                    if let Some(wait) = script.item_wait.filter(|_| status == 429) {
                        response["headers"] = json!({"Retry-After": wait});
                    }
                    responses.push(response);
                }
                if let Some(place) = script.answers_unasked.filter(|_| body.contains("\"PATCH\"")) {
                    responses.push(json!({"id": place, "status": 200, "body": {}}));
                }
                crate::fake::json_answer("200 OK", &json!({"responses": responses}).to_string())
            }
            // One item by itself, for a server that takes no batch.
            (method @ ("GET" | "PATCH"), path) if path.starts_with("/v1.0/me/events/") => {
                let id = path.rsplit('/').next().unwrap_or("").to_string();
                script.singly.push((method.to_string(), id.clone()));
                if method == "PATCH" {
                    script.changed.push(id.clone());
                    if let Some(stop) = &script.stop {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                answer(0, item(&id))
            }
            _ => status_line(404),
        }
    })
}

/// A rename of the events of both lists against `script`: what came of it, and what was changed.
fn scripted_run(script: Script, both: bool) -> (CategoryRenamed, Vec<String>) {
    let script = Arc::new(std::sync::Mutex::new(script));
    let server = scripted(&script);
    let graph = super::Graph::at(&server.url(""), TOKEN).unwrap();
    let mut lists = vec!["/v1.0/me/calendar/events".to_string()];
    if both {
        lists.push("/v1.0/me/calendars/other/events".to_string());
    }
    let items = super::recategorize::Items { lists, item: "/me/events/".into() };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {}).renamed;
    let changed = script.lock().unwrap().changed.clone();
    (came, changed)
}

#[test]
fn a_calendar_the_server_wont_list_is_counted_and_the_others_are_renamed() {
    for status in [403, 400] {
        let (came, changed) = scripted_run(Script { other_status: status, ..Default::default() }, true);
        assert!(came.done && !came.denied && !came.refused, "{status}: {came:?}");
        assert_eq!((came.changed, came.lists_refused), (1, 1), "{status}");
        assert_eq!(changed, ["m1"], "{status}");
    }
    // The main one refused, the other not: the same the other way round.
    let (came, changed) = scripted_run(Script { main_status: 403, ..Default::default() }, true);
    assert!(came.done && came.lists_refused == 1, "{came:?}");
    assert_eq!(changed, ["o1"]);
}

#[test]
fn when_no_list_says_it_nothing_is_changed_and_it_isnt_asked_again() {
    let (came, changed) = scripted_run(Script { main_status: 403, ..Default::default() }, false);
    assert!(came.denied && !came.refused && !came.done && changed.is_empty(), "{came:?}");
    let (came, changed) = scripted_run(Script { main_status: 400, ..Default::default() }, false);
    assert!(came.refused && !came.denied && !came.done && changed.is_empty(), "{came:?}");
    let (came, _) = scripted_run(Script { main_status: 403, other_status: 400, ..Default::default() }, true);
    assert!(came.denied && !came.done, "{came:?}");
    // The main list answering "not found" is no list that is gone (only a calendar named by its
    // id can be that): the server won't say, which is said, and the old name stays.
    let (came, changed) = scripted_run(Script { main_status: 404, other_status: 410, ..Default::default() }, true);
    assert!(came.refused && !came.done && !came.denied && changed.is_empty(), "{came:?}");
    // With another calendar that does answer: the main one is counted as not said, and said.
    let (came, changed) = scripted_run(Script { main_status: 404, ..Default::default() }, true);
    assert!(came.done && came.lists_refused == 1 && !came.refused, "{came:?}");
    assert_eq!(changed, ["o1"]);
    let (came, _) = scripted_run(Script { main_status: 404, other_status: 400, ..Default::default() }, true);
    assert!(came.refused && !came.done, "{came:?}");
}

#[test]
fn a_busy_or_failing_server_leaves_it_for_a_later_run() {
    for status in [429, 503, 500] {
        let (came, changed) = scripted_run(Script { main_status: status, ..Default::default() }, true);
        assert!(!came.done && !came.denied && !came.refused && changed.is_empty(), "{status}: {came:?}");
        // Its own failure counts against the tries a rename is given; being busy doesn't.
        assert_eq!(came.failed, status == 500, "{status}");
    }
}

#[test]
fn an_item_the_server_puts_off_is_counted_and_the_rename_isnt_done() {
    for status in [412, 409, 500] {
        let (came, changed) = scripted_run(Script { item_status: vec![("o1", status)], ..Default::default() }, true);
        assert!(!came.done, "{status}: {came:?}");
        assert_eq!((came.changed, came.stuck, came.skipped), (1, 1, 0), "{status}");
        assert_eq!(changed, ["m1"], "{status}");
    }
    // One it won't change for good is counted otherwise, and holds nothing up; one that is gone is nothing.
    let (came, _) = scripted_run(Script { item_status: vec![("o1", 403), ("m1", 404)], ..Default::default() }, true);
    assert!(came.done, "{came:?}");
    assert_eq!((came.changed, came.stuck, came.skipped), (0, 0, 1));
}

#[test]
fn trouble_on_a_later_page_changes_what_was_read_and_leaves_the_rest_for_a_later_run() {
    let (came, changed) = scripted_run(Script { second_page: Some(0), ..Default::default() }, false);
    assert!(came.done && changed == ["m1", "m2"], "{came:?} {changed:?}");
    for status in [403, 400] {
        let (came, changed) = scripted_run(Script { second_page: Some(status), ..Default::default() }, false);
        assert!(!came.done && !came.denied && !came.refused && came.failed, "{status}: {came:?}");
        assert_eq!((came.changed, came.lists_refused), (1, 0), "{status}");
        assert_eq!(changed, ["m1"], "{status}");
        // Not done either when nothing of what was read could be changed.
        let (came, _) = scripted_run(Script { second_page: Some(status), item_status: vec![("m1", 403)], ..Default::default() }, false);
        assert!(!came.done && came.failed, "{status}: {came:?}");
    }
}

#[test]
fn a_batch_the_server_fails_at_or_answers_with_nothing_counts_its_items_as_stuck() {
    for script in [Script { batch_status: 500, ..Default::default() }, Script { batch_empty: true, ..Default::default() }] {
        let (came, changed) = scripted_run(script, false);
        assert!(!came.done && !came.denied && !came.refused && changed.is_empty(), "{came:?}");
        assert_eq!((came.changed, came.stuck), (0, 1), "{came:?}");
    }
    // Busy is waited out, not counted.
    let (came, _) = scripted_run(Script { batch_status: 503, ..Default::default() }, false);
    assert_eq!((came.done, came.changed, came.stuck, came.failed), (false, 0, 0, false), "{came:?}");
    // An answer that leaves an item out: that one is counted, so that it isn't asked for ever.
    let (came, changed) = scripted_run(Script { batch_first_only: true, ..Default::default() }, true);
    assert_eq!((came.done, came.changed, came.stuck), (false, 1, 1), "{came:?}");
    assert_eq!(changed, ["m1"]);
}

#[test]
fn a_runs_fill_that_the_server_wont_change_is_the_end_and_says_there_may_be_more() {
    // More items than a run changes, and the server refuses each: asking again would list the
    // same, so it is done, and it says that more may keep the name.
    let (came, changed) = scripted_run(Script { pages: 21, every_item: 403, ..Default::default() }, false);
    assert!(came.done && came.more && changed.is_empty(), "{came:?}");
    assert_eq!((came.changed, came.skipped), (0, 2_000), "{came:?}");
    // Each of them gone meanwhile: a later run lists others, so it isn't the end.
    let (came, _) = scripted_run(Script { pages: 21, every_item: 404, ..Default::default() }, false);
    assert_eq!((came.done, came.more, came.changed, came.skipped), (false, false, 0, 0), "{came:?}");
    assert!(came.failed, "a server that goes on listing what it says is gone is counted, and given up in the end");
    // All of them listed: the end, and nothing more to say.
    let (came, _) = scripted_run(Script { pages: 2, every_item: 403, ..Default::default() }, false);
    assert_eq!((came.done, came.more, came.skipped), (true, false, 200), "{came:?}");
}

#[test]
fn an_item_with_many_categories_keeps_them_all_and_one_too_long_to_write_back_is_left_and_said() {
    let server = StandIn::start().unwrap();
    // Thirty names, the old one among them: each is there afterwards, the old one renamed.
    let mut many: Vec<String> = (0..29).map(|n| format!("C{n}")).collect();
    many.insert(27, "Holiday".into());
    let id = message(&server, "inbox", "Many", &many.iter().map(String::as_str).collect::<Vec<_>>());
    // More than is written back (the stand-in keeps a hundred at most): left whole, and counted.
    let mut long: Vec<String> = (0..100).map(|n| format!("L{n}")).collect();
    long[99] = "Holiday".into();
    let kept = message(&server, "inbox", "Long", &long.iter().map(String::as_str).collect::<Vec<_>>());
    server.state().mailbox.messages.get_mut(&kept).unwrap().categories.push("One more".into());
    let came = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    many[27] = "Holiday 2026".into();
    assert_eq!(categories_of(&server, &id), many);
    assert_eq!(categories_of(&server, &kept).len(), 101, "a cut copy was written");
    assert!(categories_of(&server, &kept).iter().any(|c| c == "Holiday"));
    assert_eq!((came.done, came.changed, came.skipped, came.passed), (true, 1, 0, 1), "{came:?}");
    // One whose list holds what isn't a plain name is left as it is too: no change is sent.
    let server = StandIn::start().unwrap();
    let odd = message(&server, "inbox", "Odd", &["Holiday", "Bell\u{7}"]);
    let wide = message(&server, "inbox", "Wide", &["Holiday", &"w".repeat(256)]);
    let came = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    assert_eq!((came.done, came.changed, came.skipped, came.passed), (true, 0, 0, 2), "{came:?}");
    assert_eq!(categories_of(&server, &odd), names(&["Holiday", "Bell\u{7}"]));
    assert_eq!(categories_of(&server, &wide).len(), 2);
}

// ---- a category changed on the server while the rename is under way ----

#[test]
fn a_category_given_on_the_server_after_the_lists_were_read_stays() {
    // Mail, in a batch: "Urgent" is given to a message right after the server listed it.
    let server = StandIn::start().unwrap();
    let late = message(&server, "inbox", "Flights", &["Red", "Holiday"]);
    let off = message(&server, "inbox", "Hotel", &["Holiday", "Blue"]);
    let plain = message(&server, "inbox", "Car", &["Holiday"]);
    server.state().meanwhile = vec![(late.clone(), names(&["Red", "Holiday", "Urgent"])), (off.clone(), names(&["Blue", "Later"]))];
    let came = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    assert_eq!(categories_of(&server, &late), names(&["Red", "Holiday 2026", "Urgent"]), "the category given meanwhile was lost");
    // One the old name was taken off meanwhile is not written at all, and not counted as renamed.
    assert_eq!(categories_of(&server, &off), names(&["Blue", "Later"]));
    assert_eq!(categories_of(&server, &plain), names(&["Holiday 2026"]));
    assert_eq!((came.done, came.changed, came.skipped, came.stuck, came.passed), (true, 2, 0, 0, 0), "{came:?}");

    // Mail, one request at a time.
    let server = StandIn::start().unwrap();
    let late = message(&server, "inbox", "Flights", &["Holiday"]);
    server.state().refuse_batches = true;
    server.state().meanwhile = vec![(late.clone(), names(&["Urgent", "Holiday"]))];
    let graph = super::Graph::for_mail(&server.url(), TOKEN, Arc::new(Gate::new()), super::pace::Priority::Owner, QUICK).unwrap();
    let items = super::recategorize::Items { lists: vec![graph.mail_path("/messages")], item: graph.batch_path("/messages/") };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {}).renamed;
    assert_eq!(categories_of(&server, &late), names(&["Urgent", "Holiday 2026"]));
    assert!(came.done && came.changed == 1, "{came:?}");

    // Events.
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let flight = event(&server, &main, "Flight out", &["Holiday"]);
    server.state().meanwhile = vec![(flight.clone(), names(&["Holiday", "Urgent"]))];
    let came = events_run(&server, rename("Holiday", "Holiday 2026"), "").unwrap();
    assert_eq!(event_categories(&server, &flight), names(&["Holiday 2026", "Urgent"]));
    assert!(came.done && came.changed == 1, "{came:?}");
}

#[test]
fn a_list_grown_too_long_meanwhile_is_left_whole_and_said() {
    let server = StandIn::start().unwrap();
    let id = message(&server, "inbox", "Many", &["Holiday"]);
    let mut long: Vec<String> = (0..100).map(|n| format!("L{n}")).collect();
    long.push("Holiday".into());
    server.state().meanwhile = vec![(id.clone(), long.clone())];
    let came = mail_run(&server, Some(rename("Holiday", "Holiday 2026"))).unwrap();
    assert_eq!(categories_of(&server, &id), long, "a cut copy was written");
    assert_eq!((came.done, came.changed, came.passed), (true, 0, 1), "{came:?}");
}

/// A scripted run of both lists: what came of it, and each change that was asked for.
fn scripted_writes(script: Script) -> (CategoryRenamed, Vec<(String, serde_json::Value)>) {
    let script = Arc::new(std::sync::Mutex::new(script));
    let server = scripted(&script);
    let graph = super::Graph::at(&server.url(""), TOKEN).unwrap();
    let lists = vec!["/v1.0/me/calendar/events".to_string(), "/v1.0/me/calendars/other/events".to_string()];
    let items = super::recategorize::Items { lists, item: "/me/events/".into() };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {}).renamed;
    let written = script.lock().unwrap().written.clone();
    (came, written)
}

#[test]
fn what_the_read_before_a_change_says_decides_the_change() {
    use serde_json::json;
    // The list as it is now is what is written, not the one the item was listed with.
    let (came, written) = scripted_writes(Script { read_body: vec![("m1", json!({"categories": ["Urgent", "Holiday"]}))], ..Default::default() });
    assert_eq!(
        written,
        [("m1".to_string(), json!({"categories": ["Urgent", "Holiday 2026"]})), ("o1".to_string(), json!({"categories": ["Holiday 2026"]}))]
    );
    assert!(came.done && came.changed == 2, "{came:?}");
    // No longer there, or without the old name: no change is sent for it, and it isn't counted.
    let script = Script { read_status: vec![("m1", 404)], read_body: vec![("o1", json!({"categories": ["Holiday 2026"]}))], ..Default::default() };
    let (came, written) = scripted_writes(script);
    assert!(written.is_empty(), "{written:?}");
    assert_eq!((came.done, came.changed, came.skipped, came.stuck), (true, 0, 0, 0), "{came:?}");
    // A list that can't be written back whole: left, and counted.
    let (came, written) = scripted_writes(Script { read_body: vec![("m1", json!({"categories": ["Holiday", 7]}))], ..Default::default() });
    assert_eq!(written.len(), 1);
    assert_eq!((came.done, came.changed, came.passed), (true, 1, 1), "{came:?}");
    // The read put off, answered with no list, or not answered: not written, counted, and a later run.
    for script in [
        Script { read_status: vec![("m1", 500)], ..Default::default() },
        Script { read_status: vec![("m1", 412)], ..Default::default() },
        Script { read_body: vec![("m1", json!({"id": "m1"}))], ..Default::default() },
        Script { reads_first_only: true, read_body: vec![("o1", json!({"categories": ["Holiday"]}))], ..Default::default() },
    ] {
        let first_only = script.reads_first_only;
        let (came, written) = scripted_writes(script);
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0].0, if first_only { "m1" } else { "o1" });
        assert_eq!((came.done, came.changed, came.stuck), (false, 1, 1), "{came:?}");
    }
    // The read refused for good: not written, counted as one the server won't change.
    let (came, written) = scripted_writes(Script { read_status: vec![("m1", 403)], ..Default::default() });
    assert_eq!(written.len(), 1);
    assert_eq!((came.done, came.changed, came.skipped), (true, 1, 1), "{came:?}");
    // A busy server at the read: nothing written of it, not counted, a later run.
    let (came, written) = scripted_writes(Script { read_status: vec![("m1", 429)], ..Default::default() });
    assert_eq!((came.done, came.stuck, came.failed), (false, 0, false), "{came:?} {written:?}");
    assert!(written.iter().all(|(id, _)| id != "m1"), "{written:?}");
    // The batch of changes failing as a whole, the reads answered: counted for what was to be
    // written, whatever the status (a server that read in a batch takes batches).
    for status in [500, 400] {
        let (came, _) = scripted_writes(Script { writes_status: status, ..Default::default() });
        assert_eq!((came.done, came.changed, came.stuck), (false, 0, 2), "{status}: {came:?}");
    }
    let (came, _) = scripted_writes(Script { writes_status: 503, ..Default::default() });
    assert_eq!((came.done, came.changed, came.stuck, came.failed), (false, 0, 0, false), "{came:?}");
    // An answer for an item that wasn't written (its list can't be written back) changes nothing
    // of what is said of it: it is left, and counted.
    let script = Script { read_body: vec![("o1", json!({"categories": ["Holiday", 7]}))], answers_unasked: Some("1"), ..Default::default() };
    let (came, written) = scripted_writes(script);
    assert_eq!(written.len(), 1);
    assert_eq!((came.done, came.changed, came.passed), (true, 1, 1), "{came:?}");
    // A list far too long, the old name past where it is looked for: left and counted, not taken for one without it.
    let mut long: Vec<serde_json::Value> = (0..1_200).map(|n| json!(format!("L{n}"))).collect();
    long.push(json!("Holiday"));
    let (came, written) = scripted_writes(Script { read_body: vec![("m1", json!({"categories": long}))], ..Default::default() });
    assert_eq!((written.len(), came.changed, came.passed), (1, 1, 1), "{came:?}");
}

#[test]
fn one_at_a_time_the_read_before_a_change_decides_too() {
    let server = StandIn::start().unwrap();
    let off = message(&server, "inbox", "Hotel", &["Holiday"]);
    let grown = message(&server, "inbox", "Car", &["Holiday"]);
    let gone = message(&server, "inbox", "Train", &["Holiday"]);
    let plain = message(&server, "inbox", "Boat", &["Holiday", "Red"]);
    let mut long: Vec<String> = (0..100).map(|n| format!("L{n}")).collect();
    long.push("Holiday".into());
    server.state().refuse_batches = true;
    server.state().meanwhile = vec![(off.clone(), names(&["Blue"])), (grown.clone(), long.clone())];
    // (Deleted right after it was listed: its read says it is gone.)
    let graph = super::Graph::for_mail(&server.url(), TOKEN, Arc::new(Gate::new()), super::pace::Priority::Owner, QUICK).unwrap();
    let items = super::recategorize::Items { lists: vec![graph.mail_path("/messages")], item: graph.batch_path("/messages/") };
    let mut removed = false;
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), None, &mut |_, _| {
        if !std::mem::replace(&mut removed, true) {
            server.state().mailbox.messages.remove(&gone);
        }
    })
    .renamed;
    assert_eq!(categories_of(&server, &off), names(&["Blue"]));
    assert_eq!(categories_of(&server, &grown), long);
    assert_eq!(categories_of(&server, &plain), names(&["Holiday 2026", "Red"]));
    assert_eq!((came.done, came.changed, came.skipped, came.stuck, came.passed), (true, 1, 0, 0, 1), "{came:?}");
}

#[test]
fn a_run_stopped_while_the_names_are_written_says_it_was_stopped_and_keeps_what_was_done() {
    let server = StandIn::start().unwrap();
    let ids: Vec<String> = (0..45).map(|n| message(&server, "inbox", &format!("Note {n}"), &["Holiday"])).collect();
    let graph = super::Graph::for_mail(&server.url(), TOKEN, Arc::new(Gate::new()), super::pace::Priority::Owner, QUICK).unwrap();
    let request =
        GraphMailRequest { account: 3, email: "owner@example.org".into(), rename: Some(rename("Holiday", "Holiday 2026")), ..Default::default() };
    // The owner stops the run once the first twenty were answered.
    let cancel = AtomicBool::new(false);
    let mut last = mailcraft_model::sync::GraphMailResult::default();
    super::mail::renamed_into(&graph, &request, &cancel, &mut last, &mut |piece| {
        if piece.renaming.is_some_and(|r| r.done >= 20) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
    let came = last.renamed.clone().unwrap();
    assert!(last.stopped, "a rename cut short by the owner wasn't said to be stopped");
    assert_eq!((came.done, came.changed, came.stuck), (false, 20, 0), "{came:?}");
    assert_eq!(ids.iter().filter(|id| categories_of(&server, id) == names(&["Holiday 2026"])).count(), 20);
    // A run that got to its end is not one that was stopped, whatever is asked right after it.
    let mut last = mailcraft_model::sync::GraphMailResult::default();
    super::mail::renamed_into(&graph, &request, &AtomicBool::new(false), &mut last, &mut |_| {});
    assert!(!last.stopped && last.renamed.as_ref().is_some_and(|r| r.done && r.changed == 25), "{:?}", last.renamed);
    // Stopped while the lists are read: said too, and nothing is changed.
    let late = message(&server, "inbox", "Late", &["Holiday"]);
    let came = super::recategorize::rename(
        &graph,
        &super::recategorize::Items { lists: vec![graph.mail_path("/messages")], item: graph.batch_path("/messages/") },
        &rename("Holiday", "Holiday 2026"),
        Some(&AtomicBool::new(true)),
        &mut |_, _| {},
    );
    assert!(came.stopped && !came.renamed.done && came.renamed.changed == 0);
    assert_eq!(categories_of(&server, &late), names(&["Holiday"]));
}

#[test]
fn an_item_the_server_lists_many_times_is_changed_once() {
    // (Each page of the scripted list names the same hundred ids when asked so.)
    let (came, changed) = scripted_run(Script { pages: 3, same_ids: true, ..Default::default() }, false);
    assert_eq!((came.done, came.changed), (true, 100), "{came:?}");
    assert_eq!(changed.len(), 100);
}

/// A rename of the events of both lists against `script`, with everything the run says.
fn scripted_outcome(script: Script, cancel: Option<&AtomicBool>) -> (super::recategorize::Outcome, Script) {
    let script = Arc::new(std::sync::Mutex::new(script));
    let server = scripted(&script);
    let graph = super::Graph::at(&server.url(""), TOKEN).unwrap();
    let lists = vec!["/v1.0/me/calendar/events".to_string(), "/v1.0/me/calendars/other/events".to_string()];
    let items = super::recategorize::Items { lists, item: "/me/events/".into() };
    let came = super::recategorize::rename(&graph, &items, &rename("Holiday", "Holiday 2026"), cancel, &mut |_, _| {});
    let script = std::mem::take(&mut *script.lock().unwrap());
    (came, script)
}

#[test]
fn an_item_of_a_batch_that_says_how_long_to_wait_is_heeded() {
    // Its change is put off for two minutes; the other one's goes through and is counted.
    let (came, script) = scripted_outcome(Script { item_status: vec![("o1", 429)], item_wait: Some("120"), ..Default::default() }, None);
    assert_eq!(came.busy_for, Some(120));
    assert_eq!((came.renamed.done, came.renamed.changed, came.renamed.failed), (false, 1, false), "{:?}", came.renamed);
    assert_eq!(script.changed, ["m1"]);
    // The same said of its read (the header's name in small letters): it isn't written.
    let (came, script) = scripted_outcome(Script { read_status: vec![("o1", 429)], item_wait: Some("90"), ..Default::default() }, None);
    assert_eq!(came.busy_for, Some(90));
    assert_eq!((came.renamed.done, came.renamed.changed), (false, 1), "{:?}", came.renamed);
    assert_eq!(script.written.len(), 1);
    // One that names no time is busy all the same.
    let (came, _) = scripted_outcome(Script { every_item: 429, ..Default::default() }, None);
    assert_eq!((came.busy_for, came.renamed.done, came.renamed.changed), (Some(0), false, 0));
    // What isn't a time is none.
    let (came, _) = scripted_outcome(Script { every_item: 429, item_wait: Some("soon"), ..Default::default() }, None);
    assert_eq!(came.busy_for, Some(0));
}

#[test]
fn a_run_stopped_while_items_are_changed_one_at_a_time_asks_nothing_more() {
    // The server takes no batch, and the owner stops the run as the first item is changed.
    let stop = Arc::new(AtomicBool::new(false));
    let (came, script) = scripted_outcome(Script { batch_status: 400, stop: Some(stop.clone()), ..Default::default() }, Some(&stop));
    let singly: Vec<(&str, &str)> = script.singly.iter().map(|(method, id)| (method.as_str(), id.as_str())).collect();
    assert_eq!(singly, [("GET", "m1"), ("PATCH", "m1")], "the second item was asked for after the run was stopped");
    assert!(came.stopped, "a rename cut short by the owner wasn't said to be stopped");
    assert_eq!((came.renamed.done, came.renamed.changed, came.renamed.stuck), (false, 1, 0), "{:?}", came.renamed);
    // Not stopped: both are changed, and the run isn't said to be stopped.
    let (came, script) = scripted_outcome(Script { batch_status: 400, ..Default::default() }, None);
    assert!(!came.stopped && came.renamed.done && came.renamed.changed == 2, "{:?}", came.renamed);
    assert_eq!(script.singly.len(), 4);
}
