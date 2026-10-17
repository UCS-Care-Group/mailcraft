//! A shared mailbox beside its owner's (#70), from end to end: the engine's `graph_mail_request`
//! and `apply_graph_mail` with `mailcraft_net::graph::sync_mail` in between, against the strict
//! Graph stand-in. Search, unread counts and Favorites take it in here, on this computer; what is
//! changed in it goes up to its own address, and nothing of it reaches the owner's mailbox (`/me`),
//! though both use the same ids. Nothing here leaves this machine;
//! every address and message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::atomic::AtomicBool;

use mailcraft_engine::Session;
use mailcraft_engine::cmd::folder::favorites;
use mailcraft_engine::cmd::graph_mail::{apply_graph_mail, graph_mail_request};
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use serde_json::{Value, json};

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;
/// Addresses of this file's own: the gate that counts a mailbox's requests is the process's.
const OWNER: &str = "lena@example.org";
const SHARED: &str = "frontdesk@example.org";

/// One whole run of `account` against `server`, each piece taken in as it comes.
fn run(server: &StandIn, s: &mut Session, account: Id) {
    let request = graph_mail_request(s, account).unwrap();
    let _routed = server.route();
    let cancel = AtomicBool::new(false);
    mailcraft_net::graph::sync_mail(TOKEN, &request, &cancel, &mut |piece| {
        apply_graph_mail(s, piece).unwrap();
    });
}

/// What the stand-in holds of the owner's own messages, in an order that compares.
fn owners_messages(server: &StandIn) -> Vec<String> {
    let state = server.state();
    let mut all: Vec<String> = state.mailbox.messages.iter().map(|(id, m)| format!("{id} {m:?}")).collect();
    all.sort();
    all
}

/// How many of the shared mailbox's messages the stand-in has as read.
fn theirs_read(server: &StandIn) -> usize {
    server.state().other(SHARED).unwrap().mailbox.messages.values().filter(|m| m.read).count()
}

/// Every request of the last run went to the shared mailbox's own address, none to `/me`.
fn all_at_its_address(server: &StandIn) {
    let state = server.state();
    assert!(!state.requests.is_empty());
    for r in state.requests.iter() {
        let path = r.path.join("/");
        assert!(path == "v1.0/$batch" || path.starts_with(&format!("v1.0/users/{SHARED}/")), "{} {path}", r.method);
        assert!(
            !path.starts_with("v1.0/me") && !path.contains("/me/") && !r.body.contains("\"/me/") && !r.body.contains("\"me/"),
            "{} {path} {}",
            r.method,
            r.body
        );
    }
}

fn accounts_of(s: &Session, ids: &[Id]) -> Vec<Id> {
    let mut out: Vec<Id> = ids.iter().filter_map(|id| s.mailbox.message(*id)).map(|m| m.account).collect();
    out.sort_unstable();
    out
}

