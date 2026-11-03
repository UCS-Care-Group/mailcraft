// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Junk E-mail Options (#504): the Safe and Blocked Senders lists, the commands that change them,
//! and mail sorted by them as it arrives, the move going up as the owner's own does; and the rules
//! run on what arrives in the Inbox after them (#567).

use mailcraft_model::sync::{
    FolderDelta, FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessageChange, MessageChangeDone, MessageOutcome, MessagePage, RemoteOp,
    ServerMessage, SyncResult,
};
use mailcraft_model::{Address, FlagStatus, FolderKind, Id, Importance, Message, ServerFolder};
use serde_json::json;

use crate::Session;
use crate::cmd::account::sync_request;
use crate::cmd::graph_mail::{apply_graph_mail, graph_mail_request};

fn local() -> (Session, Id) {
    let s = Session::new(mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org"));
    let a = s.mailbox.accounts[0].id;
    (s, a)
}

fn mail(folder: Id, from: &str, subject: &str) -> Message {
    Message { folder, from: Address::new("", from), subject: subject.into(), ..Default::default() }
}

fn lists(s: &Session) -> (Vec<String>, Vec<String>) {
    let account = s.mailbox.accounts[0].id;
    s.mailbox.junk_accounts.get(&account).map(|l| (l.safe.clone(), l.blocked.clone())).unwrap_or_default()
}

#[test]
fn block_sender_lists_the_sender_and_moves_the_message_and_undo_takes_both_back() {
    let (mut s, a) = local();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let junk = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    let id = s.mailbox.add_message(mail(inbox, "Offers@Shop.example", "Deals")).unwrap();
    s.run("mail.select", &json!({"id": id})).unwrap();
    let r = s.run("junk.blockSender", &json!({})).unwrap();
    assert_eq!(r["blocked"], json!(["offers@shop.example"]));
    assert_eq!(r["moved"], 1);
    assert_eq!(s.mailbox.message(id).unwrap().folder, junk);
    assert_eq!(lists(&s).1, ["offers@shop.example"]);
    assert!(s.run("edit.undo", &json!({})).unwrap() != json!(null));
    assert_eq!(s.mailbox.message(id).unwrap().folder, inbox);
    assert!(s.mailbox.junk_accounts.is_empty());
}

#[test]
fn never_block_adds_the_sender_or_the_domain_to_safe_senders_and_leaves_the_mail() {
    let (mut s, a) = local();
    let junk = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    let id = s.mailbox.add_message(mail(junk, "news@lists.example.com", "News")).unwrap();
    s.run("junk.blockSender", &json!({"ids": [id]})).unwrap();
    s.run("junk.neverBlockSender", &json!({"ids": [id]})).unwrap();
    // On one list at a time.
    assert_eq!(lists(&s), (vec!["news@lists.example.com".to_string()], vec![]));
    let r = s.run("junk.neverBlockDomain", &json!({"ids": [id]})).unwrap();
    assert_eq!(r["safe"], json!(["@lists.example.com"]));
    assert_eq!(lists(&s).0, ["news@lists.example.com", "@lists.example.com"]);
    // Nothing that is here moves when a list changes.
    assert_eq!(s.mailbox.message(id).unwrap().folder, junk);
    // A message without a sender's address has nothing to list.
    let nobody = s.mailbox.add_message(mail(junk, "", "Nobody")).unwrap();
    assert!(s.run("junk.neverBlockSender", &json!({"ids": [nobody]})).is_err());
    assert!(s.run("junk.blockSender", &json!({})).is_err(), "nothing selected");
}

#[test]
fn the_options_add_remove_and_list_entries() {
    let (mut s, a) = local();
    let r = s.run("junk.add", &json!({"list": "blocked", "entry": " @Spam.example "})).unwrap();
    assert_eq!(r["entry"], "@spam.example");
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    assert_eq!(s.run("junk.list", &json!({})).unwrap(), json!({"account": a, "safe": ["friend@example.net"], "blocked": ["@spam.example"]}));
    s.run("junk.remove", &json!({"list": "blocked", "entry": "@SPAM.example"})).unwrap();
    assert!(lists(&s).1.is_empty());
    assert!(s.run("junk.remove", &json!({"list": "blocked", "entry": "@spam.example"})).is_err());
    for bad in [json!({}), json!({"list": "other", "entry": "a@example.org"}), json!({"list": "safe", "entry": "not one"}), json!({"list": "safe"})] {
        assert!(s.run("junk.add", &bad).is_err(), "{bad}");
    }
    // Hostile entries are refused, and a list stops at its limit.
    let long = "\u{0}\u{202e}é\n@".repeat(100_000);
    assert!(s.run("junk.add", &json!({"list": "safe", "entry": long})).is_err());
    assert!(s.run("junk.remove", &json!({"list": "safe", "entry": long})).is_err());
    for i in 0..mailcraft_model::MAX_JUNK_ENTRIES {
        s.mailbox.junk_lists_mut(a).blocked.push(format!("s{i}@example.org"));
    }
    assert!(s.run("junk.add", &json!({"list": "blocked", "entry": "more@example.org"})).is_err());
}

#[test]
fn an_agent_is_asked_before_it_changes_or_reads_a_list() {
    use crate::agent::{Access, Class, access, describe};
    for id in ["junk.blockSender", "junk.neverBlockSender", "junk.neverBlockDomain", "junk.add", "junk.remove", "junk.list"] {
        assert_eq!(access(id), Some(Access::Owner(Class::Settings)), "{id}");
    }
    let (mut s, a) = local();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.add_message(mail(inbox, "offers@shop.example", "Deals")).unwrap();
    let said = describe(&s, "junk.blockSender", &json!({"ids": [id]}));
    assert!(said.contains("offers@shop.example") && said.contains("Blocked Senders"), "{said}");
    let said = describe(&s, "junk.neverBlockDomain", &json!({"ids": [id]}));
    assert!(said.contains("@shop.example") && said.contains("Safe Senders"), "{said}");
    assert!(describe(&s, "junk.add", &json!({"list": "blocked", "entry": "X@Example.org"})).contains("x@example.org"));
    // Undoing a change to the lists is a change to the settings.
    s.run("junk.add", &json!({"list": "safe", "entry": "a@example.org"})).unwrap();
    assert_eq!(s.undo_reverts(false), Some(Class::Settings));
    let _ = s.run("mail.select", &json!({"id": id}));
}

/// An IMAP account whose Inbox and Junk E-mail have been read whole once.
fn imap() -> (Session, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let first = SyncResult {
        account: a,
        folders: vec![
            FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 1, complete: true, ..Default::default() },
            FolderDelta { remote: "Junk".into(), kind: FolderKind::Junk, uid_validity: 7, uid_seen: 1, complete: true, ..Default::default() },
        ],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": first})).unwrap();
    (s, a)
}

#[test]
fn imap_mail_from_a_blocked_sender_goes_to_junk_and_a_safe_sender_s_comes_out_of_it() {
    let (mut s, a) = imap();
    s.run("junk.add", &json!({"list": "blocked", "entry": "@shop.example"})).unwrap();
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    // A rule that would file the blocked mail elsewhere never sees it.
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    s.run(
        "rules.add",
        &json!({"name": "Shop", "conditions": [{"type": "fromContains", "value": "shop"}], "actions": [{"type": "moveTo", "value": archive}]}),
    )
    .unwrap();
    let new = |uid, from: &str, subject: &str| Message {
        uid,
        from: Address::new("", from),
        subject: subject.into(),
        message_id: format!("{uid}@x"),
        ..Default::default()
    };
    let run = SyncResult {
        account: a,
        folders: vec![
            FolderDelta {
                remote: "INBOX".into(),
                kind: FolderKind::Inbox,
                uid_validity: 7,
                uid_seen: 3,
                new: vec![new(2, "offers@shop.example", "Deals"), new(3, "bo@example.org", "Hello")],
                ..Default::default()
            },
            FolderDelta {
                remote: "Junk".into(),
                kind: FolderKind::Junk,
                uid_validity: 7,
                uid_seen: 2,
                new: vec![new(2, "Friend@Example.net", "Lunch?")],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": run})).unwrap();
    let folder_of = |s: &Session, subject: &str| s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.folder);
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    assert_eq!(folder_of(&s, "Deals"), Some(junk));
    assert_eq!(folder_of(&s, "Hello"), Some(inbox));
    assert_eq!(folder_of(&s, "Lunch?"), Some(inbox));
    // Both moves go to the server at the next Send/Receive, as the owner's own do.
    let ops = sync_request(&s, a).unwrap().ops;
    let moves: Vec<(String, u32)> =
        ops.iter().filter_map(|op| if let RemoteOp::Move { from, uid, .. } = op { Some((from.clone(), *uid)) } else { None }).collect();
    assert!(moves.contains(&("INBOX".to_string(), 2)) && moves.contains(&("Junk".to_string(), 2)), "{ops:?}");
}

#[test]
fn graph_mail_from_a_blocked_sender_goes_to_junk_and_the_move_goes_up() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let junk = s.mailbox.folder_of_kind(account, FolderKind::Junk).unwrap();
    for (f, id) in [(inbox, "F-in"), (junk, "F-junk")] {
        // Each folder's first round has ended: what comes now arrives.
        s.mailbox.folder_mut(f).unwrap().server = Some(ServerFolder { id: id.into(), caught_up: true, ..Default::default() });
    }
    s.run("junk.add", &json!({"list": "blocked", "entry": "offers@shop.example"})).unwrap();
    let there = |id: &str, from: &str| GraphMessage {
        server: ServerMessage { id: id.into(), folder: "F-in".into(), ..Default::default() },
        from: Address::new("", from),
        subject: id.into(),
        ..Default::default()
    };
    let page = MessagePage {
        folder: "F-in".into(),
        messages: vec![there("M-1", "offers@shop.example"), there("M-2", "bo@example.org")],
        ..Default::default()
    };
    let result = GraphMailResult { account, email: "owner@example.org".into(), pages: vec![page], partial: true, ..Default::default() };
    apply_graph_mail(&mut s, result).unwrap();
    let id = |s: &Session, subject: &str| s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| (m.id, m.folder)).unwrap();
    let (blocked, folder) = id(&s, "M-1");
    assert_eq!(folder, junk);
    assert_eq!(id(&s, "M-2").1, inbox);
    let changes = graph_mail_request(&s, account).unwrap().messages;
    assert!(changes.iter().any(|c| matches!(c, MessageChange::Move { message, .. } if *message == blocked)), "{changes:?}");
}

/// #504 review: the owner is asked about the list the command changes, and about nothing else.
#[test]
fn the_question_names_the_list_the_command_changes() {
    use crate::agent::{Class, Verdict, check, describe};
    let (mut s, _) = local();
    let params = json!({"list": " blocked", "entry": "offers@shop.example"});
    let Verdict::Ask(Class::Settings, said) = check(&s, "junk.add", &params, &[]) else { panic!("asked") };
    assert!(said.contains("Blocked Senders") && !said.contains("Safe Senders"), "{said}");
    s.run("junk.add", &params).unwrap();
    assert_eq!(lists(&s).1, ["offers@shop.example"]);
    let said = describe(&s, "junk.remove", &json!({"list": "SAFE ", "entry": "a@example.org"}));
    assert!(said.contains("Safe Senders"), "{said}");
    // A list that is neither isn't put to the owner at all.
    for list in [json!("both"), json!(""), json!(1), json!(null)] {
        let params = json!({"list": list, "entry": "offers@shop.example"});
        assert!(matches!(check(&s, "junk.add", &params, &[]), Verdict::Refuse(_)), "{params}");
        assert!(matches!(check(&s, "junk.remove", &params, &[Class::Settings]), Verdict::Refuse(_)), "{params}");
    }
}

#[test]
fn the_owner_s_own_addresses_are_never_blocked() {
    let (mut s, a) = local();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let mine = s.mailbox.add_message(mail(inbox, "ADA@example.org", "Note to self")).unwrap();
    let e = format!("{:?}", s.run("junk.blockSender", &json!({"ids": [mine]})).unwrap_err());
    assert!(e.contains("your own"), "{e}");
    assert!(s.run("junk.add", &json!({"list": "blocked", "entry": "<ada@example.org>"})).is_err());
    assert!(s.mailbox.junk_accounts.is_empty());
    assert_eq!(s.mailbox.message(mine).unwrap().folder, inbox);
    // Never blocking it is fine, and so is blocking its domain.
    s.run("junk.add", &json!({"list": "safe", "entry": "ada@example.org"})).unwrap();
    s.run("junk.add", &json!({"list": "blocked", "entry": "@example.org"})).unwrap();
}

fn delta(remote: &str, kind: FolderKind, seen: u32, new: Vec<Message>) -> FolderDelta {
    FolderDelta { remote: remote.into(), kind, uid_validity: 7, uid_seen: seen, new, ..Default::default() }
}

fn arriving(uid: u32, from: &str, subject: &str) -> Message {
    Message { uid, from: Address::new("", from), subject: subject.into(), message_id: format!("{uid}-{subject}@x"), ..Default::default() }
}

fn folder_of(s: &Session, subject: &str) -> Option<Id> {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.folder)
}

