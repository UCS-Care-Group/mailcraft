use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{FolderOp, FolderState, RemoteId, RemoteOp, SyncRequest, SyncResult, Upload};
use mailcraft_model::{Account, AccountKind, FolderKind, MailSource, Message};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::standin::{MailStandIn, TOKEN};
use crate::sync::{Host, Limits, sync_as, sync_within};

const DAY: i64 = 86_400;
const T0: i64 = 1_790_000_000;

fn account() -> Account {
    Account {
        id: 1,
        name: "owner@example.org".into(),
        email: "owner@example.org".into(),
        kind: AccountKind::Imap,
        mail_source: MailSource::Graph,
        ..Default::default()
    }
}

/// What the engine keeps of the server between runs, as far as the tests need it.
#[derive(Default)]
struct Mirror {
    folders: BTreeMap<String, FolderState>,
    next_id: u64,
}

impl Mirror {
    fn request(&self) -> SyncRequest {
        SyncRequest { account: account(), access_token: TOKEN.into(), folders: self.folders.values().cloned().collect(), ..Default::default() }
    }

    fn take(&mut self, result: &SyncResult) {
        for delta in &result.folders {
            let next = self.next_id;
            let state = self.folders.entry(delta.remote.clone()).or_insert_with(|| FolderState {
                id: next + 100,
                remote: delta.remote.clone(),
                kind: delta.kind,
                ..Default::default()
            });
            self.next_id += 1;
            state.uid_validity = delta.uid_validity;
            state.uid_seen = state.uid_seen.max(delta.uid_seen);
            state.known_uids.retain(|uid| !delta.removed.contains(uid));
            state.remote_ids.retain(|r| !delta.removed.contains(&r.uid));
            for m in &delta.new {
                state.known_uids.push(m.uid);
                state.remote_ids.push(RemoteId { uid: m.uid, id: m.remote_id.clone() });
            }
            if !delta.delta.is_empty() {
                state.delta = delta.delta.clone();
            }
            state.mark = delta.mark.clone();
        }
    }

    fn uid_of(&self, remote: &str, id: &str) -> u32 {
        self.folders.get(remote).and_then(|f| f.remote_ids.iter().find(|r| r.id == id)).map_or(0, |r| r.uid)
    }
}

fn run(stand_in: &MailStandIn, request: &SyncRequest) -> SyncResult {
    let _routed = stand_in.route();
    sync_within(request, Limits::in_pieces())
}

fn subjects(result: &SyncResult, remote: &str) -> Vec<String> {
    let mut out: Vec<String> = result.folders.iter().filter(|f| f.remote == remote).flat_map(|f| f.new.iter().map(|m| m.subject.clone())).collect();
    out.sort();
    out
}

#[test]
fn a_first_run_lists_the_folders_and_brings_every_message() {
    let stand_in = MailStandIn::start().unwrap();
    let (inbox, projects) = {
        let mut s = stand_in.state();
        let inbox = s.folder("inbox");
        let projects = s.add_folder("Projects", Some(&inbox));
        let receipts = s.add_folder("Receipts/2026", None);
        for (n, subject) in ["Older", "Middle", "Newest"].iter().enumerate() {
            s.add_message(&inbox, subject, T0 + i64::try_from(n).unwrap() * DAY);
        }
        s.add_message(&projects, "Plan", T0);
        s.add_message(&receipts, "Receipt", T0);
        (inbox, projects)
    };
    let result = run(&stand_in, &Mirror::default().request());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let listed: Vec<(&str, &str, FolderKind)> = result.listed.iter().map(|f| (f.remote.as_str(), f.parent.as_str(), f.kind)).collect();
    assert!(listed.contains(&("INBOX", "", FolderKind::Inbox)), "{listed:?}");
    assert!(listed.contains(&("INBOX/Projects", "INBOX", FolderKind::Custom)), "{listed:?}");
    assert!(listed.contains(&("Sent Items", "", FolderKind::Sent)), "{listed:?}");
    // A `/` in a name is no level.
    assert!(listed.contains(&("Receipts_2026", "", FolderKind::Custom)), "{listed:?}");
    assert!(result.listed_whole);
    assert_eq!(subjects(&result, "INBOX"), ["Middle", "Newest", "Older"]);
    assert_eq!(subjects(&result, "INBOX/Projects"), ["Plan"]);
    let inbox_delta: Vec<_> = result.folders.iter().filter(|f| f.remote == "INBOX").collect();
    let new: Vec<&Message> = inbox_delta.iter().flat_map(|f| &f.new).collect();
    // Numbered from 1, the newest highest; each with Graph's id; the folder's next round kept.
    let mut by_uid: Vec<(u32, &str)> = new.iter().map(|m| (m.uid, m.subject.as_str())).collect();
    by_uid.sort();
    assert_eq!(by_uid, [(1, "Older"), (2, "Middle"), (3, "Newest")]);
    assert!(new.iter().all(|m| m.remote_id.starts_with("AAMkAG") && m.received >= T0));
    assert!(inbox_delta.iter().all(|f| f.uid_validity == 1));
    assert!(inbox_delta.iter().any(|f| f.delta.contains("$deltatoken=")));
    let s = stand_in.state();
    // Every request about mail asked for ids that a move doesn't change; ids were written as one
    // piece of the address each (they hold `/` and `=`).
    assert!(
        s.requests.iter().filter(|r| !r.target.contains("sendMail")).all(|r| r.status != 400),
        "{:?}",
        s.requests.iter().filter(|r| r.status == 400).collect::<Vec<_>>()
    );
    assert!(s.requests.iter().any(|r| r.batched && r.target.ends_with("/$value")));
    let _ = (inbox, projects);
}