#[test]
fn a_shared_mailbox_is_searched_counted_and_a_favourite_here_and_nothing_of_it_reaches_the_owners_mailbox() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    // Both mailboxes name their messages alike: the same ids and subjects, other messages.
    let mine = server.state().well_known("inbox");
    server.state().mailbox.generate(&mine, 3, NOW);
    let theirs = {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        let inbox = other.folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        other.mailbox.generate(&inbox, 3, NOW);
        inbox
    };
    assert_ne!(mine, theirs);

    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    run(&server, &mut s, owner);
    run(&server, &mut s, shared);
    let (my_inbox, their_inbox) = (s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap(), s.mailbox.folder_by_server(shared, &theirs).unwrap());
    assert_eq!((s.mailbox.count(my_inbox), s.mailbox.count(their_inbox)), (3, 3));
    let before = owners_messages(&server);

    // Search: All Mailboxes takes in both; the folder in view is its own mailbox's.
    s.run("view.search", &json!({"query": "message", "scope": "everywhere"})).unwrap();
    let all: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert_eq!(accounts_of(&s, &all), [owner, owner, owner, shared, shared, shared]);
    s.run("folder.select", &json!({"id": their_inbox})).unwrap();
    s.run("view.search", &json!({"query": "message", "scope": "folder"})).unwrap();
    let here: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert_eq!(accounts_of(&s, &here), [shared, shared, shared]);

    // Unread: one read in the shared mailbox here leaves the owner's as they were.
    let one = here.first().copied().unwrap();
    s.run("mail.markRead", &json!({"ids": [one]})).unwrap();
    assert_eq!((s.mailbox.unread_count(my_inbox), s.mailbox.unread_count(their_inbox)), (3, 2));
    let listed = s.run("folder.list", &json!({})).unwrap();
    let unread = |id: Id| listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).map(|a| a["unread"].clone());
    assert_eq!((unread(owner), unread(shared)), (Some(json!(3)), Some(json!(2))));

    // A favourite of the shared mailbox's, kept here.
    s.run("folder.favorite", &json!({"id": their_inbox, "value": true})).unwrap();
    assert!(favorites(&s).contains(&their_inbox));

    // The next runs: the shared mailbox's is at its own address, its read mark with it; the
    // owner's says nothing of the shared mailbox.
    server.state().requests.clear();
    run(&server, &mut s, shared);
    let asked: Vec<(String, String, String)> = server.state().requests.iter().map(|r| (r.method.clone(), r.path.join("/"), r.body.clone())).collect();
    assert!(!asked.is_empty());
    for (method, path, body) in &asked {
        assert!(path == "v1.0/$batch" || path.starts_with(&format!("v1.0/users/{SHARED}/")), "{method} {path}");
        assert!(!path.contains("/me/") && !path.starts_with("v1.0/me"), "{method} {path}");
        assert!(!body.contains("/me/") && !body.contains("\"me/"), "{body}");
    }
    server.state().requests.clear();
    run(&server, &mut s, owner);
    assert!(!server.state().requests.is_empty());
    for r in server.state().requests.iter() {
        let said = format!("{} {} {}", r.target, r.path.join("/"), r.body);
        assert!(!said.to_lowercase().contains(SHARED), "the owner's run names the shared mailbox: {said}");
    }
    assert_eq!(owners_messages(&server), before, "the owner's messages are as they were");
    // The read mark went up to the shared mailbox, and the favourite is still one.
    assert!(s.mailbox.message(one).is_some_and(|m| m.read && m.server.as_ref().is_some_and(|server| server.read)));
    assert_eq!(theirs_read(&server), 1);
    assert!(favorites(&s).contains(&their_inbox));
    let again: Value = s.run("folder.list", &json!({})).unwrap();
    assert_eq!(again["accounts"].as_array().unwrap().iter().find(|a| a["id"] == shared).map(|a| a["sharedOf"].clone()), Some(json!(owner)));
}