#[test]
fn an_account_s_first_download_is_left_as_its_server_has_it() {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    s.run("junk.add", &json!({"list": "blocked", "entry": "@shop.example"})).unwrap();
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    let first = SyncResult {
        account: a,
        folders: vec![
            FolderDelta { complete: true, ..delta("INBOX", FolderKind::Inbox, 1, vec![arriving(1, "offers@shop.example", "Old deals")]) },
            FolderDelta { complete: true, ..delta("Junk", FolderKind::Junk, 1, vec![arriving(1, "friend@example.net", "Old lunch")]) },
        ],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": first})).unwrap();
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    assert_eq!(folder_of(&s, "Old deals"), Some(inbox));
    assert_eq!(folder_of(&s, "Old lunch"), Some(junk));
    assert!(s.mailbox.pending.is_empty());
    // What arrives afterwards is sorted.
    let next = SyncResult {
        account: a,
        folders: vec![delta("INBOX", FolderKind::Inbox, 2, vec![arriving(2, "offers@shop.example", "New deals")])],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": next})).unwrap();
    assert_eq!(folder_of(&s, "New deals"), Some(junk));
}

#[test]
fn graph_s_first_round_of_a_folder_is_left_as_its_server_has_it() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), ..Default::default() });
    s.run("junk.add", &json!({"list": "blocked", "entry": "offers@shop.example"})).unwrap();
    let there = GraphMessage {
        server: ServerMessage { id: "M-1".into(), folder: "F-in".into(), ..Default::default() },
        from: Address::new("", "offers@shop.example"),
        subject: "Old deals".into(),
        ..Default::default()
    };
    let page = MessagePage { folder: "F-in".into(), messages: vec![there], ..Default::default() };
    let result = GraphMailResult { account, email: "owner@example.org".into(), pages: vec![page], partial: true, ..Default::default() };
    apply_graph_mail(&mut s, result).unwrap();
    assert_eq!(folder_of(&s, "Old deals"), Some(inbox));
}

