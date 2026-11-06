// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #575 review: arranged by attachments, a Microsoft account's message goes by what its server
//! says of its files, before it is fetched, once it is, and back to its preview again; so does
//! its paperclip. As Outlook has it: a meeting invitation's own calendar part isn't a file, a
//! calendar file attached to a mail is.

use mailcraft_model::sync::{GraphMessage, MessagePage, ServerMessage};
use mailcraft_model::{AccountKind, Folder, FolderKind, Mailbox};

use super::*;
use crate::columns::{has_files, sorts_with_files};

/// A meeting invitation: its calendar part is the body's alternative, without a name.
const INVITATION: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: Planning\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nCome along.\r\n--b\r\n\
Content-Type: text/calendar; method=REQUEST\r\n\r\nBEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n";

/// A mail with a calendar file attached, which is a request too: the hard case, as the parts list
/// has it the same as an invitation's.
const ICS_ATTACHED: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: The file\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nThe file is attached.\r\n--b\r\n\
Content-Type: text/calendar; method=REQUEST; name=\"invite.ics\"\r\nContent-Disposition: attachment; filename=\"invite.ics\"\r\n\r\n\
BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n";

const PDF_ATTACHED: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: Figures\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nFigures attached.\r\n--b\r\n\
Content-Type: application/pdf; name=\"figures.pdf\"\r\nContent-Disposition: attachment; filename=\"figures.pdf\"\r\n\r\n%PDF-1.4\r\n--b--\r\n";

/// Only a picture its HTML shows.
const INLINE_PICTURE: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: Logo\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html\r\n\r\n<p>Hi <img src=\"cid:logo@x\"></p>\r\n--b\r\n\
Content-Type: image/png; name=\"logo.png\"\r\nContent-ID: <logo@x>\r\nContent-Disposition: inline; filename=\"logo.png\"\r\n\r\nPNG\r\n--b--\r\n";

/// The four, with what Graph's `hasAttachments` says of each (Outlook's `PR_HASATTACH`).
const MAIL: [(&str, &[u8], bool); 4] =
    [("AAkI1", INVITATION, false), ("AAkI2", ICS_ATTACHED, true), ("AAkI3", PDF_ATTACHED, true), ("AAkI4", INLINE_PICTURE, false)];

fn mailbox() -> (Mailbox, Id) {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = AccountKind::Microsoft;
    let folder = mb.alloc_id();
    mb.folders.push(Folder { id: folder, account, name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() });
    mb.folder_mut(folder).unwrap().server = Some(mailcraft_model::ServerFolder { id: "F".into(), ..Default::default() });
    (mb, account)
}

/// A page describing the four; `said` false: one that says nothing of attachments.
fn page(said: bool) -> MessagePage {
    let messages = MAIL
        .iter()
        .enumerate()
        .map(|(n, (id, _, has))| GraphMessage {
            server: ServerMessage { id: (*id).into(), ..Default::default() },
            subject: format!("Mail {n}"),
            received: 60 * (n as i64 + 1),
            has_attachments: said.then_some(*has),
            ..Default::default()
        })
        .collect();
    MessagePage { folder: "F".into(), messages, ..Default::default() }
}

/// A JMAP window with marks alone: what the server says of the four's read state, nothing else.
fn marks() -> MessagePage {
    let messages = MAIL
        .iter()
        .map(|(id, _, _)| GraphMessage { server: ServerMessage { id: (*id).into(), read: true, ..Default::default() }, ..Default::default() })
        .collect();
    MessagePage { folder: "F".into(), messages, ..Default::default() }
}

/// Each of the four sorts with files, and wears the paperclip, as its server says.
#[track_caller]
fn as_outlook_has_it(mb: &Mailbox, index: &mut ServerIndex, account: Id, stage: &str) {
    for (id, _, has) in MAIL {
        let m = mb.message(index.find(mb, account, id).unwrap()).unwrap();
        assert_eq!((sorts_with_files(m), has_files(m)), (has, has), "{id} {stage}");
    }
}

