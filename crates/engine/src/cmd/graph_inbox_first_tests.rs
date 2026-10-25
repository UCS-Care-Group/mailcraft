// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: the Inbox is taken in before the run's housekeeping, so what was changed here and has not
//! gone up yet must be there, and go up, after it.

use mailcraft_model::sync::{GraphMailResult, GraphMessage, MessageChange, MessagePage, ServerMessage};
use mailcraft_model::{Folder, FolderKind, Message, ServerFolder};
use serde_json::json;

use super::*;

const T: i64 = 1_790_000_000;

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    s.clock.now = Some(T);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    (s, account)
}

fn take(s: &mut Session, account: Id, mut result: GraphMailResult) -> Value {
    result.account = account;
    result.email = "owner@example.org".into();
    apply_graph_mail(s, result).unwrap()
}

fn there(id: &str, read: bool) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: "F-in".into(), read, ..Default::default() },
        subject: id.into(),
        ..Default::default()
    }
}

#[test]
fn an_inbox_known_but_never_read_has_a_pass_from_its_start_and_one_never_listed_has_none() {
    let (mut s, account) = session();
    assert!(graph_inbox_request(&s, account).is_none(), "never listed: the run lists the folders first");
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), ..Default::default() });
    let request = graph_inbox_request(&s, account).unwrap();
    assert!(request.links.is_empty() && request.inbox_pass.is_some_and(|inbox| inbox.id == "F-in"));
    let page = MessagePage { folder: "F-in".into(), messages: vec![there("M-1", false)], link: "L1".into(), caught_up: true, ..Default::default() };
    take(&mut s, account, GraphMailResult { pages: vec![page], partial: true, inbox_pass: true, ..Default::default() });
    assert_eq!(s.mailbox.messages.values().filter(|m| m.folder == inbox && m.subject == "M-1").count(), 1);
    assert_eq!(graph_inbox_request(&s, account).unwrap().links[0].link, "L1");
}

#[test]
fn what_was_changed_here_and_not_sent_yet_wins_over_the_inbox_delta_that_comes_first() {
    let (mut s, account) = session();
    let folder = |s: &mut Session, kind: FolderKind, id: &str| {
        let f = match s.mailbox.folder_of_kind(account, kind) {
            Some(f) => f,
            None => {
                let f = s.mailbox.alloc_id();
                s.mailbox.folders.push(Folder { id: f, account, name: kind.default_name().into(), kind, ..Default::default() });
                f
            }
        };
        s.mailbox.folder_mut(f).unwrap().server = Some(ServerFolder { id: id.into(), ..Default::default() });
        if kind == FolderKind::Inbox {
            let server = s.mailbox.folder_mut(f).unwrap().server.as_mut().unwrap();
            (server.delta, server.caught_up) = ("L0".into(), true);
        }
        f
    };
    let inbox = folder(&mut s, FolderKind::Inbox, "F-in");
    let archive = folder(&mut s, FolderKind::Archive, "F-ar");
    let add = |s: &mut Session, id: &str| {
        let server = ServerMessage { id: id.into(), folder: "F-in".into(), ..Default::default() };
        s.mailbox.add_message(Message { folder: inbox, subject: id.into(), server: Some(server), ..Default::default() }).unwrap()
    };
    let (read_here, moved_here, deleted_there) = (add(&mut s, "M-1"), add(&mut s, "M-2"), add(&mut s, "M-3"));
    // Changed here, not sent: read, moved to the Archive, and and another moved to the Archive that the server took out of the Inbox meanwhile.
    s.run("mail.markRead", &json!({"ids": [read_here]})).unwrap();
    s.run("mail.move", &json!({"ids": [moved_here], "folder": archive})).unwrap();
    s.run("mail.move", &json!({"ids": [deleted_there], "folder": archive})).unwrap();
    let before = graph_mail_request(&s, account).unwrap().messages;
    assert_eq!(before.len(), 3, "{before:?}");

    // The Inbox pass comes first: the server has M-1 flagged and M-2 read, and M-3 gone from the Inbox.
    let mut flagged = there("M-1", false);
    flagged.server.flag = mailcraft_model::FlagStatus::Flagged;
    let page = MessagePage {
        folder: "F-in".into(),
        messages: vec![flagged, there("M-2", true)],
        removed: vec!["M-3".into()],
        link: "L1".into(),
        caught_up: true,
        ..Default::default()
    };
    take(&mut s, account, GraphMailResult { pages: vec![page], partial: true, inbox_pass: true, ..Default::default() });

    // Here wins for what was changed here, the server for the rest.
    let m1 = s.mailbox.message(read_here).unwrap();
    assert!(m1.read && m1.flag == mailcraft_model::FlagStatus::Flagged, "read here stays, flagged there comes");
    let m2 = s.mailbox.message(moved_here).unwrap();
    assert!(m2.read && m2.folder == archive, "moved here stays moved, read there comes");
    let m3 = s.mailbox.message(deleted_there).unwrap();
    assert_eq!(m3.folder, archive, "a message moved here is not taken back by the Inbox pass");
    // And the changes are still to go up with the run that follows, each once.
    let after = graph_mail_request(&s, account).unwrap().messages;
    let moves = after.iter().filter(|c| matches!(c, MessageChange::Move { to, .. } if to == "F-ar")).count();
    let reads = after.iter().filter(|c| matches!(c, MessageChange::Update { read: Some(true), message, .. } if *message == read_here)).count();
    // (M-3 left the Inbox on the server too: its move has nothing left to do, and nothing is
    // done twice.)
    assert_eq!((moves, reads), (1, 1), "{after:?}");
    // The Inbox is not read twice: the next pass starts from the link the pass moved on.
    assert_eq!(graph_inbox_request(&s, account).unwrap().links[0].link, "L1");
}