#[test]
fn a_safe_sender_s_mail_out_of_junk_goes_through_the_inbox_rules() {
    let (mut s, a) = imap();
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    s.run(
        "rules.add",
        &json!({"name": "Friends", "conditions": [{"type": "fromContains", "value": "friend"}], "actions": [{"type": "moveTo", "value": archive}]}),
    )
    .unwrap();
    // The rule doesn't run on what stays in Junk E-mail.
    let run = SyncResult {
        account: a,
        folders: vec![delta(
            "Junk",
            FolderKind::Junk,
            3,
            vec![arriving(2, "friend@example.net", "Lunch?"), arriving(3, "friendly@spam.example", "Win")],
        )],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": run})).unwrap();
    let junk = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    assert_eq!(folder_of(&s, "Lunch?"), Some(archive));
    assert_eq!(folder_of(&s, "Win"), Some(junk));
}

/// An IMAP account with the Blocked Senders list naming `@shop.example`, nothing of it read yet.
fn fresh_imap() -> (Session, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    s.run("junk.add", &json!({"list": "blocked", "entry": "@shop.example"})).unwrap();
    (s, a)
}

/// One piece of a run that reads the Inbox.
fn take(s: &mut Session, a: Id, piece: FolderDelta) {
    let result = SyncResult { account: a, folders: vec![piece], ..Default::default() };
    s.run("sync.apply", &json!({"result": result})).unwrap();
}

fn inbox_piece(seen: u32, new: Vec<Message>, complete: bool) -> FolderDelta {
    FolderDelta { complete, ..delta("INBOX", FolderKind::Inbox, seen, new) }
}

/// #504 re-review: a first download comes in pieces, each saying how far the folder is read
/// (just below the lowest message still to come). None of them is new mail.
#[test]
fn a_first_download_in_pieces_sorts_nothing() {
    let (mut s, a) = fresh_imap();
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    take(&mut s, a, inbox_piece(200, vec![arriving(250, "offers@shop.example", "Piece one")], false));
    take(&mut s, a, inbox_piece(100, vec![arriving(150, "offers@shop.example", "Piece two")], false));
    take(&mut s, a, inbox_piece(250, vec![arriving(101, "offers@shop.example", "Piece three")], true));
    for subject in ["Piece one", "Piece two", "Piece three"] {
        assert_eq!(folder_of(&s, subject), Some(inbox), "{subject}");
    }
    assert!(s.mailbox.pending.is_empty());
    take(&mut s, a, inbox_piece(251, vec![arriving(251, "offers@shop.example", "New")], true));
    assert_eq!(folder_of(&s, "New"), Some(junk));
    // Older mail a raised limit brings down afterwards isn't new either.
    take(&mut s, a, inbox_piece(251, vec![arriving(50, "offers@shop.example", "Older")], true));
    assert_eq!(folder_of(&s, "Older"), Some(inbox));
}

#[test]
fn a_folder_empty_at_its_first_reading_sorts_what_arrives_next() {
    let (mut s, a) = fresh_imap();
    let junk = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    take(&mut s, a, inbox_piece(0, vec![], true));
    take(&mut s, a, inbox_piece(1, vec![arriving(1, "offers@shop.example", "First ever")], true));
    assert_eq!(folder_of(&s, "First ever"), Some(junk));
}

#[test]
fn a_first_download_cut_short_and_taken_up_again_sorts_nothing() {
    let (mut s, a) = fresh_imap();
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    take(&mut s, a, inbox_piece(100, vec![arriving(150, "offers@shop.example", "Before")], false));
    // The program is closed and opened again: the mailbox comes back from its file.
    let file = serde_json::to_value(&s.mailbox).unwrap();
    let mut s = Session::new(serde_json::from_value(file).unwrap());
    // The next run brings the rest, in a piece of its own and then the last one.
    take(&mut s, a, inbox_piece(100, vec![arriving(120, "offers@shop.example", "After")], false));
    take(&mut s, a, inbox_piece(150, vec![arriving(101, "offers@shop.example", "Last")], true));
    for subject in ["Before", "After", "Last"] {
        assert_eq!(folder_of(&s, subject), Some(inbox), "{subject}");
    }
    take(&mut s, a, inbox_piece(151, vec![arriving(151, "offers@shop.example", "New")], true));
    assert_eq!(folder_of(&s, "New"), Some(junk));
}

/// A file from before a folder said it was read whole: one read up to some UID was.
#[test]
fn an_older_file_s_folders_read_before_are_taken_as_read_whole() {
    let (mut s, a) = fresh_imap();
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    take(&mut s, a, inbox_piece(10, vec![arriving(10, "bo@example.org", "Old")], true));
    let mut file = serde_json::to_value(&s.mailbox).unwrap();
    for folder in file["folders"].as_array_mut().unwrap() {
        folder.as_object_mut().unwrap().remove("whole");
    }
    let mut s = Session::new(serde_json::from_value(file).unwrap());
    assert!(s.mailbox.folder(inbox).unwrap().whole.is_none());
    take(&mut s, a, inbox_piece(11, vec![arriving(11, "offers@shop.example", "New")], true));
    assert_eq!(folder_of(&s, "New"), Some(junk));
    // Junk E-mail was never read: its first reading is left alone.
    let mut first = delta("Junk", FolderKind::Junk, 3, vec![arriving(3, "friend@example.net", "Lunch?")]);
    first.complete = true;
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    take(&mut s, a, first);
    assert_eq!(folder_of(&s, "Lunch?"), Some(junk));
}

/// #504 review: a mailbox whose first save was damaged forgets how far its folders were read and
/// fetches them again; that is a first download too, and sorts nothing.
#[test]
fn mail_fetched_again_after_a_damaged_save_is_not_sorted() {
    let (mut s, a) = fresh_imap();
    let [inbox, junk] = [FolderKind::Inbox, FolderKind::Junk].map(|k| s.mailbox.folder_of_kind(a, k).unwrap());
    take(&mut s, a, inbox_piece(10, vec![], true));
    crate::store::forget_how_far_fetched(&mut s.mailbox);
    take(&mut s, a, inbox_piece(10, vec![arriving(5, "offers@shop.example", "Old"), arriving(10, "offers@shop.example", "Older")], true));
    assert_eq!((folder_of(&s, "Old"), folder_of(&s, "Older")), (Some(inbox), Some(inbox)));
    take(&mut s, a, inbox_piece(11, vec![arriving(11, "offers@shop.example", "New")], true));
    assert_eq!(folder_of(&s, "New"), Some(junk));
}

/// A second account, as another address of the same owner.
fn second_account(s: &mut Session) -> Id {
    let mut b = s.mailbox.accounts[0].clone();
    b.id += 1_000;
    b.email = "ada@example.net".into();
    b.name = "ada@example.net".into();
    let id = b.id;
    s.mailbox.accounts.push(b);
    s.mailbox.repair();
    id
}

#[test]
fn each_account_has_its_own_lists_and_sorts_by_them_alone() {
    let (mut s, a) = local();
    let b = second_account(&mut s);
    s.run("junk.add", &json!({"account": a, "list": "blocked", "entry": "@shop.example"})).unwrap();
    s.run("junk.add", &json!({"account": b, "list": "safe", "entry": "@shop.example"})).unwrap();
    let r = s.run("junk.list", &json!({"account": a})).unwrap();
    assert_eq!((r["safe"].clone(), r["blocked"].clone()), (json!([]), json!(["@shop.example"])));
    let r = s.run("junk.list", &json!({"account": b})).unwrap();
    assert_eq!((r["safe"].clone(), r["blocked"].clone()), (json!(["@shop.example"]), json!([])));
    // New mail is sorted by the lists of the account it came to.
    let inbox_a = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let junk_a = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    let inbox_b = s.mailbox.folder_of_kind(b, FolderKind::Inbox).unwrap();
    let to_a = s.mailbox.add_message(mail(inbox_a, "x@shop.example", "A")).unwrap();
    let to_b = s.mailbox.add_message(mail(inbox_b, "x@shop.example", "B")).unwrap();
    assert!(s.mailbox.sort_junk(to_a));
    assert!(!s.mailbox.sort_junk(to_b));
    assert_eq!(s.mailbox.message(to_a).unwrap().folder, junk_a);
    // Block Sender changes the lists of the message's account only.
    let other = s.mailbox.add_message(mail(inbox_b, "y@promo.example", "P")).unwrap();
    s.run("junk.blockSender", &json!({"ids": [other]})).unwrap();
    assert_eq!(s.mailbox.junk_accounts[&b].blocked, ["y@promo.example"]);
    assert_eq!(s.mailbox.junk_accounts[&a].blocked, ["@shop.example"]);
    // Without `account`, the selected message's account; a stranger is refused.
    assert!(s.run("junk.list", &json!({"account": 424_242})).is_err());
    s.run("mail.select", &json!({"id": other})).unwrap();
    assert_eq!(s.run("junk.list", &json!({})).unwrap()["account"], json!(b));
    // Removing an account takes its lists with it.
    s.mailbox.remove_account(b).unwrap();
    assert!(!s.mailbox.junk_accounts.contains_key(&b));
}

#[test]
fn an_agent_is_asked_about_the_account_whose_list_changes() {
    use crate::agent::{Class, Verdict, check, describe};
    let (mut s, _) = local();
    let b = second_account(&mut s);
    let params = json!({"account": b, "list": "blocked", "entry": "x@shop.example"});
    let said = describe(&s, "junk.add", &params);
    assert!(said.contains("ada@example.net"), "{said}");
    assert!(matches!(check(&s, "junk.add", &params, &[]), Verdict::Ask(Class::Settings, _)));
    let stranger = json!({"account": 424_242, "list": "safe", "entry": "x@y.example"});
    assert!(matches!(check(&s, "junk.add", &stranger, &[Class::Settings]), Verdict::Refuse(_)));
}

// #567: mail that comes through Graph goes through the rules after the lists, as IMAP mail does.

#[test]
fn a_rule_moves_new_mail_that_comes_through_graph() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    for (f, id) in [(inbox, "F-in"), (archive, "F-arch")] {
        s.mailbox.folder_mut(f).unwrap().server = Some(ServerFolder { id: id.into(), caught_up: true, ..Default::default() });
    }
    s.run(
        "rules.add",
        &json!({"name": "News", "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "moveTo", "value": archive}]}),
    )
    .unwrap();
    let there = GraphMessage {
        server: ServerMessage { id: "M-1".into(), folder: "F-in".into(), ..Default::default() },
        from: Address::new("", "news@paper.example"),
        subject: "Morning news".into(),
        ..Default::default()
    };
    let page = MessagePage { folder: "F-in".into(), messages: vec![there], ..Default::default() };
    let result = GraphMailResult { account, email: "owner@example.org".into(), pages: vec![page], partial: true, ..Default::default() };
    apply_graph_mail(&mut s, result.clone()).unwrap();
    assert_eq!(folder_of(&s, "Morning news"), Some(archive), "the rule did not move mail that arrived through Graph");
    // The move goes up as the owner's own does, and the same page again doesn't take it back.
    let moved = s.mailbox.messages.values().find(|m| m.subject == "Morning news").unwrap().id;
    let changes = graph_mail_request(&s, account).unwrap().messages;
    assert!(changes.iter().any(|c| matches!(c, MessageChange::Move { message, to, .. } if *message == moved && to == "F-arch")), "{changes:?}");
    apply_graph_mail(&mut s, result).unwrap();
    assert_eq!(folder_of(&s, "Morning news"), Some(archive));
}

// Control: the same rule on an IMAP account.
#[test]
fn a_rule_moves_new_mail_that_comes_through_imap() {
    let (mut s, a) = imap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    s.run(
        "rules.add",
        &json!({"name": "News", "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "moveTo", "value": archive}]}),
    )
    .unwrap();
    let run = SyncResult {
        account: a,
        folders: vec![delta("INBOX", FolderKind::Inbox, 2, vec![arriving(2, "news@paper.example", "Morning news")])],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": run})).unwrap();
    assert_eq!(folder_of(&s, "Morning news"), Some(archive));
}

/// A Microsoft account kept in step through Graph, whose Inbox (`F-in`), Junk E-mail (`F-junk`)
/// and Archive (`F-arch`) have had their first round when `caught_up`; a rule files mail from
/// anyone at `shop.example` or named `friend` into the Archive.
fn graph_with_rule(caught_up: bool) -> (Session, Id, [Id; 3]) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = [FolderKind::Inbox, FolderKind::Junk, FolderKind::Archive].map(|k| s.mailbox.folder_of_kind(account, k).unwrap());
    for (f, id) in folders.into_iter().zip(["F-in", "F-junk", "F-arch"]) {
        s.mailbox.folder_mut(f).unwrap().server = Some(ServerFolder { id: id.into(), caught_up, ..Default::default() });
    }
    for who in ["shop", "friend"] {
        let archive = folders[2];
        s.run(
            "rules.add",
            &json!({"name": who, "conditions": [{"type": "fromContains", "value": who}], "actions": [{"type": "moveTo", "value": archive}]}),
        )
        .unwrap();
    }
    (s, account, folders)
}

/// One page of `folder` bringing messages from these senders, each with its sender as its subject.
fn graph_page(account: Id, folder: &str, from: &[&str]) -> GraphMailResult {
    let messages = from
        .iter()
        .map(|from| GraphMessage {
            server: ServerMessage { id: format!("{folder}/{from}"), folder: folder.into(), ..Default::default() },
            from: Address::new("", *from),
            subject: (*from).into(),
            ..Default::default()
        })
        .collect();
    let page = MessagePage { folder: folder.into(), messages, ..Default::default() };
    GraphMailResult { account, email: "owner@example.org".into(), pages: vec![page], partial: true, ..Default::default() }
}

#[test]
fn graph_mail_goes_through_the_rules_after_the_lists() {
    let (mut s, account, [inbox, junk, archive]) = graph_with_rule(true);
    s.run("junk.add", &json!({"list": "blocked", "entry": "@shop.example"})).unwrap();
    s.run("junk.add", &json!({"list": "safe", "entry": "friend@example.net"})).unwrap();
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["offers@shop.example", "bo@example.org"])).unwrap();
    apply_graph_mail(&mut s, graph_page(account, "F-junk", &["friend@example.net", "friendly@spam.example"])).unwrap();
    // A blocked sender's mail goes to Junk E-mail and the rule that would file it never sees it.
    assert_eq!(folder_of(&s, "offers@shop.example"), Some(junk));
    assert_eq!(folder_of(&s, "bo@example.org"), Some(inbox));
    // A safe sender's mail out of Junk E-mail goes through the rules; what stays there doesn't.
    assert_eq!(folder_of(&s, "friend@example.net"), Some(archive));
    assert_eq!(folder_of(&s, "friendly@spam.example"), Some(junk));
}

#[test]
fn graph_s_first_round_of_a_folder_runs_no_rules() {
    let (mut s, account, [inbox, ..]) = graph_with_rule(false);
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["offers@shop.example"])).unwrap();
    assert_eq!(folder_of(&s, "offers@shop.example"), Some(inbox));
    // Nor does a folder read afresh.
    let (mut s, account, [inbox, ..]) = graph_with_rule(true);
    let mut afresh = graph_page(account, "F-in", &["offers@shop.example"]);
    afresh.pages[0].afresh = true;
    apply_graph_mail(&mut s, afresh).unwrap();
    assert_eq!(folder_of(&s, "offers@shop.example"), Some(inbox));
    assert!(graph_mail_request(&s, account).unwrap().messages.is_empty());
}

