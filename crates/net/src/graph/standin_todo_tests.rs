// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's To Do and categories, and its loopback start, held to what this crate sends and
//! to what the reference says. Synthetic data only.

use std::io::{Read, Write};
use std::net::TcpStream;

use mailcraft_model::TaskPatch;
use mailcraft_model::remote_tasks::{TaskOp, TaskOpKind, TaskResult, TasksUpload};
use mailcraft_time::Zone;
use serde_json::{Value, json};

use super::standin::{StandIn, TOKEN};
use super::tasks::{push_tasks_at, tasks_at};

/// One raw request with a token and a JSON body: the status and the body as JSON.
fn call(server: &StandIn, method: &str, path: &str, body: Option<&str>) -> (u16, Value) {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let status = raw.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap();
    let json = raw.split_once("\r\n\r\n").map(|(_, b)| b).and_then(|b| serde_json::from_str(b).ok()).unwrap_or(Value::Null);
    (status, json)
}

fn create(title: &str) -> TaskOp {
    TaskOp { kind: TaskOpKind::Create, task: 7, patch: TaskPatch { title: Some(title.into()), ..Default::default() }, ..Default::default() }
}

fn upload(ops: Vec<TaskOp>) -> TasksUpload {
    TasksUpload { account: 1, email: "owner@example.org".into(), ops }
}

#[test]
fn it_listens_on_the_port_it_is_told_and_on_loopback_only() {
    // A port that is free now: bind one, note it, let it go.
    let free = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let told = StandIn::start_on(free).unwrap();
    assert_eq!(told.port, free);
    assert_eq!(told.url(), format!("http://127.0.0.1:{free}"));
    // It is taken now, so asking for it again fails rather than listening twice.
    assert!(StandIn::start_on(free).is_err());
    // Dropped, it lets the port go.
    drop(told);
    let again = StandIn::start_on(free).unwrap();
    assert_eq!(again.port, free);
    // Port 0 picks another one, on loopback too.
    let picked = StandIn::start_on(0).unwrap();
    assert_ne!(picked.port, free);
    assert!(picked.url().starts_with("http://127.0.0.1:"));
}