#[test]
fn changes_in_a_shared_mailbox_go_up_to_it_alone_delete_goes_to_its_own_deleted_items_and_undo_goes_up_too() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    // Both mailboxes hold the same ids: a change sent to the owner's mailbox would land there.
    let mine = server.state().well_known("inbox");
    let own = server.state().mailbox.generate(&mine, 4, NOW);
    let (their_inbox_id, ids) = {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        let inbox = other.folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        let ids = other.mailbox.generate(&inbox, 4, NOW);
        (inbox, ids)
    };
    assert_eq!(own, ids);
    let their_deleted_id = server.state().other(SHARED).unwrap().folders.iter().find(|f| f.well_known == "deleteditems").unwrap().id.clone();

    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    run(&server, &mut s, owner);
    run(&server, &mut s, shared);
    let before = owners_messages(&server);
    let here = |s: &Session, id: &str| {
        s.mailbox.messages.values().find(|m| m.account == shared && m.server.as_ref().is_some_and(|x| x.id == id)).map(|m| m.id).unwrap()
    };
    let (a, b, c, d) = (here(&s, &ids[0]), here(&s, &ids[1]), here(&s, &ids[2]), here(&s, &ids[3]));
    let their_inbox = s.mailbox.folder_by_server(shared, &their_inbox_id).unwrap();
    let their_deleted = s.mailbox.folder_of_kind(shared, FolderKind::Deleted).unwrap();
    assert_eq!(s.mailbox.folder(their_deleted).and_then(|f| f.server.as_ref()).map(|x| x.id.clone()), Some(their_deleted_id.clone()));

    // Read, flag, a category; a folder made in it and a message moved into it; a
    // copy within it; a delete (to its own Deleted Items); a delete for good.
    s.run("mail.markRead", &json!({"ids": [a]})).unwrap();
    s.run("mail.flag", &json!({"id": a})).unwrap();
    s.run("mail.categorize", &json!({"ids": [a], "category": "Blue category"})).unwrap();
    let ours = s.run("folder.new", &json!({"parent": their_inbox, "name": "Ours"})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.move", &json!({"ids": [b], "folder": ours})).unwrap();
    s.run("mail.copy", &json!({"ids": [a], "folder": ours})).unwrap();
    s.run("mail.deletePermanently", &json!({"ids": [d]})).unwrap();
    s.run("mail.delete", &json!({"ids": [c]})).unwrap();
    assert_eq!(s.mailbox.message(c).map(|m| m.folder), Some(their_deleted), "its own Deleted Items here");
    server.state().requests.clear();
    run(&server, &mut s, shared);
    all_at_its_address(&server);
    // A second run sends nothing more: everything was taken.
    assert_eq!(mailcraft_engine::cmd::graph_mail::unpushed(&s, shared), 0);
    {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        let made = other.folders.iter().find(|f| f.name == "Ours").map(|f| f.id.clone()).expect("the folder is made in the shared mailbox");
        let m = &other.mailbox.messages;
        let first = m.get(&ids[0]).unwrap();
        assert!(first.read && first.flag == "flagged" && first.categories.iter().any(|c| c == "Blue category"), "{first:?}");
        assert_eq!(m.get(&ids[1]).map(|x| x.folder.clone()), Some(made.clone()));
        assert_eq!(m.values().filter(|x| x.folder == made).count(), 2, "the moved one and the copy");
        assert_eq!(m.get(&ids[2]).map(|x| x.folder.clone()), Some(their_deleted_id.clone()), "in its own Deleted Items");
        assert!(!m.contains_key(&ids[3]), "deleted for good");
    }
    assert_eq!(owners_messages(&server), before, "the owner's messages are as they were");

    // Undo the delete, which reached the server: it goes back with the next run.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(c).map(|m| m.folder), Some(their_inbox));
    server.state().requests.clear();
    run(&server, &mut s, shared);
    all_at_its_address(&server);
    assert_eq!(server.state().other(SHARED).unwrap().mailbox.messages.get(&ids[2]).map(|x| x.folder.clone()), Some(their_inbox_id.clone()));
    assert_eq!(owners_messages(&server), before, "the owner's messages are still as they were");

    // Between the owner's mailbox and the shared one: refused, and nothing changes anywhere.
    let my_inbox = s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    let e = s.run("mail.move", &json!({"ids": [a], "folder": my_inbox})).unwrap_err().to_string();
    assert!(e.contains("only within one mailbox"), "{e}");
    assert_eq!(s.mailbox.message(a).map(|m| m.account), Some(shared));

    // Access taken away: a change waits, counted, and the run says so for Send/Receive Progress.
    s.run("mail.markUnread", &json!({"ids": [a]})).unwrap();
    server.state().other(SHARED).unwrap().mailbox.faults.insert(ids[0].clone(), mailcraft_net::graph::standin::Fault::Denied);
    run(&server, &mut s, shared);
    assert!(mailcraft_engine::cmd::graph_mail::unpushed(&s, shared) >= 1, "the change still waits");
    assert!(s.mailbox.message(a).is_some_and(|m| !m.read), "and is not put back");
    let progress = s.run("sync.progress", &json!({})).unwrap();
    let row = progress["accounts"].as_array().unwrap().iter().find(|r| r["id"] == shared).cloned().unwrap();
    assert!(row["problems"].as_array().unwrap().iter().any(|p| p.as_str().is_some_and(|p| p.contains("Full Access"))), "{row}");
    assert!(server.state().other(SHARED).unwrap().mailbox.messages.get(&ids[0]).is_some_and(|m| m.read), "nothing changed there");
    assert_eq!(owners_messages(&server), before);
}