#[test]
fn the_rules_leave_a_shared_mailbox_s_mail_as_its_server_has_it() {
    let (mut s, owner, _) = graph_with_rule(true);
    // (A move out of a shared mailbox waits for the whole message: a flag would be there at once.)
    s.run("rules.add", &json!({"name": "Flag", "conditions": [{"type": "fromContains", "value": "shop"}], "actions": [{"type": "flag"}]})).unwrap();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "support@example.org"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "S-in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = FolderListing { folders, complete: true };
    let listed = GraphMailResult { account: shared, email: "support@example.org".into(), listing: Some(listing), ..Default::default() };
    apply_graph_mail(&mut s, listed).unwrap();
    let inbox = s.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server.as_mut().unwrap().caught_up = true;
    let page = GraphMailResult { email: "support@example.org".into(), ..graph_page(shared, "S-in", &["offers@shop.example"]) };
    apply_graph_mail(&mut s, page).unwrap();
    let m = s.mailbox.messages.values().find(|m| m.subject == "offers@shop.example").unwrap();
    assert_eq!((m.folder, m.flag), (inbox, FlagStatus::None));
}

/// A Microsoft account whose Inbox (`F-in`) has had its first round, beside an account on this
/// computer; a rule files mail from anyone named `news` into the second's Archive. The two
/// accounts, and that Archive.
fn graph_rule_to_local() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let local = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let kept_here = s.mailbox.folder_of_kind(local, FolderKind::Archive).unwrap();
    s.run(
        "rules.add",
        &json!({"name": "News", "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "moveTo", "value": kept_here}]}),
    )
    .unwrap();
    (s, account, local, kept_here)
}

#[test]
fn undo_never_takes_away_graph_mail_a_rule_moved_out_of_its_account() {
    let (mut s, account, local, kept_here) = graph_rule_to_local();
    // A step of the owner's, from before the mail arrives.
    let local_inbox = s.mailbox.folder_of_kind(local, FolderKind::Inbox).unwrap();
    let other = s.mailbox.add_message(mail(local_inbox, "bo@example.org", "Hello")).unwrap();
    s.run("mail.flag", &json!({"id": other})).unwrap();
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["news@paper.example"])).unwrap();
    let m = s.mailbox.messages.values().find(|m| m.subject == "news@paper.example").unwrap();
    assert_eq!((m.account, m.folder, m.moving_out.is_some()), (local, kept_here, true));
    let id = m.id;
    // There is no going back past it: not even once the server's copy is removed.
    assert_eq!(s.undo_label(), None);
    let done = MessageChangeDone {
        change: MessageChange::MovedOut { message: id, id: "F-in/news@paper.example".into() },
        outcome: MessageOutcome::Done { categories: None, flag: None, folder: String::new(), id: String::new() },
        there: None,
    };
    let result = GraphMailResult { account, email: "owner@example.org".into(), messages: vec![done], partial: true, ..Default::default() };
    apply_graph_mail(&mut s, result).unwrap();
    assert!(s.run("edit.undo", &json!({})).is_err());
    assert!(s.mailbox.message(id).is_some_and(|m| m.moving_out.is_none()));
    assert_eq!(s.mailbox.message(other).unwrap().flag, FlagStatus::Flagged);
    // A step after it goes back as ever, and keeps the mail.
    s.run("mail.markRead", &json!({"id": other})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.message(id).is_some());
}

