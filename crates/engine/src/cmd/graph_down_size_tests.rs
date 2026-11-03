// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #300: a Microsoft account's message keeps the size of the MIME fetched whole, through its
//! being kept as its preview and the deltas that say no size.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{AccountKind, Folder, FolderKind, Mailbox, Message, ServerFolder};

use super::*;

const MIME: &[u8] = b"From: Kit <kit@example.org>\r\nTo: avery@mail.example\r\nSubject: The whole of it\r\nMessage-ID: <w1@example.org>\r\nContent-Type: text/plain\r\n\r\nAll of the text, and then some more of it.\r\n";

/// A Microsoft account's Inbox with one message the delta brought as its headers and preview.
fn mailbox(draft: bool) -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = AccountKind::Microsoft;
    let folder = mb.alloc_id();
    let server = Some(ServerFolder { id: "F".into(), ..Default::default() });
    mb.folders.push(Folder { id: folder, account, name: "Inbox".into(), kind: FolderKind::Inbox, server, ..Default::default() });
    let server = ServerMessage { id: "AAkA1".into(), folder: "F".into(), change_key: "CK1".into(), ..Default::default() };
    let body = Body { text: "All of the text".into(), html: None };
    let m = Message { folder, unfetched: true, draft, server: Some(server), subject: "The whole of it".into(), body, ..Default::default() };
    let id = mb.add_message(m).unwrap();
    (mb, account, id)
}

/// The delta saying `id` again, with no size, as a delta always does.
fn delta(id: &str, change_key: &str) -> MessagePage {
    let server = ServerMessage { id: id.into(), folder: "F".into(), change_key: change_key.into(), read: true, ..Default::default() };
    MessagePage {
        folder: "F".into(),
        messages: vec![GraphMessage { server, subject: "The whole of it".into(), ..Default::default() }],
        ..Default::default()
    }
}

fn size(mb: &Mailbox, id: Id) -> u64 {
    mb.message(id).unwrap().server.as_ref().unwrap().size
}

#[test]
fn the_size_fetched_whole_outlasts_the_preview_and_the_delta() {
    let (mut mb, account, id) = mailbox(false);
    let mut index = ServerIndex::default();
    assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", MIME));
    assert_eq!(size(&mb, id), MIME.len() as u64);
    // Outside the mail to keep offline: back to its preview, and its size is still the whole's.
    thin_message(mb.message_mut(id).unwrap(), "All of the text".into());
    let m = mb.message(id).unwrap();
    assert!(m.unfetched && m.size() < MIME.len() as u64);
    assert_eq!(m.list_size(), Some(MIME.len() as u64));
    // A delta, read elsewhere: it is read here too, and its size stays.
    take_page(&mut mb, &mut index, account, &delta("AAkA1", "CK1"), &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.read && m.unfetched);
    assert_eq!((size(&mb, id), m.list_size()), (MIME.len() as u64, Some(MIME.len() as u64)));
    // Saying the same again changes nothing.
    let mut down = Down::default();
    take_page(&mut mb, &mut index, account, &delta("AAkA1", "CK1"), &mut down);
    assert_eq!(down.changed, 0);
}

#[test]
fn a_size_the_server_gives_is_taken_and_one_it_doesnt_keeps_the_last() {
    let (mut mb, account, id) = mailbox(false);
    let mut index = ServerIndex::default();
    let mut page = delta("AAkA1", "CK1");
    page.messages[0].server.size = 30 << 20;
    take_page(&mut mb, &mut index, account, &page, &mut Down::default());
    assert_eq!(size(&mb, id), 30 << 20);
    take_page(&mut mb, &mut index, account, &delta("AAkA1", "CK1"), &mut Down::default());
    assert_eq!(size(&mb, id), 30 << 20);
    // Fetched whole, it is the size of what was fetched, measured here.
    assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", MIME));
    assert_eq!(size(&mb, id), MIME.len() as u64);
}

/// A draft changed elsewhere is fetched again, whatever the size of the version before, and has
/// the size of what came the second time.
#[test]
fn a_draft_fetched_again_has_its_new_size() {
    let (mut mb, account, id) = mailbox(true);
    let mut index = ServerIndex::default();
    assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", MIME));
    assert_eq!(size(&mb, id), MIME.len() as u64);
    // Larger than the background fetch takes (10 MB), with an attachment since taken off on the web.
    mb.message_mut(id).unwrap().server.as_mut().unwrap().size = 15 << 20;
    let mut page = delta("AAkA1", "CK2");
    page.messages[0].draft = Some(true);
    take_page(&mut mb, &mut index, account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.unfetched && m.draft, "changed elsewhere, it is to be fetched again");
    // The old version's size isn't the new one's: it is in the background fetch, and the list
    // says what is here meanwhile.
    assert_eq!(size(&mb, id), 0);
    assert_eq!(fetch_queue(&mb, account, 10, 0), ["AAkA1"]);
    assert_eq!(m.list_size(), Some(m.size()));
    let longer = [MIME, b"A paragraph added on the web.\r\n".as_slice()].concat();
    assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", &longer));
    assert_eq!(size(&mb, id), longer.len() as u64);
    // Fetched once more, as it is after the paragraph was taken out again: a size already
    // known gives way to the one measured.
    assert!(take_fetched(&mut mb, &mut index, account, "AAkA1", MIME));
    assert_eq!(size(&mb, id), MIME.len() as u64);
}

/// A message being moved out of the account has no server state of its own here, and fetching
/// it whole gives it none.
#[test]
fn a_message_being_moved_out_is_given_no_server_state() {
    let (mut mb, account, id) = mailbox(false);
    let m = mb.message_mut(id).unwrap();
    m.server = None;
    m.moving_out = Some(mailcraft_model::sync::MovingOut { account, id: "AAkA1".into(), ..Default::default() });
    assert!(take_fetched(&mut mb, &mut ServerIndex::default(), account, "AAkA1", MIME));
    let m = mb.message(id).unwrap();
    assert!(m.server.is_none() && !m.unfetched);
    assert_eq!(m.list_size(), Some(m.size()));
}

/// #611: a draft that was sent keeps its id when the server files it in Sent Items; the delta
/// then says it is no longer a draft, and the copy here stops being shown as one.
#[test]
fn a_draft_the_server_says_was_sent_is_no_longer_a_draft() {
    let (mut mb, account, id) = mailbox(true);
    mb.message_mut(id).unwrap().server_draft = Some(Default::default());
    mb.message_mut(id).unwrap().unfetched = false;
    let mut index = ServerIndex::default();
    // A delta that leaves `isDraft` out says nothing of it: still a draft.
    take_page(&mut mb, &mut index, account, &delta("AAkA1", "CK1"), &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.server_draft.is_some());
    let mut page = delta("AAkA1", "CK2");
    page.messages[0].draft = Some(false);
    take_page(&mut mb, &mut index, account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(!m.draft && m.server_draft.is_none());
    // What was sent is fetched afresh: it may not be the draft last seen here.
    assert!(m.unfetched, "the sent version is to be fetched");
}

/// A draft saved here that waits to go up is a draft still, whatever the server's copy says.
#[test]
fn a_draft_waiting_to_go_up_stays_a_draft() {
    let (mut mb, account, id) = mailbox(true);
    mb.message_mut(id).unwrap().server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, ..Default::default() });
    let mut index = ServerIndex::default();
    let mut page = delta("AAkA1", "CK1");
    page.messages[0].draft = Some(false);
    take_page(&mut mb, &mut index, account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.server_draft.is_some());
}
