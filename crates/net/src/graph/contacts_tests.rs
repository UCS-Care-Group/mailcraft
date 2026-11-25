// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts both ways against a small stand-in for Graph's contacts on loopback (#1917): folders
//! and their children, lists of changes from a position and a position forgotten, shared
//! mailboxes, every answer to a change, hostile answers and the caps. Nobody's real mailbox:
//! `example.org` people only.

use std::sync::{Arc, Mutex};

use mailcraft_model::Labeled;
use mailcraft_model::contact_sync::{ContactChangeKind, ContactField, ContactFolder, ContactOp, ContactResult, ContactsUpload, FolderOwner};
use mailcraft_model::remote::RemoteContact;
use proptest::prelude::*;
use serde_json::{Value, json};

use super::contacts::{push_contacts_at, read_contacts_at};
use crate::fake::{FakeHttp, json_answer};
use crate::http;

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";
const MAIN: &str = "main-folder=";
const SALES: &str = "sales@example.org";

/// One contact the stand-in keeps: its id, its folder, its properties, and when it last changed
/// (the stand-in's own count of changes).
#[derive(Clone)]
struct Kept {
    id: String,
    folder: String,
    body: Value,
    changed: u64,
    gone: bool,
}

/// Graph's contacts as the stand-in has them: mailboxes by `me` or address, each with folders
/// `(id, name, parent)` and contacts; who may be read; what is answered next to a change.
#[derive(Default)]
struct State {
    folders: Vec<(String, String, String, String)>,
    contacts: Vec<Kept>,
    count: u64,
    /// The count from which a list of changes is no longer had (a position older is 410).
    forgot_before: u64,
    /// Mailboxes besides `me` that may be read.
    open: Vec<String>,
    /// The status the next change is answered with, when it isn't to be done.
    next_status: Option<u16>,
    made: u64,
    /// The list of the main folder's children fails.
    fail_listing: bool,
}

impl State {
    fn add_folder(&mut self, mailbox: &str, id: &str, name: &str, parent: &str) {
        self.folders.push((mailbox.into(), id.into(), name.into(), parent.into()));
    }

    fn add(&mut self, mailbox: &str, folder: &str, id: &str, body: Value) {
        self.count += 1;
        let folder = if folder == MAIN { format!("{mailbox}:{MAIN}") } else { folder.to_string() };
        self.contacts.push(Kept { id: id.into(), folder, body, changed: self.count, gone: false });
    }
}

fn person(id: &str, name: &str, email: &str) -> Value {
    json!({"id": id, "changeKey": format!("ck-{id}"), "displayName": name, "givenName": name, "emailAddresses": [{"name": format!("{name} (work)"), "address": email}]})
}

/// The stand-in, answering as Graph's reference has it for what it knows, and 400 else.
fn stand_in(state: Arc<Mutex<State>>) -> FakeHttp {
    let port = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let known = port.clone();
    let server = FakeHttp::start(move |_, request| {
        let base = format!("http://127.0.0.1:{}", known.load(std::sync::atomic::Ordering::Relaxed));
        answer(&mut state.lock().unwrap(), request, &base)
    });
    port.store(server.port, std::sync::atomic::Ordering::Relaxed);
    server
}

fn page(value: Vec<Value>, link: (&str, String)) -> Vec<u8> {
    let mut page = json!({"value": value});
    page[link.0] = json!(link.1);
    json_answer("200 OK", &page.to_string())
}

fn error(status: &str, code: &str) -> Vec<u8> {
    json_answer(status, &json!({"error": {"code": code, "message": "no"}}).to_string())
}