#[test]
fn a_rule_that_files_graph_mail_within_its_account_leaves_undo_as_it_was() {
    let (mut s, account, [_, _, archive]) = graph_with_rule(true);
    let before = s.undo_label();
    assert!(before.is_some());
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["offers@shop.example"])).unwrap();
    assert_eq!(folder_of(&s, "offers@shop.example"), Some(archive));
    assert_eq!(s.undo_label(), before);
}

/// One page of `F-in` from these senders, going on at a next page (`caught_up` false) or a delta
/// link.
fn inbox_page(account: Id, from: &[&str], caught_up: bool, afresh: bool) -> GraphMailResult {
    let mut result = graph_page(account, "F-in", from);
    let page = &mut result.pages[0];
    page.link = format!("https://graph.example.org/delta?{}={}", if caught_up { "$deltatoken" } else { "$skiptoken" }, from.join(","));
    (page.caught_up, page.afresh) = (caught_up, afresh);
    result
}

#[test]
fn every_page_of_a_later_round_goes_through_the_lists_and_the_rules() {
    let (mut s, account, [inbox, junk, archive]) = graph_with_rule(true);
    s.run("junk.add", &json!({"list": "blocked", "entry": "@spam.example"})).unwrap();
    // Back after a while: more has changed than one page holds, read in pieces, and a run that
    // ended before the round did goes on from the next page.
    apply_graph_mail(&mut s, inbox_page(account, &["one@shop.example", "bo@example.org"], false, false)).unwrap();
    apply_graph_mail(&mut s, inbox_page(account, &["two@shop.example", "deals@spam.example"], false, false)).unwrap();
    apply_graph_mail(&mut s, inbox_page(account, &["three@shop.example"], true, false)).unwrap();
    for later in ["one@shop.example", "two@shop.example", "three@shop.example"] {
        assert_eq!(folder_of(&s, later), Some(archive), "{later}");
    }
    assert_eq!(folder_of(&s, "deals@spam.example"), Some(junk));
    assert_eq!(folder_of(&s, "bo@example.org"), Some(inbox));
}