#[test]
fn arranged_by_attachments_a_message_stays_where_its_server_puts_it_fetched_or_not() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    as_outlook_has_it(&mb, &mut index, account, "before it is fetched");

    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
    }
    // The parts as they come: the attached file and the invitation's own part look the same.
    let ics = |mb: &Mailbox, index: &mut ServerIndex, id| {
        let m = mb.message(index.find(mb, account, id).unwrap()).unwrap();
        m.attachments.iter().find(|a| crate::session::is_calendar_part(a)).map(|a| (a.name.clone(), a.inline, a.content_id.clone()))
    };
    assert_eq!(ics(&mb, &mut index, "AAkI1"), ics(&mb, &mut index, "AAkI2"));
    as_outlook_has_it(&mb, &mut index, account, "fetched");

    // Described again, by Graph and by a window with marks alone.
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    as_outlook_has_it(&mb, &mut index, account, "described again");
    super::super::jmap_down::take_window(&mut mb, &mut index, account, &marks(), false, &mut Down::default());
    as_outlook_has_it(&mb, &mut index, account, "marks alone");

    // Its parts' bytes let go of: the calendar parts say nothing of what they are.
    for (id, _, _) in MAIL {
        let here = index.find(&mb, account, id).unwrap();
        for a in &mut mb.message_mut(here).unwrap().attachments {
            a.data = std::sync::Arc::new(Vec::new());
        }
    }
    as_outlook_has_it(&mb, &mut index, account, "bytes on disk");

    // Back to its preview, described while it is, and fetched again.
    for (id, _, _) in MAIL {
        let here = index.find(&mb, account, id).unwrap();
        thin_message(mb.message_mut(here).unwrap(), "Preview".into());
    }
    as_outlook_has_it(&mb, &mut index, account, "thinned");
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    as_outlook_has_it(&mb, &mut index, account, "thinned, described again");
    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
    }
    as_outlook_has_it(&mb, &mut index, account, "fetched again");
}

