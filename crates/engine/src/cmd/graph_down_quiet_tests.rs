// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1784: an answer that never named a property says nothing about it. Kerio's light lists carry
//! no read state; a client that reads that silence as "unread" turned a folder of read mail
//! unread on screen — and the three-way merge, seeing the damage as a local change, would have
//! pushed it to the server. Silence is held, never applied: the merge keeps what it has, and a
//! server that does speak is still followed.

use mailcraft_model::sync::{GraphMessage, MessagePage, ServerMessage};
use mailcraft_model::{FlagStatus, Folder, FolderKind, Mailbox, Message, ServerFolder};

use super::*;

/// A Microsoft account with one read, unflagged message in its Inbox, as the server last said.
fn mailbox() -> (Mailbox, Id, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.accounts[0].id;
    let folder = mb.alloc_id();
    let server = Some(ServerFolder { id: "F".into(), ..Default::default() });
    mb.folders.push(Folder { id: folder, account, name: "Inbox".into(), kind: FolderKind::Inbox, server, ..Default::default() });
    let server = ServerMessage {
        id: "E1".into(),
        folder: "F".into(),
        read: true,
        read_said: true,
        flag: FlagStatus::Flagged,
        flag_said: true,
        ..Default::default()
    };
    let m = Message { folder, read: true, flag: FlagStatus::Flagged, server: Some(server), subject: "Heavy".into(), ..Default::default() };
    let id = mb.add_message(m).unwrap();
    (mb, account, id)
}

/// The message after a page lists it with `now` as the server's answer.
fn apply(now: ServerMessage) -> (bool, FlagStatus) {
    let (mut mb, account, id) = mailbox();
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server: now, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    (m.read, m.flag)
}

/// The fault itself, pinned: a listing that says nothing about the read state must not turn a
/// read message unread, whatever the bool in the answer happens to be.
#[test]
fn an_answer_silent_about_the_read_state_leaves_a_read_message_read() {
    let silent = ServerMessage { id: "E1".into(), folder: "F".into(), ..Default::default() };
    assert_eq!(apply(silent), (true, FlagStatus::Flagged), "silence is silence: held, not applied");
}

/// The same for the flag: an answer that never named it says nothing — the light list of a
/// stepped-down run asks no flag, and its silence must not clear a flagged message.
#[test]
fn an_answer_silent_about_the_flag_leaves_a_flagged_message_flagged() {
    let silent = ServerMessage { id: "E1".into(), folder: "F".into(), read: true, read_said: true, ..Default::default() };
    assert_eq!(apply(silent), (true, FlagStatus::Flagged), "a page that says nothing about flags clears nothing");
}

/// And the fix must not mute the server that does speak: the ordinary following still applies.
#[test]
fn an_answer_that_does_speak_is_still_followed() {
    let speaking = ServerMessage {
        id: "E1".into(),
        folder: "F".into(),
        read: false,
        read_said: true,
        flag: FlagStatus::None,
        flag_said: true,
        ..Default::default()
    };
    assert_eq!(apply(speaking), (false, FlagStatus::None), "a server that says unread and unflagged is followed, as ever");
}

/// The snapshot, not just the body: a silent answer must leave the agreed state intact, or the
/// held `read` and a rewritten snapshot differ by an invention — which the up-direction reads as
/// a change made here, and would push. And a later answer that does speak must still be
/// followed: a held value must not be mistaken for a local edit contradicting it.
#[test]
fn a_silence_leaves_the_agreed_state_intact_and_a_later_word_is_still_followed() {
    let (mut mb, account, id) = mailbox();
    let silent = ServerMessage { id: "E1".into(), folder: "F".into(), ..Default::default() };
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server: silent, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    {
        let m = mb.message(id).unwrap();
        let server = m.server.as_ref().unwrap();
        assert_eq!((m.read, server.read), (true, true), "body and snapshot hold the same read state: nothing waiting to push");
        assert_eq!((m.flag, server.flag), (FlagStatus::Flagged, FlagStatus::Flagged), "and the same flag");
    }
    // The server speaks afterwards: still followed, as one untouched by the silence.
    let speaking = ServerMessage {
        id: "E1".into(),
        folder: "F".into(),
        read: false,
        read_said: true,
        flag: FlagStatus::None,
        flag_said: true,
        ..Default::default()
    };
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server: speaking, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert_eq!((m.read, m.flag), (false, FlagStatus::None), "the webmail change after the silence is taken, not swallowed");
}

/// A sent box that a silent listing fills: mail arrives there already sent and read, so the
/// first landing guesses read — Kerio's own unread counter for Sent Items says the same, and
/// the folder badge agreed with it against the list. A server that does say unread is still
/// followed into the sent box: the guess only fills the silence.
#[test]
fn a_silent_listing_fills_a_sent_box_with_read_mail_and_a_said_unread_is_still_taken() {
    let (mut mb, account, _) = mailbox();
    let sent = mb.alloc_id();
    mb.folders.push(Folder {
        id: sent,
        account,
        name: "Sent Items".into(),
        kind: FolderKind::Sent,
        server: Some(ServerFolder { id: "S".into(), ..Default::default() }),
        ..Default::default()
    });
    let silent = ServerMessage { id: "E7".into(), folder: "S".into(), ..Default::default() };
    let said = ServerMessage { id: "E8".into(), folder: "S".into(), read: false, read_said: true, ..Default::default() };
    let page = MessagePage {
        folder: "S".into(),
        messages: vec![GraphMessage { server: silent, ..Default::default() }, GraphMessage { server: said, ..Default::default() }],
        ..Default::default()
    };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let of = |id: &str| mb.messages.iter().map(|(_, m)| m).find(|m| m.server.as_ref().is_some_and(|s| s.id == id)).map(|m| m.read);
    assert_eq!(of("E7"), Some(true), "the sent box guesses read from silence, and agrees its snapshot to it");
    assert_eq!(of("E8"), Some(false), "a server that says unread to the sent box is believed");
}

/// The guess belongs to sent boxes alone: an inbox silence still lands unread, for new mail is
/// unread near enough always, and nothing here is invented about the other kinds.
#[test]
fn an_inbox_silence_still_lands_unread() {
    let (mut mb, account, _) = mailbox();
    let silent = ServerMessage { id: "E9".into(), folder: "F".into(), ..Default::default() };
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server: silent, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let found = mb.messages.iter().map(|(_, m)| m).find(|m| m.server.as_ref().is_some_and(|s| s.id == "E9"));
    assert_eq!(found.map(|m| m.read), Some(false), "the inbox guess stays unread, and known to be a guess");
}

/// A brand-new message with no server word about the read state still lands unread: there is
/// nothing to hold — the page is all there is. This pins the boundary of the rule: it guards the
/// merge, not the first landing.
#[test]
fn a_new_message_the_server_said_nothing_about_lands_unread() {
    let (mut mb, account, _) = mailbox();
    let fresh = ServerMessage { id: "E9".into(), folder: "F".into(), ..Default::default() };
    let page = MessagePage { folder: "F".into(), messages: vec![GraphMessage { server: fresh, ..Default::default() }], ..Default::default() };
    take_page(&mut mb, &mut ServerIndex::default(), account, &page, &mut Down::default());
    let found = mb.messages.iter().map(|(_, m)| m).find(|m| m.server.as_ref().is_some_and(|s| s.id == "E9"));
    assert_eq!(
        found.map(|m| (m.read, m.server.as_ref().unwrap().read_said)),
        Some((false, false)),
        "unread for now, and known to be a guess, not a statement"
    );
}