/// The messages of the owner's own folder `well_known` (`drafts`, `sentitems`) on the stand-in.
fn owners_in(server: &StandIn, well_known: &str) -> Vec<String> {
    let state = server.state();
    let folder = state.well_known(well_known);
    let mut all: Vec<String> = state.mailbox.messages.iter().filter(|(_, m)| m.folder == folder).map(|(id, m)| format!("{id} {m:?}")).collect();
    all.sort();
    all
}

/// The ids the stand-in has in the shared mailbox's folder `well_known`.
fn theirs_in(server: &StandIn, well_known: &str) -> Vec<String> {
    let mut state = server.state();
    let other = state.other(SHARED).unwrap();
    let folder = other.folders.iter().find(|f| f.well_known == well_known).unwrap().id.clone();
    let mut all: Vec<String> = other.mailbox.messages.iter().filter(|(_, m)| m.folder == folder).map(|(id, _)| id.clone()).collect();
    all.sort();
    all
}

/// A session with the owner and the shared mailbox, both read once, against `server`.
fn owner_and_shared(server: &StandIn) -> (Session, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    run(server, &mut s, owner);
    run(server, &mut s, shared);
    (s, owner, shared)
}

#[test]
fn mail_written_in_a_shared_mailbox_is_kept_in_its_drafts_and_sent_through_it_and_the_owners_drafts_and_sent_items_stay_as_they_were() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    // Both mailboxes have the same ids: three in the Inbox each, and the owner three more in his
    // Drafts and three in his Sent Items, under the ids the shared mailbox's next ones will have.
    // A draft made or sent at `/me` would land on (or beside) his.
    let mine = server.state().well_known("inbox");
    let own = server.state().mailbox.generate(&mine, 3, NOW);
    let my_drafts = server.state().well_known("drafts");
    let my_sent = server.state().well_known("sentitems");
    let mut owners_ids = server.state().mailbox.generate(&my_drafts, 3, NOW);
    owners_ids.extend(server.state().mailbox.generate(&my_sent, 3, NOW));
    let ids = {
        let mut state = server.state();
        let other = state.other(SHARED).unwrap();
        let inbox = other.folders.iter().find(|f| f.well_known == "inbox").unwrap().id.clone();
        other.mailbox.generate(&inbox, 3, NOW)
    };
    assert_eq!(own, ids);
    let (mut s, owner, shared) = owner_and_shared(&server);
    let before = owners_messages(&server);
    let (drafts_before, sent_before) = (owners_in(&server, "drafts"), owners_in(&server, "sentitems"));
    assert_eq!((drafts_before.len(), sent_before.len()), (3, 3));
    let here_in = |s: &Session, account: Id, kind: FolderKind| s.mailbox.folder_of_kind(account, kind).map_or(0, |f| s.mailbox.count(f));
    let (my_drafts_here, my_sent_here) = (here_in(&s, owner, FolderKind::Drafts), here_in(&s, owner, FolderKind::Sent));

    // A reply to one of its messages, saved: made there as an answer, in its own Drafts.
    let first = s.mailbox.messages.values().find(|m| m.account == shared && m.server.as_ref().is_some_and(|x| x.id == ids[0])).unwrap().id;
    s.run("mail.select", &json!({"id": first})).unwrap();
    s.run("mail.reply", &json!({})).unwrap();
    s.run("compose.set", &json!({"to": "kit@example.net"})).unwrap();
    let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
    server.state().requests.clear();
    run(&server, &mut s, shared);
    all_at_its_address(&server);
    let made = theirs_in(&server, "drafts");
    assert_eq!(made.len(), 1, "in its own Drafts");
    assert!(made.first().is_some_and(|id| owners_ids.contains(id)), "under an id the owner's mailbox has too: {made:?}");
    assert!(server.state().requests.iter().any(|r| r.path.last().is_some_and(|p| p == "createReply")));
    let kept = s.mailbox.message(draft).unwrap();
    assert_eq!((kept.account, kept.from.email.as_str()), (shared, SHARED));
    assert_eq!(kept.server.as_ref().map(|x| x.id.clone()), made.first().cloned());

    // Sent as that draft, from there: it leaves its Drafts, the copy is in its Sent Items.
    s.run("mail.editDraft", &json!({"id": draft})).unwrap();
    s.run("compose.set", &json!({"subject": "RE: Message 0, answered"})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    // And a message never saved, begun in one of its folders.
    s.run("folder.select", &json!({"id": s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap()})).unwrap();
    s.run("mail.new", &json!({"to": "kit@example.net", "subject": "Opening hours", "body": "Nine to five."})).unwrap();
    s.run("compose.send", &json!({})).unwrap();
    assert_eq!(here_in(&s, shared, FolderKind::Outbox), 2);
    server.state().requests.clear();
    run(&server, &mut s, shared);
    all_at_its_address(&server);
    let sent: Vec<(String, String, String)> = server.state().sent.iter().map(|x| (x.subject.clone(), x.how.clone(), x.mailbox.clone())).collect();
    assert_eq!(
        sent,
        [
            ("RE: Message 0, answered".to_string(), "send".to_string(), format!("users/{SHARED}")),
            ("Opening hours".to_string(), "sendMail".to_string(), format!("users/{SHARED}")),
        ]
    );
    assert!(theirs_in(&server, "drafts").is_empty(), "the draft left its Drafts");
    let their_sent = theirs_in(&server, "sentitems");
    assert_eq!(their_sent.len(), 2, "both copies in its own Sent Items");
    assert!(their_sent.iter().any(|id| owners_ids.contains(id)), "under ids the owner's mailbox has too: {their_sent:?}");
    assert_eq!(here_in(&s, shared, FolderKind::Outbox), 0);
    // Its copies come down into its own Sent Items here.
    run(&server, &mut s, shared);
    assert_eq!(here_in(&s, shared, FolderKind::Sent), 2);

    // The owner's Drafts and Sent Items are as they were, there and here, and his own run says
    // nothing of any of it.
    assert_eq!((owners_in(&server, "drafts"), owners_in(&server, "sentitems")), (drafts_before, sent_before));
    assert_eq!(owners_messages(&server), before, "the owner's messages are as they were");
    server.state().requests.clear();
    run(&server, &mut s, owner);
    for r in server.state().requests.iter() {
        let said = format!("{} {} {}", r.target, r.path.join("/"), r.body);
        assert!(!said.to_lowercase().contains(SHARED) && !said.contains("Opening hours"), "the owner's run: {said}");
    }
    assert_eq!((here_in(&s, owner, FolderKind::Drafts), here_in(&s, owner, FolderKind::Sent)), (my_drafts_here, my_sent_here));
    assert_eq!(owners_messages(&server), before);
}