#[test]
fn a_client_reads_and_writes_to_dos_through_it() {
    let server = StandIn::start().unwrap();
    let list = server.state().default_todo_list();
    server.state().add_todo(&list, "Call Bo").unwrap();
    let got = tasks_at(&server.url(), TOKEN, Zone::UTC).unwrap();
    assert!(got.complete);
    // The flagged-mail list is left alone.
    assert_eq!(got.lists.len(), 1);
    assert_eq!(got.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Call Bo"]);

    let done = push_tasks_at(&server.url(), TOKEN, &upload(vec![create("Send the notes")]), Zone::UTC);
    assert!(!done.token_refused);
    let [made] = done.outcomes.as_slice() else { panic!("{done:?}") };
    assert_eq!(made.result, TaskResult::Done);
    assert_eq!((made.status, made.list.as_str()), (201, list.as_str()));
    assert!(!made.id.is_empty());

    let change = TaskOp {
        kind: TaskOpKind::Update,
        id: made.id.clone(),
        list: list.clone(),
        patch: TaskPatch { completed: Some(Some(1_790_000_000)), importance: Some(mailcraft_model::Importance::High), ..Default::default() },
        ..Default::default()
    };
    let delete = TaskOp { kind: TaskOpKind::Delete, id: made.id.clone(), list: list.clone(), ..Default::default() };
    let done = push_tasks_at(&server.url(), TOKEN, &upload(vec![change]), Zone::UTC);
    assert_eq!(done.outcomes.first().map(|o| o.result), Some(TaskResult::Done));
    let kept = server.state().todo.iter().flat_map(|l| l.tasks.iter()).find(|t| t.title == "Send the notes").cloned().unwrap();
    assert_eq!((kept.status.as_str(), kept.importance.as_str()), ("completed", "high"));
    assert!(kept.completed.is_some());

    let done = push_tasks_at(&server.url(), TOKEN, &upload(vec![delete.clone()]), Zone::UTC);
    assert_eq!(done.outcomes.first().map(|o| o.result), Some(TaskResult::Done));
    // Gone now: a second deletion says so.
    let done = push_tasks_at(&server.url(), TOKEN, &upload(vec![delete]), Zone::UTC);
    assert_eq!(done.outcomes.first().map(|o| o.result), Some(TaskResult::Gone));
}

#[test]
fn a_task_takes_only_what_a_task_has() {
    let server = StandIn::start().unwrap();
    let list = server.state().default_todo_list();
    let tasks = format!("/v1.0/me/todo/lists/{list}/tasks");
    let (status, made) = call(&server, "POST", &tasks, Some(&json!({"title": "A", "importance": "low"}).to_string()));
    assert_eq!(status, 201, "{made}");
    assert_eq!((made["status"].as_str(), made["importance"].as_str()), (Some("notStarted"), Some("low")));
    for bad in [
        json!({"title": 5}),
        json!({"status": "done"}),
        json!({"importance": "urgent"}),
        json!({"flavour": "mint"}),
        json!({"dueDateTime": "tomorrow"}),
        json!({"dueDateTime": {"dateTime": "tomorrow", "timeZone": "UTC"}}),
        json!({"dueDateTime": {"dateTime": "2026-09-21T00:00:00", "timeZone": ""}}),
        json!({"dueDateTime": {"dateTime": "20260921T00:00:00", "timeZone": "UTC"}}),
        json!({"dueDateTime": {"dateTime": "2026-09-21T00:00:00", "timeZone": "Mars/Olympus_Mons"}}),
        json!({"reminderDateTime": {"dateTime": "2026-09-21T00:00:00", "timeZone": "Mars/Olympus_Mons"}}),
        json!({"categories": [1]}),
    ] {
        let (status, body) = call(&server, "POST", &tasks, Some(&bad.to_string()));
        assert_eq!(status, 400, "{bad} {body}");
    }
    // None of the refused ones was made.
    assert_eq!(server.state().todo.iter().map(|l| l.tasks.len()).sum::<usize>(), 1);
    let id = made["id"].as_str().unwrap();
    let one = format!("{tasks}/{id}");
    let (status, body) = call(&server, "PATCH", &one, Some(&json!({"status": "completed"}).to_string()));
    assert_eq!(status, 200);
    assert!(body["completedDateTime"].is_object());
    let (_, body) = call(&server, "PATCH", &one, Some(&json!({"status": "inProgress"}).to_string()));
    assert!(body.get("completedDateTime").is_none());
    let both = json!({"status": "notStarted", "completedDateTime": {"dateTime": "2026-09-21T00:00:00", "timeZone": "UTC"}});
    let (_, body) = call(&server, "PATCH", &one, Some(&both.to_string()));
    assert!(body.get("completedDateTime").is_none(), "{body}");
    assert_eq!(call(&server, "GET", &format!("{tasks}/nothing"), None).0, 404);
    assert_eq!(call(&server, "DELETE", &one, None).0, 204);
}

#[test]
fn lists_are_made_renamed_and_deleted_but_not_the_well_known_ones() {
    let server = StandIn::start().unwrap();
    let (status, made) = call(&server, "POST", "/v1.0/me/todo/lists", Some(&json!({"displayName": "Errands"}).to_string()));
    assert_eq!(status, 201);
    let one = format!("/v1.0/me/todo/lists/{}", made["id"].as_str().unwrap());
    assert_eq!(call(&server, "PATCH", &one, Some(&json!({"displayName": "Chores"}).to_string())).1["displayName"], "Chores");
    assert_eq!(call(&server, "DELETE", &one, None).0, 204);
    let default = format!("/v1.0/me/todo/lists/{}", server.state().default_todo_list());
    assert_eq!(call(&server, "DELETE", &default, None).0, 400);
    let (_, all) = call(&server, "GET", "/v1.0/me/todo/lists", None);
    assert_eq!(all["value"].as_array().map(Vec::len), Some(2));
}

#[test]
fn categories_have_a_name_once_and_a_colour_that_exists() {
    let server = StandIn::start().unwrap();
    let path = "/v1.0/me/outlook/masterCategories";
    let (status, made) = call(&server, "POST", path, Some(&json!({"displayName": "Work", "color": "preset9"}).to_string()));
    assert_eq!(status, 201, "{made}");
    assert_eq!(call(&server, "POST", path, Some(&json!({"displayName": "work", "color": "preset1"}).to_string())).0, 409);
    assert_eq!(call(&server, "POST", path, Some(&json!({"displayName": "Odd", "color": "preset25"}).to_string())).0, 400);
    assert_eq!(call(&server, "POST", path, Some(&json!({"displayName": "Num", "color": 42}).to_string())).0, 400);
    let one = format!("{path}/{}", made["id"].as_str().unwrap());
    assert_eq!(call(&server, "PATCH", &one, Some(&json!({"displayName": "Other"}).to_string())).0, 400);
    assert_eq!(call(&server, "PATCH", &one, Some(&json!({"color": "preset2"}).to_string())).1["color"], "preset2");
    assert_eq!(call(&server, "GET", path, None).1["value"].as_array().map(Vec::len), Some(1));
    assert_eq!(call(&server, "DELETE", &one, None).0, 204);
    assert_eq!(call(&server, "GET", &one, None).0, 404);
}

#[test]
fn more_than_a_page_of_lists_comes_with_a_next_link() {
    let server = StandIn::start().unwrap();
    for n in 0..12 {
        let made = call(&server, "POST", "/v1.0/me/todo/lists", Some(&json!({"displayName": format!("L{n}")}).to_string()));
        assert_eq!(made.0, 201);
    }
    let (_, first) = call(&server, "GET", "/v1.0/me/todo/lists", None);
    assert_eq!(first["value"].as_array().map(Vec::len), Some(10));
    let next = first["@odata.nextLink"].as_str().unwrap();
    let (_, second) = call(&server, "GET", next.trim_start_matches(&server.url()), None);
    assert_eq!(second["value"].as_array().map(Vec::len), Some(4));
    assert!(second.get("@odata.nextLink").is_none());
}

#[test]
fn dropping_it_is_bounded_with_a_trickling_client_and_one_that_never_reads() {
    let server = StandIn::start().unwrap();
    let port = server.port;
    // One that trickles a byte every 100 ms and never finishes its request.
    let mut slow = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let trickler = std::thread::spawn(move || {
        for byte in b"GET /v1.0/me/todo/lists HTTP/1.1\r\nX-Slow: yes".iter().cycle().take(200) {
            if slow.write_all(&[*byte]).is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    let started = std::time::Instant::now();
    drop(server);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "drop took {:?}", started.elapsed());
    let _ = trickler.join();
    // The port is free again.
    drop(StandIn::start_on(port).unwrap());

    // One that asks and never reads the answer.
    let server = StandIn::start().unwrap();
    let port = server.port;
    let mut silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
    silent.write_all(format!("GET /v1.0/me/todo/lists HTTP/1.1\r\nAuthorization: Bearer {TOKEN}\r\n\r\n").as_bytes()).unwrap();
    let started = std::time::Instant::now();
    drop(server);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "drop took {:?}", started.elapsed());
    drop(StandIn::start_on(port).unwrap());
    drop(silent);
}
