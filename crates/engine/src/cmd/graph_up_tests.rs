//! Changes of messages of a Microsoft account, as the engine works them out and takes in what
//! became of them. (Against the Graph stand-in itself: `apps/mailcraft-cli/tests/graph_changes.rs`.)

use mailcraft_model::sync::{GraphMailResult, GraphMessage, MessageChangeDone, MovingOut};
use mailcraft_model::{Account, Folder, Importance, ServerFolder};
use serde_json::json;

use super::*;
use crate::cmd::graph_mail::{apply_graph_mail, graph_mail_request};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

/// A session with a Microsoft account whose Inbox, Archive and Deleted Items the server has, a
/// message in its Inbox the server has said things of, and a local account: the session, the
/// account, the message and the local account's folder.
fn setup() -> (Session, Id, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    for (kind, id) in [(FolderKind::Inbox, "F-in"), (FolderKind::Archive, "F-ar"), (FolderKind::Deleted, "F-de")] {
        let folder = match s.mailbox.folder_of_kind(account, kind) {
            Some(folder) => folder,
            None => {
                let folder = s.mailbox.alloc_id();
                s.mailbox.folders.push(Folder { id: folder, account, name: kind.default_name().into(), kind, ..Default::default() });
                folder
            }
        };
        s.mailbox.folder_mut(folder).unwrap().server = Some(ServerFolder { id: id.into(), name: kind.default_name().into(), ..Default::default() });
    }
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: "M-1".into(), folder: "F-in".into(), categories: vec!["Red category".into()], ..Default::default() };
    let m = s
        .mailbox
        .add_message(Message {
            folder: inbox,
            subject: "Invoice 42".into(),
            categories: vec!["Red category".into()],
            server: Some(server),
            ..Default::default()
        })
        .unwrap();
    let local = s.mailbox.add_account(Account { name: "On this computer".into(), ..Default::default() });
    let kept = s.mailbox.add_folder(local, None, "Kept").unwrap();
    (s, account, m, kept)
}

fn changes(s: &Session, account: Id) -> Vec<MessageChange> {
    message_changes(s, account, 100).0
}

#[test]
fn nothing_changed_here_sends_nothing() {
    let (s, account, _, _) = setup();
    assert_eq!(changes(&s, account), []);
    // Categories in another order or case are the same categories.
    let (mut s, account, m, _) = setup();
    s.mailbox.message_mut(m).unwrap().categories = vec!["RED CATEGORY".into()];
    assert_eq!(changes(&s, account), []);
}