#[test]
fn the_next_run_brings_only_what_changed_came_and_went() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    let (a, b, _c) = {
        let mut s = stand_in.state();
        (s.add_message(&inbox, "A", T0), s.add_message(&inbox, "B", T0 + 1), s.add_message(&inbox, "C", T0 + 2))
    };
    let mut mirror = Mirror::default();
    mirror.take(&run(&stand_in, &mirror.request()));
    {
        let mut s = stand_in.state();
        s.edit(&a, |m| {
            m.read = true;
            m.categories = vec!["Red category".into()];
        });
        s.remove(&b);
        s.add_message(&inbox, "D", T0 + 3);
        s.requests.clear();
    }
    let result = run(&stand_in, &mirror.request());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let inbox_deltas: Vec<_> = result.folders.iter().filter(|f| f.remote == "INBOX").collect();
    let ua = mirror.uid_of("INBOX", &a);
    let ub = mirror.uid_of("INBOX", &b);
    let flags: Vec<_> = inbox_deltas.iter().flat_map(|f| &f.flags).collect();
    assert!(flags.iter().any(|f| f.uid == ua && f.read), "{flags:?}");
    let categories: Vec<_> = inbox_deltas.iter().flat_map(|f| &f.categories).collect();
    assert!(categories.iter().any(|c| c.uid == ua && c.names == ["Red category"]));
    assert!(inbox_deltas.iter().any(|f| f.removed == [ub]));
    assert_eq!(subjects(&result, "INBOX"), ["D"]);
    assert_eq!(inbox_deltas.iter().flat_map(|f| &f.new).map(|m| m.uid).collect::<Vec<_>>(), [4]);
    // From where the last round ended.
    assert!(stand_in.state().requests.iter().any(|r| r.target.contains("$deltatoken=")));
}

#[test]
fn a_folder_cut_short_by_a_limit_keeps_its_old_round() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    for n in 0..5 {
        stand_in.state().add_message(&inbox, &format!("M{n}"), T0 + n);
    }
    let mut mirror = Mirror::default();
    let mut request = mirror.request();
    request.max_per_folder = 2;
    let first = run(&stand_in, &request);
    // The newest win, and the round isn't taken for done.
    assert_eq!(subjects(&first, "INBOX"), ["M3", "M4"]);
    assert!(first.folders.iter().filter(|f| f.remote == "INBOX").all(|f| f.delta.is_empty()));
    mirror.take(&first);
    let mut request = mirror.request();
    request.max_per_folder = 0;
    let second = run(&stand_in, &request);
    assert_eq!(subjects(&second, "INBOX"), ["M0", "M1", "M2"]);
    assert!(second.folders.iter().any(|f| f.remote == "INBOX" && !f.delta.is_empty()));
    let uids: Vec<u32> = second.folders.iter().flat_map(|f| &f.new).map(|m| m.uid).collect();
    assert!(uids.iter().all(|uid| *uid > 2), "{uids:?}");
}

#[test]
fn a_round_graph_forgot_starts_again_and_finds_what_went() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    let (a, _b) = {
        let mut s = stand_in.state();
        (s.add_message(&inbox, "A", T0), s.add_message(&inbox, "B", T0 + 1))
    };
    let mut mirror = Mirror::default();
    mirror.take(&run(&stand_in, &mirror.request()));
    let ua = mirror.uid_of("INBOX", &a);
    {
        let mut s = stand_in.state();
        s.remove(&a);
        s.forget_rounds();
    }
    let result = run(&stand_in, &mirror.request());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.folders.iter().any(|f| f.remote == "INBOX" && f.removed == [ua]));
    assert!(subjects(&result, "INBOX").is_empty());
    assert!(stand_in.state().requests.iter().any(|r| r.status == 410));
}

