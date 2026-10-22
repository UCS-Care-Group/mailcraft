// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's folders against the strict Graph stand-in, and the reader of a folder
//! against whatever can be thrown at it. Nothing here leaves this machine; every name and every
//! id is invented.

use std::time::{Duration, Instant};

use proptest::prelude::*;

use super::*;
use crate::graph::standin::{StandIn, TOKEN};

/// Quick to ask again, so that the tests don't wait on doubling.
const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };

fn request(changes: Vec<FolderChange>) -> GraphMailRequest {
    GraphMailRequest { account: 3, email: "owner@example.org".into(), changes, ..Default::default() }
}

fn run(server: &StandIn, changes: Vec<FolderChange>) -> GraphMailResult {
    sync_folders_with(&server.url(), TOKEN, &request(changes), Arc::new(Gate::new()), QUICK)
}

/// `Name` and the name of the folder it lies in (empty: the top), for every folder listed.
fn tree(listing: &FolderListing) -> Vec<(String, String)> {
    let name = |id: &str| listing.folders.iter().find(|f| f.id == id).map(|f| f.name.clone()).unwrap_or_default();
    listing.folders.iter().map(|f| (f.name.clone(), name(&f.parent))).collect()
}

#[test]
fn every_folder_is_listed_whole_under_its_parent() {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    let inbox = state.well_known("inbox");
    let sales = state.add_folder("Sales/Marketing", Some(&inbox));
    state.add_folder("A\\B", Some(&sales));
    state.add_folder("Q3 / Q4", None);
    let hidden = state.add_folder("Hidden things", None);
    state.add_folder("Under the hidden one", Some(&hidden));
    let search = state.add_folder("Unread mail", None);
    for folder in state.folders.iter_mut() {
        folder.hidden |= folder.id == hidden;
        folder.search |= folder.id == search;
    }
    // A server that lists hidden folders whatever it is asked, two to a page.
    state.lists_hidden = true;
    state.folder_page = 2;
    drop(state);

    let result = run(&server, vec![]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let listing = result.listing.unwrap();
    assert!(listing.complete);
    let tree = tree(&listing);
    // Names with `/` and `\` are names, under the folder they are in.
    assert!(tree.contains(&("Sales/Marketing".into(), "Inbox".into())), "{tree:?}");
    assert!(tree.contains(&("A\\B".into(), "Sales/Marketing".into())), "{tree:?}");
    assert!(tree.contains(&("Q3 / Q4".into(), String::new())), "{tree:?}");
    // Hidden folders and search folders are not shown, nor what lies under them.
    let names: Vec<&str> = listing.folders.iter().map(|f| f.name.as_str()).collect();
    assert!(!names.contains(&"Hidden things") && !names.contains(&"Under the hidden one") && !names.contains(&"Unread mail"), "{names:?}");
    // The special folders are known for what they are; the top of the mailbox isn't a folder.
    let kind = |name: &str| listing.folders.iter().find(|f| f.name == name).map(|f| f.kind);
    assert_eq!(kind("Inbox"), Some(FolderKind::Inbox));
    assert_eq!(kind("Deleted Items"), Some(FolderKind::Deleted));
    assert_eq!(kind("Junk Email"), Some(FolderKind::Junk));
    assert_eq!(kind("Sales/Marketing"), Some(FolderKind::Custom));
    assert!(!names.contains(&"Top of Information Store"));
    // Parents come before what lies in them.
    let at = |name: &str| names.iter().position(|n| *n == name).unwrap();
    assert!(at("Inbox") < at("Sales/Marketing") && at("Sales/Marketing") < at("A\\B"));

    // Every request asked for immutable ids, those inside a batch too.
    let state = server.state();
    assert!(!state.requests.is_empty());
    for asked in &state.requests {
        assert!(asked.header("prefer").contains("IdType=\"ImmutableId\""), "{} {}", asked.method, asked.target);
    }
    // And the pages were followed.
    assert!(state.requests.iter().any(|asked| asked.target.contains("%24skip=2") || asked.target.contains("$skip=2")));
}

#[test]
fn folders_made_renamed_moved_and_deleted_here_are_so_there() {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    let inbox = state.well_known("inbox");
    let deleted = state.well_known("deleteditems");
    let old = state.add_folder("Old name", Some(&inbox));
    let wanders = state.add_folder("Wanders", None);
    let going = state.add_folder("Going", None);
    let gone_for_good = state.add_folder("Gone for good", Some(&deleted));
    drop(state);

    let changes = vec![
        FolderChange::Create { folder: 40, name: "Receipts/2026".into(), parent: String::new(), parent_folder: None },
        // Under the one just made, which has no server id until it is.
        FolderChange::Create { folder: 41, name: "A\\B".into(), parent: String::new(), parent_folder: Some(40) },
        FolderChange::Create { folder: 42, name: "In the Inbox".into(), parent: inbox.clone(), parent_folder: None },
        FolderChange::Rename { folder: 43, id: old.clone(), name: "New name".into() },
        FolderChange::Move { folder: 44, id: wanders.clone(), parent: inbox.clone() },
        FolderChange::Move { folder: 45, id: going.clone(), parent: deleted.clone() },
        FolderChange::Delete { id: gone_for_good.clone() },
    ];
    let result = run(&server, changes);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.changes.len(), 7);
    assert!(result.changes.iter().all(|done| matches!(done.outcome, ChangeOutcome::Done { .. })), "{:?}", result.changes);

    let state = server.state();
    let receipts = state.folder_named("Receipts/2026").unwrap();
    assert_eq!(receipts.parent, state.well_known("msgfolderroot"));
    assert_eq!(state.folder_named("A\\B").unwrap().parent, receipts.id);
    assert_eq!(state.folder_named("In the Inbox").unwrap().parent, inbox);
    assert_eq!((state.folder_named("New name").unwrap().id.as_str(), state.folder_named("Old name")), (old.as_str(), None));
    assert_eq!(state.folder_named("Wanders").unwrap().parent, inbox);
    assert_eq!(state.folder_named("Going").unwrap().parent, deleted);
    assert!(state.folder_named("Gone for good").is_none());
    // What was made comes back with its server id, and where it is (the top: empty).
    let made = |n: usize| match &result.changes[n].outcome {
        ChangeOutcome::Done { folder } => folder.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!((made(0).id.as_str(), made(0).parent.as_str()), (receipts.id.as_str(), ""));
    assert_eq!(made(1).parent, receipts.id);
    assert_eq!((made(3).name.as_str(), made(3).parent.as_str()), ("New name", inbox.as_str()));
    // The listing after it shows the server as it is now.
    let listing = result.listing.unwrap();
    assert!(listing.complete && listing.folders.iter().any(|f| f.name == "New name") && !listing.folders.iter().any(|f| f.name == "Old name"));
    // A move or a folder made is a POST, a rename a PATCH, a delete a DELETE.
    let methods: Vec<&str> = state.requests.iter().filter(|a| a.method != "GET" && !a.target.contains("$batch")).map(|a| a.method.as_str()).collect();
    assert_eq!(methods, ["POST", "POST", "POST", "PATCH", "POST", "POST", "DELETE"]);
}

#[test]
fn a_folder_made_before_whose_answer_was_lost_is_found_not_made_twice() {
    let server = StandIn::start().unwrap();
    let there = server.state().add_folder("Receipts", None);
    let result = run(&server, vec![FolderChange::Create { folder: 40, name: "receipts".into(), parent: String::new(), parent_folder: None }]);
    match &result.changes[0].outcome {
        ChangeOutcome::Done { folder } => assert_eq!((folder.id.as_str(), folder.name.as_str()), (there.as_str(), "Receipts")),
        other => panic!("{other:?}"),
    }
    assert_eq!(server.state().folders.iter().filter(|f| f.name.eq_ignore_ascii_case("receipts")).count(), 1);
}

#[test]
fn what_the_server_refuses_or_no_longer_has_is_said() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ghost = "AQMkAGV4YW1wbGUtb3JnLW1haWxib3gtZm9sZGVycwAuAAADgone-folder-0000AAAA=".to_string();
    let result = run(
        &server,
        vec![
            // The special folders keep their names.
            FolderChange::Rename { folder: 41, id: inbox, name: "Mail".into() },
            FolderChange::Rename { folder: 42, id: ghost.clone(), name: "Anything".into() },
            FolderChange::Delete { id: ghost },
        ],
    );
    assert!(matches!(&result.changes[0].outcome, ChangeOutcome::Refused { why } if why.contains("may not")), "{:?}", result.changes);
    assert_eq!((&result.changes[1].outcome, &result.changes[2].outcome), (&ChangeOutcome::Gone, &ChangeOutcome::Gone));
    assert!(result.errors.is_empty() && result.listing.is_some_and(|l| l.complete));
}