#[test]
fn read_flag_categories_and_importance_go_as_one_change_and_a_folder_as_a_move() {
    let (mut s, account, m, _) = setup();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.run("mail.markRead", &json!({"ids": [m]})).unwrap();
    s.run("mail.flag", &json!({"ids": [m], "due": "2027-01-10"})).unwrap();
    s.run("mail.categorize", &json!({"ids": [m], "category": "Blue category"})).unwrap();
    s.mailbox.message_mut(m).unwrap().importance = Importance::Low;
    s.run("mail.move", &json!({"ids": [m], "folder": archive})).unwrap();
    let due = s.mailbox.message(m).unwrap().flag_due.unwrap();
    let all = changes(&s, account);
    assert_eq!(all.len(), 2, "{all:?}");
    let MessageChange::Update { read, flag, categories, importance, .. } = &all[0] else { panic!("{all:?}") };
    assert_eq!((*read, *importance), (Some(true), Some(Importance::Low)));
    // Due before today: it started then too.
    assert_eq!(flag.clone().unwrap(), FlagUp { status: FlagStatus::Flagged, start: Some(due), due: Some(due), completed: None });
    assert_eq!(
        categories.clone().unwrap(),
        CategoriesUp { synced: vec!["Red category".into()], here: vec!["Red category".into(), "Blue category".into()] }
    );
    assert_eq!(all[1], MessageChange::Move { message: m, id: "M-1".into(), to: "F-ar".into(), to_folder: None });
    // A flag due later starts today; one that the server had started keeps that day.
    s.run("mail.flag", &json!({"ids": [m], "due": "2027-02-01"})).unwrap();
    let today = NOW - NOW % 86_400;
    let flag_of = |s: &Session| match &changes(s, account)[0] {
        MessageChange::Update { flag, .. } => flag.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(flag_of(&s).unwrap().start, Some(today));
    if let Some(server) = s.mailbox.message_mut(m).unwrap().server.as_mut() {
        (server.flag, server.flag_start) = (FlagStatus::Flagged, Some(today - 86_400));
    }
    assert_eq!(flag_of(&s).unwrap().start, Some(today - 86_400));
    // Complete: when.
    s.run("mail.complete", &json!({"ids": [m]})).unwrap();
    assert_eq!(flag_of(&s).unwrap(), FlagUp { status: FlagStatus::Complete, completed: Some(NOW), ..Default::default() });
}

#[test]
fn copies_and_deletes_for_good_wait_in_the_queue() {
    let (mut s, account, m, _) = setup();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [m], "folder": archive})).unwrap()["copies"][0].as_u64().unwrap();
    let all = changes(&s, account);
    let [MessageChange::Copy { copy: c, id, to, marker, .. }] = all.as_slice() else { panic!("{all:?}") };
    assert_eq!((*c, id.as_str(), to.as_str()), (copy, "M-1", "F-ar"));
    assert!(marker.starts_with("mailcraft-") && marker.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'), "{marker}");
    // Another copy of the same message has another marker.
    let other = s.run("mail.copy", &json!({"ids": [m], "folder": archive})).unwrap()["copies"][0].as_u64().unwrap();
    let markers: Vec<String> =
        s.mailbox.queued.iter().filter_map(|q| if let Queued::CopyMessage { marker, .. } = q { Some(marker.clone()) } else { None }).collect();
    assert_eq!(markers.len(), 2);
    assert_ne!(markers[0], markers[1]);
    // A copy deleted here before the server made it isn't made.
    s.run("mail.deletePermanently", &json!({"ids": [other]})).unwrap();
    assert_eq!(s.mailbox.queued.len(), 1);
    // Delete for good: Graph's delete.
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    assert!(changes(&s, account).contains(&MessageChange::Delete { id: "M-1".into() }));
    // Delete: a move into Deleted Items.
    let (mut s, account, m, _) = setup();
    s.run("mail.delete", &json!({"ids": [m]})).unwrap();
    assert_eq!(changes(&s, account), [MessageChange::Move { message: m, id: "M-1".into(), to: "F-de".into(), to_folder: None }]);
}

#[test]
fn a_move_out_of_the_account_waits_for_the_whole_message_and_the_save() {
    let (mut s, account, m, kept) = setup();
    s.mailbox.message_mut(m).unwrap().unfetched = true;
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    let moved = s.mailbox.message(m).unwrap();
    assert_eq!(moved.moving_out, Some(MovingOut { account, id: "M-1".into() }));
    assert!(moved.server.is_none() && moved.folder == kept);
    // Fetched first of all, whatever its size.
    assert_eq!(super::super::graph_down::fetch_queue(&s.mailbox, account, 5), ["M-1"]);
    assert_eq!(changes(&s, account), []);
    // Whole, but not saved.
    s.mailbox.message_mut(m).unwrap().unfetched = false;
    s.mailbox_changed();
    assert_eq!(changes(&s, account), []);
    // Saved (as the host's own saver says, or a file that holds it as it is).
    s.note_saved();
    assert_eq!(changes(&s, account), [MessageChange::MovedOut { message: m, id: "M-1".into() }]);
    // Moved back before that went: it is the server's message again, and goes where it was put.
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": inbox})).unwrap();
    let back = s.mailbox.message(m).unwrap();
    assert!(back.moving_out.is_none() && back.server.as_ref().is_some_and(|server| server.id == "M-1"));
    assert_eq!(changes(&s, account), [MessageChange::Move { message: m, id: "M-1".into(), to: "F-in".into(), to_folder: None }]);
    // Gone from the server before it was fetched: kept as it is, incomplete.
    let (mut s, account, m, kept) = setup();
    s.mailbox.message_mut(m).unwrap().unfetched = true;
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    assert_eq!(missing(&mut s.mailbox, account, &["M-1".to_string()]), 1);
    let kept_here = s.mailbox.message(m).unwrap();
    assert!(kept_here.incomplete && kept_here.moving_out.is_none() && kept_here.unfetched);
    assert!(super::super::graph_down::fetch_queue(&s.mailbox, account, 5).is_empty());
}

