//! Shared mailboxes through Microsoft Graph (#70): opened under a Microsoft account, read with
//! its sign-in at their own address, and kept apart from it. (Against the Graph stand-in:
//! `crates/net/src/graph/mail/shared_tests.rs`.)

use mailcraft_model::sync::{
    FolderChange, FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessageChange, MessageChangeDone, MessageOutcome, MessagePage,
    ServerMessage,
};
use mailcraft_model::{AccountKind, FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::cmd::graph_mail::{apply_graph_mail, graph_fetch_request, graph_mail_request};

const OWNER: &str = "owner@example.org";
const SHARED: &str = "support@example.org";

/// A session with a Microsoft account kept in step through Graph, and a local one; their ids.
fn session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    s.shared_mailboxes = true;
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    (s, local, owner)
}

fn add(s: &mut Session, params: serde_json::Value) -> Result<Id, String> {
    s.run("account.addSharedMailbox", &params).map(|v| v["id"].as_u64().unwrap_or(0)).map_err(|e| e.to_string())
}

/// What a mailbox's server lists: the Inbox and the Deleted Items, by ids that every mailbox of
/// these tests uses alike.
fn listing(account: Id, email: &str) -> GraphMailResult {
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("in", "Inbox", FolderKind::Inbox), folder("de", "Deleted Items", FolderKind::Deleted)];
    GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() }
}

fn page(account: Id, email: &str, ids: &[&str]) -> GraphMailResult {
    let messages = ids
        .iter()
        .map(|id| GraphMessage {
            server: ServerMessage { id: (*id).into(), folder: "in".into(), ..Default::default() },
            subject: format!("{email}: {id}"),
            ..Default::default()
        })
        .collect();
    let page = MessagePage {
        folder: "in".into(),
        messages,
        link: "https://graph.example.org/delta?$deltatoken=1".into(),
        caught_up: true,
        ..Default::default()
    };
    GraphMailResult { account, email: email.into(), pages: vec![page], whole: true, ..Default::default() }
}

#[test]
fn a_shared_mailbox_is_added_under_a_microsoft_account_and_removed_again() {
    let (mut s, local, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    let a = s.mailbox.account(shared).unwrap();
    assert_eq!((a.email.as_str(), a.kind, a.shared_of), (SHARED, AccountKind::Microsoft, Some(owner)));
    let listed = s.run("account.list", &json!({})).unwrap();
    assert!(listed["accounts"].as_array().unwrap().iter().any(|a| a["sharedOf"] == json!(owner) && a["email"] == SHARED));
    // Refused: under an account that isn't a Microsoft one, twice, an address that isn't one.
    assert!(add(&mut s, json!({"account": local, "address": "sales@example.org"})).unwrap_err().contains("Microsoft Graph"));
    assert!(add(&mut s, json!({"address": "Support@Example.org"})).unwrap_err().contains("already here"));
    assert!(add(&mut s, json!({"address": "support@example.org/../../me"})).unwrap_err().contains("isn't an e-mail address"));
    assert!(add(&mut s, json!({"account": shared, "address": "sales@example.org"})).unwrap_err().contains("can't open another"));
    assert!(add(&mut s, json!({})).is_err());
    // Removed by its address, or its id; an account that isn't one isn't removed this way.
    assert!(s.run("account.removeSharedMailbox", &json!({"id": owner})).is_err());
    assert!(s.run("account.removeSharedMailbox", &json!({"address": "nobody@example.org"})).is_err());
    s.run("account.removeSharedMailbox", &json!({"address": "SUPPORT@example.org"})).unwrap();
    assert!(s.mailbox.account(shared).is_none() && s.mailbox.folders.iter().all(|f| f.account != shared));
    let again = add(&mut s, json!({"account": owner, "address": SHARED})).unwrap();
    s.run("account.removeSharedMailbox", &json!({"id": again})).unwrap();
    assert!(s.run("account.removeSharedMailbox", &json!({"id": again})).is_err());
}

#[test]
fn the_last_account_is_not_removed_with_its_shared_mailboxes() {
    let mut s = Session::empty();
    s.shared_mailboxes = true;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    assert!(s.run("account.remove", &json!({"id": owner})).unwrap_err().to_string().contains("last account"));
    assert!(s.mailbox.account(shared).is_some());
    s.run("account.add", &json!({"email": "ada@example.org"})).unwrap();
    s.run("account.remove", &json!({"id": owner})).unwrap();
    assert!(s.mailbox.account(shared).is_none(), "it goes with its owner");
    assert!(s.mailbox.folder(s.view.folder).is_some(), "the folder on screen is one that is there");
}

#[test]
fn a_shared_mailbox_is_read_at_its_own_address_with_its_owners_sign_in() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    let mine = graph_mail_request(&s, owner).unwrap();
    assert_eq!((mine.email.as_str(), mine.shared), (OWNER, false));
    let theirs = graph_mail_request(&s, shared).unwrap();
    assert_eq!((theirs.account, theirs.email.as_str(), theirs.shared), (shared, SHARED, true));
    let fetch = graph_fetch_request(&s, shared, None, 10).unwrap();
    assert_eq!((fetch.email.as_str(), fetch.shared), (SHARED, true));
    // Its Graph request by command says so too.
    let asked = s.run("sync.graphRequest", &json!({"account": shared})).unwrap();
    assert_eq!((asked["email"].as_str(), asked["shared"].as_bool()), (Some(SHARED), Some(true)));
    // An answer for the owner's mailbox is not taken in by the shared one, nor the other way.
    assert!(apply_graph_mail(&mut s, listing(shared, OWNER)).is_err());
    assert!(apply_graph_mail(&mut s, listing(owner, SHARED)).is_err());
    // A shared mailbox whose owner is gone (a file may say so) isn't read at all.
    if let Ok(a) = s.mailbox.account_mut(shared) {
        a.shared_of = Some(9_999);
    }
    assert!(graph_mail_request(&s, shared).unwrap_err().to_string().contains("account is gone"));
}

