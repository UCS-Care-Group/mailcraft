// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message sent from a Microsoft account without being saved as a draft goes as MIME
//! (`sendMail`), and Exchange files its copy in Sent Items with `isDraft` still true. What is in
//! Sent Items was sent: the list never shows it as [Draft].

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{AccountKind, Address, FolderKind, Mailbox, ServerFolder};

use super::*;

/// A Microsoft account whose Drafts and Sent Items its server knows as `FD` and `FS`.
fn mailbox() -> (Mailbox, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = AccountKind::Microsoft;
    for (kind, id) in [(FolderKind::Drafts, "FD"), (FolderKind::Sent, "FS")] {
        let folder = mb.folder_of_kind(account, kind).unwrap();
        mb.folder_mut(folder).unwrap().server = Some(ServerFolder { id: id.into(), ..Default::default() });
    }
    (mb, account)
}

/// A page of folder `folder`'s delta with one message, `isDraft` as the server says it.
fn page(folder: &str, id: &str, draft: Option<bool>) -> MessagePage {
    let server = ServerMessage { id: id.into(), folder: folder.into(), change_key: "CK1".into(), read: true, ..Default::default() };
    let message =
        GraphMessage { server, subject: "Quarterly figures".into(), to: vec![Address::new("Kit", "kit@example.org")], draft, ..Default::default() };
    MessagePage { folder: folder.into(), messages: vec![message], ..Default::default() }
}

/// The one message in the account's folder that the server knows as `server`.
fn only_in<'a>(mb: &'a Mailbox, account: Id, server: &str) -> &'a Message {
    let folder = mb.folder_by_server(account, server).unwrap();
    let mut there = mb.messages_in(folder);
    let m = there.next().unwrap();
    assert!(there.next().is_none());
    m
}

#[test]
fn a_message_sent_as_mime_is_no_draft_in_sent_items() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    assert!(!only_in(&mb, account, "FS").draft, "a sent message is not shown as [Draft]");
    // The server goes on saying so: it stays sent, and nothing changes.
    let mut down = Down::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut down);
    assert!(!only_in(&mb, account, "FS").draft);
    assert_eq!(down.changed, 0);
}

#[test]
fn a_draft_in_drafts_is_still_a_draft() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    assert!(only_in(&mb, account, "FD").draft);
}

/// A draft saved on the server, changed here since (it waits to go up) and sent from another
/// program meanwhile: the server files it in Sent Items under the same id with a new change key.
/// Both are kept (S26), but the server's copy is the sent message, not a draft of its own; what
/// was changed here stays a draft, to be made on the server anew.
#[test]
fn a_draft_changed_here_and_sent_elsewhere_keeps_the_sent_copy_as_sent() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let here = only_in(&mb, account, "FD").id;
    {
        let m = mb.message_mut(here).unwrap();
        m.subject = "Quarterly figures, revised".into();
        m.server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, ..Default::default() });
    }
    let mut sent = page("FS", "AAkD1", Some(false));
    sent.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &sent, &mut Down::default());
    let m = mb.message(here).unwrap();
    assert!(
        m.draft && m.server.is_none() && m.server_draft.as_ref().is_some_and(|d| d.pending),
        "what was changed here stays a draft, to go up anew"
    );
    let copy = only_in(&mb, account, "FS");
    assert!(!copy.draft && copy.server.as_ref().is_some_and(|s| s.id == "AAkD1"), "the server's copy is the sent message");
    // The server's copy that the server still calls a draft (moved into Sent Items by the owner,
    // not sent: a message sent as MIME comes under an id of its own) is a draft of its own, as
    // Outlook keeps it.
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD2", Some(true)), &mut Down::default());
    let here = only_in(&mb, account, "FD").id;
    mb.message_mut(here).unwrap().server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, ..Default::default() });
    let mut moved = page("FS", "AAkD2", Some(true));
    moved.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &moved, &mut Down::default());
    assert!(mb.message(here).unwrap().draft && only_in(&mb, account, "FS").draft);
    // The server's copy, a draft on the server's word, is one still when the mailbox is saved
    // and opened again, before and after it is fetched whole: the repair of sent copies an
    // earlier version took for drafts leaves it alone.
    let copy = only_in(&mb, account, "FS").id;
    let reopened = |mb: &Mailbox| crate::io::read_mailbox(&crate::io::write_mailbox(mb).unwrap()).unwrap();
    assert!(reopened(&mb).message(copy).unwrap().draft, "the unsent server copy lost its draft mark on reopen");
    assert!(take_fetched(&mut mb, &mut index, account, "AAkD2", b"From: avery@mail.example\r\nSubject: Quarterly figures\r\n\r\nSoon.\r\n"));
    let opened = reopened(&mb);
    assert!(opened.message(copy).unwrap().draft, "lost its draft mark on reopen after a fetch");
    assert!(opened.message(copy).unwrap().server_draft.as_ref().is_some_and(|d| d.theirs && d.synced.is_some()));
    // What was changed here is still a draft to go up anew.
    assert!(opened.message(here).unwrap().draft);
}