#[test]
fn nothing_comes_into_a_microsoft_account_from_elsewhere_and_an_unfetched_message_isnt_copied_out() {
    let (mut s, account, m, kept) = setup();
    let local = s.mailbox.add_message(Message { folder: kept, subject: "Here only".into(), draft: true, ..Default::default() }).unwrap();
    let received = s.mailbox.add_message(Message { folder: kept, subject: "Received".into(), ..Default::default() }).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    for command in ["mail.move", "mail.copy"] {
        let refused = s.run(command, &json!({"ids": [local], "folder": inbox})).unwrap_err();
        assert!(refused.to_string().contains(mailcraft_model::INTO_GRAPH), "{refused}");
        // Received mail in a local folder never goes (S30).
        let refused = s.run(command, &json!({"ids": [received], "folder": inbox})).unwrap_err();
        assert_eq!(refused.to_string(), mailcraft_model::STAYS_LOCAL);
    }
    s.mailbox.message_mut(m).unwrap().unfetched = true;
    let refused = s.run("mail.copy", &json!({"ids": [m], "folder": kept})).unwrap_err();
    assert!(refused.to_string().contains("\"Invoice 42\" isn't downloaded yet"), "{refused}");
    // Whole, it is copied out as any message is: a copy of its own, not on a server.
    s.mailbox.message_mut(m).unwrap().unfetched = false;
    let copy = s.run("mail.copy", &json!({"ids": [m], "folder": kept})).unwrap()["copies"][0].as_u64().unwrap();
    let c = s.mailbox.message(copy).unwrap();
    assert!(c.server.is_none() && c.moving_out.is_none() && s.mailbox.queued.is_empty());
}

/// R3 spec S30: mail imported for a Microsoft account goes to a local folder, and stays there.
#[test]
fn mail_imported_for_a_microsoft_account_goes_to_a_local_folder_and_stays_there() {
    let (mut s, account, m, _) = setup();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let eml = s.run("file.exportMessage", &json!({"id": m})).unwrap();
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    let before = s.mailbox.accounts.len();
    let r = s.run("file.import", &json!({"name": "in.eml", "data": eml["data"]})).unwrap();
    let local = r["folder"].as_u64().unwrap();
    let f = s.mailbox.folder(local).unwrap().clone();
    assert_eq!((f.name.as_str(), r["local"].as_bool(), s.mailbox.accounts.len()), (mailcraft_model::IMPORTED, Some(true), before));
    assert_eq!(s.mailbox.account(f.account).unwrap().kind, mailcraft_model::AccountKind::Local);
    // It is shown where it went, and nothing of it goes up.
    assert_eq!(s.view.folder, local);
    let imported = r["ids"][0].as_u64().unwrap();
    assert!(s.mailbox.message(imported).unwrap().server.is_none() && changes(&s, account).is_empty());
    // Again into the same folder; into Drafts, the local Drafts, as drafts.
    let again = s.run("file.import", &json!({"name": "in.eml", "data": eml["data"], "folder": inbox})).unwrap();
    assert_eq!(again["folder"].as_u64(), Some(local));
    let draft = s.run("file.import", &json!({"name": "in.eml", "data": eml["data"], "folder": drafts})).unwrap();
    let local_drafts = s.mailbox.folder(draft["folder"].as_u64().unwrap()).unwrap();
    assert_eq!((local_drafts.account, local_drafts.kind), (f.account, FolderKind::Drafts));
    assert!(s.mailbox.message(draft["ids"][0].as_u64().unwrap()).unwrap().draft);
    // Moving it into the account is refused with the reason, and changes nothing.
    for command in ["mail.move", "mail.copy"] {
        let refused = s.run(command, &json!({"ids": [imported], "folder": inbox})).unwrap_err();
        assert_eq!(refused.to_string(), mailcraft_model::STAYS_LOCAL);
        assert_eq!(s.mailbox.message(imported).unwrap().folder, local);
    }
    // Mail imported into another account's folder lands there, as before.
    let kept = s.mailbox.folders.iter().find(|f| f.name == "Kept").unwrap().id;
    let there = s.run("file.import", &json!({"name": "in.eml", "data": eml["data"], "folder": kept})).unwrap();
    assert_eq!((there["folder"].as_u64(), there["local"].as_bool()), (Some(kept), Some(false)));
}