#[test]
fn the_same_server_ids_in_two_mailboxes_are_two_messages() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    apply_graph_mail(&mut s, listing(owner, OWNER)).unwrap();
    apply_graph_mail(&mut s, listing(shared, SHARED)).unwrap();
    let (mine, theirs) = (s.mailbox.folder_by_server(owner, "in").unwrap(), s.mailbox.folder_by_server(shared, "in").unwrap());
    assert_ne!(mine, theirs);
    // Each mailbox names its messages by itself: the same ids, other messages.
    for _ in 0..2 {
        apply_graph_mail(&mut s, page(owner, OWNER, &["AAkA1", "AAkA2"])).unwrap();
        apply_graph_mail(&mut s, page(shared, SHARED, &["AAkA1", "AAkA2", "AAkA3"])).unwrap();
    }
    assert_eq!((s.mailbox.count(mine), s.mailbox.count(theirs)), (2, 3), "and a page that came twice changes nothing");
    assert!(s.mailbox.messages_in(theirs).all(|m| m.account == shared && m.subject.starts_with(SHARED)));
    assert!(s.mailbox.messages_in(mine).all(|m| m.account == owner && m.subject.starts_with(OWNER)));
}

#[test]
fn mail_written_in_a_shared_mailbox_is_from_it_kept_in_its_drafts_and_sent_through_it() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    s.view.folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    // New mail begun in one of its folders is from it, and kept in its own Drafts.
    s.run("mail.new", &json!({"to": "bob@example.org", "subject": "Hello"})).unwrap();
    assert_eq!(s.compose.as_ref().map(|c| c.account), Some(shared));
    s.run("compose.saveDraft", &json!({})).unwrap();
    let draft = s.mailbox.messages.values().find(|m| m.subject == "Hello").cloned().unwrap();
    assert_eq!((draft.account, draft.from.email.as_str()), (shared, SHARED));
    assert_eq!(s.mailbox.folder(draft.folder).map(|f| (f.account, f.kind)), Some((shared, FolderKind::Drafts)));
    assert!(draft.server_draft.is_some(), "a draft its server is to have");
    // It goes up in the shared mailbox's own request, and never in its owner's.
    let theirs = graph_mail_request(&s, shared).unwrap();
    assert!(theirs.shared && theirs.drafts.len() == 1 && theirs.drafts[0].message == draft.id, "{theirs:?}");
    let own = graph_mail_request(&s, owner).unwrap();
    assert!(own.drafts.is_empty() && own.sends.is_empty(), "{own:?}");
    // Sent: in its Outbox, from its address, going with its own run.
    s.run("compose.send", &json!({})).unwrap();
    let outbox = s.mailbox.folder_of_kind(shared, FolderKind::Outbox).unwrap();
    let sent: Vec<_> = s.mailbox.messages_in(outbox).map(|m| (m.subject.clone(), m.from.email.clone())).collect();
    assert_eq!(sent, [("Hello".to_string(), SHARED.to_string())]);
    let theirs = graph_mail_request(&s, shared).unwrap();
    assert_eq!(theirs.sends.len(), 1, "{theirs:?}");
    assert!(graph_mail_request(&s, owner).unwrap().sends.is_empty());
    // The From picker can change it: then it is the owner's, in his own Drafts.
    s.run("mail.new", &json!({"to": "bob@example.org", "subject": "Mine"})).unwrap();
    s.run("compose.set", &json!({"account": owner})).unwrap();
    s.run("compose.saveDraft", &json!({})).unwrap();
    let mine = s.mailbox.messages.values().find(|m| m.subject == "Mine").cloned().unwrap();
    assert_eq!((mine.account, mine.from.email.as_str()), (owner, OWNER));
    assert_eq!(s.mailbox.folder(mine.folder).map(|f| (f.account, f.kind)), Some((owner, FolderKind::Drafts)));
}

