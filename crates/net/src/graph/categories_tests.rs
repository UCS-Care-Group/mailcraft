// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's list of categories told what was done to it here (#661), against a small
//! stand-in on loopback that keeps `masterCategories` and answers as the documented service does.
//! Nothing here leaves this machine.

use std::sync::{Arc, Mutex};

use mailcraft_model::Rgb;
use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::{CategoriesUpload, CategoryChange, CategoryChangeKind, CategoryResult};
use serde_json::{Value, json};

use super::categories::{color_name, push_categories_at};
use crate::fake::{FakeHttp, json_answer};
use crate::http;

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";

#[derive(Default)]
struct Server {
    categories: Vec<Value>,
    /// Answer every write with this status instead of doing it.
    write_status: Option<u16>,
    /// Answer the list with this status.
    list_status: Option<u16>,
    /// Categories it has and doesn't list yet (made there just now).
    unlisted: Vec<Value>,
    /// Every request, as `METHOD target`.
    asked: Vec<String>,
    next: u32,
}

fn raw(status: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes()
}

fn serve(state: &Arc<Mutex<Server>>) -> FakeHttp {
    let state = state.clone();
    FakeHttp::start(move |_, request| {
        let mut server = state.lock().unwrap();
        let (head, body) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
        let line = head.lines().next().unwrap_or("");
        if !head.lines().any(|l| l == format!("Authorization: Bearer {TOKEN}")) {
            return raw("401 Unauthorized");
        }
        let mut parts = line.split(' ');
        let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        server.asked.push(format!("{method} {target}"));
        let pieces: Vec<&str> = target.split('?').next().unwrap_or("").split('/').collect();
        if pieces.get(1..5) != Some(&["v1.0", "me", "outlook", "masterCategories"]) {
            return raw("404 Not Found");
        }
        let json = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
        let at = |server: &Server, piece: &str| server.categories.iter().position(|c| http::encode(c["id"].as_str().unwrap_or("")) == piece);
        match (method, pieces.get(5)) {
            ("GET", None) => match server.list_status {
                Some(status) => raw(&format!("{status} Other")),
                None => json_answer("200 OK", &json!({"value": server.categories}).to_string()),
            },
            _ if server.write_status.is_some() => raw(&format!("{} Other", server.write_status.unwrap_or(0))),
            ("POST", None) => {
                let name = json["displayName"].as_str().unwrap_or("").to_lowercase();
                if server.categories.iter().chain(&server.unlisted).any(|c| c["displayName"].as_str().unwrap_or("").to_lowercase() == name) {
                    return raw("409 Conflict");
                }
                server.next += 1;
                let made = json!({"id": format!("new/{}+=", server.next), "displayName": json["displayName"], "color": json["color"]});
                server.categories.push(made.clone());
                json_answer("201 Created", &made.to_string())
            }
            ("PATCH", Some(piece)) => match at(&server, piece) {
                // Only the colour may change: the name is the category's for good.
                Some(_) if json.get("displayName").is_some() => raw("400 Bad Request"),
                Some(i) => {
                    server.categories[i]["color"] = json["color"].clone();
                    let changed = server.categories[i].clone();
                    json_answer("200 OK", &changed.to_string())
                }
                None => raw("404 Not Found"),
            },
            ("DELETE", Some(piece)) => match at(&server, piece) {
                Some(i) => {
                    server.categories.remove(i);
                    raw("204 No Content")
                }
                None => raw("404 Not Found"),
            },
            _ => raw("405 Method Not Allowed"),
        }
    })
}

fn state(categories: Vec<Value>) -> Arc<Mutex<Server>> {
    Arc::new(Mutex::new(Server { categories, ..Default::default() }))
}

fn change(kind: CategoryChangeKind, name: &str, preset: usize) -> CategoryChange {
    CategoryChange { kind, name: name.into(), color: OUTLOOK_COLORS[preset] }
}

fn upload(changes: Vec<CategoryChange>) -> CategoriesUpload {
    CategoriesUpload { account: 1, email: "ada@example.org".into(), changes }
}

fn writes(s: &Arc<Mutex<Server>>) -> Vec<String> {
    s.lock().unwrap().asked.iter().filter(|a| !a.starts_with("GET ")).cloned().collect()
}

#[test]
fn made_recoloured_and_deleted_by_name_each_with_the_servers_own_id() {
    let s = state(vec![
        json!({"id": "AAMk/red+1=", "displayName": "Red category", "color": "preset0"}),
        json!({"id": "AAMk-old", "displayName": "Old", "color": "preset7"}),
    ]);
    let server = serve(&s);
    let changes = vec![
        change(CategoryChangeKind::Delete, "Old", 7),
        change(CategoryChangeKind::Create, "Проект", 7),
        change(CategoryChangeKind::Color, "red category", 4),
    ];
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes));
    assert!(!done.token_refused);
    let results: Vec<CategoryResult> = done.outcomes.iter().map(|o| o.result).collect();
    assert_eq!(results, [CategoryResult::Done; 3], "{:?}", done.outcomes);
    assert_eq!(done.outcomes[2].color, OUTLOOK_COLORS[4]);
    // Made by this one: not found there.
    assert!(done.outcomes.iter().all(|o| !o.found), "{:?}", done.outcomes);
    // An id is one piece of the address, whatever it holds.
    assert_eq!(
        writes(&s),
        [
            "DELETE /v1.0/me/outlook/masterCategories/AAMk-old",
            "POST /v1.0/me/outlook/masterCategories",
            "PATCH /v1.0/me/outlook/masterCategories/AAMk%2Fred%2B1%3D"
        ]
    );
    let there = s.lock().unwrap().categories.clone();
    assert_eq!(there.len(), 2);
    assert_eq!((there[0]["displayName"].as_str(), there[0]["color"].as_str()), (Some("Red category"), Some("preset4")));
    assert_eq!((there[1]["displayName"].as_str(), there[1]["color"].as_str()), (Some("Проект"), Some("preset7")));
}