/// A draft that came down from Drafts and that the owner moves into Sent Items by mistake:
/// Outlook keeps it as a draft (`isDraft` stays true), and so does this mailbox, when the move
/// has gone up and the delta lists it there, and when it is moved back to Drafts: the mark is
/// never taken from a draft the mailbox knows but on the server's word.
#[test]
fn a_draft_moved_into_sent_items_stays_a_draft() {
    let (mut mb, account) = mailbox();
    let (drafts, sent) = (mb.folder_of_kind(account, FolderKind::Drafts).unwrap(), mb.folder_of_kind(account, FolderKind::Sent).unwrap());
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    mb.move_message(id, sent).unwrap();
    // The move went up: the next delta lists it in Sent Items, a draft still.
    let mut there = page("FS", "AAkD1", Some(true));
    there.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &there, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.folder == sent, "in Sent Items, server isDraft=true, but draft={}", m.draft);
    // Moved back.
    mb.move_message(id, drafts).unwrap();
    let mut back = page("FD", "AAkD1", Some(true));
    back.messages[0].server.change_key = "CK3".into();
    take_page(&mut mb, &mut index, account, &back, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.folder == drafts, "back in Drafts, server isDraft=true, but draft={}", m.draft);
}

/// The same, with the mailbox saved and opened again before the move goes up (offline, or the
/// program quit right away): the server's copy still lies in Drafts, and the repair that puts
/// right sent copies kept as drafts leaves it alone, so that a refused move has a draft to put
/// back. Then the move goes up, and it is moved back, as above.
#[test]
fn a_draft_moved_into_sent_items_before_a_reopen_stays_a_draft() {
    let (mut mb, account) = mailbox();
    let (drafts, sent) = (mb.folder_of_kind(account, FolderKind::Drafts).unwrap(), mb.folder_of_kind(account, FolderKind::Sent).unwrap());
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    mb.move_message(id, sent).unwrap();
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let mut mb = crate::io::read_mailbox(&bytes).unwrap();
    let m = mb.message(id).unwrap();
    assert!(m.draft, "repair cleared the draft mark of a draft whose server copy is still in Drafts");
    assert!(m.folder == sent && m.server.as_ref().is_some_and(|s| s.folder == "FD"));
    let mut index = ServerIndex::default();
    let mut there = page("FS", "AAkD1", Some(true));
    there.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &there, &mut Down::default());
    assert!(mb.message(id).unwrap().draft);
    mb.move_message(id, drafts).unwrap();
    let mut back = page("FD", "AAkD1", Some(true));
    back.messages[0].server.change_key = "CK3".into();
    take_page(&mut mb, &mut index, account, &back, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.folder == drafts, "back in Drafts, server isDraft=true, but draft={}", m.draft);
}

/// A draft written here and saved on the server (it keeps its `server_draft`, with the reply
/// link in it), moved into Sent Items, the move gone up and the mailbox opened again: a sent
/// copy never has a `server_draft`, so it is told from one and stays a draft.
#[test]
fn a_draft_written_here_and_moved_into_sent_items_stays_a_draft_when_the_mailbox_opens() {
    let (mut mb, account) = mailbox();
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    {
        let m = mb.message_mut(id).unwrap();
        m.server_draft = Some(mailcraft_model::sync::ServerDraft { marker: "mailcraft-draft-1-1".into(), ..Default::default() });
    }
    mb.move_message(id, sent).unwrap();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkD1", Some(true)), &mut Down::default());
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let opened = crate::io::read_mailbox(&bytes).unwrap();
    let m = opened.message(id).unwrap();
    assert!(m.draft && m.server_draft.is_some() && m.server.as_ref().is_some_and(|s| s.folder == "FS"));
}