#[test]
fn a_reply_to_mail_in_a_shared_mailbox_is_from_it_and_made_there_as_an_answer() {
    let (mut s, owner, shared) = both();
    let theirs = theirs(&s, shared);
    s.run("mail.select", &json!({"id": theirs[0]})).unwrap();
    s.run("mail.reply", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().map(|c| c.account), Some(shared));
    s.run("compose.set", &json!({"to": "bob@example.org"})).unwrap();
    s.run("compose.saveDraft", &json!({})).unwrap();
    let asked = graph_mail_request(&s, shared).unwrap();
    let draft = asked.drafts.first().cloned().unwrap();
    // Made as an answer of the message in the shared mailbox, by its own id there.
    assert_eq!(draft.of.as_ref().map(|of| of.id.as_str()), Some("AAkA1"));
    assert!(graph_mail_request(&s, owner).unwrap().drafts.is_empty());
    assert_eq!(s.mailbox.message(draft.message).map(|m| m.from.email.clone()), Some(SHARED.to_string()));
}

#[test]
fn a_shared_mailbox_has_signatures_of_its_own_and_none_by_default() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    s.run("signature.set", &json!({"name": "Mine", "text": "Lena"})).unwrap();
    s.run("signature.set", &json!({"name": "Desk", "text": "The front desk"})).unwrap();
    s.run("signature.defaults", &json!({"account": owner, "new": "Mine", "reply": "Mine"})).unwrap();
    let a = s.mailbox.account(shared).unwrap();
    assert_eq!((a.signature.as_str(), a.reply_signature()), ("", ""), "none until the owner chooses one");
    s.view.folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    s.run("mail.new", &json!({})).unwrap();
    assert!(!s.compose.as_ref().unwrap().body.plain().contains("Lena"), "not its owner's");
    s.run("compose.discard", &json!({})).unwrap();
    // Begun from the owner and then from it in the From picker: his signature goes with him.
    s.run("mail.new", &json!({"account": owner})).unwrap();
    assert!(s.compose.as_ref().unwrap().body.plain().contains("Lena"));
    s.run("compose.set", &json!({"account": shared})).unwrap();
    assert!(!s.compose.as_ref().unwrap().body.plain().contains("Lena"));
    s.run("compose.discard", &json!({})).unwrap();
    // Chosen for it, as for an account.
    s.run("signature.defaults", &json!({"account": shared, "new": "Desk"})).unwrap();
    s.run("mail.new", &json!({})).unwrap();
    let body = s.compose.as_ref().unwrap().body.plain();
    assert!(body.contains("The front desk") && !body.contains("Lena"), "{body:?}");
    // The owner's are as they were.
    let o = s.mailbox.account(owner).unwrap();
    assert_eq!((o.signature.as_str(), o.reply_signature()), ("Mine", "Mine"));
}