#[test]
fn changes_made_here_reach_the_server() {
    let stand_in = MailStandIn::start().unwrap();
    let (inbox, archive) = {
        let s = stand_in.state();
        (s.folder("inbox"), s.folder("archive"))
    };
    let (a, b, c, d) = {
        let mut s = stand_in.state();
        let a = s.add_message(&inbox, "A", T0);
        let b = s.add_message(&inbox, "B", T0 + 1);
        let c = s.add_message(&inbox, "C", T0 + 2);
        let d = s.add_message(&inbox, "D", T0 + 3);
        s.edit(&b, |m| m.categories = vec!["Blue".into(), "Green".into()]);
        (a, b, c, d)
    };
    let mut mirror = Mirror::default();
    mirror.take(&run(&stand_in, &mirror.request()));
    let uid = |id: &str| mirror.uid_of("INBOX", id);
    let mut request = mirror.request();
    request.ops = vec![
        RemoteOp::Flags { folder: "INBOX".into(), uid: uid(&a), read: Some(true), flagged: Some(true), answered: Some(true) },
        RemoteOp::Categories { folder: "INBOX".into(), uid: uid(&b), add: vec!["Red".into()], remove: vec!["blue".into()] },
        RemoteOp::Move { message: 50, from: "INBOX".into(), uid: uid(&c), to: "Archive".into(), to_folder: 60, create: vec!["2026".into()] },
        RemoteOp::Delete { from: "INBOX".into(), uid: uid(&d) },
        // A message nobody knows: nothing to do, and done.
        RemoteOp::Flags { folder: "INBOX".into(), uid: 999, read: Some(false), flagged: None, answered: None },
    ];
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.flagged.len(), 2);
    assert_eq!(result.categorized.len(), 1);
    assert_eq!(result.moved.len(), 1);
    assert_eq!((result.moved[0].new_uid, result.moved[0].uid_validity), (0, 0));
    assert_eq!(result.deleted.len(), 1);
    assert_eq!(result.created.len(), 1);
    assert_eq!((result.created[0].folder, result.created[0].remote.as_str()), (60, "Archive/2026"));
    let s = stand_in.state();
    let a = s.message(&a).unwrap();
    assert!(a.read && a.flagged);
    assert_eq!(s.message(&b).unwrap().categories, ["Green", "Red"]);
    let made = s.folders.iter().find(|f| f.name == "2026" && f.parent == archive).unwrap().id.clone();
    // The same id after the move: it was asked for that way.
    assert_eq!(s.message(&c).unwrap().folder, made);
    assert!(s.message(&d).is_none());
}

#[test]
fn folders_made_renamed_and_deleted_here_change_on_the_server() {
    let stand_in = MailStandIn::start().unwrap();
    let (old, inside_deleted) = {
        let mut s = stand_in.state();
        let old = s.add_folder("Old", None);
        let deleted = s.folder("deleteditems");
        let inside = s.add_folder("Gone", Some(&deleted));
        (old, inside)
    };
    let mut request = Mirror::default().request();
    request.folder_ops = vec![
        FolderOp::Create { folder: 70, parent: "INBOX".into(), name: "Clients".into() },
        FolderOp::Rename { folder: 71, from: "Old".into(), parent: Some("Deleted Items".into()), name: "Old".into() },
        FolderOp::Delete { remote: "Deleted Items/Gone".into() },
        FolderOp::Delete { remote: "Sent Items".into() },
    ];
    let result = run(&stand_in, &request);
    let pushed: Vec<(u64, &str, &str, bool)> = result.folders_pushed.iter().map(|f| (f.folder, f.from.as_str(), f.to.as_str(), f.refused)).collect();
    assert_eq!(
        pushed,
        [
            (70, "", "INBOX/Clients", false),
            (71, "Old", "Deleted Items/Old", false),
            (0, "Deleted Items/Gone", "", false),
            (0, "Sent Items", "", true)
        ]
    );
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.listed.iter().any(|f| f.remote == "INBOX/Clients"));
    assert!(result.listed.iter().any(|f| f.remote == "Deleted Items/Old"));
    let s = stand_in.state();
    assert_eq!(s.folders.iter().find(|f| f.id == old).unwrap().parent, s.folder("deleteditems"));
    assert!(!s.folders.iter().any(|f| f.id == inside_deleted));
}

#[test]
fn a_draft_goes_up_once_and_the_outbox_is_sent() {
    let stand_in = MailStandIn::start().unwrap();
    let mut mirror = Mirror::default();
    mirror.take(&run(&stand_in, &mirror.request()));
    let draft = Message {
        id: 80,
        folder: 81,
        message_id: "draft-1@example.org".into(),
        subject: "Half written".into(),
        from: mailcraft_model::Address { name: "Owner".into(), email: "owner@example.org".into() },
        date: T0,
        ..Default::default()
    };
    let going = Message {
        id: 90,
        message_id: "going-1@example.org".into(),
        subject: "Off it goes".into(),
        from: mailcraft_model::Address { name: "Owner".into(), email: "owner@example.org".into() },
        to: vec![mailcraft_model::Address { name: "Ada".into(), email: "ada@example.org".into() }],
        date: T0,
        ..Default::default()
    };
    let mut request = mirror.request();
    request.uploads = vec![Upload { message: draft.clone(), to: "Drafts".into(), copy_from: String::new(), copy_uid: 0, copy_only: false }];
    request.outbox = vec![going];
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!((result.sent.as_slice(), result.filed.as_slice()), ([90].as_slice(), [90].as_slice()));
    assert_eq!(result.uploaded.len(), 1);
    assert_eq!((result.uploaded[0].message, result.uploaded[0].uid, result.uploaded[0].flags.draft), (80, 0, true));
    let drafts = stand_in.state().folder("drafts");
    assert_eq!(stand_in.state().subjects(&drafts), ["Half written"]);
    assert_eq!(stand_in.state().sent.len(), 1);
    // Asked again (an answer that was lost): found there, and not put there twice.
    let mut again = mirror.request();
    again.uploads = vec![Upload { message: draft, to: "Drafts".into(), copy_from: String::new(), copy_uid: 0, copy_only: false }];
    let result = run(&stand_in, &again);
    assert_eq!(result.uploaded.len(), 1);
    assert_eq!(stand_in.state().subjects(&drafts), ["Half written"]);
}