/// Mail sent before the fix: the server never says it again, so the mailbox puts it right as it
/// opens. A draft waiting to go up, wherever it is, is left alone, and so is a draft in Drafts.
#[test]
fn sent_messages_kept_as_drafts_before_are_put_right_when_the_mailbox_opens() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let (sent, draft) = (only_in(&mb, account, "FS").id, only_in(&mb, account, "FD").id);
    // As a mailbox saved before the fix has it.
    mb.message_mut(sent).unwrap().draft = true;
    let fixed = mb.repair();
    assert!(!mb.message(sent).unwrap().draft, "the sent message is no longer [Draft]");
    assert!(mb.message(draft).unwrap().draft, "a draft in Drafts stays one");
    // Not a repair: the window doesn't say "Your mailbox was repaired" for it.
    assert!(fixed.is_empty(), "{fixed:?}");
}

/// A sent copy an earlier version kept as a draft and that was fetched whole since (opening it,
/// or the background fetch): the fetch gives a draft a `server_draft` holding what the server
/// had of it, and that alone doesn't make it a draft written here. Put right as the mailbox
/// opens, as one never fetched is.
#[test]
fn a_sent_copy_kept_as_a_draft_and_fetched_whole_is_put_right_when_the_mailbox_opens() {
    const MIME: &[u8] = b"From: Avery <avery@mail.example>\r\nTo: Kit <kit@example.org>\r\nSubject: Quarterly figures\r\nMessage-ID: <q1@mail.example>\r\n\r\nThe figures.\r\n";
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    let sent = only_in(&mb, account, "FS").id;
    mb.message_mut(sent).unwrap().draft = true;
    assert!(take_fetched(&mut mb, &mut index, account, "AAkS1", MIME));
    let m = mb.message(sent).unwrap();
    assert!(m.server_draft.as_ref().is_some_and(|d| d.only_fetched()) && !m.unfetched, "the fetch gave it a server_draft");
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let opened = crate::io::read_mailbox(&bytes).unwrap();
    let m = opened.message(sent).unwrap();
    assert!(!m.draft && m.server_draft.is_none(), "still a draft after a fetch");
    // A draft in Drafts fetched whole has the same server_draft, and stays a draft.
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let draft = only_in(&mb, account, "FD").id;
    assert!(take_fetched(&mut mb, &mut index, account, "AAkD1", MIME));
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let opened = crate::io::read_mailbox(&bytes).unwrap();
    assert!(opened.message(draft).unwrap().draft);
}

/// A draft new to this mailbox in Sent Items (another program moved it there, or the account
/// was added fresh) is taken for sent, as a MIME-sent copy is; moved back to Drafts, where the
/// server says it is a draft, it is one again.
#[test]
fn a_draft_new_to_the_mailbox_in_sent_items_is_a_draft_again_once_back_in_drafts() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FS").id;
    assert!(!mb.message(id).unwrap().draft);
    // Moved back to Drafts in Outlook: the delta lists it there, a draft.
    let mut gone = MessagePage { folder: "FS".into(), removed: vec!["AAkD1".into()], ..Default::default() };
    gone.link = "next".into();
    take_page(&mut mb, &mut index, account, &gone, &mut Down::default());
    let mut back = page("FD", "AAkD1", Some(true));
    back.messages[0].server.change_key = "CK2".into();
    let mut down = Down::default();
    take_page(&mut mb, &mut index, account, &back, &mut down);
    let m = mb.message(id).unwrap();
    assert!(m.draft && m.unfetched, "a draft again, to be fetched afresh");
    assert_eq!(mb.folder_by_server(account, "FD"), Some(m.folder));
    assert_eq!(down.changed, 1);
}