#[test]
fn a_send_exchange_wont_let_go_as_a_shared_mailbox_is_held_and_says_its_address() {
    use mailcraft_model::sync::{SEND_AS_DENIED, SendDone, SendOutcome};
    let (mut s, owner, shared) = both();
    s.view.folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    s.run("mail.new", &json!({"to": "bob@example.org", "subject": "Hello"})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let mut result = GraphMailResult { account: shared, email: SHARED.into(), ..Default::default() };
    result.sent = vec![SendDone { message: id, outcome: SendOutcome::Held { lost: false, why: SEND_AS_DENIED.into() }, draft: None }];
    apply_graph_mail(&mut s, result).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.held.contains(&format!("permission to send as {SHARED}")) && m.held.contains("Nothing was sent from another address"), "{}", m.held);
    assert_eq!(s.mailbox.folder(m.folder).map(|f| f.account), Some(shared), "still in its own Outbox");
    // Not sent again by itself, and never from the owner.
    assert!(graph_mail_request(&s, shared).unwrap().sends.is_empty());
    assert!(graph_mail_request(&s, owner).unwrap().sends.is_empty());
    // What may go into the log says no address.
    assert!(!s.sync_status.contains(SHARED), "{}", s.sync_status);
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    assert_eq!(graph_mail_request(&s, shared).unwrap().sends.len(), 1);
}

#[test]
fn mail_waiting_in_a_shared_mailbox_is_held_with_why_when_sending_from_it_isnt_allowed() {
    let (mut s, owner, shared) = both();
    s.view.folder = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    s.run("mail.new", &json!({"to": "bob@example.org", "subject": "One"})).unwrap();
    let one = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    for (why, says) in [("declined", "wasn't allowed"), ("approval", "administrator has to approve"), ("organisation", "doesn't let")] {
        let held = s.run("sync.holdSends", &json!({"account": shared, "why": why})).unwrap();
        assert_eq!(held["held"], json!([one]), "{why}");
        let m = s.mailbox.message(one).unwrap();
        assert!(m.held.starts_with("It wasn't sent") && m.held.contains(says), "{why}: {}", m.held);
        assert!(graph_mail_request(&s, shared).unwrap().sends.is_empty());
        s.run("mail.sendAgain", &json!({"id": one})).unwrap();
    }
    // Only a shared mailbox's, and only for the reasons it knows.
    assert!(s.run("sync.holdSends", &json!({"account": owner, "why": "declined"})).is_err());
    assert!(s.run("sync.holdSends", &json!({"account": shared, "why": "because"})).is_err());
    assert!(s.run("sync.holdSends", &json!({"why": "declined"})).is_err());
    assert!(s.mailbox.message(one).is_some_and(|m| m.held.is_empty()));
}

#[test]
fn a_shared_mailbox_keeps_its_address_and_its_owners_sign_in() {
    let (mut s, _, _) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    let moved = s.run("account.edit", &json!({"id": shared, "email": "other@example.org"})).unwrap_err();
    assert!(moved.to_string().contains("can't be changed"), "{moved}");
    s.run("account.edit", &json!({"id": shared, "name": "Support", "kind": "imap", "incoming": {"host": "imap.example.org"}})).ok();
    let a = s.mailbox.account(shared).unwrap();
    assert_eq!((a.kind, a.incoming.is_none(), a.oauth.is_none(), a.shared_of.is_some()), (AccountKind::Microsoft, true, true, true));
}

/// A shared mailbox and its owner's, each with its Inbox and Deleted Items and the same ids
/// ("AAkA1"…, "in", "de"): what goes up for one must never name the other's.
fn both() -> (Session, Id, Id) {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    apply_graph_mail(&mut s, listing(shared, SHARED)).unwrap();
    apply_graph_mail(&mut s, page(shared, SHARED, &["AAkA1", "AAkA2"])).unwrap();
    apply_graph_mail(&mut s, listing(owner, OWNER)).unwrap();
    apply_graph_mail(&mut s, page(owner, OWNER, &["AAkA1"])).unwrap();
    (s, owner, shared)
}

fn theirs(s: &Session, shared: Id) -> Vec<Id> {
    let mut all: Vec<Id> = s.mailbox.messages.values().filter(|m| m.account == shared).map(|m| m.id).collect();
    all.sort_unstable();
    all
}

#[test]
fn what_is_changed_in_a_shared_mailbox_goes_up_in_its_own_request_and_never_in_its_owners() {
    let (mut s, owner, shared) = both();
    let theirs = theirs(&s, shared);
    s.run("mail.markRead", &json!({"ids": theirs})).unwrap();
    s.run("mail.flag", &json!({"id": theirs[0]})).unwrap();
    let inbox = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    s.run("folder.new", &json!({"parent": inbox, "name": "Ours"})).unwrap();
    s.view.folder = inbox;
    let asked = graph_mail_request(&s, shared).unwrap();
    assert!(asked.shared && asked.email == SHARED);
    assert_eq!(asked.messages.len(), 2, "{asked:?}");
    assert!(asked.messages.iter().all(|c| matches!(c, MessageChange::Update { read: Some(true), .. })));
    assert!(matches!(asked.changes.as_slice(), [FolderChange::Create { parent, .. }] if parent == "in"), "{:?}", asked.changes);
    // No drafts and nothing to send were written in it.
    assert!(asked.drafts.is_empty() && asked.sends.is_empty());
    // The owner's own request has none of them.
    let own = graph_mail_request(&s, owner).unwrap();
    assert!(!own.shared && own.messages.is_empty() && own.changes.is_empty(), "{own:?}");
}