#[test]
fn no_page_of_a_first_round_or_of_a_round_read_afresh_is_sorted() {
    let (mut s, account, [inbox, _, archive]) = graph_with_rule(false);
    apply_graph_mail(&mut s, inbox_page(account, &["one@shop.example"], false, false)).unwrap();
    apply_graph_mail(&mut s, inbox_page(account, &["two@shop.example"], true, false)).unwrap();
    // Read afresh (the server no longer took the link): none of its pages either.
    apply_graph_mail(&mut s, inbox_page(account, &["three@shop.example"], false, true)).unwrap();
    apply_graph_mail(&mut s, inbox_page(account, &["four@shop.example"], true, false)).unwrap();
    for old in ["one@shop.example", "two@shop.example", "three@shop.example", "four@shop.example"] {
        assert_eq!(folder_of(&s, old), Some(inbox), "{old}");
    }
    // What comes after each is new mail.
    apply_graph_mail(&mut s, inbox_page(account, &["five@shop.example"], false, false)).unwrap();
    assert_eq!(folder_of(&s, "five@shop.example"), Some(archive));
    // A mailbox whose save was damaged reads its folders from the start again: a first round.
    crate::store::forget_how_far_fetched(&mut s.mailbox);
    apply_graph_mail(&mut s, inbox_page(account, &["six@shop.example"], false, false)).unwrap();
    assert_eq!(folder_of(&s, "six@shop.example"), Some(inbox));
}

