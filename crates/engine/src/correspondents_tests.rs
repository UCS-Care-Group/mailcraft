// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::{Account, Address, Body, Contact, FolderKind, Id, Labeled, Mailbox, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::correspondents::{messages_for, paginate_latest, summaries};
use crate::{Filter, Session};

fn ts(day: u8, hour: u8, min: u8) -> i64 {
    DateTime::new(Date::new(2026, 11, day).unwrap(), hour, min, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn add_msg(mb: &mut Mailbox, folder: Id, from: (&str, &str), to: &[(&str, &str)], subject: &str, when: i64, text: &str, read: bool) -> Id {
    let m = Message {
        folder,
        from: Address::new(from.0, from.1),
        to: to.iter().map(|p| Address::new(p.0, p.1)).collect(),
        subject: subject.into(),
        date: when,
        received: when,
        body: Body { text: text.into(), html: None },
        read,
        ..Default::default()
    };
    mb.add_message(m).unwrap()
}

fn test_session() -> Session {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let acc1 = mb.accounts[0].id;
    let inbox1 = mb.folder_of_kind(acc1, FolderKind::Inbox).unwrap();
    let sent1 = mb.folder_of_kind(acc1, FolderKind::Sent).unwrap();
    let archive1 = mb.folder_of_kind(acc1, FolderKind::Archive).unwrap();
    let deleted1 = mb.folder_of_kind(acc1, FolderKind::Deleted).unwrap();
    let junk1 = mb.folder_of_kind(acc1, FolderKind::Junk).unwrap();

    // Second account to test `allAccounts` toggle.
    let acc2 =
        mb.add_account(Account { name: "Work".into(), email: "avery@work.example".into(), display_name: "Avery Work".into(), ..Default::default() });
    let inbox2 = mb.folder_of_kind(acc2, FolderKind::Inbox).unwrap();

    // Peter has multiple unrelated subjects across Inbox, Sent, and Archive, plus Deleted/Junk noise.
    add_msg(
        &mut mb,
        inbox1,
        ("Peter B.", "Peter@Example.ORG"),
        &[("Avery", "avery@example.org")],
        "Data availability",
        ts(20, 8, 30),
        "Data is ready",
        false,
    );
    add_msg(
        &mut mb,
        sent1,
        ("Avery", "avery@example.org"),
        &[("Peter", "peter@example.org")],
        "Re: Data availability",
        ts(20, 8, 54),
        "Thanks!",
        true,
    );
    add_msg(
        &mut mb,
        archive1,
        ("Peter Berthelemy", "peter@example.org"),
        &[("Avery", "avery@example.org")],
        "MATLAB source code",
        ts(21, 9, 20),
        "Attached script",
        false,
    );
    // Deleted and Junk messages from Peter must NOT appear.
    add_msg(&mut mb, deleted1, ("Peter", "peter@example.org"), &[("Avery", "avery@example.org")], "Deleted note", ts(22, 10, 0), "ignore", false);
    add_msg(&mut mb, junk1, ("Peter", "peter@example.org"), &[("Avery", "avery@example.org")], "Junk note", ts(22, 10, 5), "ignore", false);

    // Peter's alternate email in account 1 + same person in account 2.
    add_msg(
        &mut mb,
        inbox1,
        ("Peter Alt", "peter.alt@example.org"),
        &[("Avery", "avery@example.org")],
        "Alt address mail",
        ts(22, 11, 0),
        "From alt",
        true,
    );
    add_msg(
        &mut mb,
        inbox2,
        ("Peter Work", "peter@example.org"),
        &[("Work", "avery@work.example")],
        "Second account mail",
        ts(23, 12, 0),
        "Cross account",
        false,
    );

    // Contact card linking Bob's two emails automatically.
    mb.add_contact(Contact {
        first: "Bob".into(),
        last: "Smith".into(),
        emails: vec![Labeled::new("work", "bob@example.org"), Labeled::new("home", "bob.home@example.org")],
        ..Default::default()
    });
    add_msg(&mut mb, inbox1, ("Bob", "bob@example.org"), &[("Avery", "avery@example.org")], "Topic A", ts(19, 14, 0), "hello", true);
    add_msg(&mut mb, inbox1, ("Bob Home", "bob.home@example.org"), &[("Avery", "avery@example.org")], "Topic B", ts(21, 15, 0), "hi again", false);

    Session::at(mb, ts(24, 9, 0), 0)
}

#[test]
fn aggregates_by_folder_scope_and_respects_all_accounts_toggle() {
    let mut s = test_session();
    // In Inbox (default active folder): lists only correspondents with received mail in Inbox.
    let list = summaries(&s, false, Filter::All, "");
    let keys: Vec<&str> = list.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(keys, ["peter.alt@example.org", "bob.home@example.org", "peter@example.org"]);

    let peter = list.iter().find(|c| c.key == "peter@example.org").unwrap();
    assert_eq!(peter.message_count, 1);
    assert_eq!(peter.unread_count, 1);
    assert_eq!(peter.latest_subject, "Data availability");
    assert_eq!(peter.name, "Peter B.");

    let bob = list.iter().find(|c| c.key == "bob.home@example.org").unwrap();
    assert_eq!(bob.name, "Bob Smith");
    assert_eq!(bob.message_count, 2);
    assert_eq!(bob.unread_count, 1);
    assert_eq!(bob.addresses, vec!["bob.home@example.org", "bob@example.org"]);

    // Cross-account Inbox mode includes account 2's Inbox message for Peter.
    let list_all = summaries(&s, true, Filter::All, "");
    let peter_all = list_all.iter().find(|c| c.key == "peter@example.org").unwrap();
    assert_eq!(peter_all.message_count, 2);
    assert_eq!(peter_all.unread_count, 2);
    assert_eq!(peter_all.latest_subject, "Second account mail");

    // In Sent folder: lists ONLY correspondents to whom we sent mail (excludes Bob and Peter Alt).
    let acc1 = s.mailbox.accounts[0].id;
    s.view.folder = s.mailbox.folder_of_kind(acc1, FolderKind::Sent).unwrap();
    let sent_list = summaries(&s, false, Filter::All, "");
    let sent_keys: Vec<&str> = sent_list.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(sent_keys, ["peter@example.org"]);
    assert_eq!(sent_list[0].latest_subject, "Re: Data availability");

    // Chat timeline (`messages_for`) still aggregates the full conversation across Inbox, Sent, and Archive.
    let (_, full_ids) = messages_for(&s, "peter@example.org", false, Filter::All, "");
    assert_eq!(full_ids.len(), 3);
}

#[test]
fn manual_merge_and_unmerge_are_non_destructive_and_undoable() {
    let mut s = test_session();
    let msg_count_before = s.mailbox.messages.len();

    let res = run(&mut s, "mail.mergeCorrespondents", json!({"addresses": ["peter@example.org", "peter.alt@example.org"], "name": "Prof. Peter"}));
    assert_eq!(res["groups"], 1);
    assert_eq!(s.mailbox.messages.len(), msg_count_before);

    let list = summaries(&s, false, Filter::All, "");
    let peter = list.iter().find(|c| c.key == "peter.alt@example.org").unwrap();
    assert_eq!(peter.name, "Prof. Peter");
    assert_eq!(peter.message_count, 2);
    assert_eq!(peter.addresses, vec!["peter.alt@example.org", "peter@example.org"]);
    assert!(peter.manually_merged);

    // Chronological order oldest -> newest.
    let (_, ids) = messages_for(&s, "peter@example.org", false, Filter::All, "");
    assert_eq!(ids.len(), 4);
    let subjects: Vec<&str> = ids.iter().map(|id| s.mailbox.message(*id).unwrap().subject.as_str()).collect();
    assert_eq!(subjects, ["Data availability", "Re: Data availability", "MATLAB source code", "Alt address mail"]);

    // Pagination window.
    let (page, older) = paginate_latest(&ids, 2, 0);
    assert_eq!(page, &ids[2..4]);
    assert_eq!(older, 2);

    // Undo restores the unmerged state.
    assert!(s.undo());
    let list_after_undo = summaries(&s, false, Filter::All, "");
    assert_eq!(list_after_undo.len(), 3);
    assert!(s.redo());
    let list_after_redo = summaries(&s, false, Filter::All, "");
    assert_eq!(list_after_redo.len(), 2);

    // Unmerge splits them back cleanly without touching messages.
    let un = run(&mut s, "mail.unmergeCorrespondent", json!({"address": "peter.alt@example.org"}));
    assert_eq!(un["changed"], true);
    assert_eq!(un["groups"], 0);
    assert_eq!(s.mailbox.messages.len(), msg_count_before);
}

#[test]
fn view_commands_and_in_conversation_search_work() {
    let mut s = test_session();
    // Turning on thread conversations, then switching to contact conversations turns thread view off.
    run(&mut s, "view.conversations", json!({"value": true}));
    assert!(s.view.conversations);

    let toggled = run(&mut s, "view.contactConversations", json!({"value": true, "allAccounts": true}));
    assert_eq!(toggled["value"], true);
    assert_eq!(toggled["allAccounts"], true);
    assert!(!s.view.conversations);
    assert!(s.view.contact_conversations);

    run(&mut s, "mail.selectCorrespondent", json!({"key": "peter@example.org"}));
    assert_eq!(s.view.selected_correspondent.as_deref(), Some("peter@example.org"));

    let res = run(&mut s, "mail.correspondentMessages", json!({"query": "MATLAB", "limit": 10}));
    assert_eq!(res["total"], 1);
    assert_eq!(res["messages"][0]["subject"], "MATLAB source code");

    // Switching back to thread conversations turns contact view off.
    run(&mut s, "view.conversations", json!({"value": true}));
    assert!(s.view.conversations);
    assert!(!s.view.contact_conversations);
}

#[test]
fn click_to_mark_read_option_b_and_message_id_work() {
    let mut s = test_session();
    // Add a draft to Peter so we verify drafts are never auto-marked read.
    let drafts1 = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Drafts).unwrap();
    let draft_id = s
        .mailbox
        .add_message(Message {
            folder: drafts1,
            from: Address::new("Avery", "avery@example.org"),
            to: vec![Address::new("Peter", "peter@example.org")],
            subject: "Draft to Peter".into(),
            date: ts(22, 12, 0),
            received: ts(22, 12, 0),
            body: Body { text: "WIP".into(), html: None },
            read: false,
            draft: true,
            ..Default::default()
        })
        .unwrap();

    // Auto-selecting on entering contact view does NOT mark anything read.
    run(&mut s, "view.contactConversations", json!({"value": true}));
    let (_, peter_ids) = messages_for(&s, "peter@example.org", false, Filter::All, "");
    let unread_before: Vec<Id> = peter_ids.iter().copied().filter(|id| !s.mailbox.message(*id).unwrap().read).collect();
    assert_eq!(unread_before.len(), 3); // Data availability, MATLAB source code, and the Draft.
    let rev_before = s.rev();

    // Clicking Peter's row (`mail.selectCorrespondent` without messageId) uses Option B:
    // only the latest unread non-draft message is marked read, without adding an undo step.
    run(&mut s, "mail.selectCorrespondent", json!({"key": "peter@example.org"}));
    assert!(!s.mailbox.message(peter_ids[0]).unwrap().read, "older unread message stays unread");
    assert!(s.mailbox.message(peter_ids[2]).unwrap().read, "latest unread non-draft is marked read");
    assert!(!s.mailbox.message(draft_id).unwrap().read, "drafts are never auto-marked read");
    assert!(!s.can_undo(), "click-to-mark-read does not pollute undo history");
    assert!(s.rev() > rev_before);

    // Clicking a specific older unread bubble marks that message read too.
    run(&mut s, "mail.selectCorrespondent", json!({"key": "peter@example.org", "messageId": peter_ids[0]}));
    assert!(s.mailbox.message(peter_ids[0]).unwrap().read);
    assert_eq!(s.sel.focus(), Some(peter_ids[0]));

    // With `mark_read_on_open = false` or `reading_pane = Pane::Off`, selection marks nothing read.
    run(&mut s, "mail.markUnread", json!({"ids": [peter_ids[0], peter_ids[2]]}));
    s.prefs.mark_read_on_open = false;
    run(&mut s, "mail.selectCorrespondent", json!({"key": "peter@example.org"}));
    assert!(!s.mailbox.message(peter_ids[2]).unwrap().read);

    s.prefs.mark_read_on_open = true;
    s.view.reading_pane = crate::session::Pane::Off;
    run(&mut s, "mail.selectCorrespondent", json!({"key": "peter@example.org", "messageId": peter_ids[0]}));
    assert!(!s.mailbox.message(peter_ids[0]).unwrap().read);
}

#[test]
fn selection_scope_identity_resolution_custom_folders_and_mailbox_replace() {
    let mut s = test_session();
    let acc1 = s.mailbox.accounts[0].id;
    let acc2 = s.mailbox.accounts[1].id;
    let custom1 = s.mailbox.add_folder(acc1, None, "Projects").unwrap();
    let custom2 = s.mailbox.add_folder(acc2, None, "Receipts").unwrap();
    add_msg(&mut s.mailbox, custom1, ("Alpha", "alpha@example.org"), &[("Avery", "avery@example.org")], "P1", ts(23, 1, 0), "p1", false);
    add_msg(&mut s.mailbox, custom2, ("Beta", "beta@example.org"), &[("Work", "avery@work.example")], "R1", ts(23, 2, 0), "r1", false);

    // 1) Custom folder with allAccounts=true only lists correspondents from the active Custom folder.
    s.view.folder = custom1;
    let custom_list = summaries(&s, true, Filter::All, "");
    assert_eq!(custom_list.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(), ["alpha@example.org"]);

    // 2) Junk E-mail folder excludes Junk messages consistently across list and timeline.
    s.view.folder = s.mailbox.folder_of_kind(acc1, FolderKind::Junk).unwrap();
    assert!(summaries(&s, false, Filter::All, "").is_empty());

    // 3) Selecting correspondent in account 2 with explicit allAccounts:true remembers that scope
    // for implicit-key `mail.correspondentMessages`, while explicit `allAccounts:false` overrides it.
    s.view.folder = s.mailbox.folder_of_kind(acc1, FolderKind::Inbox).unwrap();
    let inbox2 = s.mailbox.folder_of_kind(acc2, FolderKind::Inbox).unwrap();
    add_msg(&mut s.mailbox, inbox2, ("Acc2Only", "acc2only@example.org"), &[("Work", "avery@work.example")], "Acc2 hello", ts(23, 3, 0), "hi", false);
    s.view.contact_all_accounts = false;
    run(&mut s, "mail.selectCorrespondent", json!({"key": "acc2only@example.org", "allAccounts": true}));
    let implicit = run(&mut s, "mail.correspondentMessages", json!({}));
    assert_eq!(implicit["total"], 1);
    let err = s.run("mail.correspondentMessages", &json!({"allAccounts": false})).unwrap_err();
    assert!(err.to_string().contains("no conversation"));

    // 4) Merging selected `z@example.org` with Contact-linked `bob.home@example.org` resolves
    // through IdentityMap to canonical key `bob.home@example.org` (`bob@example.org` & `z@example.org`).
    let inbox1 = s.mailbox.folder_of_kind(acc1, FolderKind::Inbox).unwrap();
    add_msg(&mut s.mailbox, inbox1, ("Zed", "z@example.org"), &[("Avery", "avery@example.org")], "Z mail", ts(23, 4, 0), "z", true);
    run(&mut s, "mail.selectCorrespondent", json!({"key": "z@example.org"}));
    run(&mut s, "mail.mergeCorrespondents", json!({"addresses": ["z@example.org", "bob.home@example.org"]}));
    assert_eq!(s.view.selected_correspondent.as_deref(), Some("bob.home@example.org"));

    // 5) `replace_mailbox` clears transient selected correspondent while preserving view mode.
    run(&mut s, "view.contactConversations", json!({"value": true, "allAccounts": true}));
    s.replace_mailbox(Mailbox::with_local_account("New", "new@example.org"));
    assert!(s.view.selected_correspondent.is_none());
    assert!(s.view.selected_correspondent_all_accounts.is_none());
    assert!(s.view.contact_conversations);
    assert!(s.view.contact_all_accounts);
}