#[test]
fn a_refused_sign_in_ends_the_run_with_a_line_that_says_so() {
    let stand_in = MailStandIn::start().unwrap();
    let mut request = Mirror::default().request();
    request.access_token = "another.token".into();
    let result = run(&stand_in, &request);
    assert!(result.folders.is_empty() && result.listed.is_empty());
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].contains("didn't take the sign-in"), "{:?}", result.errors);
    assert!(!result.errors[0].contains("another.token"));
}

#[test]
fn not_now_is_waited_for_and_asked_again() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    stand_in.state().add_message(&inbox, "A", T0);
    {
        let mut s = stand_in.state();
        s.throttle = 1;
        s.throttle_batch = 1;
    }
    let result = run(&stand_in, &Mirror::default().request());
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(subjects(&result, "INBOX"), ["A"]);
    assert!(stand_in.state().requests.iter().filter(|r| r.status == 429).count() >= 2);
}

#[test]
fn a_next_page_on_another_server_is_not_followed() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    for n in 0..30 {
        stand_in.state().add_message(&inbox, &format!("M{n}"), T0 + n);
    }
    stand_in.state().page_size = 5;
    stand_in.state().astray = true;
    let result = run(&stand_in, &Mirror::default().request());
    assert!(result.errors.iter().any(|e| e.contains("can't be followed")), "{:?}", result.errors);
    assert!(result.folders.iter().all(|f| f.delta.is_empty() && f.new.is_empty()));
}

#[test]
fn a_round_that_is_not_json_is_an_error_and_nothing_more() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    stand_in.state().add_message(&inbox, "A", T0);
    stand_in.state().junk = true;
    let result = run(&stand_in, &Mirror::default().request());
    assert!(result.errors.iter().any(|e| e.contains("isn't the JSON")), "{:?}", result.errors);
    assert!(result.folders.iter().all(|f| f.new.is_empty() && f.removed.is_empty()));
}

#[test]
fn a_large_folder_comes_in_pieces_that_keep_the_count_below_what_is_still_to_come() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    for n in 0..45 {
        stand_in.state().add_message(&inbox, &format!("M{n:02}"), T0 + n);
    }
    let request = Mirror::default().request();
    let mut pieces = Vec::new();
    {
        let _routed = stand_in.route();
        let limits = Limits { batch_messages: 10, ..Limits::in_pieces() };
        sync_as(&request, limits, Host::default(), &AtomicBool::new(false), &mut |piece| pieces.push(piece));
    }
    let inbox_pieces: Vec<_> = pieces.iter().flat_map(|p| &p.folders).filter(|f| f.remote == "INBOX").collect();
    assert!(inbox_pieces.len() > 2);
    let (last, before) = inbox_pieces.split_last().unwrap();
    assert!(before.iter().all(|f| f.uid_seen == 0 && f.delta.is_empty()));
    assert_eq!(last.uid_seen, 45);
    assert!(!last.delta.is_empty());
    let total: usize = inbox_pieces.iter().map(|f| f.new.len()).sum();
    assert_eq!(total, 45);
    // The newest first.
    assert_eq!(inbox_pieces[0].new.iter().map(|m| m.uid).max(), Some(45));
}

#[test]
fn hostile_json_answers_never_panic() {
    for body in [
        json!({"value": [{"id": 5}, {"id": ""}, {"id": "x".repeat(10_000)}, {"id": "ok", "isRead": "yes", "flag": 3, "categories": [1, null, "a".repeat(500), "", "Ok"]}]}),
        json!({"value": "nope"}),
        json!([1, 2, 3]),
        json!(null),
    ] {
        if let Some(Value::Array(list)) = body.get("value") {
            for entry in list {
                let _ = super::seen(entry);
            }
        }
        let _ = super::object(body.to_string().as_bytes());
    }
    assert_eq!(super::categories_in(Some(&json!(["Red", "red", " Blue ", "", "x\u{7}y"]))), ["Red", "Blue", "xy"]);
}

proptest! {
    #[test]
    fn arbitrary_bytes_as_answers_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(value) = super::object(&bytes) {
            let _ = super::seen(&value);
            let _ = super::categories_in(value.get("categories"));
        }
    }

    #[test]
    fn arbitrary_entries_never_panic(id in ".{0,600}", read in any::<bool>(), names in proptest::collection::vec(".{0,80}", 0..40), received in ".{0,40}") {
        let entry = json!({"id": id, "isRead": read, "categories": names, "receivedDateTime": received, "flag": {"flagStatus": "flagged"}});
        let _ = super::seen(&entry);
        let names = super::categories_in(entry.get("categories"));
        prop_assert!(names.len() <= mailcraft_model::sync::MAX_MESSAGE_KEYWORDS);
    }
}

#[test]
fn a_connection_that_failed_is_tried_again_and_a_change_that_may_have_gone_is_not() {
    use crate::NetError;
    use mailcraft_proto::ProtoError;
    let tls = NetError::Tls { host: "graph.microsoft.com".into(), reason: "the server took too long".into() };
    let stalled = NetError::Proto(ProtoError::Io("the server took too long to answer".into()));
    // Nothing went to Graph: any request is asked again.
    assert!(super::passing(&tls, false) && super::passing(&tls, true));
    // It stalled after going out: only one that reads.
    assert!(super::passing(&stalled, true) && !super::passing(&stalled, false));
    // A refused sign-in is no trouble of the connection.
    assert!(!super::passing(&NetError::Proto(ProtoError::Auth("no".into())), true));
}