#[test]
fn delete_in_a_shared_mailbox_goes_to_its_own_deleted_items_and_undo_is_another_change() {
    let (mut s, owner, shared) = both();
    let one = theirs(&s, shared)[0];
    let their_deleted = s.mailbox.folder_of_kind(shared, FolderKind::Deleted).unwrap();
    let my_deleted = s.mailbox.folder_of_kind(owner, FolderKind::Deleted).unwrap();
    s.run("mail.delete", &json!({"ids": [one]})).unwrap();
    assert_eq!(s.mailbox.message(one).map(|m| (m.folder, m.account)), Some((their_deleted, shared)));
    assert_eq!(s.mailbox.messages_in(my_deleted).count(), 0);
    let asked = graph_mail_request(&s, shared).unwrap();
    let [change @ MessageChange::Move { to, .. }] = asked.messages.as_slice() else { panic!("{:?}", asked.messages) };
    assert_eq!(to, "de", "its own Deleted Items, by its own id");
    assert!(graph_mail_request(&s, owner).unwrap().messages.is_empty());
    // It reached the server; undo moves it back with the next run.
    let outcome = MessageOutcome::Done { categories: None, flag: None, folder: "de".into(), id: String::new() };
    let done = GraphMailResult {
        account: shared,
        email: SHARED.into(),
        messages: vec![MessageChangeDone { change: change.clone(), outcome, there: None }],
        ..Default::default()
    };
    apply_graph_mail(&mut s, done).unwrap();
    assert!(graph_mail_request(&s, shared).unwrap().messages.is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    let inbox = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    assert_eq!(s.mailbox.message(one).map(|m| m.folder), Some(inbox));
    let back = graph_mail_request(&s, shared).unwrap().messages;
    assert!(matches!(back.as_slice(), [MessageChange::Move { to, .. }] if to == "in"), "{back:?}");
    assert!(graph_mail_request(&s, owner).unwrap().messages.is_empty());

    // Deleted for good: the delete goes to its server, by its own id.
    s.run("mail.deletePermanently", &json!({"ids": [one]})).unwrap();
    let gone = graph_mail_request(&s, shared).unwrap().messages;
    assert!(gone.iter().any(|c| matches!(c, MessageChange::Delete { id } if id == "AAkA1")), "{gone:?}");
    assert!(graph_mail_request(&s, owner).unwrap().messages.is_empty());
}

#[test]
fn mail_doesnt_move_or_copy_between_a_shared_mailbox_and_another_mailbox() {
    let (mut s, owner, shared) = both();
    let one = theirs(&s, shared)[0];
    let mine = s.mailbox.messages.values().find(|m| m.account == owner).map(|m| m.id).unwrap();
    let (my_inbox, their_inbox) =
        (s.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap());
    let before = s.mailbox.clone();
    for (ids, folder) in [(one, my_inbox), (mine, their_inbox)] {
        for command in ["mail.move", "mail.copy"] {
            let e = s.run(command, &json!({"ids": [ids], "folder": folder})).unwrap_err().to_string();
            assert!(e.contains("only within one mailbox"), "{command}: {e}");
        }
    }
    // Between two shared mailboxes too.
    let other = add(&mut s, json!({"address": "sales@example.org"})).unwrap();
    apply_graph_mail(&mut s, listing(other, "sales@example.org")).unwrap();
    let their_other = s.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    let e = s.run("mail.move", &json!({"ids": [one], "folder": their_other})).unwrap_err().to_string();
    assert!(e.contains("only within one mailbox"), "{e}");
    assert_eq!(s.mailbox.message(one), before.message(one));
    assert_eq!(s.mailbox.message(mine), before.message(mine));
    assert!(graph_mail_request(&s, shared).unwrap().messages.is_empty() && graph_mail_request(&s, owner).unwrap().messages.is_empty());
}

#[test]
fn moved_out_of_a_shared_mailbox_its_copy_there_goes_once_it_is_safe_here() {
    let (mut s, owner, shared) = both();
    let one = theirs(&s, shared)[0];
    let local = s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Local).map(|a| a.id).unwrap();
    let kept = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [one], "folder": kept})).unwrap();
    let m = s.mailbox.message(one).unwrap();
    assert_eq!((m.account, m.moving_out.as_ref().map(|o| (o.account, o.id.as_str()))), (local, Some((shared, "AAkA1"))));
    // Not fetched whole yet: nothing is removed there.
    s.mailbox.message_mut(one).unwrap().unfetched = true;
    assert!(graph_mail_request(&s, shared).unwrap().messages.is_empty());
    // Whole and saved: the shared mailbox's copy goes, in its own request alone.
    s.mailbox.message_mut(one).unwrap().unfetched = false;
    s.note_saved();
    let asked = graph_mail_request(&s, shared).unwrap().messages;
    assert!(matches!(asked.as_slice(), [MessageChange::MovedOut { id, .. }] if id == "AAkA1"), "{asked:?}");
    assert!(graph_mail_request(&s, owner).unwrap().messages.is_empty());
}