#[test]
fn a_busy_server_pauses_the_mailbox_and_the_run_goes_on_after() {
    let server = StandIn::start().unwrap();
    server.state().add_folder("Projects", None);
    {
        let mut state = server.state();
        // The list of folders is told twice to come back in a second.
        for _ in 0..2 {
            state.fail_next.push(("GET /v1.0/me/mailFolders".into(), 429, "TooManyRequests".into()));
        }
        state.retry_after = Some(1);
    }
    let gate = Arc::new(Gate::new());
    let started = Instant::now();
    let result = sync_folders_with(&server.url(), TOKEN, &request(vec![]), gate.clone(), QUICK);
    // A second each time, as the server said (not the few milliseconds of `QUICK`).
    assert!(started.elapsed() >= Duration::from_millis(1_950), "waited {:?}", started.elapsed());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.busy_for, None);
    assert!(result.listing.unwrap().folders.iter().any(|f| f.name == "Projects"));
    // The server was asked again after it said so, and got through.
    let statuses: Vec<u16> = server.state().requests.iter().map(|a| a.status).collect();
    assert!(statuses.contains(&429) && statuses.last() == Some(&200), "{statuses:?}");
}

#[test]
fn a_server_busy_for_longer_than_a_run_waits_ends_the_run_and_shuts_the_gate() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.throttle_folders = 100;
        state.retry_after = Some(600);
    }
    let gate = Arc::new(Gate::new());
    let result = sync_folders_with(&server.url(), TOKEN, &request(vec![]), gate.clone(), QUICK);
    assert!(result.listing.is_none() && !result.errors.is_empty());
    assert!(result.busy_for.is_some_and(|seconds| seconds >= 590), "{:?}", result.busy_for);
    // Nobody asks the mailbox anything meanwhile: the next run is told at once.
    assert!(gate.paused_for().is_some());
    let asked = server.state().requests.len();
    let again = sync_folders_with(&server.url(), TOKEN, &request(vec![]), gate, QUICK);
    assert!(again.busy_for.is_some() && server.state().requests.len() == asked);
}