fn answer(state: &mut State, request: &str, base: &str) -> Vec<u8> {
    let line = request.lines().next().unwrap_or("");
    let mut words = line.split(' ');
    let (method, target) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    if !request.contains(&format!("Authorization: Bearer {TOKEN}")) {
        return error("401 Unauthorized", "InvalidAuthenticationToken");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let query = http::pairs(query);
    let param = |name: &str| query.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let segments: Vec<String> = path.trim_start_matches("/v1.0/").split('/').map(http::decode).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let (mailbox, rest) = match segments.as_slice() {
        ["me", rest @ ..] => ("me".to_string(), rest),
        ["users", address, rest @ ..] => (address.to_string(), rest),
        _ => return error("400 Bad Request", "BadRequest"),
    };
    if mailbox != "me" && !state.open.contains(&mailbox) {
        return error("403 Forbidden", "ErrorAccessDenied");
    }
    let main = format!("{mailbox}:{MAIN}");
    let folder_of = |id: &str| if id == MAIN { main.clone() } else { id.to_string() };
    let body = request.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    match (method, rest) {
        ("GET", ["contactFolders"]) if state.fail_listing => error("500 Internal Server Error", "Busy"),
        ("GET", ["contactFolders"]) => {
            let list =
                state.folders.iter().filter(|f| f.0 == mailbox && f.3.is_empty()).map(|f| json!({"id": f.1, "displayName": f.2})).collect::<Vec<_>>();
            json_answer("200 OK", &json!({"value": list}).to_string())
        }
        ("GET", ["contactFolders", parent, "childFolders"]) => {
            let list =
                state.folders.iter().filter(|f| f.0 == mailbox && f.3 == *parent).map(|f| json!({"id": f.1, "displayName": f.2})).collect::<Vec<_>>();
            json_answer("200 OK", &json!({"value": list}).to_string())
        }
        ("GET", ["contacts"]) => {
            let first: Vec<Value> =
                state.contacts.iter().filter(|c| c.folder == main && !c.gone).take(1).map(|_| json!({"parentFolderId": MAIN})).collect();
            json_answer("200 OK", &json!({"value": first}).to_string())
        }
        ("GET", ["contactFolders", folder, "contacts", "delta"]) => {
            let folder = folder_of(folder);
            let from: u64 = param("$deltatoken").and_then(|t| t.parse().ok()).unwrap_or(0);
            if from > 0 && from < state.forgot_before {
                return error("410 Gone", "SyncStateNotFound");
            }
            let skip: usize = param("$skiptoken").and_then(|t| t.parse().ok()).unwrap_or(0);
            let changed: Vec<&Kept> = state.contacts.iter().filter(|c| c.folder == folder && c.changed > from && !(from == 0 && c.gone)).collect();
            let entries: Vec<Value> = changed
                .iter()
                .skip(skip)
                .take(2)
                .map(|c| if c.gone { json!({"id": c.id, "@removed": {"reason": "deleted"}}) } else { c.body.clone() })
                .collect();
            let here = format!("{base}{path}");
            if changed.len() > skip + 2 {
                let token = from.to_string();
                page(
                    entries,
                    ("@odata.nextLink", format!("{here}?{}", http::form(&[("$deltatoken", &token), ("$skiptoken", &(skip + 2).to_string())]))),
                )
            } else {
                page(entries, ("@odata.deltaLink", format!("{here}?{}", http::form(&[("$deltatoken", &state.count.max(1).to_string())]))))
            }
        }
        ("GET", [.., "contacts", id]) => match state.contacts.iter().find(|c| c.id == *id && !c.gone) {
            Some(c) => json_answer("200 OK", &json!({"emailAddresses": c.body["emailAddresses"]}).to_string()),
            None => error("404 Not Found", "ErrorItemNotFound"),
        },
        (_, _) if method != "GET" && state.next_status.is_some() => {
            let status = state.next_status.take().unwrap_or(500);
            let words = match status {
                429 => "429 Too Many Requests\r\nRetry-After: 120",
                403 => "403 Forbidden",
                404 => "404 Not Found",
                409 => "409 Conflict",
                412 => "412 Precondition Failed",
                400 => "400 Bad Request",
                _ => "500 Internal Server Error",
            };
            error(words, "Refused")
        }
        ("POST", [.., "contacts"]) => {
            let folder = match rest {
                ["contacts"] => main.clone(),
                ["contactFolders", folder, "contacts"] => folder_of(folder),
                _ => return error("400 Bad Request", "BadRequest"),
            };
            state.made += 1;
            let id = format!("made-{}=", state.made);
            let mut made: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            made["id"] = json!(id);
            made["changeKey"] = json!("ck-made");
            state.count += 1;
            state.contacts.push(Kept { id, folder, body: made.clone(), changed: state.count, gone: false });
            json_answer("201 Created", &made.to_string())
        }
        ("PATCH", [.., "contacts", id]) => {
            state.count += 1;
            let count = state.count;
            let Some(c) = state.contacts.iter_mut().find(|c| c.id == *id && !c.gone) else { return error("404 Not Found", "ErrorItemNotFound") };
            let patch: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            for (name, value) in patch.as_object().into_iter().flatten() {
                c.body[name] = value.clone();
            }
            c.body["changeKey"] = json!(format!("ck-{count}"));
            c.changed = count;
            json_answer("200 OK", &c.body.to_string())
        }
        ("DELETE", [.., "contacts", id]) => {
            state.count += 1;
            let count = state.count;
            match state.contacts.iter_mut().find(|c| c.id == *id && !c.gone) {
                Some(c) => {
                    (c.gone, c.changed) = (true, count);
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
                }
                None => error("404 Not Found", "ErrorItemNotFound"),
            }
        }
        _ => error("400 Bad Request", "BadRequest"),
    }
}

/// A mailbox with two contacts in its main folder, a folder Team with one, and Team/Old under it.
fn mailbox() -> Arc<Mutex<State>> {
    let mut state = State::default();
    state.add("me", MAIN, "ada=", person("ada=", "Ada", "ada@example.org"));
    state.add("me", MAIN, "bo=", person("bo=", "Bo", "bo@example.org"));
    state.add_folder("me", "team=", "Team", "");
    state.add_folder("me", "old=", "Old", "team=");
    state.add("me", "team=", "cy=", person("cy=", "Cy", "cy@example.org"));
    Arc::new(Mutex::new(state))
}

/// What the mailbox would keep of a read: each folder with its position.
fn kept_from(read: &super::ContactsRead) -> Vec<ContactFolder> {
    read.downloads
        .iter()
        .map(|d| {
            let f = d.folder.clone().unwrap_or_default();
            ContactFolder {
                remote: f.id,
                name: f.name,
                owner: f.owner,
                writable: d.folder.as_ref().is_none_or(|f| f.writable),
                delta: d.delta.clone(),
                ..Default::default()
            }
        })
        .collect()
}

#[test]
fn every_folder_is_read_whole_the_first_time_parents_first() {
    let before = mailcraft_time::now();
    let server = stand_in(mailbox());
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert!(read.errors.is_empty() && !read.token_refused, "{:?}", read.errors);
    assert_eq!(read.downloads.len(), 3);
    // Each says when its reading started.
    assert!(read.downloads.iter().all(|d| d.read_at >= before && d.read_at <= mailcraft_time::now()));
    let main = &read.downloads[0];
    assert!(main.folder.is_none() && main.complete && !main.delta.is_empty());
    let mut names: Vec<&str> = main.contacts.iter().map(|c| c.display_name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Ada", "Bo"]);
    let team = read.downloads[1].folder.as_ref().unwrap();
    assert_eq!((team.id.as_str(), team.name.as_str(), team.parent.as_str(), team.writable), ("team=", "Team", "", true));
    let old = read.downloads[2].folder.as_ref().unwrap();
    assert_eq!((old.id.as_str(), old.parent.as_str()), ("old=", "team="));
    assert!(read.downloads[2].complete && read.downloads[2].contacts.is_empty());
    // Categories the server doesn't name are left as they are here; those it names are read.
    assert_eq!(read.downloads[1].contacts[0].categories, None);
}

#[test]
fn a_later_round_brings_only_what_changed_and_what_went() {
    let state = mailbox();
    let server = stand_in(state.clone());
    let first = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    {
        let mut s = state.lock().unwrap();
        s.count += 1;
        let count = s.count;
        let bo = s.contacts.iter_mut().find(|c| c.id == "bo=").unwrap();
        (bo.body["displayName"], bo.changed) = (json!("Bo Marsh"), count);
        s.count += 1;
        let count = s.count;
        let ada = s.contacts.iter_mut().find(|c| c.id == "ada=").unwrap();
        (ada.gone, ada.changed) = (true, count);
    }
    let second = read_contacts_at(&server.url(""), TOKEN, &kept_from(&first), &[]);
    let main = &second.downloads[0];
    assert!(!main.complete, "a list of changes is not the whole folder");
    assert_eq!(main.contacts.iter().map(|c| c.display_name.as_str()).collect::<Vec<_>>(), ["Bo Marsh"]);
    assert_eq!(main.gone, ["ada="]);
    assert!(!main.delta.is_empty() && main.delta != first.downloads[0].delta);
    // Nothing changed in Team: nothing comes, and it is still not the whole folder.
    assert!(second.downloads[1].contacts.is_empty() && !second.downloads[1].complete);
}

#[test]
fn a_position_the_server_forgot_reads_the_folder_whole_again() {
    let state = mailbox();
    let server = stand_in(state.clone());
    let first = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    {
        let mut s = state.lock().unwrap();
        s.count += 5;
        s.forgot_before = s.count;
    }
    let again = read_contacts_at(&server.url(""), TOKEN, &kept_from(&first), &[]);
    assert!(again.errors.is_empty(), "{:?}", again.errors);
    let main = &again.downloads[0];
    assert!(main.complete && main.contacts.len() == 2 && !main.delta.is_empty());
}

#[test]
fn pages_are_followed_to_the_end_and_a_position_elsewhere_is_never_asked() {
    let state = mailbox();
    for n in 0..5 {
        let id = format!("p{n}=");
        state.lock().unwrap().add("me", "team=", &id, person(&id, &format!("P{n}"), &format!("p{n}@example.org")));
    }
    let server = stand_in(state);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert_eq!(read.downloads[1].contacts.len(), 6);
    assert!(read.downloads[1].complete);
    // A saved position that names another server (a mailbox file says anything) is not followed:
    // the folder is read whole from Graph itself, and nothing goes elsewhere.
    let astray =
        ContactFolder { delta: "https://graph.example.net/v1.0/me/contactFolders/x/contacts/delta?$deltatoken=9".into(), ..Default::default() };
    let read = read_contacts_at(&server.url(""), TOKEN, &[astray], &[]);
    assert!(read.downloads[0].complete);
}

#[test]
fn a_shared_mailboxs_folders_are_read_at_its_address_and_one_not_open_is_passed_over() {
    let state = mailbox();
    {
        let mut s = state.lock().unwrap();
        s.open.push(SALES.into());
        s.add(SALES, MAIN, "dee=", person("dee=", "Dee", "dee@example.org"));
        s.add_folder(SALES, "leads=", "Leads", "");
    }
    let server = stand_in(state);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[SALES.into(), "closed@example.org".into()]);
    assert!(read.errors.is_empty() && !read.token_refused, "{:?}", read.errors);
    let shared: Vec<_> = read.downloads.iter().filter(|d| d.folder.as_ref().is_some_and(|f| !f.owner.is_own())).collect();
    assert_eq!(shared.len(), 2);
    let main = shared[0].folder.as_ref().unwrap();
    assert_eq!((main.id.as_str(), &main.owner, main.writable), ("", &FolderOwner::Mailbox(SALES.into()), true));
    assert_eq!(shared[0].contacts[0].display_name, "Dee");
    // A folder a change was refused in once stays one that can only be read.
    let refused = ContactFolder { remote: "leads=".into(), owner: FolderOwner::Mailbox(SALES.into()), writable: false, ..Default::default() };
    let read = read_contacts_at(&server.url(""), TOKEN, &[refused], &[SALES.into()]);
    let leads = read.downloads.iter().filter_map(|d| d.folder.as_ref()).find(|f| f.id == "leads=").unwrap();
    assert!(!leads.writable);
}

#[test]
fn a_token_that_isnt_taken_stops_the_read() {
    let server = stand_in(mailbox());
    let read = read_contacts_at(&server.url(""), "another.tok-en_0123456789", &[], &[]);
    assert!(read.token_refused && read.downloads.is_empty());
}

fn op(kind: ContactChangeKind, remote: &str, fields: &[ContactField], values: RemoteContact) -> ContactOp {
    ContactOp { kind, contact: 7, remote: remote.into(), fields: fields.iter().copied().collect(), values, ..Default::default() }
}

fn upload(ops: Vec<ContactOp>) -> ContactsUpload {
    ContactsUpload { account: 1, email: "owner@example.org".into(), ops }
}

#[test]
fn a_new_contact_is_made_in_its_folder_and_comes_back_named() {
    let state = mailbox();
    let server = stand_in(state.clone());
    let values = RemoteContact {
        display_name: "Eve Stone".into(),
        first: "Eve".into(),
        emails: vec![Labeled::new("", "eve@example.org")],
        phones: vec![Labeled::new("mobile", "+44 7700 900001"), Labeled::new("work", "+44 20 7946 0001"), Labeled::new("home", "+44 20 7946 0002")],
        websites: vec!["https://eve.example.org".into(), "https://more.example.org".into()],
        birthday: mailcraft_time::Date::new(1990, 4, 2),
        categories: Some(vec!["Friends".into()]),
        ..Default::default()
    };
    let mut new = op(ContactChangeKind::Create, "", &ContactField::ALL, values);
    new.folder = "team=".into();
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![new]));
    let outcome = &done.outcomes[0];
    assert_eq!(
        (outcome.result, outcome.status, outcome.remote.as_str(), outcome.change_key.as_str()),
        (ContactResult::Done, 201, "made-1=", "ck-made")
    );
    let s = state.lock().unwrap();
    let made = s.contacts.iter().find(|c| c.id == "made-1=").unwrap();
    assert_eq!(made.folder, "team=");
    assert_eq!(made.body["emailAddresses"], json!([{"address": "eve@example.org", "name": "Eve Stone"}]));
    assert_eq!(made.body["mobilePhone"], json!("+44 7700 900001"));
    assert_eq!(made.body["businessPhones"], json!(["+44 20 7946 0001"]));
    assert_eq!(made.body["homePhones"], json!(["+44 20 7946 0002"]));
    assert_eq!(made.body["businessHomePage"], json!("https://eve.example.org"));
    assert_eq!(made.body["birthday"], json!("1990-04-02T12:00:00Z"));
    assert_eq!(made.body["categories"], json!(["Friends"]));
}