#[test]
fn shared_mailboxes_are_off_until_they_are_switched_on_for_testing() {
    let (mut s, _, _) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    s.shared_mailboxes = false;
    assert!(add(&mut s, json!({"address": "sales@example.org"})).unwrap_err().contains("MAILCRAFT_SHARED_MAILBOXES"));
    assert!(graph_mail_request(&s, shared).is_err() && graph_fetch_request(&s, shared, None, 10).is_err());
    // What was opened while they were on can still be closed.
    s.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
}

#[test]
fn a_shared_mailbox_keeps_as_much_mail_whole_as_its_owners_account() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    apply_graph_mail(&mut s, listing(shared, SHARED)).unwrap();
    apply_graph_mail(&mut s, page(shared, SHARED, &["AAkA1"])).unwrap();
    // Each message is 30 MB: over the owner's 10 MB, under its Everything.
    let theirs: Vec<Id> = s.mailbox.messages.values().filter(|m| m.account == shared).map(|m| m.id).collect();
    for id in theirs {
        if let Some(server) = s.mailbox.message_mut(id).ok().and_then(|m| m.server.as_mut()) {
            server.size = 30 << 20;
        }
    }
    // (A file may say anything of the shared mailbox's own: it isn't what counts.)
    if let Ok(a) = s.mailbox.account_mut(shared) {
        a.download_in_full = mailcraft_model::DownloadInFull::Everything;
    }
    assert!(graph_fetch_request(&s, shared, None, 10).unwrap().ids.is_empty());
    if let Ok(a) = s.mailbox.account_mut(owner) {
        a.download_in_full = mailcraft_model::DownloadInFull::Everything;
    }
    if let Ok(a) = s.mailbox.account_mut(shared) {
        a.download_in_full = mailcraft_model::DownloadInFull::UpTo10Mb;
    }
    assert_eq!(graph_fetch_request(&s, shared, None, 10).unwrap().ids, ["AAkA1"]);
}

