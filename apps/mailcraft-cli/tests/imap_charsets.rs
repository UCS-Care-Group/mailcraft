// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! IMAP mail kept with characters that couldn't be read (#650), from end to end: a mailbox from
//! before Windows-1250 was read has U+FFFD where š, Š and € were. Opened, those messages are
//! marked, and the next Send/Receive downloads them again by UID (`BODY.PEEK[]`, so nothing is
//! marked read on the server), once; what the owner did to them stays, nothing comes twice, and
//! a message whose own bytes are broken isn't downloaded at every run. Every message is invented.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]

#[path = "../../../crates/net/src/fake.rs"]
mod fake;

use fake::{Fake, FakeImap, FakeMailbox, FakeMessage, ImapWorld};
use mailcraft_engine::Session;
use mailcraft_engine::cmd::account::{apply_sync, sync_request};
use mailcraft_engine::cmd::imap_again::{MAX_AGAIN, mark_unreadable};
use mailcraft_model::{FlagStatus, Id, Mailbox};
use serde_json::json;

const PASSWORD: &str = "hunter2";

/// A Windows-1250 message: the sender's name, subject and text with Š, š and the euro sign in it.
fn central_european(n: u32) -> Vec<u8> {
    let mut mail = format!(
        "From: =?windows-1250?Q?Barbara_=8Akrlec?= <barbara@example.org>\r\nTo: Ada <ada@example.org>\r\n\
         Subject: =?windows-1250?Q?Spo=9Atovani_partner_{n}?=\r\nMessage-ID: <cp1250-{n}@example.org>\r\nMIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=windows-1250\r\nContent-Transfer-Encoding: 8bit\r\n\r\n"
    )
    .into_bytes();
    mail.extend_from_slice(b"Spo\x9Atovani, v znesku 155,81 \x80.\r\n");
    mail
}

/// A message that says it is UTF-8 and isn't: it is U+FFFD whenever it is read.
fn broken() -> Vec<u8> {
    b"From: Kit <kit@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Still \xff\r\nMessage-ID: <broken@example.org>\r\n\
      Content-Type: text/plain; charset=utf-8\r\n\r\nBroken \xff text.\r\n"
        .to_vec()
}

fn fake(uid: u32, flags: &str, body: Vec<u8>) -> FakeMessage {
    FakeMessage { uid, flags: flags.into(), internal_date: " 1-Jul-2003 09:52:37 +0000".into(), body, claimed_size: None }
}

struct Rig {
    imap: FakeImap,
    s: Session,
    account: Id,
}

impl Rig {
    fn new(messages: Vec<FakeMessage>) -> Rig {
        let inbox = FakeMailbox { raw: "INBOX".into(), flags: "\\HasNoChildren".into(), uid_validity: 11, messages, refuse_open: false, next_uid: 0 };
        let imap = Fake::imap(ImapWorld { user: "ada@example.org".into(), password: PASSWORD.into(), mailboxes: vec![inbox], ..Default::default() });
        let mut s = Session::at(Mailbox::with_local_account("", ""), 1_800_000_000, 0);
        let servers = json!({
            "email": "ada@example.org", "displayName": "Ada",
            "incoming": {"host": "127.0.0.1", "port": imap.port, "security": "none"},
        });
        let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
        Rig { imap, s, account }
    }

    fn sync(&mut self) {
        let mut request = sync_request(&self.s, self.account).unwrap();
        request.password = PASSWORD.into();
        let result = mailcraft_net::sync(&request);
        assert_eq!(result.errors, Vec::<String>::new());
        apply_sync(&mut self.s, result).unwrap();
        assert!(self.s.mailbox.clone().repair().is_empty());
    }

    /// How many whole messages the server was asked for so far, and whether any was asked for in
    /// a way that marks it read.
    fn downloads(&self) -> usize {
        let world = self.imap.world();
        assert!(world.log.iter().all(|line| !line.contains("BODY[")), "a fetch that sets \\Seen");
        world.log.iter().filter(|line| line.contains("BODY.PEEK[]")).count()
    }

    fn by_uid(&self, uid: u32) -> Id {
        self.s.mailbox.messages.values().find(|m| m.uid == uid).map(|m| m.id).unwrap()
    }

    /// As a version before #178 kept it: U+FFFD where the characters were.
    fn damage(&mut self, uid: u32) {
        let id = self.by_uid(uid);
        let m = self.s.mailbox.message_mut(id).unwrap();
        m.subject = "Spo\u{FFFD}tovani partner".into();
        m.from.name = "Barbara \u{FFFD}krlec".into();
        m.body.text = "Spo\u{FFFD}tovani, v znesku 155,81 .\r\n".into();
    }
}