#[test]
fn a_send_exchange_wont_let_go_as_a_shared_mailbox_is_held_with_its_address_and_never_sent_from_the_owners() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    server.state().other(SHARED).unwrap().send_as_denied = true;
    let (mut s, owner, shared) = owner_and_shared(&server);
    let before = owners_messages(&server);
    s.run("folder.select", &json!({"id": s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap()})).unwrap();
    s.run("mail.new", &json!({"to": "kit@example.net", "subject": "Denied", "body": "Hello."})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    server.state().requests.clear();
    run(&server, &mut s, shared);
    all_at_its_address(&server);
    let m = s.mailbox.message(id).unwrap();
    assert!(m.held.contains(&format!("you don't have permission to send as {SHARED}")), "{}", m.held);
    assert!(server.state().sent.is_empty(), "nothing went, from any address");
    // Not sent again by itself, by it or by its owner.
    run(&server, &mut s, shared);
    run(&server, &mut s, owner);
    assert!(server.state().sent.is_empty());
    assert_eq!(owners_messages(&server), before);
    // Given the permission, and sent again: it goes, through its own address.
    server.state().other(SHARED).unwrap().send_as_denied = false;
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    run(&server, &mut s, shared);
    let sent: Vec<String> = server.state().sent.iter().map(|x| x.mailbox.clone()).collect();
    assert_eq!(sent, [format!("users/{SHARED}")]);
    assert_eq!(owners_messages(&server), before);
}