#[test]
fn a_change_sends_only_its_fields_and_the_addresses_keep_their_names() {
    let state = mailbox();
    state.lock().unwrap().contacts[0].body["imAddresses"] = json!(["ada@im.example.org"]);
    let server = stand_in(state.clone());
    let values = RemoteContact {
        display_name: "Ada L".into(),
        title: "Engineer".into(),
        emails: vec![Labeled::new("", "ADA@example.org"), Labeled::new("", "ada.l@example.org")],
        ..Default::default()
    };
    let change = op(ContactChangeKind::Change, "ada=", &[ContactField::Title, ContactField::Emails], values);
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![change]));
    assert_eq!(done.outcomes[0].result, ContactResult::Done);
    assert!(done.outcomes[0].change_key.starts_with("ck-"));
    let patch = (0..server.asked()).map(|n| server.request(n)).find(|r| r.starts_with("PATCH")).unwrap();
    let body: Value = serde_json::from_str(patch.split_once("\r\n\r\n").unwrap().1).unwrap();
    let mut keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    keys.sort_unstable();
    assert_eq!(keys, ["emailAddresses", "jobTitle"], "the display name wasn't changed here: it isn't sent");
    assert_eq!(body["emailAddresses"][0], json!({"address": "ADA@example.org", "name": "Ada (work)"}));
    assert_eq!(body["emailAddresses"][1]["address"], json!("ada.l@example.org"));
    let s = state.lock().unwrap();
    assert_eq!(s.contacts[0].body["imAddresses"], json!(["ada@im.example.org"]), "what MailCraft doesn't show stays");
    assert_eq!(s.contacts[0].body["displayName"], json!("Ada"));
}