/// A draft the server has, sent here: it waits in the Outbox with the draft's server id until the
/// send clears. The server listing it in Drafts meanwhile (the send refused or its answer lost, a
/// listing afresh, another program touching the draft) leaves it the message queued: not a
/// [Draft] in the Outbox with the server's fields, which would hold it from going.
#[test]
fn a_draft_sent_here_and_still_listed_in_drafts_stays_queued_in_the_outbox() {
    let (mut mb, account) = mailbox();
    let outbox = mb.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    {
        // As compose leaves it when the owner sends it.
        let m = mb.message_mut(id).unwrap();
        (m.folder, m.draft, m.unfetched) = (outbox, false, false);
        m.subject = "Quarterly figures, final".into();
        m.to = vec![Address::new("Kit", "kit@example.org"), Address::new("Lee", "lee@example.org")];
        m.server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, marker: "mc-1".into(), ..Default::default() });
    }
    for key in ["CK1", "CK2"] {
        let mut listed = page("FD", "AAkD1", Some(true));
        listed.messages[0].server.change_key = key.into();
        take_page(&mut mb, &mut index, account, &listed, &mut Down::default());
        let m = mb.message(id).unwrap();
        assert!(!m.draft, "still the message queued ({key})");
        assert_eq!(m.folder, outbox, "still in the Outbox ({key})");
        assert_eq!(m.subject, "Quarterly figures, final");
        assert_eq!(m.to.len(), 2);
        assert!(!m.unfetched);
        assert_eq!(super::super::graph_send::waiting(&mb, account), 1, "still waits to be sent ({key})");
    }
}

/// #1160: a draft the server has, sent here as that draft (`…/messages/{id}/send`), keeps its id
/// when Exchange files it in Sent Items, but the delta of the same run may still list it in
/// Drafts, a draft, before Exchange has moved it. It is the message just sent: never a [Draft]
/// in Drafts that looks unsent and invites sending it twice. It is in Sent Items from the moment
/// the send clears, and stays one message there when the server lists it there.
#[test]
fn a_draft_sent_here_is_never_a_draft_again_while_the_server_files_it() {
    let (mut mb, account) = mailbox();
    let (outbox, drafts, sent) = (
        mb.folder_of_kind(account, FolderKind::Outbox).unwrap(),
        mb.folder_of_kind(account, FolderKind::Drafts).unwrap(),
        mb.folder_of_kind(account, FolderKind::Sent).unwrap(),
    );
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    {
        // As compose leaves it when the owner sends it.
        let m = mb.message_mut(id).unwrap();
        (m.folder, m.draft, m.unfetched) = (outbox, false, false);
        m.server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, marker: "mc-1".into(), ..Default::default() });
    }
    let done =
        mailcraft_model::sync::SendDone { message: id, outcome: mailcraft_model::sync::SendOutcome::Sent, draft: None, draft_left: String::new() };
    super::super::graph_send::send_done(&mut mb, account, &done, mailcraft_i18n::Lang::English, &mut Vec::new());
    let in_sent = |mb: &Mailbox| mb.messages_in(sent).map(|m| (m.id, m.draft)).collect::<Vec<_>>();
    assert_eq!(in_sent(&mb), vec![(id, false)], "the message sent is in Sent Items at once");
    assert_eq!(mb.messages_in(outbox).count(), 0);
    // Exchange files it in Sent Items itself: no move goes up for it, which it would refuse while
    // the message is being sent and put it back into Drafts here.
    assert_eq!(moves(&mb), 0, "a move into Sent Items went up for a message the server files there");
    // The same run's delta still lists it in Drafts, a draft.
    let mut index = ServerIndex::default();
    let mut listed = page("FD", "AAkD1", Some(true));
    listed.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &listed, &mut Down::default());
    assert_eq!(mb.messages_in(drafts).count(), 0, "a message sent here came back as a draft in Drafts");
    assert_eq!(in_sent(&mb), vec![(id, false)]);
    assert_eq!(moves(&mb), 0);
    // The next run: gone from Drafts, and listed in Sent Items, a draft still or not.
    let gone = MessagePage { folder: "FD".into(), removed: vec!["AAkD1".into()], ..Default::default() };
    take_page(&mut mb, &mut index, account, &gone, &mut Down::default());
    for draft in [Some(true), Some(false)] {
        let mut filed = page("FS", "AAkD1", draft);
        filed.messages[0].server.change_key = "CK3".into();
        take_page(&mut mb, &mut index, account, &filed, &mut Down::default());
        assert_eq!(in_sent(&mb), vec![(id, false)], "one sent message in Sent Items ({draft:?})");
        assert!(mb.message(id).unwrap().server.as_ref().is_some_and(|s| s.folder == "FS" && !s.gone));
        assert_eq!(moves(&mb), 0);
    }
    assert_eq!(mb.messages_in(drafts).count(), 0);
    assert!(!mb.message(id).unwrap().sent_here, "filed by the server, it is a message as any other");
}