#[test]
fn a_shared_mailbox_and_its_link_to_its_owner_survive_a_save_and_a_reopen() {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    apply_graph_mail(&mut s, listing(shared, SHARED)).unwrap();
    apply_graph_mail(&mut s, page(shared, SHARED, &["AAkA1"])).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-shared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shared.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    // Saved again after a change, as the log of records has it, then opened afresh.
    crate::store::save(&path, &s.mailbox).unwrap();
    let opened = crate::store::open(&path).unwrap().mailbox;
    assert_eq!(opened.account(shared).and_then(|a| a.shared_of), Some(owner));
    assert_eq!(opened.owner_of(shared).map(|o| o.id), Some(owner));
    assert!(opened.folder_by_server(shared, "in").is_some());
    assert_eq!(opened.messages.values().filter(|m| m.account == shared).count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- where it shows: search, unread counts, Favorites (#70, second slice) ----

/// Messages in the folder `folder` (by its server id) of a mailbox, all with the one subject, the
/// first `unread` of them unread.
fn mail_in(account: Id, email: &str, folder: &str, ids: &[&str], subject: &str, unread: usize) -> GraphMailResult {
    let messages = ids
        .iter()
        .enumerate()
        .map(|(i, id)| GraphMessage {
            server: ServerMessage { id: (*id).into(), folder: folder.into(), read: i >= unread, ..Default::default() },
            subject: subject.into(),
            sent: 1_791_900_000 + i as i64,
            received: 1_791_900_000 + i as i64,
            ..Default::default()
        })
        .collect();
    let page = MessagePage {
        folder: folder.into(),
        messages,
        link: format!("https://graph.example.org/delta?$deltatoken={folder}"),
        caught_up: true,
        ..Default::default()
    };
    GraphMailResult { account, email: email.into(), pages: vec![page], whole: true, ..Default::default() }
}

/// An owner and a shared mailbox whose servers use the same ids for other messages: in each, two
/// in the Inbox with the subject "Quarterly figures" (one unread in the owner's, both in the
/// shared mailbox's) and one in its Deleted Items with that subject too.
fn two_mailboxes() -> (Session, Id, Id) {
    let (mut s, _, owner) = session();
    let shared = add(&mut s, json!({"address": SHARED})).unwrap();
    for (account, email, unread) in [(owner, OWNER, 1), (shared, SHARED, 2)] {
        apply_graph_mail(&mut s, listing(account, email)).unwrap();
        apply_graph_mail(&mut s, mail_in(account, email, "in", &["AAkA1", "AAkA2"], "Quarterly figures", unread)).unwrap();
        apply_graph_mail(&mut s, mail_in(account, email, "de", &["AAkA9"], "Quarterly figures", 1)).unwrap();
    }
    (s, owner, shared)
}

fn found(s: &mut Session, query: &str, scope: &str) -> Vec<Id> {
    s.run("view.search", &json!({"query": query, "scope": scope})).unwrap();
    s.rows().iter().map(|r| r.id).collect()
}

#[test]
fn a_search_of_all_mailboxes_takes_in_a_shared_mailbox_and_one_of_its_folder_stays_there() {
    let (mut s, owner, shared) = two_mailboxes();
    let (mine, theirs) = (s.mailbox.folder_by_server(owner, "in").unwrap(), s.mailbox.folder_by_server(shared, "in").unwrap());
    let account_of = |s: &Session, ids: &[Id]| -> Vec<Id> {
        let mut out: Vec<Id> = ids.iter().filter_map(|id| s.mailbox.message(*id)).map(|m| m.account).collect();
        out.sort_unstable();
        out
    };
    // All Mailboxes: both Inboxes, the same server ids two messages each; no Deleted Items.
    let all = found(&mut s, "quarterly", "everywhere");
    assert_eq!(account_of(&s, &all), [owner, owner, shared, shared]);
    assert!(all.iter().filter_map(|id| s.mailbox.message(*id)).all(|m| m.folder == mine || m.folder == theirs));
    // The folder in view: the shared mailbox's Inbox is its own, and the owner's is his.
    s.run("folder.select", &json!({"id": theirs})).unwrap();
    let here = found(&mut s, "quarterly", "folder");
    assert_eq!(account_of(&s, &here), [shared, shared]);
    s.run("folder.select", &json!({"id": mine})).unwrap();
    let here = found(&mut s, "quarterly", "folder");
    assert_eq!(account_of(&s, &here), [owner, owner]);
    // And one that reads what is unread finds each mailbox's own.
    let unread = found(&mut s, "quarterly is:unread", "everywhere");
    assert_eq!(account_of(&s, &unread), [owner, shared, shared]);
}

#[test]
fn unread_is_counted_for_each_mailbox_and_never_added_to_its_owners() {
    let (mut s, owner, shared) = two_mailboxes();
    let (mine, theirs) = (s.mailbox.folder_by_server(owner, "in").unwrap(), s.mailbox.folder_by_server(shared, "in").unwrap());
    assert_eq!((s.mailbox.unread_count(mine), s.mailbox.unread_count(theirs)), (1, 2));
    let listed = s.run("folder.list", &json!({})).unwrap();
    let entry = |id: Id| listed["accounts"].as_array().unwrap().iter().find(|a| a["id"] == id).unwrap().clone();
    // Its Inbox and its Deleted Items: its own, on its own entry.
    assert_eq!((entry(owner)["unread"].as_u64(), entry(shared)["unread"].as_u64()), (Some(2), Some(3)));
    assert_eq!(entry(shared)["sharedOf"], owner);
    assert!(entry(owner).get("sharedOf").is_none());
    let inbox = |a: &serde_json::Value| a["folders"].as_array().unwrap().iter().find(|f| f["kind"] == "inbox").unwrap()["unread"].clone();
    assert_eq!((inbox(&entry(owner)), inbox(&entry(shared))), (json!(1), json!(2)));
    // A conversation is its mailbox's: the same subject in both is two rows, each its own count.
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s.run("view.search", &json!({"query": "quarterly", "scope": "everywhere"})).unwrap();
    let rows = s.rows();
    assert_eq!(rows.len(), 2);
    let mut counts: Vec<(Id, usize, usize)> =
        rows.iter().map(|r| (s.mailbox.message(r.id).map_or(0, |m| m.account), r.members.len(), r.unread)).collect();
    counts.sort_unstable();
    assert_eq!(counts, [(owner, 2, 1), (shared, 2, 2)]);
}

#[test]
fn a_shared_mailboxs_folders_are_favourites_kept_here_whatever_its_owners_come_from() {
    let (mut s, owner, shared) = two_mailboxes();
    let (mine, theirs) = (s.mailbox.folder_by_server(owner, "in").unwrap(), s.mailbox.folder_by_server(shared, "in").unwrap());
    // Added without any: Outlook doesn't put a shared mailbox's Inbox among the Favorites.
    assert!(!crate::cmd::folder::favorites(&s).contains(&theirs));
    // The owner's Favorites are Outlook's list, read from his server.
    s.mailbox.set_server_favorites(owner, Some(vec![mine]));
    assert!(crate::cmd::folder::favorites_from_server(&s, owner) && !crate::cmd::folder::favorites_from_server(&s, shared));
    let before = graph_mail_request(&s, shared).unwrap();
    assert_eq!(s.run("folder.favorite", &json!({"id": theirs})).unwrap()["favorite"], true);
    // Outlook's for his own first, then those marked here (the local account's Inbox, then the
    // shared mailbox's).
    let listed = crate::cmd::folder::favorites(&s);
    assert_eq!((listed.first(), listed.last()), (Some(&mine), Some(&theirs)), "{listed:?}");
    let shown = s.run("view.favorites", &json!({})).unwrap();
    assert_eq!(shown["favorites"].as_array().and_then(|all| all.last()).map(|f| f["account"].clone()), Some(json!(shared)));
    // The owner's own are still changed in Outlook alone.
    let other = s.mailbox.folder_by_server(owner, "de").unwrap();
    assert!(s.run("folder.favorite", &json!({"id": other})).unwrap_err().to_string().contains("Outlook"));
    // Nothing of it is for any server: the shared mailbox's run asks for what it asked before.
    let after = graph_mail_request(&s, shared).unwrap();
    assert_eq!(serde_json::to_value(&before).unwrap(), serde_json::to_value(&after).unwrap());
    assert!(after.changes.is_empty() && after.messages.is_empty() && after.drafts.is_empty() && after.sends.is_empty());
    let own = graph_mail_request(&s, owner).unwrap();
    assert!(own.changes.is_empty() && own.messages.is_empty(), "nor the owner's: {own:?}");
    // Undone and done again.
    assert!(s.undo());
    assert!(!crate::cmd::folder::favorites(&s).contains(&theirs));
    s.run("folder.favorite", &json!({"id": theirs, "value": true})).unwrap();
    // With the owner's own Favorites shown instead, his and the shared mailbox's marks show.
    s.run("view.favorites", &json!({"own": true})).unwrap();
    let listed = crate::cmd::folder::favorites(&s);
    assert!(listed.contains(&theirs) && listed.contains(&mine), "{listed:?}");
}

#[test]
fn a_shared_mailboxs_favourite_and_its_link_to_its_owner_survive_a_save_and_a_reopen() {
    let (mut s, owner, shared) = two_mailboxes();
    let theirs = s.mailbox.folder_by_server(shared, "in").unwrap();
    s.run("folder.favorite", &json!({"id": theirs, "value": true})).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-shared-fav-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shared.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    let mut opened = Session::at(crate::store::open(&path).unwrap().mailbox, 0, 0);
    opened.shared_mailboxes = true;
    assert_eq!(opened.mailbox.owner_of(shared).map(|o| o.id), Some(owner));
    assert!(crate::cmd::folder::favorites(&opened).contains(&theirs));
    assert_eq!(found(&mut opened, "quarterly", "everywhere").len(), 4);
    let _ = std::fs::remove_dir_all(&dir);
}