// #606 review: what else the rules do to Graph mail, and a page or a run that is not as usual.

/// A page that lists a message twice brings it once, though a rule moved it out of the account
/// at its first listing; and so does the same page in a later run.
#[test]
fn a_message_a_page_lists_twice_is_moved_out_of_its_account_once() {
    let (mut s, account, local, kept_here) = graph_rule_to_local();
    let twice = graph_page(account, "F-in", &["news@paper.example", "news@paper.example"]);
    apply_graph_mail(&mut s, twice.clone()).unwrap();
    apply_graph_mail(&mut s, twice).unwrap();
    let copies: Vec<(Id, Id)> = s.mailbox.messages.values().filter(|m| m.subject == "news@paper.example").map(|m| (m.account, m.folder)).collect();
    assert_eq!(copies, [(local, kept_here)]);
}

#[test]
fn a_rule_that_deletes_graph_mail_moves_it_to_deleted_items_on_the_server_too() {
    let (mut s, account, _) = graph_with_rule(true);
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    s.mailbox.folder_mut(deleted).unwrap().server = Some(ServerFolder { id: "F-del".into(), caught_up: true, ..Default::default() });
    s.run("rules.add", &json!({"name": "Bin", "conditions": [{"type": "fromContains", "value": "bin"}], "actions": [{"type": "delete"}]})).unwrap();
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["bin@spam.example"])).unwrap();
    let m = s.mailbox.messages.values().find(|m| m.subject == "bin@spam.example").unwrap();
    assert_eq!(m.folder, deleted);
    let changes = graph_mail_request(&s, account).unwrap().messages;
    assert!(changes.iter().any(|c| matches!(c, MessageChange::Move { message, to, .. } if *message == m.id && to == "F-del")), "{changes:?}");
}

