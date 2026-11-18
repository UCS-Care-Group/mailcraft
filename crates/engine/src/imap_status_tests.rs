// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: what an IMAP server's STATUS said of a folder read whole is kept with the folder, handed
//! back to the next run while it is good for passing the folder over, and let go of when it isn't.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{FolderKind, ImapStatus};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::*;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn status() -> ImapStatus {
    ImapStatus { messages: 3, uid_next: 4, unseen: 1, limit: 0, at: 0 }
}

fn read(account: u64, uid_validity: u32, complete: bool, status: Option<ImapStatus>) -> SyncResult {
    SyncResult {
        account,
        folders: vec![FolderDelta { remote: "Work".into(), uid_validity, uid_seen: 3, complete, status, ..Default::default() }],
        ..Default::default()
    }
}

fn state_of(s: &Session, account: u64) -> Option<ImapStatus> {
    let request = cmd::account::sync_request(s, account).unwrap();
    request.folders.iter().find(|f| f.remote == "Work").and_then(|f| f.status)
}

#[test]
fn a_status_kept_with_a_whole_reading_is_good_for_an_hour() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let a = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(state_of(&s, a), None);

    cmd::account::apply_sync(&mut s, read(a, 7, true, Some(status()))).unwrap();
    let kept = state_of(&s, a).unwrap();
    assert_eq!((kept.messages, kept.uid_next, kept.unseen, kept.at), (3, 4, 1, now()));
    let folder = s.mailbox.folders.iter().find(|f| f.remote == "Work").unwrap();
    assert_eq!(folder.imap_status, Some(kept));

    // Not any more an hour after.
    s.clock.now = Some(now() + 3_599);
    assert!(state_of(&s, a).is_some());
    s.clock.now = Some(now() + 3_600);
    assert_eq!(state_of(&s, a), None);
    // A clock put back is never trusted.
    s.clock.now = Some(now() - 1);
    assert_eq!(state_of(&s, a), None);
}

#[test]
fn a_reading_that_didnt_come_to_the_end_or_a_new_numbering_leaves_none() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let a = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    cmd::account::apply_sync(&mut s, read(a, 7, true, Some(status()))).unwrap();
    // Read again but not to its end: what was kept is not replaced, and the next STATUS differs from it.
    cmd::account::apply_sync(&mut s, read(a, 7, false, None)).unwrap();
    assert!(state_of(&s, a).is_some());
    // Read whole with no STATUS to keep (the server gave none): nothing is kept.
    cmd::account::apply_sync(&mut s, read(a, 7, true, None)).unwrap();
    assert_eq!(state_of(&s, a), None);
    // Numbered anew: what was kept is for numbers that mean nothing now.
    cmd::account::apply_sync(&mut s, read(a, 7, true, Some(status()))).unwrap();
    cmd::account::apply_sync(&mut s, read(a, 8, false, None)).unwrap();
    assert_eq!(state_of(&s, a), None);
    let _ = FolderKind::Custom;
}

#[test]
fn a_damaged_file_whose_mail_is_fetched_again_passes_no_folder_over() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let a = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    cmd::account::apply_sync(&mut s, read(a, 7, true, Some(status()))).unwrap();
    assert!(state_of(&s, a).is_some());

    crate::store::forget_how_far_fetched(&mut s.mailbox);
    assert_eq!(state_of(&s, a), None);
}