/// A message fetched by a build from before the server's answer was kept has none: it goes by its
/// parts, and a window with marks alone doesn't take that for "no files".
#[test]
fn a_message_from_before_goes_by_its_parts_until_its_server_says() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
        let here = index.find(&mb, account, id).unwrap();
        mb.message_mut(here).unwrap().server_attachments = None;
    }
    let pdf = index.find(&mb, account, "AAkI3").unwrap();
    let invitation = index.find(&mb, account, "AAkI1").unwrap();
    let keys = |mb: &Mailbox| [pdf, invitation].map(|id| sorts_with_files(mb.message(id).unwrap()));
    assert_eq!(keys(&mb), [true, false]);
    super::super::jmap_down::take_window(&mut mb, &mut index, account, &marks(), false, &mut Down::default());
    assert_eq!(mb.message(pdf).unwrap().server_attachments, None);
    assert_eq!(keys(&mb), [true, false]);
    // Back to its preview and fetched again, and as a message found unreadable is: it stays.
    let attached = index.find(&mb, account, "AAkI2").unwrap();
    let keys = |mb: &Mailbox| [pdf, invitation, attached].map(|id| sorts_with_files(mb.message(id).unwrap()));
    let before = keys(&mb);
    assert_eq!(before, [true, false, false]);
    for id in [pdf, invitation, attached] {
        thin_message(mb.message_mut(id).unwrap(), "Preview".into());
    }
    assert_eq!(keys(&mb), before);
    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
    }
    assert_eq!(keys(&mb), before);
    assert_eq!(mb.message(attached).unwrap().server_attachments, None);
    // Kept with characters that couldn't be read: fetched again.
    for id in [pdf, invitation, attached] {
        mb.message_mut(id).unwrap().subject.push(char::REPLACEMENT_CHARACTER);
    }
    mb.redecoded = false;
    assert_eq!(refetch_unreadable(&mut mb), 3);
    assert!(mb.message(invitation).unwrap().unfetched);
    assert_eq!(keys(&mb), before);
    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
    }
    assert_eq!(keys(&mb), before);
    assert_eq!(mb.message(pdf).unwrap().server_attachments, None);
    // Its server says: from then on that.
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    assert_eq!(mb.message(pdf).unwrap().server_attachments, Some(true));

    // In the mailbox file: "no" stays "no", and none stays none.
    let mut m = mb.message(invitation).unwrap().clone();
    assert_eq!(m.server_attachments, Some(false));
    let json = serde_json::to_string(&m).unwrap();
    assert!(json.contains(r#""serverAttachments":false"#), "{json}");
    assert_eq!(serde_json::from_str::<Message>(&json).unwrap().server_attachments, Some(false));
    m.server_attachments = None;
    let json = serde_json::to_string(&m).unwrap();
    assert!(!json.contains("serverAttachments"), "{json}");
    assert_eq!(serde_json::from_str::<Message>(&json).unwrap().server_attachments, None);
}

/// A draft goes by its own files, whatever its server last said of them: it is written here and
/// goes up later. Saved, its files changed, and undone, it is where its files put it.
#[test]
fn a_draft_goes_by_its_own_files() {
    use serde_json::json;

    let mut s = crate::session::Session::empty();
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let drafts = s.mailbox.folder_of_kind(account, FolderKind::Drafts).unwrap();
    let read = crate::mime::parse(PDF_ATTACHED);
    let m = Message {
        account,
        folder: drafts,
        draft: true,
        subject: "Plan".into(),
        to: vec![mailcraft_model::Address::new("", "kit@example.net")],
        body: read.body,
        attachments: read.attachments,
        server: Some(ServerMessage { id: "D-1".into(), folder: "F-dr".into(), ..Default::default() }),
        // What a page said before its file was taken off here.
        server_attachments: Some(false),
        server_draft: Some(mailcraft_model::sync::ServerDraft { marker: "mailcraft-draft-1".into(), ..Default::default() }),
        ..Default::default()
    };
    let id = s.mailbox.add_message(m).unwrap();
    let key = |s: &crate::session::Session| {
        let m = s.mailbox.message(id).unwrap();
        (sorts_with_files(m), has_files(m))
    };
    assert_eq!(key(&s), (true, true));
    s.run("mail.editDraft", &json!({"id": id})).unwrap();
    s.run("compose.set", &json!({"subject": "Plan B"})).unwrap();
    assert_eq!(s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64(), Some(id));
    assert_eq!(key(&s), (true, true));
    s.run("compose.removeAttachment", &json!({"index": 0})).unwrap();
    s.run("compose.saveDraft", &json!({})).unwrap();
    assert_eq!(key(&s), (false, false));
    // A page says the server still has its file (it hasn't gone up yet).
    s.mailbox.message_mut(id).unwrap().server_attachments = Some(true);
    assert_eq!(key(&s), (false, false));
    s.run("compose.close", &json!({})).ok();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().attachments.len(), 1);
    assert_eq!(key(&s), (true, true));
}

/// A server that doesn't say whether a message has attachments: fetched, it goes by its parts,
/// and a "no" kept from before isn't taken for an answer.
#[test]
fn a_message_its_server_said_nothing_of_goes_by_its_parts() {
    let (mut mb, account) = mailbox();
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page(false), &mut Down::default());
    let pdf = index.find(&mb, account, "AAkI3").unwrap();
    assert_eq!(mb.message(pdf).unwrap().server_attachments, None);
    assert!(take_fetched(&mut mb, &mut index, account, "AAkI3", PDF_ATTACHED));
    let m = mb.message(pdf).unwrap();
    assert_eq!((m.server_attachments, sorts_with_files(m), has_files(m)), (None, true, true));
}