#[test]
fn a_deletion_is_done_and_one_already_gone_is_gone() {
    let server = stand_in(mailbox());
    let ops = vec![
        op(ContactChangeKind::Delete, "bo=", &[], RemoteContact::default()),
        op(ContactChangeKind::Delete, "bo=", &[], RemoteContact::default()),
    ];
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(ops));
    let results: Vec<_> = done.outcomes.iter().map(|o| (o.result, o.status)).collect();
    assert_eq!(results, [(ContactResult::Done, 204), (ContactResult::Gone, 404)]);
}

#[test]
fn every_refusal_maps_as_the_events_do_and_busy_stops_the_rest() {
    for (status, kind, result) in [
        (403, ContactChangeKind::Change, ContactResult::Denied),
        (404, ContactChangeKind::Change, ContactResult::Gone),
        (400, ContactChangeKind::Change, ContactResult::Refused),
        (409, ContactChangeKind::Change, ContactResult::Refused),
        (409, ContactChangeKind::Create, ContactResult::Failed),
        (412, ContactChangeKind::Change, ContactResult::Refused),
        (500, ContactChangeKind::Change, ContactResult::Failed),
        (429, ContactChangeKind::Delete, ContactResult::Failed),
    ] {
        let state = mailbox();
        state.lock().unwrap().next_status = Some(status);
        let server = stand_in(state);
        let first = op(kind, "bo=", &[ContactField::Title], RemoteContact { title: "x".into(), ..Default::default() });
        let second = op(ContactChangeKind::Delete, "ada=", &[], RemoteContact::default());
        let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![first, second]));
        assert_eq!((done.outcomes[0].result, done.outcomes[0].status), (result, status), "{status} {kind:?}");
        let stopped = result == ContactResult::Failed;
        assert_eq!(done.outcomes.len(), if stopped { 1 } else { 2 }, "{status}");
        if status == 429 {
            assert_eq!(done.outcomes[0].retry_after, Some(120));
        }
        assert!(!done.outcomes[0].error.contains(TOKEN));
    }
}