#[test]
fn new_mail_in_the_inbox_comes_down_while_a_long_run_is_busy_with_other_folders() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = {
        let mut s = stand_in.state();
        let inbox = s.folder("inbox");
        s.add_message(&inbox, "Before the run", T0);
        let archive = s.add_folder("Old mail", None);
        for n in 0..6 {
            s.add_message(&archive, &format!("Old {n}"), T0 - (n + 1) * DAY);
        }
        inbox
    };
    // One message at a time, and the Inbox is due another look whenever it is asked.
    let limits = Limits { fetch_messages: 1, batch_messages: 1, inbox_every_graph: std::time::Duration::ZERO, ..Limits::in_pieces() };
    let mut pieces: Vec<SyncResult> = Vec::new();
    let mut added = false;
    {
        let _routed = stand_in.route();
        let request = Mirror::default().request();
        sync_as(&request, limits, Host::default(), &AtomicBool::new(false), &mut |piece| {
            // Once the run is under way with the old mail, a message arrives in the Inbox.
            if !added && piece.folders.iter().any(|f| f.remote == "Old mail" && !f.new.is_empty()) {
                stand_in.state().add_message(&inbox, "Arrived during the run", T0 + DAY);
                added = true;
            }
            pieces.push(piece);
        });
    }
    assert!(added);
    // It came in this run, with a number of its own, and once.
    let inbox_new: Vec<&Message> = pieces.iter().flat_map(|p| &p.folders).filter(|f| f.remote == "INBOX").flat_map(|f| &f.new).collect();
    let mut seen: Vec<(&str, u32)> = inbox_new.iter().map(|m| (m.subject.as_str(), m.uid)).collect();
    seen.sort();
    assert_eq!(seen, [("Arrived during the run", 2), ("Before the run", 1)]);
    // And before the old mail was all down: it didn't wait for the run's end.
    let arrived_at = pieces.iter().position(|p| p.folders.iter().any(|f| f.new.iter().any(|m| m.subject == "Arrived during the run"))).unwrap();
    let last_old = pieces.iter().rposition(|p| p.folders.iter().any(|f| f.remote == "Old mail" && !f.new.is_empty())).unwrap();
    assert!(arrived_at < last_old, "{arrived_at} {last_old}");
    // The next run brings nothing twice.
    let mut mirror = Mirror::default();
    for piece in &pieces {
        mirror.take(piece);
    }
    let again = run(&stand_in, &mirror.request());
    assert!(again.folders.iter().all(|f| f.new.is_empty()), "{:?}", again.folders.iter().map(|f| (&f.remote, f.new.len())).collect::<Vec<_>>());
}

#[test]
fn a_look_at_the_inbox_alone_reads_it_and_nothing_else_through_graph() {
    let stand_in = MailStandIn::start().unwrap();
    {
        let mut s = stand_in.state();
        let inbox = s.folder("inbox");
        s.add_message(&inbox, "New in the Inbox", T0);
        let other = s.add_folder("Old mail", None);
        s.add_message(&other, "Old", T0 - DAY);
    }
    let mut request = Mirror::default().request();
    request.inbox_only = true;
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(subjects(&result, "INBOX"), ["New in the Inbox"]);
    assert!(result.folders.iter().all(|f| f.remote == "INBOX"));
    assert!(result.listed.is_empty() && !result.listed_whole);
}

// ---- a shared mailbox, read through Graph with its owner's token ----

/// The shared mailbox the owner opens; the stand-in is set up as that mailbox's.
const SHARED: &str = "support@example.org";

/// What the engine asks of a run for a shared mailbox whose owner's mail goes through Graph: the
/// shared mailbox's address, its owner, Graph as the source, and the owner's token.
fn shared_request(mirror: &Mirror) -> SyncRequest {
    let mut request = mirror.request();
    request.account = Account { id: 8, name: SHARED.into(), email: SHARED.into(), shared_of: Some(1), ..account() };
    request.send_login = "owner@example.org".into();
    request
}

/// The stand-in as the shared mailbox: its token reaches `/users/support@example.org/…`.
fn shared_stand_in() -> MailStandIn {
    let stand_in = MailStandIn::start().unwrap();
    stand_in.state().opens = vec![SHARED.into()];
    stand_in
}

/// Whether every request about mail named the shared mailbox (`/v1.0/users/{address}/…`), inside
/// a batch too; the batch itself is `/v1.0/$batch`.
fn all_named_the_mailbox(requests: &[super::standin::Asked]) -> bool {
    requests.iter().all(|r| {
        let path: Vec<&str> = r.path.iter().map(String::as_str).collect();
        matches!(path.as_slice(), ["v1.0", "$batch"]) || matches!(path.as_slice(), ["v1.0", "users", address, ..] if *address == SHARED)
    })
}

