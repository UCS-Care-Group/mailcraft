// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list of the phone: filters, conversations, search and the Inbox split, read
//! without moving the session's own view, and the triage commands with their undo, through
//! `Core::execute` as the Android app runs them.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::sync::{FolderDelta, GmailSorted, SyncResult};
use mailcraft_model::{Address, FolderKind, GmailCategory, Message};
use serde_json::{Value, json};

use crate::fake_tests::{FakePlatform, block_on, sample_core};
use crate::{Core, CoreError, ListQuery};

fn query(folder: u64) -> ListQuery {
    ListQuery { folder, filter: "all".into(), split: String::new(), conversations: false, search: String::new(), everywhere: false }
}

fn inbox(core: &Core) -> u64 {
    block_on(core.folders(0)).unwrap().folders.iter().find(|f| f.kind == "inbox").unwrap().id
}

fn exec(core: &Core, id: &str, params: Value) -> Result<Value, CoreError> {
    block_on(core.execute(id.to_string(), params.to_string())).map(|s| serde_json::from_str(&s).unwrap())
}

fn all(core: &Core, q: ListQuery) -> Vec<crate::MessageRow> {
    block_on(core.message_list(q, 0, 200)).unwrap().rows
}

#[test]
fn filters_narrow_the_list_and_the_total_counts_the_whole_list() {
    let core = sample_core();
    let folder = inbox(&core);
    let everything = block_on(core.message_list(query(folder), 0, 3)).unwrap();
    assert_eq!(everything.rows.len(), 3);
    assert!(everything.total > 3);
    let unread = all(&core, ListQuery { filter: "unread".into(), ..query(folder) });
    assert!(!unread.is_empty() && unread.len() < usize::try_from(everything.total).unwrap());
    assert!(unread.iter().all(|r| r.unread));
    let flagged = all(&core, ListQuery { filter: "flagged".into(), ..query(folder) });
    assert!(flagged.iter().all(|r| r.flagged));
    let with_files = all(&core, ListQuery { filter: "attachments".into(), ..query(folder) });
    assert!(with_files.iter().all(|r| r.has_attachments));
    // A window past the end is empty, and says how long the list is.
    let past = block_on(core.message_list(query(folder), everything.total, 10)).unwrap();
    assert!(past.rows.is_empty());
    assert_eq!(past.total, everything.total);
}

#[test]
fn a_search_reads_what_is_on_the_phone_and_conversations_fold_rows() {
    let core = sample_core();
    let folder = inbox(&core);
    let plain = all(&core, query(folder));
    let word = plain.first().and_then(|r| r.subject.split_whitespace().find(|w| w.len() > 3)).unwrap().to_string();
    let found = all(&core, ListQuery { search: word.clone(), ..query(folder) });
    assert!(!found.is_empty() && found.len() <= plain.len());
    assert!(found.iter().any(|r| r.subject.contains(&word) || r.preview.contains(&word) || r.from_name.contains(&word)));
    assert!(all(&core, ListQuery { search: "zzzz-nothing-like-this-zzzz".into(), ..query(folder) }).is_empty());
    let folded = all(&core, ListQuery { conversations: true, ..query(folder) });
    assert!(folded.len() <= plain.len());
    assert!(folded.iter().all(|r| r.members >= 1));
    assert!(plain.iter().all(|r| r.members == 1));
}

#[test]
fn reading_a_list_leaves_the_sessions_own_view_as_it_was() {
    let core = sample_core();
    let folder = inbox(&core);
    let before = block_on(core.call(|s| Ok((format!("{:?}", s.view), s.rev())))).unwrap();
    let other = block_on(core.folders(0)).unwrap().folders.iter().map(|f| f.id).find(|id| *id != folder).unwrap();
    for q in [
        ListQuery { filter: "unread".into(), search: "a".into(), conversations: true, everywhere: true, ..query(other) },
        ListQuery { search: "x".into(), ..query(folder) },
    ] {
        let _ = block_on(core.message_list(q, 0, 50));
    }
    let after = block_on(core.call(|s| Ok((format!("{:?}", s.view), s.rev())))).unwrap();
    assert_eq!(before, after);
}

#[test]
fn a_query_that_names_nothing_real_is_refused() {
    let core = sample_core();
    let folder = inbox(&core);
    for q in [
        ListQuery { folder: u64::MAX, ..query(folder) },
        ListQuery { filter: "nonsense".into(), ..query(folder) },
        ListQuery { split: "social".into(), ..query(folder) },
        ListQuery { split: "nonsense".into(), ..query(folder) },
    ] {
        assert!(matches!(block_on(core.message_list(q.clone(), 0, 5)), Err(CoreError::BadRequest { .. })), "{q:?}");
    }
    assert!(block_on(core.inbox_split(u64::MAX)).is_err());
    // The sample's Inbox is nobody's split.
    assert!(block_on(core.inbox_split(folder)).unwrap().tabs.is_empty());
}