#[test]
fn a_401_stops_the_upload_and_says_the_sign_in_was_refused() {
    let server = stand_in(mailbox());
    let ops = vec![op(ContactChangeKind::Delete, "bo=", &[], RemoteContact::default())];
    let done = push_contacts_at(&server.url(""), "other.tok-en_0123456789", &upload(ops));
    assert!(done.token_refused && done.outcomes.is_empty());
}

#[test]
fn an_op_that_cant_be_one_is_refused_before_anything_goes() {
    let server = stand_in(mailbox());
    let mut astray = op(ContactChangeKind::Delete, "../../x", &[], RemoteContact::default());
    astray.remote = "a b".into();
    let mut nobody = op(ContactChangeKind::Delete, "bo=", &[], RemoteContact::default());
    nobody.owner = FolderOwner::Mailbox("not an address".into());
    let empty = op(ContactChangeKind::Change, "bo=", &[], RemoteContact::default());
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![astray, nobody, empty]));
    assert!(done.outcomes.iter().all(|o| o.result == ContactResult::Refused && o.status == 0));
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_folder_with_more_contacts_than_are_kept_is_read_in_part_with_no_position() {
    let many: Vec<Value> = (0..super::MAX_FOLDER_CONTACTS + 3).map(|n| json!({"id": format!("c{n}"), "displayName": "X"})).collect();
    let body = json!({"value": many, "@odata.deltaLink": "http://127.0.0.1/v1.0/me/contactFolders/f/contacts/delta?$deltatoken=1"}).to_string();
    let server = FakeHttp::start(move |_, request| {
        let asked = request.lines().next().unwrap_or("");
        if asked.contains("/users/") && asked.contains("/contacts/delta") {
            json_answer("200 OK", &body)
        } else {
            json_answer("200 OK", "{\"value\": []}")
        }
    });
    let known = ContactFolder { remote: "f".into(), owner: FolderOwner::Person("pat@example.org".into()), writable: true, ..Default::default() };
    let read = read_contacts_at(&server.url(""), TOKEN, &[known], &[]);
    let part = read.downloads.iter().find(|d| d.folder.as_ref().is_some_and(|f| f.id == "f")).unwrap();
    assert_eq!(part.contacts.len(), super::MAX_FOLDER_CONTACTS);
    assert!(!part.complete && part.delta.is_empty());
}