#[test]
fn a_shared_mailbox_is_read_and_changed_as_its_own_address() {
    let stand_in = shared_stand_in();
    let (inbox, a) = {
        let mut s = stand_in.state();
        let inbox = s.folder("inbox");
        let a = s.add_message(&inbox, "Printer on fire", T0);
        s.add_message(&inbox, "Refund", T0 + 1);
        let escalations = s.add_folder("Escalations", None);
        s.add_message(&escalations, "Call back", T0);
        (inbox, a)
    };
    let mut mirror = Mirror::default();
    let result = run(&stand_in, &shared_request(&mirror));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!((result.account, result.email.as_str()), (8, SHARED));
    assert_eq!(subjects(&result, "INBOX"), ["Printer on fire", "Refund"]);
    assert_eq!(subjects(&result, "Escalations"), ["Call back"]);
    assert!(result.listed.iter().any(|f| f.remote == "Escalations"));
    assert!(result.folders.iter().any(|f| f.remote == "INBOX" && f.delta.contains("/users/") && f.delta.contains("$deltatoken=")));
    {
        let s = stand_in.state();
        assert!(all_named_the_mailbox(&s.requests), "{:?}", s.requests.iter().map(|r| &r.target).collect::<Vec<_>>());
        assert!(s.requests.iter().any(|r| r.batched && r.target.ends_with("/$value")), "messages came in a batch");
        assert!(!s.requests.iter().any(|r| r.target.contains("/me/")), "nothing was asked of the owner's own mailbox");
    }
    mirror.take(&result);
    // A change made here goes up to the shared mailbox, and the next round reads from where
    // the last ended, by the address the stand-in named.
    stand_in.state().requests.clear();
    let mut request = shared_request(&mirror);
    request.ops =
        vec![RemoteOp::Flags { folder: "INBOX".into(), uid: mirror.uid_of("INBOX", &a), read: Some(true), flagged: Some(true), answered: None }];
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.flagged.len(), 1);
    let s = stand_in.state();
    let m = s.message(&a).unwrap();
    assert!(m.read && m.flagged);
    assert!(all_named_the_mailbox(&s.requests), "{:?}", s.requests.iter().map(|r| &r.target).collect::<Vec<_>>());
    assert!(s.requests.iter().any(|r| r.method == "PATCH" && r.target.contains("/users/")));
    assert!(s.requests.iter().any(|r| r.target.contains("$deltatoken=")));
    let _ = inbox;
}

/// A message from the shared mailbox, a reply, signed.
fn from_shared() -> Message {
    Message {
        id: 90,
        message_id: "going-1@example.org".into(),
        subject: "Your printer".into(),
        from: mailcraft_model::Address { name: "Support".into(), email: SHARED.into() },
        to: vec![mailcraft_model::Address { name: "Ada".into(), email: "ada@example.org".into() }],
        in_reply_to: "fire-1@example.org".into(),
        references: vec!["fire-1@example.org".into()],
        date: T0,
        ..Default::default()
    }
}

fn signed(message: Message) -> Message {
    let raw = b"Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nWe are on our way.\r\n--b\r\nContent-Type: application/pkcs7-signature\r\n\r\nSIGNATURE\r\n--b--\r\n".to_vec();
    Message { secured: Some(mailcraft_model::Secured { kind: mailcraft_model::Protection::SmimeSigned, raw: std::sync::Arc::new(raw) }), ..message }
}

#[test]
fn mail_from_a_shared_mailbox_goes_out_as_the_mailbox_itself_and_is_filed_there() {
    let stand_in = shared_stand_in();
    let mut request = shared_request(&Mirror::default());
    request.outbox = vec![signed(from_shared())];
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    // Sent, and the copy is in the shared mailbox's own Sent Items (the run read it there).
    assert_eq!((result.sent.as_slice(), result.filed.as_slice()), ([90].as_slice(), [90].as_slice()));
    assert_eq!(subjects(&result, "Sent Items"), ["Your printer"]);
    let s = stand_in.state();
    let sending: Vec<_> = s.requests.iter().filter(|r| r.target.ends_with("/sendMail")).collect();
    assert_eq!(sending.len(), 1, "{:?}", s.requests.iter().map(|r| &r.target).collect::<Vec<_>>());
    assert_eq!(sending[0].path, ["v1.0", "users", SHARED, "sendMail"]);
    assert!(sending[0].header("content-type").starts_with("text/plain"));
    assert!(s.sent_as.is_empty(), "nothing went through the owner's own sending");
    // As an Internet message, whole: the signature, and the threading.
    let went = String::from_utf8_lossy(&s.sent[0]);
    assert!(went.contains("From: Support <support@example.org>"), "{went}");
    assert!(went.contains("In-Reply-To: <fire-1@example.org>") && went.contains("References: <fire-1@example.org>"), "{went}");
    assert!(went.contains("multipart/signed") && went.contains("SIGNATURE"), "{went}");
    assert_eq!(s.subjects(&s.folder("sentitems")), ["Your printer"]);
}