/// The moves into another folder that would go up for the account's messages.
fn moves(mb: &Mailbox) -> usize {
    let account = mb.accounts[0].id;
    let s = crate::Session::at(mb.clone(), 1_800_000_000, 0);
    let (changes, _) = super::super::graph_up::message_changes(&s, account, 100);
    changes.iter().filter(|c| matches!(c, mailcraft_model::sync::MessageChange::Move { .. })).count()
}

/// #1160 review: a draft sent here that the server has listed in Sent Items already, then listed
/// in Drafts, a draft, by a listing made before Exchange filed it (a later page of the same run).
/// That listing is out of date: the message sent stays in Sent Items, no draft, and nothing goes
/// up for it. Once the server says it is gone from Drafts, it is a message as any other.
#[test]
fn a_draft_sent_here_and_listed_in_sent_items_is_not_taken_back_to_drafts_by_a_listing_from_before() {
    let (mut mb, account) = mailbox();
    let (outbox, drafts, sent) = (
        mb.folder_of_kind(account, FolderKind::Outbox).unwrap(),
        mb.folder_of_kind(account, FolderKind::Drafts).unwrap(),
        mb.folder_of_kind(account, FolderKind::Sent).unwrap(),
    );
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD1", Some(true)), &mut Down::default());
    let id = only_in(&mb, account, "FD").id;
    {
        let m = mb.message_mut(id).unwrap();
        (m.folder, m.draft, m.unfetched) = (outbox, false, false);
        m.server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, marker: "mc-1".into(), ..Default::default() });
    }
    let done =
        mailcraft_model::sync::SendDone { message: id, outcome: mailcraft_model::sync::SendOutcome::Sent, draft: None, draft_left: String::new() };
    super::super::graph_send::send_done(&mut mb, account, &done, mailcraft_i18n::Lang::English, &mut Vec::new());
    assert!(mb.message(id).unwrap().sent_here);
    let in_sent = |mb: &Mailbox| mb.messages_in(sent).map(|m| (m.id, m.draft)).collect::<Vec<_>>();
    // Sent Items lists it first, a draft by Exchange's word.
    let mut filed = page("FS", "AAkD1", Some(true));
    filed.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &filed, &mut Down::default());
    assert_eq!(in_sent(&mb), vec![(id, false)]);
    // Then Drafts, from before it was filed.
    let mut stale = page("FD", "AAkD1", Some(true));
    stale.messages[0].server.change_key = "CK1".into();
    let mut down = Down::default();
    take_page(&mut mb, &mut index, account, &stale, &mut down);
    assert_eq!(mb.messages_in(drafts).count(), 0, "followed back into Drafts as a draft");
    assert_eq!(in_sent(&mb), vec![(id, false)]);
    assert_eq!(down.changed, 0);
    assert!(mb.message(id).unwrap().server.as_ref().is_some_and(|s| s.folder == "FS" && !s.gone));
    assert_eq!(moves(&mb), 0);
    // The next run: gone from Drafts. It stays in Sent Items, where the server has it.
    let gone = MessagePage { folder: "FD".into(), removed: vec!["AAkD1".into()], ..Default::default() };
    take_page(&mut mb, &mut index, account, &gone, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert!(!m.sent_here && m.server.as_ref().is_some_and(|s| s.folder == "FS" && !s.gone));
    assert_eq!(in_sent(&mb), vec![(id, false)]);
    assert_eq!(moves(&mb), 0);
}

/// A mailbox file from before #1160 has no `sentHere`: its messages read as not sent here.
#[test]
fn a_message_from_an_older_file_is_not_marked_sent_here() {
    let m: Message = serde_json::from_str(r#"{"id": 7, "subject": "Older"}"#).unwrap();
    assert!(!m.sent_here);
    let mut marked = m.clone();
    marked.sent_here = true;
    let back: Message = serde_json::from_str(&serde_json::to_string(&marked).unwrap()).unwrap();
    assert!(back.sent_here);
    assert!(!serde_json::to_string(&m).unwrap().contains("sentHere"));
}

/// A JMAP account's Sent folder is read through the same path; its server keeps its own word
/// on what is a draft, and nothing is taken for sent there.
#[test]
fn a_jmap_accounts_sent_folder_keeps_the_servers_word() {
    let (mut mb, account) = mailbox();
    mb.accounts[0].kind = AccountKind::Jmap;
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "j1", Some(true)), &mut Down::default());
    assert!(only_in(&mb, account, "FS").draft);
}