#[test]
fn hostile_answers_are_errors_never_more() {
    for body in ["[]", "{\"value\": 7}", "not json", "{\"value\": [1, \"x\", null, {\"id\": \"\"}]}"] {
        let server = FakeHttp::answering(json_answer("200 OK", body));
        let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
        assert!(!read.token_refused, "{body}");
        assert!(read.downloads.iter().all(|d| d.contacts.is_empty()), "{body}");
    }
    // A page larger than is read is an error.
    let huge = format!("{{\"value\": [], \"pad\": \"{}\"}}", "x".repeat(super::MAX_PAGE + 10));
    let server = FakeHttp::answering(json_answer("200 OK", &huge));
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert!(read.downloads.is_empty() && !read.errors.is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    #[test]
    fn any_answer_to_a_read_or_a_change_never_panics(body in proptest::collection::vec(any::<u8>(), 0..400), status in 100u16..600) {
        let text = String::from_utf8_lossy(&body).into_owned();
        let reply = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
        let server = FakeHttp::answering(reply.into_bytes());
        let _ = read_contacts_at(&server.url(""), TOKEN, &[], &["sales@example.org".into()]);
        let ops = vec![
            op(ContactChangeKind::Create, "", &ContactField::ALL, RemoteContact::default()),
            op(ContactChangeKind::Change, "bo=", &[ContactField::Emails], RemoteContact::default()),
        ];
        let _ = push_contacts_at(&server.url(""), TOKEN, &upload(ops));
    }
}

#[test]
fn what_goes_up_is_what_graph_has_places_for_in_order() {
    let state = mailbox();
    let server = stand_in(state.clone());
    let values = RemoteContact {
        emails: (1..=4).map(|n| Labeled::new("", format!("bo{n}@example.org"))).collect(),
        phones: (1..=3).map(|n| Labeled::new("mobile", format!("+44 7700 90000{n}"))).collect(),
        websites: vec!["https://one.example.org".into(), "https://two.example.org".into()],
        ..Default::default()
    };
    let change = op(ContactChangeKind::Change, "bo=", &[ContactField::Emails, ContactField::Phones, ContactField::Websites], values);
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![change]));
    assert_eq!(done.outcomes[0].result, ContactResult::Done);
    let s = state.lock().unwrap();
    let bo = &s.contacts.iter().find(|c| c.id == "bo=").unwrap().body;
    let addresses: Vec<&str> = bo["emailAddresses"].as_array().unwrap().iter().map(|e| e["address"].as_str().unwrap()).collect();
    assert_eq!(addresses, ["bo1@example.org", "bo2@example.org", "bo3@example.org"]);
    // The other mobile numbers have no place: they aren't put in as business numbers.
    assert_eq!((&bo["mobilePhone"], &bo["businessPhones"], &bo["homePhones"]), (&json!("+44 7700 900001"), &json!([]), &json!([])));
    assert_eq!(bo["businessHomePage"], json!("https://one.example.org"));
}