#[test]
fn a_sign_in_that_does_not_cover_sending_as_the_mailbox_sends_through_the_owner() {
    // The token reaches nothing as `/users/…` (403): the Outbox goes through the owner's own
    // sending, with the shared mailbox as the sender, and the copy is the owner's.
    let stand_in = MailStandIn::start().unwrap();
    let mut request = shared_request(&Mirror::default());
    request.outbox = vec![from_shared()];
    let result = run(&stand_in, &request);
    assert_eq!(result.sent, [90]);
    assert!(result.filed.is_empty(), "the copy isn't in the shared mailbox's Sent Items");
    {
        let s = stand_in.state();
        assert_eq!(s.sent_as.len(), 1);
        assert_eq!(s.sent_as[0]["from"]["emailAddress"]["address"], SHARED);
        assert!(s.sent.is_empty());
    }
    // The rest of the run is one clear line: the mailbox can't be opened.
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0].starts_with("Couldn't read the list of folders: ") && result.errors[0].contains("may not read or change its mail"),
        "{:?}",
        result.errors
    );
    assert!(result.folders.is_empty() && result.listed.is_empty());
    assert!(!format!("{result:?}").contains(TOKEN));

    // Without the Send As right either: said as that, in one line, and nothing went.
    let stand_in = MailStandIn::start().unwrap();
    stand_in.state().send_as_denied = true;
    let result = run(&stand_in, &request);
    assert!(result.sent.is_empty());
    let line =
        result.errors.iter().find(|e| e.starts_with("Couldn't send \"Your printer\" from support@example.org:")).expect("a line about sending");
    assert!(line.contains("Send As right") && line.contains("SendAsDenied"), "{line}");
    assert!(stand_in.state().sent_as.is_empty());

    // Exchange saying the Send As right is missing to the mailbox's own sending: no other way is tried.
    let stand_in = shared_stand_in();
    stand_in.state().fail_next = vec![("POST".into(), "/sendMail".into(), 403, "ErrorSendAsDenied".into())];
    let result = run(&stand_in, &request);
    assert!(result.sent.is_empty());
    assert!(result.errors.iter().any(|e| e.contains("Send As right") && e.contains("support@example.org")), "{:?}", result.errors);
    let s = stand_in.state();
    assert!(s.sent_as.is_empty() && s.sent.is_empty());
    assert_eq!(s.requests.iter().filter(|r| r.target.ends_with("/sendMail")).count(), 1);
}

#[test]
fn a_signed_message_is_never_sent_stripped_of_its_signature() {
    // Through the owner's own sending there is no place for the signature: refused, not stripped.
    let stand_in = MailStandIn::start().unwrap();
    let _routed = stand_in.route();
    match super::super::send_as(TOKEN, &signed(from_shared()), SHARED) {
        Err(crate::NetError::Config(why)) => assert!(why.contains("signed or encrypted") && why.contains("Mail.Send.Shared"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(stand_in.state().requests.is_empty(), "nothing was sent anywhere");
    drop(_routed);
    // The same from a run whose sign-in doesn't cover sending as the mailbox: said, and not sent.
    let mut request = shared_request(&Mirror::default());
    request.outbox = vec![signed(from_shared())];
    let result = run(&stand_in, &request);
    assert!(result.sent.is_empty() && result.filed.is_empty());
    assert!(
        result.errors.iter().any(|e| e.starts_with("Couldn't send \"Your printer\" from support@example.org:") && e.contains("Mail.Send.Shared")),
        "{:?}",
        result.errors
    );
    let s = stand_in.state();
    assert!(s.sent.is_empty() && s.sent_as.is_empty());
}

#[test]
fn a_mailbox_the_token_may_not_open_is_one_clear_error() {
    let stand_in = MailStandIn::start().unwrap();
    stand_in.state().opens = vec!["finance@example.org".into()];
    let request = shared_request(&Mirror::default());
    let result = run(&stand_in, &request);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].contains("may not read or change its mail through Microsoft Graph"), "{:?}", result.errors);
    assert!(result.folders.is_empty() && result.listed.is_empty() && !result.listed_whole);
    assert!(!result.errors[0].contains(TOKEN));
    // An address that can't be one is refused before anything is asked.
    let mut request = shared_request(&Mirror::default());
    request.account.email = "not an address".into();
    let result = run(&stand_in, &request);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].contains("isn't the address of a mailbox"), "{:?}", result.errors);
}

/// How many rounds of folders' messages the stand-in was asked for.
fn rounds_asked(stand_in: &MailStandIn) -> usize {
    stand_in.state().requests.iter().filter(|r| r.target.contains("/messages/delta")).count()
}

/// How many rounds of the messages of the folder `id` it was asked for.
fn rounds_of(stand_in: &MailStandIn, id: &str) -> usize {
    stand_in.state().requests.iter().filter(|r| r.target.contains("/messages/delta") && r.path.iter().any(|part| part == id)).count()
}