#[test]
fn what_a_rule_marks_on_graph_mail_goes_up() {
    let (mut s, account, [inbox, ..]) = graph_with_rule(true);
    s.run(
        "rules.add",
        &json!({"name": "Boss", "conditions": [{"type": "fromContains", "value": "boss"}], "actions": [
            {"type": "flag"}, {"type": "markRead"}, {"type": "categorize", "value": "Red"}, {"type": "setImportance", "value": "high"}
        ]}),
    )
    .unwrap();
    apply_graph_mail(&mut s, graph_page(account, "F-in", &["boss@example.org"])).unwrap();
    let m = s.mailbox.messages.values().find(|m| m.subject == "boss@example.org").unwrap();
    assert_eq!(m.folder, inbox);
    let changes = graph_mail_request(&s, account).unwrap().messages;
    let [MessageChange::Update { message, read, flag, categories, importance, .. }] = changes.as_slice() else { panic!("{changes:?}") };
    assert_eq!(*message, m.id);
    assert_eq!(*read, Some(true));
    assert_eq!(flag.as_ref().map(|f| f.status), Some(FlagStatus::Flagged));
    assert_eq!(categories.as_ref().map(|c| c.here.clone()), Some(vec!["Red".to_string()]));
    assert_eq!(*importance, Some(Importance::High));
}

/// A later round that a run left partway is sorted on after the mailbox is saved and opened again.
#[test]
fn a_later_round_left_partway_is_sorted_on_after_a_save_and_a_reopen() {
    let (mut s, account, [_, _, archive]) = graph_with_rule(true);
    apply_graph_mail(&mut s, inbox_page(account, &["one@shop.example"], false, false)).unwrap();
    let dir = std::env::temp_dir().join(format!("mailcraft-567-reopen-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("later-round.mailcraft");
    crate::store::save(&path, &s.mailbox).unwrap();
    let mut s = Session::new(crate::store::open(&path).unwrap().mailbox);
    apply_graph_mail(&mut s, inbox_page(account, &["two@shop.example"], false, false)).unwrap();
    assert_eq!(folder_of(&s, "two@shop.example"), Some(archive));
    let _ = std::fs::remove_dir_all(&dir);
}