/// A PDF and a calendar file attached to a mail, one that isn't meeting mail (a published event).
const PDF_AND_ICS: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: The file\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nBoth attached.\r\n--b\r\n\
Content-Type: application/pdf; name=\"figures.pdf\"\r\nContent-Disposition: attachment; filename=\"figures.pdf\"\r\n\r\n%PDF-1.4\r\n--b\r\n\
Content-Type: text/calendar; method=PUBLISH; name=\"invite.ics\"\r\nContent-Disposition: attachment; filename=\"invite.ics\"\r\n\r\n\
BEGIN:VCALENDAR\r\nMETHOD:PUBLISH\r\nBEGIN:VEVENT\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n--b--\r\n";

/// Two files attached: one removed here leaves the other.
const TWO_PDFS: &[u8] = b"From: Kit <kit@example.org>\r\nSubject: Figures\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nFigures attached.\r\n--b\r\n\
Content-Type: application/pdf; name=\"q1.pdf\"\r\nContent-Disposition: attachment; filename=\"q1.pdf\"\r\n\r\n%PDF-1.4\r\n--b\r\n\
Content-Type: application/pdf; name=\"q2.pdf\"\r\nContent-Disposition: attachment; filename=\"q2.pdf\"\r\n\r\n%PDF-1.4\r\n--b--\r\n";