#[test]
fn a_folder_is_never_made_twice_by_asking_again() {
    let server = StandIn::start().unwrap();
    server.state().retry_after = Some(0);
    // The batch of reads is a POST too: told "later", it is asked again.
    server.state().fail_next.push(("POST /v1.0/$batch".into(), 429, "TooManyRequests".into()));
    // The POST that makes the folder is told "later": it is not asked again by itself.
    server.state().fail_next.push(("POST /v1.0/me/mailFolders".into(), 429, "TooManyRequests".into()));
    let once = || vec![FolderChange::Create { folder: 40, name: "Once".into(), parent: String::new(), parent_folder: None }];
    let result = run(&server, once());
    assert!(result.changes.is_empty() && !result.errors.is_empty() && result.busy_for.is_some(), "{result:?}");
    let posts = |server: &StandIn| server.state().requests.iter().filter(|a| a.method == "POST" && a.target.ends_with("mailFolders")).count();
    let batches = server.state().requests.iter().filter(|a| a.target.ends_with("$batch")).count();
    assert_eq!((posts(&server), batches), (1, 2));
    assert!(server.state().folder_named("Once").is_none());
    // The next run makes it, once.
    let result = run(&server, once());
    assert!(matches!(result.changes[0].outcome, ChangeOutcome::Done { .. }));
    assert_eq!((posts(&server), server.state().folders.iter().filter(|f| f.name == "Once").count()), (2, 1));
}

#[test]
fn a_listing_cut_short_says_so() {
    let server = StandIn::start().unwrap();
    let mut state = server.state();
    let inbox = state.well_known("inbox");
    let projects = state.add_folder("Projects", Some(&inbox));
    state.add_folder("Apollo", Some(&projects));
    state.add_folder("Receipts", None);
    state.broken_lists.push(projects);
    drop(state);
    let result = run(&server, vec![]);
    let listing = result.listing.unwrap();
    assert!(!listing.complete);
    let names: Vec<&str> = listing.folders.iter().map(|f| f.name.as_str()).collect();
    // Everything else is there; what was under the folder that couldn't be read is not.
    assert!(names.contains(&"Projects") && names.contains(&"Receipts") && !names.contains(&"Apollo"), "{names:?}");
}

/// How many requests the folders' part of a run asked.
fn folder_requests(result: &GraphMailResult) -> u32 {
    result.timings.phases.iter().filter(|p| p.phase == SyncPhase::Folders).map(|p| p.requests).sum()
}

