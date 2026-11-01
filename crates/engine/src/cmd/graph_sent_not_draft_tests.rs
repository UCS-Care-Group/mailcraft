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
fn a_sent_message_an_earlier_run_took_for_a_draft_is_put_right() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    // As a mailbox kept before this fix has it.
    let id = only_in(&mb, account, "FS").id;
    mb.message_mut(id).unwrap().draft = true;
    take_page(&mut mb, &mut index, account, &page("FS", "AAkS1", Some(true)), &mut Down::default());
    assert!(!mb.message(id).unwrap().draft);
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
    // The same for one the server files with `isDraft` still true, as a message sent as MIME.
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page("FD", "AAkD2", Some(true)), &mut Down::default());
    let here = only_in(&mb, account, "FD").id;
    mb.message_mut(here).unwrap().server_draft = Some(mailcraft_model::sync::ServerDraft { pending: true, ..Default::default() });
    let mut sent = page("FS", "AAkD2", Some(true));
    sent.messages[0].server.change_key = "CK2".into();
    take_page(&mut mb, &mut index, account, &sent, &mut Down::default());
    assert!(mb.message(here).unwrap().draft && !only_in(&mb, account, "FS").draft);
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