/// With no local account, importing for a Microsoft account makes one.
#[test]
fn importing_for_a_microsoft_account_without_a_local_account_makes_one() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.accounts.iter().all(|a| a.kind == mailcraft_model::AccountKind::Microsoft));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mbox = "From a@example.org Thu Jan 14 08:00:00 2027\nFrom: a@example.org\nSubject: One\n\nHi\n\nFrom b@example.org Thu Jan 14 09:00:00 2027\nFrom: b@example.org\nSubject: Two\n\nHi\n";
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, mbox);
    let r = s.run("file.import", &json!({"name": "all.mbox", "data": data, "folder": inbox})).unwrap();
    assert_eq!(r["messages"], 2);
    let f = s.mailbox.folder(r["folder"].as_u64().unwrap()).unwrap();
    let a = s.mailbox.account(f.account).unwrap();
    assert_eq!((a.kind, a.name.as_str(), f.name.as_str()), (mailcraft_model::AccountKind::Local, "On this computer", mailcraft_model::IMPORTED));
    assert_eq!(s.mailbox.messages_in(inbox).count(), 0);
}

fn result(account: Id, messages: Vec<MessageChangeDone>) -> GraphMailResult {
    GraphMailResult { account, email: "owner@example.org".into(), messages, ..Default::default() }
}

#[test]
fn what_was_made_is_the_last_synced_state_and_undo_is_another_change() {
    let (mut s, account, m, _) = setup();
    s.run("mail.markRead", &json!({"ids": [m]})).unwrap();
    s.run("mail.categorize", &json!({"ids": [m], "category": "Blue category"})).unwrap();
    assert!(s.can_undo());
    let change = graph_mail_request(&s, account).unwrap().messages.remove(0);
    // The server had added Green meanwhile: the merge is what it has now.
    let merged = vec!["Red category".to_string(), "Green category".into(), "Blue category".into()];
    let outcome = MessageOutcome::Done { categories: Some(merged.clone()), flag: None, folder: String::new(), id: String::new() };
    let answer = apply_graph_mail(&mut s, result(account, vec![MessageChangeDone { change, outcome, there: None }])).unwrap();
    assert_eq!(answer["pushed"], 1);
    let here = s.mailbox.message(m).unwrap();
    let server = here.server.as_ref().unwrap();
    assert!(server.read && here.read);
    assert_eq!((&here.categories, &server.categories), (&merged, &merged));
    assert_eq!(changes(&s, account), []);
    // The history stays (R3.5): undoing is another change. Blue goes off again, and Green, which
    // the server added meanwhile, stays; read stays as the server has it.
    assert!(s.can_undo());
    s.run("edit.undo", &json!({})).unwrap();
    let here = s.mailbox.message(m).unwrap();
    assert_eq!(here.categories, ["Red category", "Green category"]);
    assert!(here.read);
    let all = changes(&s, account);
    let [MessageChange::Update { read: None, categories: Some(up), .. }] = all.as_slice() else { panic!("{all:?}") };
    assert_eq!((&up.synced, &up.here), (&merged, &here.categories));
}

#[test]
fn a_refusal_is_put_back_and_said_once_per_kind() {
    let (mut s, account, m, _) = setup();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let mut ids = vec![m];
    for n in 2..5 {
        let server = ServerMessage { id: format!("M-{n}"), folder: "F-in".into(), ..Default::default() };
        ids.push(s.mailbox.add_message(Message { folder: inbox, subject: format!("Note {n}"), server: Some(server), ..Default::default() }).unwrap());
    }
    s.run("mail.markRead", &json!({"ids": ids})).unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": archive})).unwrap();
    let all = graph_mail_request(&s, account).unwrap().messages;
    let done: Vec<MessageChangeDone> = all
        .into_iter()
        .map(|change| {
            let outcome = match &change {
                MessageChange::Move { .. } => MessageOutcome::Refused { status: 403 },
                MessageChange::Update { id, .. } if id == "M-1" => MessageOutcome::Refused { status: 400 },
                _ => MessageOutcome::Gone,
            };
            MessageChangeDone { change, outcome, there: None }
        })
        .collect();
    let answer = apply_graph_mail(&mut s, result(account, done)).unwrap();
    assert_eq!(
        answer["errors"],
        json!([
            "Couldn't mark 3 messages as read: the server says they no longer exist.",
            "Couldn't mark 'Invoice 42' as read: the server says it can't make that change.",
            "Couldn't move 'Invoice 42' to Archive: the server says this account may not change it.",
        ])
    );
    assert!(s.sync_status.starts_with("Couldn't mark 3 messages as read"), "{}", s.sync_status);

    // Put back: unread, and in the Inbox. The others are gone from the server, and leave once the
    // run has seen every folder.
    let back = s.mailbox.message(m).unwrap();
    assert!(!back.read && back.folder == inbox);
    assert!(ids[1..].iter().all(|id| s.mailbox.message(*id).unwrap().server.as_ref().unwrap().gone));
    assert_eq!(changes(&s, account), []);
    // The next run's status doesn't say it again.
    apply_graph_mail(&mut s, result(account, Vec::new())).unwrap();
    assert!(!s.sync_status.contains("Couldn't"), "{}", s.sync_status);
}