/// #575 review 3: a file removed here (Remove Attachment) takes the paperclip and the place among
/// the messages with files with it, whatever the server, never told, says after; undone, they're
/// back. Back to its preview, it keeps none; fetched again, the server's copy brings the file back,
/// and its paperclip with it.
#[test]
fn a_file_removed_here_takes_the_paperclip_with_it_until_the_servers_copy_is_back() {
    use serde_json::json;

    let (mb, account) = mailbox();
    let mut s = crate::session::Session::new(mb);
    let mut index = ServerIndex::default();
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", PDF_ATTACHED));
    let pdf = index.find(&s.mailbox, account, "AAkI3").unwrap();
    let key = |s: &crate::session::Session| {
        let m = s.mailbox.message(pdf).unwrap();
        (sorts_with_files(m), has_files(m), m.has_attachments())
    };
    assert_eq!(key(&s), (true, true, true));
    // One that isn't there: nothing changes.
    assert!(s.run("mail.removeAttachment", &json!({"id": pdf, "index": 5})).is_err());
    assert_eq!(s.mailbox.message(pdf).unwrap().attachments_changed_here, None);

    s.run("mail.removeAttachment", &json!({"id": pdf, "index": 0})).unwrap();
    assert_eq!(key(&s), (false, false, false));
    for (stage, said) in [("says it has", Some(true)), ("says it hasn't", Some(false)), ("says nothing", None)] {
        let mut described = page(true);
        described.messages.iter_mut().for_each(|m| m.has_attachments = said);
        take_page(&mut s.mailbox, &mut index, account, &described, &mut Down::default());
        assert_eq!(key(&s), (false, false, false), "the server {stage}");
    }
    super::super::jmap_down::take_window(&mut s.mailbox, &mut index, account, &marks(), false, &mut Down::default());
    assert_eq!(key(&s), (false, false, false), "marks alone");
    // What its parts said stands, whatever the server says; what it says is kept.
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((m.attachments_changed_here, m.server_attachments), (Some(false), Some(true)));

    // In the mailbox file.
    let m = s.mailbox.message(pdf).unwrap().clone();
    let saved = serde_json::to_string(&m).unwrap();
    assert!(saved.contains(r#""attachmentsChangedHere":false"#), "{saved}");
    assert_eq!(serde_json::from_str::<Message>(&saved).unwrap().attachments_changed_here, Some(false));

    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(key(&s), (true, true, true), "undone");
    assert_eq!(s.mailbox.message(pdf).unwrap().attachments_changed_here, None, "undone: what stood for them too");
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(key(&s), (false, false, false), "redone");

    // Back to its preview: no paperclip, and the server saying it has files doesn't put one back.
    thin_message(s.mailbox.message_mut(pdf).unwrap(), "Preview".into());
    assert_eq!(key(&s), (false, false, false), "thinned");
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert_eq!(key(&s), (false, false, false), "thinned, described again");

    // Fetched again: the server's copy, with its file.
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", PDF_ATTACHED));
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((m.attachments_changed_here, m.server_attachments), (None, Some(true)));
    assert_eq!(key(&s), (true, true, true), "fetched again");
    let saved = serde_json::to_string(s.mailbox.message(pdf).unwrap()).unwrap();
    assert!(!saved.contains("attachmentsChangedHere"), "{saved}");
}

/// A PDF and a calendar file attached: the PDF removed here, the calendar file is still a file,
/// for the paperclip, the arrangement and the chips alike, and still once its bytes are let go
/// of or it is back to its preview.
#[test]
fn a_calendar_file_left_after_a_removal_is_still_a_file() {
    use serde_json::json;

    let (mb, account) = mailbox();
    let mut s = crate::session::Session::new(mb);
    let mut index = ServerIndex::default();
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI2", PDF_AND_ICS));
    let mail = index.find(&s.mailbox, account, "AAkI2").unwrap();
    let pdf = s.mailbox.message(mail).unwrap().attachments.iter().position(|a| a.name == "figures.pdf").unwrap();
    s.run("mail.removeAttachment", &json!({"id": mail, "index": pdf})).unwrap();
    let key = |s: &crate::session::Session| {
        let m = s.mailbox.message(mail).unwrap();
        (sorts_with_files(m), has_files(m), m.has_attachments())
    };
    assert_eq!(key(&s), (true, true, true));
    for a in &mut s.mailbox.message_mut(mail).unwrap().attachments {
        a.data = std::sync::Arc::new(Vec::new());
    }
    assert_eq!(key(&s), (true, true, true), "bytes on disk");
    thin_message(s.mailbox.message_mut(mail).unwrap(), "Preview".into());
    assert_eq!(key(&s), (true, true, true), "thinned");
    // Fetched again, both files are back, and the server's answer, kept all along, says.
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI2", PDF_AND_ICS));
    assert_eq!(key(&s), (true, true, true), "fetched again");
    let mail = s.mailbox.message(mail).unwrap();
    assert_eq!((mail.attachments.len(), mail.attachments_changed_here), (2, None));
}

/// Waiting to be fetched again (kept with characters that couldn't be read) with its parts still
/// here: one removed meanwhile takes the paperclip with it too.
#[test]
fn a_file_removed_while_the_message_waits_to_be_fetched_again() {
    use serde_json::json;

    let (mb, account) = mailbox();
    let mut s = crate::session::Session::new(mb);
    let mut index = ServerIndex::default();
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", PDF_ATTACHED));
    let pdf = index.find(&s.mailbox, account, "AAkI3").unwrap();
    s.mailbox.message_mut(pdf).unwrap().subject.push(char::REPLACEMENT_CHARACTER);
    s.mailbox.redecoded = false;
    assert_eq!(refetch_unreadable(&mut s.mailbox), 1);
    assert!(s.mailbox.message(pdf).unwrap().unfetched);
    s.run("mail.removeAttachment", &json!({"id": pdf, "index": 0})).unwrap();
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((sorts_with_files(m), has_files(m), m.has_attachments()), (false, false, false));
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((sorts_with_files(m), has_files(m), m.has_attachments()), (false, false, false));
}

/// One of two files removed here: the other keeps the paperclip.
#[test]
fn one_of_two_files_removed_here_leaves_the_paperclip() {
    use serde_json::json;

    let (mb, account) = mailbox();
    let mut s = crate::session::Session::new(mb);
    let mut index = ServerIndex::default();
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", TWO_PDFS));
    let pdf = index.find(&s.mailbox, account, "AAkI3").unwrap();
    s.run("mail.removeAttachment", &json!({"id": pdf, "index": 0})).unwrap();
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((m.attachments.len(), sorts_with_files(m), has_files(m), m.has_attachments()), (1, true, true, true));
    // The other removed too, the server's copy fetched again, then the second removal undone:
    // the one file is back, and what its parts said with it.
    s.run("mail.removeAttachment", &json!({"id": pdf, "index": 0})).unwrap();
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", TWO_PDFS));
    assert_eq!(s.mailbox.message(pdf).unwrap().attachments_changed_here, None);
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((m.attachments.len(), m.attachments_changed_here), (1, Some(true)));
}

/// Removed here, then on the server too, then the removal here undone: the file is back here,
/// and wears the paperclip the server's answer gave it then, not the one it gives now.
#[test]
fn a_removal_undone_after_the_server_lost_the_file_too_keeps_the_paperclip() {
    use serde_json::json;

    let (mb, account) = mailbox();
    let mut s = crate::session::Session::new(mb);
    let mut index = ServerIndex::default();
    take_page(&mut s.mailbox, &mut index, account, &page(true), &mut Down::default());
    assert!(take_fetched(&mut s.mailbox, &mut index, account, "AAkI3", PDF_ATTACHED));
    let pdf = index.find(&s.mailbox, account, "AAkI3").unwrap();
    s.run("mail.removeAttachment", &json!({"id": pdf, "index": 0})).unwrap();
    let mut gone = page(true);
    gone.messages.iter_mut().for_each(|m| m.has_attachments = Some(false));
    take_page(&mut s.mailbox, &mut index, account, &gone, &mut Down::default());
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(pdf).unwrap();
    assert_eq!((m.attachments.len(), m.attachments_changed_here, m.server_attachments), (1, Some(true), Some(false)));
    assert_eq!((sorts_with_files(m), has_files(m), m.has_attachments()), (true, true, true));
}

/// A JMAP account: what its server says of attachments puts the paperclip on mail not fetched
/// yet, but isn't kept as an answer: such a server may count an invitation's own calendar part
/// (RFC 8621 §4.1.4). Fetched, a message goes by its parts. An answer an earlier build kept goes.
#[test]
fn a_jmap_servers_answer_is_for_mail_not_fetched_yet() {
    let (mut mb, account) = mailbox();
    mb.accounts[0].kind = AccountKind::Jmap;
    let mut index = ServerIndex::default();
    take_page(&mut mb, &mut index, account, &page(true), &mut Down::default());
    for (id, _, has) in MAIL {
        let m = mb.message(index.find(&mb, account, id).unwrap()).unwrap();
        assert_eq!((m.server_attachments, m.attachments_on_server, sorts_with_files(m)), (None, has, has), "{id} before it is fetched");
    }
    for (id, mime, _) in MAIL {
        assert!(take_fetched(&mut mb, &mut index, account, id, mime));
    }
    let key = |mb: &Mailbox, index: &mut ServerIndex, id| {
        let m = mb.message(index.find(mb, account, id).unwrap()).unwrap();
        (sorts_with_files(m), has_files(m))
    };
    assert_eq!(key(&mb, &mut index, "AAkI1"), (false, false), "the invitation");
    assert_eq!(key(&mb, &mut index, "AAkI3"), (true, true), "the PDF");
    let pdf = index.find(&mb, account, "AAkI3").unwrap();
    for said in [true, false] {
        mb.message_mut(pdf).unwrap().server_attachments = Some(false);
        take_page(&mut mb, &mut index, account, &page(said), &mut Down::default());
        assert_eq!(mb.message(pdf).unwrap().server_attachments, None, "the server said: {said}");
    }
    assert_eq!(key(&mb, &mut index, "AAkI3"), (true, true));
}
