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