/// Forty folders at the top with two subfolders each, the first of them with one of its own,
/// and the first folder with 148 more (two pages): `(top folders, all made)`.
fn many_folders(server: &StandIn) -> (Vec<String>, usize) {
    let mut state = server.state();
    let mut tops = Vec::new();
    let mut made = 0;
    for n in 0..40 {
        let top = state.add_folder(&format!("Project {n}"), None);
        let first = state.add_folder(&format!("Project {n} notes"), Some(&top));
        state.add_folder(&format!("Project {n} drafts"), Some(&top));
        state.add_folder(&format!("Project {n} old notes"), Some(&first));
        made += 4;
        tops.push(top);
    }
    for m in 0..148 {
        state.add_folder(&format!("Receipt {m}"), Some(&tops[0]));
        made += 1;
    }
    (tops, made)
}

#[test]
fn a_level_of_the_tree_is_listed_twenty_folders_to_a_batch() {
    let server = StandIn::start().unwrap();
    let (_, made) = many_folders(&server);
    let result = run(&server, vec![]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let listing = result.listing.as_ref().unwrap();
    assert!(listing.complete);
    let tree = tree(listing);
    assert!(tree.contains(&("Project 39 old notes".into(), "Project 39 notes".into())), "{tree:?}");
    assert!(tree.contains(&("Receipt 147".into(), "Project 0".into())), "{tree:?}");
    let ours = tree.iter().filter(|(name, _)| name.starts_with("Project") || name.starts_with("Receipt")).count();
    assert_eq!(ours, made);
    // The special folders (one batch), the top (one page), the forty under it (two batches,
    // and the second page of the first one's 150), the forty under those (two batches): seven,
    // where a request for every folder with subfolders was 83.
    assert_eq!(folder_requests(&result), 7);
    // Every one of them asked for immutable ids, those inside a batch too.
    for asked in &server.state().requests {
        assert!(asked.header("prefer").contains("IdType=\"ImmutableId\""), "{} {}", asked.method, asked.target);
    }
}

#[test]
fn a_list_that_fails_inside_a_batch_leaves_the_listing_incomplete() {
    let server = StandIn::start().unwrap();
    let (tops, _) = many_folders(&server);
    server.state().broken_lists.push(tops[25].clone());
    let result = run(&server, vec![]);
    let listing = result.listing.unwrap();
    // Never a tree that looks whole without what lies under the folder that couldn't be read
    // (#305): nothing is removed for being missing from it.
    assert!(!listing.complete);
    let names: Vec<&str> = listing.folders.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"Project 25") && !names.contains(&"Project 25 notes"), "{names:?}");
    // Its neighbours in the same batch, and what lies under them, are there.
    assert!(names.contains(&"Project 24 notes") && names.contains(&"Project 26 old notes"), "{names:?}");
}

#[test]
fn a_server_that_wont_take_a_batch_is_asked_one_folder_at_a_time() {
    let server = StandIn::start().unwrap();
    let (_, made) = many_folders(&server);
    server.state().refuse_batches = true;
    let result = run(&server, vec![]);
    let listing = result.listing.unwrap();
    assert!(listing.complete);
    let ours = listing.folders.iter().filter(|f| f.name.starts_with("Project") || f.name.starts_with("Receipt")).count();
    assert_eq!(ours, made);
}

#[test]
fn a_sign_in_that_isnt_taken_lists_nothing() {
    let server = StandIn::start().unwrap();
    let result = sync_folders_with(&server.url(), "another.token-0000", &request(vec![]), Arc::new(Gate::new()), QUICK);
    assert!(result.listing.is_none() && !result.errors.is_empty());
    assert!(!result.errors.iter().any(|e| e.contains("another.token")));
}

fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        "\\PC{0,24}".prop_map(Value::from),
        "(AQMk[A-Za-z0-9_=-]{0,40}|#microsoft.graph.mailSearchFolder|Inbox|A/B|\\\\|\u{0}|)".prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::from),
            proptest::collection::vec(("(id|displayName|parentFolderId|childFolderCount|isHidden|@odata.type|\\PC{0,6})", inner), 0..8)
                .prop_map(|fields| Value::Object(fields.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn any_answer_is_a_folder_or_nothing(value in any_json(), bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        if let Some(entry) = entry_of(&value) {
            prop_assert!(!entry.id.is_empty() && entry.id.len() <= MAX_ID && entry.id.bytes().all(|b| b.is_ascii_graphic()));
            prop_assert!(!entry.name.is_empty() && entry.name.chars().count() <= MAX_NAME && !entry.name.chars().any(char::is_control));
        }
        // A page of anything at all: a list of entries, or an error, never a panic.
        if let Ok(page) = crate::graph::page_of(&bytes) {
            for value in &page.entries {
                let _ = entry_of(value);
            }
        }
        let page = serde_json::to_vec(&json!({"value": [value]})).unwrap_or_default();
        prop_assert!(crate::graph::page_of(&page).is_ok());
    }
}