#[test]
fn imap_mail_kept_with_unreadable_characters_is_downloaded_again_once() {
    let mut r = Rig::new(vec![fake(1, "", central_european(1)), fake(2, "\\Seen", central_european(2)), fake(3, "", broken())]);
    r.sync();
    // (Read properly as it comes down today.)
    assert_eq!(r.s.mailbox.message(r.by_uid(1)).unwrap().subject, "Spoštovani partner 1");
    assert_eq!(r.downloads(), 3);

    // The mailbox as it was kept before: uid 1 and 2 damaged, 2 flagged and categorised here.
    r.damage(1);
    r.damage(2);
    let two = r.by_uid(2);
    {
        let m = r.s.mailbox.message_mut(two).unwrap();
        m.flag = FlagStatus::Flagged;
        m.categories = vec!["Blue category".into()];
    }
    let (folder, count) = (r.s.mailbox.message(two).unwrap().folder, r.s.mailbox.messages.len());

    // Uid 1 and 2 are marked, and uid 3 too: its own bytes are broken, which a look cannot tell.
    assert_eq!(mark_unreadable(&mut r.s.mailbox), 3);
    assert_eq!(mark_unreadable(&mut r.s.mailbox), 0, "marked once");
    // Nothing changes until the server is asked.
    assert!(r.s.mailbox.message(two).unwrap().subject.contains('\u{FFFD}'));
    let request = sync_request(&r.s, r.account).unwrap();
    assert_eq!(request.folders.iter().map(|f| f.again.clone()).find(|a| !a.is_empty()), Some(vec![1, 2, 3]));

    r.sync();
    assert_eq!(r.downloads(), 6, "the three asked for, whole, by UID");
    let m = r.s.mailbox.message(two).unwrap();
    assert_eq!((m.subject.as_str(), m.from.name.as_str()), ("Spoštovani partner 2", "Barbara Škrlec"));
    assert_eq!(m.body.text.trim(), "Spoštovani, v znesku 155,81 €.");
    // Everything else as it was.
    assert!(m.read && m.flag == FlagStatus::Flagged && m.categories == ["Blue category"] && m.folder == folder && !m.refetch);
    assert_eq!(r.s.mailbox.messages.len(), count, "no copy");
    // The server's marks are as they were, but for the flag set here, which goes up as it should:
    // asked for with PEEK, nothing was marked read.
    let flags: Vec<(u32, String)> = r.imap.world().mailbox("INBOX").messages.iter().map(|m| (m.uid, m.flags.clone())).collect();
    assert_eq!(flags, [(1, String::new()), (2, "\\Seen \\Flagged".into()), (3, String::new())]);
    // The one whose bytes are broken is as it was, and settled all the same.
    let three = r.s.mailbox.message(r.by_uid(3)).unwrap();
    assert!(three.body.text.contains('\u{FFFD}') && !three.refetch);

    // Never again: not at the next run, nor after a save and an open.
    r.sync();
    assert_eq!(r.downloads(), 6);
    let json = serde_json::to_string(&r.s.mailbox).unwrap();
    assert!(!json.contains(r#""refetch":true"#));
    let mut again: Mailbox = serde_json::from_str(&json).unwrap();
    // Not marked again, though message 3 still holds U+FFFD.
    assert_eq!(mark_unreadable(&mut again), 0);
    assert_eq!(mark_unreadable(&mut r.s.mailbox), 0);
}

#[test]
fn what_is_asked_for_again_is_bounded_and_a_message_the_server_lost_is_left() {
    let mut r = Rig::new((1..=3).map(|uid| fake(uid, "", central_european(uid))).collect());
    r.sync();
    let before = r.downloads();
    for uid in 1..=3 {
        r.damage(uid);
    }
    assert_eq!(mark_unreadable(&mut r.s.mailbox), 3);
    // Gone from the server meanwhile: not asked for, and removed by the run as any other.
    r.imap.world().mailbox("INBOX").messages.retain(|m| m.uid != 2);
    r.sync();
    assert_eq!(r.downloads() - before, 2);
    assert!(r.s.mailbox.messages.values().all(|m| !m.refetch && !m.subject.contains('\u{FFFD}')));
    assert_eq!(r.s.mailbox.messages.len(), 2);

    // A request never asks for more than a bound, over all folders.
    let mut many = Mailbox::with_local_account("a@example.org", "");
    let account = many.accounts[0].id;
    let inbox = many.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    for uid in 1..=(MAX_AGAIN as u32 + 50) {
        let message = mailcraft_model::Message { account, folder: inbox, uid, subject: "\u{FFFD}".into(), refetch: true, ..Default::default() };
        many.add_message(message).unwrap();
    }
    assert_eq!(mailcraft_engine::cmd::imap_again::wanted(&many, account)[&inbox].len(), MAX_AGAIN);
}

#[test]
fn a_refused_download_again_is_told_once_and_not_asked_for_again() {
    let mut r = Rig::new(vec![fake(1, "", central_european(1)), fake(2, "", central_european(2))]);
    r.sync();
    r.damage(1);
    r.damage(2);
    assert_eq!(mark_unreadable(&mut r.s.mailbox), 2);
    // The server answers NO to the download of uid 1.
    r.imap.world().refuse_uid = Some(1);
    let mut request = sync_request(&r.s, r.account).unwrap();
    request.password = PASSWORD.into();
    let result = mailcraft_net::sync(&request);
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.errors[0].contains("again"), "{:?}", result.errors);
    apply_sync(&mut r.s, result).unwrap();
    // Uid 2 is read anew, uid 1 is settled as it was.
    assert_eq!(r.s.mailbox.message(r.by_uid(2)).unwrap().subject, "Spo\u{161}tovani partner 2");
    let one = r.s.mailbox.message(r.by_uid(1)).unwrap();
    assert!(one.subject.contains('\u{FFFD}') && !one.refetch && one.refetched);
    // Nothing more is asked for, nor said.
    let before = r.imap.world().log.len();
    r.sync();
    assert!(r.imap.world().log[before..].iter().all(|line| !line.contains("BODY.PEEK[]")));
}