#[test]
fn a_folder_whose_counts_are_as_the_listing_said_is_not_asked_for_its_round() {
    use crate::sync::FULL_SCAN_EVERY;
    let stand_in = MailStandIn::start().unwrap();
    let (inbox, old, a) = {
        let mut s = stand_in.state();
        let inbox = s.folder("inbox");
        let old = s.add_folder("Old mail", None);
        let a = s.add_message(&old, "Old A", T0 - DAY);
        s.add_message(&old, "Old B", T0 - 2 * DAY);
        for (n, subject) in ["One", "Two", "Three"].iter().enumerate() {
            s.add_message(&inbox, subject, T0 + i64::try_from(n).unwrap());
        }
        (inbox, old, a)
    };
    let mut mirror = Mirror::default();
    mirror.take(&run(&stand_in, &mirror.request()));
    let kept = &mirror.folders["Old mail"];
    assert_eq!((kept.mark.exists, kept.mark.unread, kept.mark.quick_runs), (2, 2, 0));
    assert_eq!((mirror.folders["INBOX"].mark.exists, mirror.folders["INBOX"].mark.unread), (3, 3));
    // Nothing changed: the listing says the same counts, and no round is asked for; every
    // folder is spoken of, with one more quick run.
    let runs_before = rounds_asked(&stand_in);
    let quiet = run(&stand_in, &mirror.request());
    assert!(quiet.errors.is_empty(), "{:?}", quiet.errors);
    assert_eq!(rounds_asked(&stand_in), runs_before);
    assert!(quiet.folders.iter().all(|f| f.new.is_empty() && f.flags.is_empty() && f.removed.is_empty()));
    assert!(quiet.folders.iter().any(|f| f.remote == "Old mail") && quiet.folders.iter().any(|f| f.remote == "INBOX"));
    mirror.take(&quiet);
    assert_eq!(mirror.folders["Old mail"].mark.quick_runs, 1);
    // A message read elsewhere: the unread count alone changed, and that folder's round is
    // asked for, no other's.
    stand_in.state().edit(&a, |m| m.read = true);
    let result = run(&stand_in, &mirror.request());
    assert_eq!(rounds_asked(&stand_in), runs_before + 1);
    let ua = mirror.uid_of("Old mail", &a);
    assert!(result.folders.iter().filter(|f| f.remote == "Old mail").flat_map(|f| &f.flags).any(|f| f.uid == ua && f.read));
    mirror.take(&result);
    assert_eq!((mirror.folders["Old mail"].mark.unread, mirror.folders["Old mail"].mark.quick_runs), (1, 0));
    // A message came: that folder's round, no other's.
    stand_in.state().add_message(&inbox, "Four", T0 + DAY);
    let result = run(&stand_in, &mirror.request());
    assert_eq!((rounds_asked(&stand_in), subjects(&result, "INBOX")), (runs_before + 2, vec!["Four".to_string()]));
    mirror.take(&result);
    assert_eq!(mirror.folders["INBOX"].mark.exists, 4);
    // Every few runs a folder's round is asked for whatever the counts say: as many may have
    // come in as went out. (The Inbox was just read: its count of quick runs starts here.)
    let before = rounds_of(&stand_in, &inbox);
    for run_number in 1..FULL_SCAN_EVERY {
        mirror.take(&run(&stand_in, &mirror.request()));
        assert_eq!((rounds_of(&stand_in, &inbox), mirror.folders["INBOX"].mark.quick_runs), (before, run_number), "run {run_number}");
    }
    mirror.take(&run(&stand_in, &mirror.request()));
    assert_eq!((rounds_of(&stand_in, &inbox), mirror.folders["INBOX"].mark.quick_runs), (before + 1, 0));
    // A message moved by the run itself into a folder: that folder is read whatever its count
    // (the listing was from before the move).
    let moved = stand_in.state().subjects(&old);
    assert_eq!(moved.len(), 2);
    let mut request = mirror.request();
    let two = mirror.folders["INBOX"].remote_ids.iter().find(|r| r.uid == 2).unwrap().id.clone();
    request.ops.push(RemoteOp::Move { message: 5, from: "INBOX".into(), uid: 2, to: "Old mail".into(), to_folder: 101, create: Vec::new() });
    let before = rounds_asked(&stand_in);
    let result = run(&stand_in, &request);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.moved.len() == 1 && stand_in.state().message(&two).is_some_and(|m| m.folder == old));
    assert!(result.folders.iter().filter(|f| f.remote == "Old mail").any(|f| f.new.iter().any(|m| m.subject == "Two")));
    assert!(rounds_asked(&stand_in) >= before + 1);
}

#[test]
fn not_now_beyond_what_is_waited_for_ends_the_run_with_when_to_come_back() {
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    stand_in.state().add_message(&inbox, "A", T0);
    // Told "not now" every time, with a second to wait: asked again after that long, and once
    // more, then the run ends saying when to come back, apart from the problems to tell.
    stand_in.state().throttle = 100;
    let result = run(&stand_in, &Mirror::default().request());
    assert_eq!(result.retry_after, Some(1));
    assert!(result.busy.contains("Microsoft Graph is busy; try again in a second"), "{}", result.busy);
    assert!(!result.busy.contains(TOKEN));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.folders.is_empty());
    let asked = stand_in.state().requests.len();
    assert!((2..=8).contains(&asked), "{asked} requests");
    // Told once: asked again after the wait, and the run goes through without a word.
    let stand_in = MailStandIn::start().unwrap();
    let inbox = stand_in.state().folder("inbox");
    stand_in.state().add_message(&inbox, "A", T0);
    stand_in.state().throttle = 1;
    let result = run(&stand_in, &Mirror::default().request());
    assert!(result.errors.is_empty() && result.retry_after.is_none() && result.busy.is_empty(), "{:?}", result.errors);
    assert_eq!(subjects(&result, "INBOX"), ["A"]);
    assert_eq!(stand_in.state().requests.iter().filter(|r| r.status == 429).count(), 1);
}