/// The answer to the one delete for good of a run.
fn delete_answered(s: &mut Session, account: Id, outcome: MessageOutcome, there: Option<GraphMessage>) -> serde_json::Value {
    let change = graph_mail_request(s, account).unwrap().messages.into_iter().find(|c| matches!(c, MessageChange::Delete { .. })).unwrap();
    apply_graph_mail(s, result(account, vec![MessageChangeDone { change, outcome, there: there.map(Box::new) }])).unwrap()
}

#[test]
fn a_refused_delete_for_good_is_put_back_at_once_with_all_that_was_fetched() {
    let (mut s, account, m, _) = setup();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.message_mut(m).unwrap().body.text = "Please pay by Friday.".into();
    // Marked read, then deleted for good: neither reached the server.
    s.run("mail.markRead", &json!({"ids": [m]})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    assert!(s.mailbox.message(m).is_none());
    let answer = delete_answered(&mut s, account, MessageOutcome::Refused { status: 403 }, None);
    assert_eq!(answer["errors"], json!(["Couldn't delete 'Invoice 42' for good: the server doesn't allow it."]));
    // Back as the server has it (unread, in the Inbox), with its text, and nothing to send.
    let back = s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap();
    assert_eq!((back.folder, back.read, back.body.text.as_str()), (inbox, false, "Please pay by Friday."));
    assert_eq!(back.server.as_ref().map(|s| s.id.as_str()), Some("M-1"));
    assert!(s.mailbox.queued.is_empty());
    assert_eq!(changes(&s, account), []);

    // Read on the server after the refusal: its folder and state are the server's now.
    let (mut s, account, m, _) = setup();
    s.mailbox.message_mut(m).unwrap().body.text = "Kept".into();
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    let mut there = GraphMessage { subject: "Invoice 42".into(), ..Default::default() };
    there.server = ServerMessage { id: "M-1".into(), folder: "F-ar".into(), read: true, importance: Importance::High, ..Default::default() };
    let answer = delete_answered(&mut s, account, MessageOutcome::Refused { status: 400 }, Some(there.clone()));
    assert_eq!(answer["errors"], json!(["Couldn't delete 'Invoice 42' for good: the server says it can't make that change."]));
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let back = s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap();
    assert_eq!((back.folder, back.read, back.importance, back.body.text.as_str()), (archive, true, Importance::High, "Kept"));
    assert_eq!(changes(&s, account), []);

    // Nothing kept here (a queue from before): from what the server said, to be fetched whole.
    let (mut s, account, m, _) = setup();
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    for q in &mut s.mailbox.queued {
        if let Queued::DeleteMessage { kept, .. } = q {
            *kept = None;
        }
    }
    delete_answered(&mut s, account, MessageOutcome::Refused { status: 403 }, Some(there));
    let back = s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap();
    assert!(back.unfetched && back.folder == archive);
    assert_eq!(changes(&s, account), []);

    // Gone from the server already: it stays gone, and nothing is said.
    let (mut s, account, m, _) = setup();
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    let answer = delete_answered(&mut s, account, MessageOutcome::Gone, None);
    assert_eq!(answer["errors"], json!([]));
    assert!(s.mailbox.messages.values().all(|m| m.subject != "Invoice 42"));
    assert!(s.mailbox.queued.is_empty());
}

#[test]
fn a_refused_delete_of_a_message_being_moved_out_puts_it_back_where_the_server_has_it() {
    let (mut s, account, m, kept) = setup();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    assert!(s.mailbox.message(m).unwrap().moving_out.is_some());
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    let mut there = GraphMessage { subject: "Invoice 42".into(), ..Default::default() };
    there.server = ServerMessage { id: "M-1".into(), folder: "F-in".into(), ..Default::default() };
    delete_answered(&mut s, account, MessageOutcome::Refused { status: 403 }, Some(there));
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let back = s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap();
    assert_eq!((back.account, back.folder, back.moving_out.is_none()), (account, inbox, true));
    assert_eq!(changes(&s, account), []);
    // Not to be read either: back where it was here, still being moved.
    let (mut s, account, m, kept) = setup();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    delete_answered(&mut s, account, MessageOutcome::Refused { status: 403 }, None);
    let back = s.mailbox.messages.values().find(|m| m.subject == "Invoice 42").unwrap();
    assert_eq!((back.folder, back.moving_out.as_ref().map(|o| o.id.as_str())), (kept, Some("M-1")));
}

#[test]
fn categories_changed_on_both_sides_merge_when_the_servers_come_down_first() {
    let (mut s, account, m, _) = setup();
    s.run("mail.categorize", &json!({"ids": [m], "category": "Blue category"})).unwrap();
    // The server's delta comes before this was sent: Red taken away and Green added there.
    let page = mailcraft_model::sync::MessagePage {
        folder: "F-in".into(),
        messages: vec![mailcraft_model::sync::GraphMessage {
            server: ServerMessage {
                id: "M-1".into(),
                folder: "F-in".into(),
                categories: vec!["Green category".into()],
                read: true,
                ..Default::default()
            },
            subject: "Invoice 42".into(),
            ..Default::default()
        }],
        link: "https://graph.microsoft.com/v1.0/x".into(),
        ..Default::default()
    };
    apply_graph_mail(&mut s, GraphMailResult { pages: vec![page], ..result(account, Vec::new()) }).unwrap();
    let here = s.mailbox.message(m).unwrap();
    assert_eq!(here.categories, ["Green category", "Blue category"]);
    assert!(here.read, "not changed here: the server's");
    // What goes up is Blue added to the server's list as it is then.
    let MessageChange::Update { categories, .. } = &changes(&s, account)[0] else { panic!() };
    assert_eq!(
        categories.clone().unwrap(),
        CategoriesUp { synced: vec!["Green category".into()], here: vec!["Green category".into(), "Blue category".into()] }
    );
}

#[test]
fn outlooks_colours_win_for_the_categories_on_its_list() {
    let (mut s, account, _, _) = setup();
    let download = mailcraft_model::remote::CategoriesDownload {
        account,
        email: "owner@example.org".into(),
        categories: vec![mailcraft_model::Category { name: "Blue category".into(), color: mailcraft_model::Rgb(1, 2, 3) }],
    };
    s.run("sync.applyCategories", &json!({"download": download})).unwrap();
    assert_eq!(s.mailbox.outlook_categories, ["Blue category"]);
    let refused = s.run("category.set", &json!({"name": "blue category", "color": "#2E8B57"})).unwrap_err();
    assert_eq!(refused.to_string(), super::super::category::COLOUR_IN_OUTLOOK);
    assert_eq!(s.mailbox.category("Blue category").unwrap().color, mailcraft_model::Rgb(1, 2, 3));
    // One that isn't on Outlook's list takes the colour, and says it isn't there.
    s.run("category.set", &json!({"name": "Travel", "color": "#2E8B57"})).unwrap();
    let list = s.run("category.list", &json!({})).unwrap();
    let outlook = |name: &str| list["categories"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()["outlook"].clone();
    assert_eq!((outlook("Blue category"), outlook("Travel")), (json!(true), json!(false)));
    // An account that isn't kept in step through Graph has no such list.
    let (mut s, _, _, _) = setup();
    let list = s.run("category.list", &json!({})).unwrap();
    assert!(list["categories"].as_array().unwrap().iter().all(|c| c["outlook"].is_null()));
    s.run("category.set", &json!({"name": "Blue category", "color": "#2E8B57"})).unwrap();
}

#[test]
fn a_message_the_server_wouldnt_give_is_passed_over_in_the_background_but_still_asked_for_when_opened() {
    use crate::cmd::graph_mail::{apply_graph_fetch, graph_fetch_request};
    use mailcraft_model::sync::GraphFetchResult;
    let (mut s, account, m, _) = setup();
    s.mailbox.message_mut(m).unwrap().unfetched = true;
    assert_eq!(graph_fetch_request(&s, account, None, 10).unwrap().ids, ["M-1"]);
    let failed = GraphFetchResult { account, email: "owner@example.org".into(), failed: vec!["M-1".into()], ..Default::default() };
    let said = apply_graph_fetch(&mut s, failed).unwrap();
    assert_eq!(said["failed"], 1);
    // Not asked for again by itself this session, so that it can't hold the rest up.
    assert!(graph_fetch_request(&s, account, None, 10).unwrap().ids.is_empty());
    // Opened, it is.
    assert_eq!(graph_fetch_request(&s, account, Some(m), 1).unwrap().ids, ["M-1"]);
    assert!(s.mailbox.message(m).unwrap().unfetched);
}