fn message(uid: u32, subject: &str, category: GmailCategory) -> Message {
    Message {
        uid,
        from: Address::new("Rowan", "rowan@sender.example"),
        subject: subject.into(),
        message_id: format!("<{uid}@sender.example>"),
        date: 1_700_000_000 + i64::from(uid),
        gmail_category: Some(category),
        gmail_id: 9_000 + u64::from(uid),
        ..Default::default()
    }
}

fn gmail_core() -> Arc<Core> {
    Core::start(FakePlatform::new(), || {
        let mut s = Session::empty();
        let account = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
        let id = s.run("account.add", &account).ok().and_then(|v| v["id"].as_u64()).unwrap_or(0);
        let inbox = s.mailbox.folder_of_kind(id, FolderKind::Inbox).unwrap_or(0);
        let new = vec![
            message(1, "Dinner", GmailCategory::Primary),
            message(2, "Sale", GmailCategory::Promotions),
            message(3, "Offer", GmailCategory::Promotions),
            message(4, "Tagged", GmailCategory::Social),
        ];
        let gmail =
            new.iter().map(|m| GmailSorted { uid: m.uid, category: m.gmail_category.unwrap_or(GmailCategory::Primary), id: m.gmail_id }).collect();
        let folder = FolderDelta {
            id: Some(inbox),
            remote: "INBOX".into(),
            kind: FolderKind::Inbox,
            uid_validity: 7,
            uid_seen: 4,
            new,
            gmail,
            ..Default::default()
        };
        let result = SyncResult { account: id, folders: vec![folder], ..Default::default() };
        let _ = s.run("sync.apply", &json!({"result": result}));
        s
    })
    .unwrap()
}

#[test]
fn an_inbox_split_has_its_tabs_with_counts_and_each_tab_lists_its_own_mail() {
    let core = gmail_core();
    let folder = inbox(&core);
    let split = block_on(core.inbox_split(folder)).unwrap();
    let codes: Vec<&str> = split.tabs.iter().map(|t| t.code.as_str()).collect();
    assert_eq!(codes, ["primary", "social", "promotions", "updates", "forums"]);
    let promotions = split.tabs.iter().find(|t| t.code == "promotions").unwrap();
    assert_eq!((promotions.total, promotions.unread), (2, 2));
    let rows = all(&core, ListQuery { split: "promotions".into(), ..query(folder) });
    let mut subjects: Vec<&str> = rows.iter().map(|r| r.subject.as_str()).collect();
    subjects.sort_unstable();
    assert_eq!(subjects, ["Offer", "Sale"]);
    assert_eq!(all(&core, ListQuery { split: "primary".into(), ..query(folder) }).len(), 1);
    assert_eq!(all(&core, query(folder)).len(), 4);
}

#[test]
fn triage_commands_change_the_list_and_undo_puts_it_back() {
    let core = sample_core();
    let folder = inbox(&core);
    let rows = all(&core, query(folder));
    let first = rows[0].clone();
    let total = block_on(core.message_list(query(folder), 0, 1)).unwrap().total;

    // Read and unread, flag and unflag.
    exec(&core, "mail.markUnread", json!({"ids": [first.id]})).unwrap();
    assert!(all(&core, query(folder))[0].unread);
    exec(&core, "mail.markRead", json!({"ids": [first.id]})).unwrap();
    assert!(!all(&core, query(folder))[0].unread);
    exec(&core, "mail.toggleFlag", json!({"ids": [first.id]})).unwrap();
    assert_ne!(all(&core, query(folder))[0].flagged, first.flagged);

    // Delete goes from the list, and undo brings it back.
    exec(&core, "mail.delete", json!({"ids": [first.id]})).unwrap();
    let gone = block_on(core.message_list(query(folder), 0, 1)).unwrap();
    assert_eq!(gone.total, total - 1);
    exec(&core, "edit.undo", json!({})).unwrap();
    assert_eq!(block_on(core.message_list(query(folder), 0, 1)).unwrap().total, total);

    // Archive, and a move to another folder.
    let target = block_on(core.folders(0)).unwrap().folders.iter().find(|f| f.kind == "drafts").map(|f| f.id).unwrap();
    exec(&core, "mail.move", json!({"ids": [first.id], "folder": target})).unwrap();
    assert_eq!(block_on(core.message_list(query(folder), 0, 1)).unwrap().total, total - 1);
    assert!(all(&core, query(target)).iter().any(|r| r.id == first.id));
    exec(&core, "edit.undo", json!({})).unwrap();
    exec(&core, "mail.categorize", json!({"category": "Red category", "value": true, "ids": [first.id]})).unwrap();
    assert!(all(&core, query(folder)).iter().find(|r| r.id == first.id).unwrap().categories.contains(&"Red category".to_string()));
}