#[test]
fn nothing_is_sent_for_what_the_server_has_already() {
    let s = state(vec![json!({"id": "A", "displayName": "Red category", "color": "preset0"})]);
    let server = serve(&s);
    let changes = vec![
        // Made there already, in that colour (whatever the case of its name).
        change(CategoryChangeKind::Create, "RED CATEGORY", 0),
        // Deleted there already, and recoloured after it was deleted there.
        change(CategoryChangeKind::Delete, "Gone", 3),
        change(CategoryChangeKind::Color, "Gone too", 3),
    ];
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes));
    assert!(done.outcomes.iter().all(|o| o.result == CategoryResult::Done), "{:?}", done.outcomes);
    assert!(writes(&s).is_empty(), "{:?}", writes(&s));
    // The one made there already is the server's own; the others weren't made at all.
    assert_eq!(done.outcomes.iter().map(|o| o.found).collect::<Vec<_>>(), [true, false, false]);
    // A change with nothing in it costs nothing at all.
    let done = push_categories_at(&server.url(""), TOKEN, &upload(Vec::new()));
    assert!(done.outcomes.is_empty());
    assert_eq!(s.lock().unwrap().asked.len(), 1);
}

#[test]
fn one_made_there_meanwhile_is_taken_for_made() {
    let s = state(Vec::new());
    // Made there between the read of the list and the creation.
    s.lock().unwrap().unlisted.push(json!({"id": "B", "displayName": "Twice", "color": "preset1"}));
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(vec![change(CategoryChangeKind::Create, "twice", 2)]));
    assert_eq!(done.outcomes[0].result, CategoryResult::Done, "{:?}", done.outcomes);
    assert!(done.outcomes[0].found, "the server's own");
    assert_eq!(writes(&s), ["POST /v1.0/me/outlook/masterCategories"]);
}

#[test]
fn a_creation_of_a_name_the_server_has_in_another_colour_says_it_was_found_there() {
    let s = state(vec![json!({"id": "A", "displayName": "Red category", "color": "preset0"})]);
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(vec![change(CategoryChangeKind::Create, "red category", 4)]));
    assert_eq!(done.outcomes[0].result, CategoryResult::Done, "{:?}", done.outcomes);
    assert!(done.outcomes[0].found);
    assert_eq!(writes(&s), ["PATCH /v1.0/me/outlook/masterCategories/A"]);
}

#[test]
fn a_sign_in_without_leave_to_change_the_list_stops_the_upload() {
    let s = state(Vec::new());
    s.lock().unwrap().write_status = Some(403);
    let server = serve(&s);
    let changes = vec![change(CategoryChangeKind::Create, "One", 0), change(CategoryChangeKind::Create, "Two", 1)];
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes.clone()));
    assert_eq!(done.outcomes.len(), 1);
    assert_eq!(done.outcomes[0].result, CategoryResult::Denied);
    assert_eq!(writes(&s).len(), 1);

    // Busy: nothing more is tried, and all of it goes again later.
    let s = state(Vec::new());
    s.lock().unwrap().write_status = Some(503);
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes.clone()));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [CategoryResult::Failed]);

    // Refused for good: the others still go.
    let s = state(Vec::new());
    s.lock().unwrap().write_status = Some(400);
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes.clone()));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [CategoryResult::Refused, CategoryResult::Refused]);
    assert!(done.outcomes.iter().all(|o| !o.error.is_empty() && !o.error.contains(TOKEN)));

    // A list that can't be read: nothing is sent, nothing is said, and it goes again later.
    let s = state(Vec::new());
    s.lock().unwrap().list_status = Some(503);
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes.clone()));
    assert!(done.outcomes.is_empty() && !done.token_refused);
    assert!(writes(&s).is_empty());

    // One the server won't let read either is no list to change.
    let s = state(Vec::new());
    s.lock().unwrap().list_status = Some(403);
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), TOKEN, &upload(changes));
    assert_eq!(done.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [CategoryResult::Denied]);
}

#[test]
fn a_refused_sign_in_sends_nothing_more() {
    let s = state(Vec::new());
    let server = serve(&s);
    let done = push_categories_at(&server.url(""), "another.tok-en", &upload(vec![change(CategoryChangeKind::Create, "One", 0)]));
    assert!(done.token_refused && done.outcomes.is_empty());
    assert!(writes(&s).is_empty());
}

#[test]
fn a_colour_is_sent_as_the_servers_own_name_for_it() {
    for (i, color) in OUTLOOK_COLORS.iter().enumerate() {
        assert_eq!(color_name(*color), format!("preset{i}"));
    }
    // Any other colour goes as the nearest of them.
    assert_eq!(color_name(Rgb(0xD0, 0x3B, 0x3B)), "preset0");
    assert_eq!(color_name(Rgb(0x2F, 0x70, 0xD1)), "preset7");
}