/// Saved through the file and read back, as the program opens it.
#[test]
fn a_mailbox_file_with_sent_messages_kept_as_drafts_opens_with_them_sent() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    let sent = only_in(&mb, account, "FS").id;
    mb.message_mut(sent).unwrap().draft = true;
    let bytes = crate::io::write_mailbox(&mb).unwrap();
    let opened = crate::io::read_mailbox(&bytes).unwrap();
    assert!(!opened.message(sent).unwrap().draft);
}

/// A Microsoft account's message in Sent Items here, its server copy `AAkD1` in the server's
/// folder `server_folder`, still marked as a draft sent here (#1160 review): a mark that outlived
/// its use. The mailbox, the account, the message and the Inbox.
fn marked_sent_here(server_folder: &str) -> (Mailbox, Id, Id, Id) {
    let (mut mb, account) = mailbox();
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    mb.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "FI".into(), ..Default::default() });
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkD1", Some(false)), &mut Down::default());
    let id = only_in(&mb, account, "FS").id;
    let m = mb.message_mut(id).unwrap();
    m.sent_here = true;
    m.server.as_mut().unwrap().folder = server_folder.into();
    (mb, account, id, inbox)
}

/// #1160 review: a mark left on a message the server has filed (Drafts read afresh after Exchange
/// filed it, so that no removal came) never keeps a move of the owner's from going up. Moved to
/// the Inbox it goes up; moved back to Sent Items, so does that move: once each.
#[test]
fn a_draft_sent_here_and_filed_is_moved_as_any_other() {
    let (mut mb, _, id, inbox) = marked_sent_here("FS");
    let sent = mb.message(id).unwrap().folder;
    assert_eq!(moves(&mb), 0);
    mb.move_message(id, inbox).unwrap();
    assert_eq!(moves(&mb), 1, "a move to the Inbox goes up");
    // The move made there.
    mb.message_mut(id).unwrap().server.as_mut().unwrap().folder = "FI".into();
    assert_eq!(moves(&mb), 0);
    mb.move_message(id, sent).unwrap();
    assert_eq!(moves(&mb), 1, "the move back to Sent Items goes up");
    // And a mark that is somehow still there on a message the server has outside Drafts holds no
    // move back.
    let (mb, ..) = marked_sent_here("FI");
    assert_eq!(moves(&mb), 1);
}

/// #1160 review: a copy of a draft sent here is not one; nor is it once moved by the owner.
#[test]
fn a_copy_or_a_move_of_a_draft_sent_here_is_not_marked() {
    let (mut mb, _, id, inbox) = marked_sent_here("FD");
    let copy = mb.copy_message(id, inbox).unwrap();
    assert!(!mb.message(copy).unwrap().sent_here);
    assert!(mb.message(id).unwrap().sent_here);
    mb.move_message(id, inbox).unwrap();
    assert!(!mb.message(id).unwrap().sent_here);
}

/// #1160 review: Drafts read afresh after Sent Items has listed a draft sent here, and the
/// listing (made before Exchange filed it) still has it as a draft. Out of date: the message sent
/// stays in Sent Items, no draft, and nothing goes up for it. The mark stays (no removal comes);
/// it holds no move once the server has the message outside Drafts.
#[test]
fn drafts_read_afresh_with_a_draft_sent_here_and_filed_leaves_it_sent() {
    let (mut mb, account, id, _) = marked_sent_here("FS");
    let sent = mb.message(id).unwrap().folder;
    let drafts = mb.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let mut index = ServerIndex::default();
    // Another folder read afresh first changes nothing of it.
    let inbox_afresh = MessagePage { folder: "FI".into(), afresh: true, caught_up: true, ..Default::default() };
    take_page(&mut mb, &mut index, account, &inbox_afresh, &mut Down::default());
    let mut stale = page("FD", "AAkD1", Some(true));
    stale.afresh = true;
    stale.messages[0].server.change_key = "CK0".into();
    take_page(&mut mb, &mut index, account, &stale, &mut Down::default());
    let m = mb.message(id).unwrap();
    assert_eq!((m.folder, m.draft), (sent, false), "followed back into Drafts as a draft");
    assert!(m.server.as_ref().is_some_and(|s| s.folder == "FS" && !s.gone));
    assert_eq!(mb.messages_in(drafts).count(), 0);
    assert_eq!(moves(&mb), 0);
}