#[test]
fn an_entry_that_comes_again_in_a_round_is_as_the_last_says_a_deletion_too() {
    let state = mailbox();
    for n in 0..3 {
        let id = format!("p{n}=");
        state.lock().unwrap().add("me", "team=", &id, person(&id, &format!("P{n}"), &format!("p{n}@example.org")));
    }
    let server = stand_in(state.clone());
    let first = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    // Changed, then deleted, in the same round (on two pages); and another changed twice.
    {
        let mut s = state.lock().unwrap();
        for id in ["p0=", "p1=", "cy="] {
            s.count += 1;
            let count = s.count;
            let c = s.contacts.iter_mut().find(|c| c.id == id).unwrap();
            (c.body["displayName"], c.changed) = (json!(format!("{id} again")), count);
        }
        // The same contact listed again later in the round, deleted.
        let mut copy = s.contacts.iter().find(|c| c.id == "p0=").unwrap().clone();
        s.count += 1;
        (copy.gone, copy.changed) = (true, s.count);
        s.contacts.push(copy);
    }
    let second = read_contacts_at(&server.url(""), TOKEN, &kept_from(&first), &[]);
    let team = &second.downloads[1];
    assert_eq!(team.gone, ["p0="], "{team:?}");
    assert!(team.contacts.iter().all(|c| c.id != "p0="), "{team:?}");
    assert!(!team.delta.is_empty());
}

#[test]
fn a_main_folder_whose_contacts_name_no_folder_is_not_taken_for_an_empty_one() {
    let server = FakeHttp::start(|_, request| {
        let line = request.lines().next().unwrap_or("");
        if line.contains("/me/contacts?") {
            json_answer("200 OK", "{\"value\": [{\"parentFolderId\": \"\"}]}")
        } else {
            json_answer("200 OK", "{\"value\": []}")
        }
    });
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert!(read.downloads.iter().all(|d| d.folder.is_some() || !d.complete), "{:?}", read.downloads);
    assert!(!read.errors.is_empty());
}

#[test]
fn reading_the_address_names_first_keeps_the_wait_asked_and_never_sends_on_a_bad_answer() {
    for (reply, wait) in [
        (b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 120\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), Some(120)),
        (json_answer("200 OK", "{\"emailAddresses\": \"nope\"}"), None),
        (json_answer("200 OK", "not json"), None),
    ] {
        let server = FakeHttp::answering(reply);
        let change = op(
            ContactChangeKind::Change,
            "bo=",
            &[ContactField::Emails],
            RemoteContact { emails: vec![Labeled::new("", "bo@example.org")], ..Default::default() },
        );
        let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![change]));
        assert_eq!(done.outcomes[0].result, ContactResult::Failed);
        assert_eq!(done.outcomes[0].retry_after, wait);
        assert_eq!(server.asked(), 1, "no PATCH after it");
    }
}

#[test]
fn a_list_of_folders_that_fails_still_reads_the_main_folder() {
    let state = mailbox();
    state.lock().unwrap().fail_listing = true;
    let server = stand_in(state);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert!(!read.errors.is_empty());
    assert!(read.downloads.iter().any(|d| d.folder.is_none() && d.contacts.len() == 2 && d.complete), "{:?}", read.downloads);
}

#[test]
fn an_empty_main_folder_is_all_of_it_with_no_position() {
    let state = mailbox();
    state.lock().unwrap().contacts.retain(|c| c.folder != format!("me:{MAIN}"));
    let server = stand_in(state);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    let main = read.downloads.iter().find(|d| d.folder.is_none()).unwrap();
    assert!(main.contacts.is_empty() && main.complete && main.delta.is_empty());
    // A page that isn't a list is no proof of anything.
    for body in [r#"{"value": 7}"#, "[]", "not json"] {
        let server = FakeHttp::start(move |_, request| {
            if request.lines().next().unwrap_or("").contains("/me/contacts?") {
                json_answer("200 OK", body)
            } else {
                json_answer("200 OK", r#"{"value": []}"#)
            }
        });
        let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
        assert!(read.downloads.iter().all(|d| d.folder.is_some()), "{body}");
    }
}

#[test]
fn a_whole_read_that_couldnt_read_an_entry_keeps_no_position() {
    let state = mailbox();
    state.lock().unwrap().add("me", "team=", "odd=", json!({"id": "odd=", "displayName": 7}));
    state.lock().unwrap().contacts.last_mut().unwrap().body = json!("not a contact");
    let server = stand_in(state);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    let team = read.downloads.iter().find(|d| d.folder.as_ref().is_some_and(|f| f.id == "team=")).unwrap();
    assert!(!team.complete && team.delta.is_empty(), "{team:?}");
    assert_eq!(team.contacts.len(), 1);
}

#[test]
fn address_names_that_arent_all_entries_are_an_answer_that_couldnt_be_read() {
    let server = FakeHttp::answering(json_answer("200 OK", "{\"emailAddresses\": [{\"address\": \"bo@example.org\", \"name\": \"Bo\"}, null]}"));
    let change = op(
        ContactChangeKind::Change,
        "bo=",
        &[ContactField::Emails],
        RemoteContact { emails: vec![Labeled::new("", "bo@example.org")], ..Default::default() },
    );
    let done = push_contacts_at(&server.url(""), TOKEN, &upload(vec![change]));
    assert_eq!(done.outcomes[0].result, ContactResult::Failed);
    assert!(!done.outcomes[0].error.is_empty());
    assert_eq!(server.asked(), 1, "no PATCH after it");
}

#[test]
fn a_main_folder_list_that_never_ends_is_not_taken_for_an_empty_folder() {
    let port = Arc::new(std::sync::atomic::AtomicU16::new(0));
    let known = port.clone();
    let server = FakeHttp::start(move |n, request| {
        let line = request.lines().next().unwrap_or("");
        if line.contains("/me/contacts?") {
            let base = format!("http://127.0.0.1:{}", known.load(std::sync::atomic::Ordering::Relaxed));
            let more = format!("{base}/v1.0/me/contacts?$top=1&$skiptoken={n}");
            json_answer("200 OK", &json!({"value": [], "@odata.nextLink": more}).to_string())
        } else {
            json_answer("200 OK", r#"{"value": []}"#)
        }
    });
    port.store(server.port, std::sync::atomic::Ordering::Relaxed);
    let read = read_contacts_at(&server.url(""), TOKEN, &[], &[]);
    assert!(read.downloads.iter().all(|d| d.folder.is_some()), "{:?}", read.downloads);
    assert!(!read.errors.is_empty());
}
