// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo and Redo of a command that brought a body in from disk while it ran, for an account
//! kept in step with its server (#688): the body is never lost with it.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{AccountKind, Action, Condition, FlagStatus, FolderKind, Rule, ServerFolder};

use super::*;

/// A mailbox like [`mailbox`] whose account is a Microsoft account (`kind`), its folders and
/// messages on the server, saved at `path` and opened anew with every long body left on disk.
/// Answers the session, the mailbox as saved, and an old message whose long text says
/// "quixotic", which is kept on disk.
fn opened(path: &Path, kind: AccountKind) -> (Session, Mailbox, Id) {
    let mut mb = mailbox(24);
    let account = mb.accounts[0].id;
    mb.accounts[0].kind = kind;
    for (n, f) in mb.folders.iter_mut().filter(|f| f.account == account).enumerate() {
        f.server = Some(ServerFolder { id: format!("F-{n}"), name: f.name.clone(), ..Default::default() });
    }
    let ids: Vec<Id> = mb.messages.keys().copied().collect();
    for (n, id) in ids.iter().enumerate() {
        let m = mb.message_mut(*id).unwrap();
        m.server = Some(ServerMessage { id: format!("M-{n}"), folder: "F-0".into(), ..Default::default() });
    }
    let old = *newest_first(&mb).iter().rev().find(|id| mb.message(**id).unwrap().body.text.len() > INLINE_MAX).unwrap();
    mb.message_mut(old).unwrap().body.text.push_str("Only here: quixotic.\n");
    save(path, &mb).unwrap();
    clear_cache();
    let s = session_without_bodies(path);
    assert!(s.mailbox.message(old).unwrap().body_is_kept());
    (s, mb, old)
}

/// A rule that moves what says "quixotic" in its body to the account's Archive.
fn quixotic_to_archive(s: &mut Session) -> Id {
    let account = s.mailbox.accounts[0].id;
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    let id = s.mailbox.alloc_id();
    s.mailbox.rules.push(Rule {
        id,
        name: "Quixotic".into(),
        enabled: true,
        conditions: vec![Condition::BodyContains("quixotic".into())],
        actions: vec![Action::MoveTo(archive)],
        stop: false,
    });
    archive
}

/// The text of message `id` as the session reads it, from memory or from disk.
fn text_of(s: &Session, id: Id) -> String {
    let m = s.mailbox.message(id).unwrap();
    if m.body_is_kept() { String::from_utf8(s.content(id, crate::Part::Text).unwrap().to_vec()).unwrap() } else { m.body.text.clone() }
}

fn undo_and_redo_keep_the_body(kind: AccountKind, name: &str) {
    let tmp = Temp::new(name);
    let path = tmp.file();
    let (mut s, mb, old) = opened(&path, kind);
    let was = mb.message(old).unwrap().clone();
    let inbox = s.mailbox.inbox().unwrap();
    let archive = quixotic_to_archive(&mut s);
    assert_eq!(s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"].as_u64(), Some(1));
    assert_eq!(s.mailbox.message(old).unwrap().folder, archive);

    // Undone: back in the Inbox, its text whole, here and in the file once it is saved.
    s.run("edit.undo", &json!({})).unwrap();
    let m = s.mailbox.message(old).unwrap();
    assert_eq!(m.folder, inbox);
    assert_eq!(text_of(&s, old), was.body.text);
    assert_eq!(m.body.html, was.body.html);
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let reopened = whole(&path);
    let m = reopened.message(old).unwrap();
    assert_eq!((m.folder, &m.body.text, &m.body.html), (inbox, &was.body.text, &was.body.html));

    // Redone: in the Archive again, its text still whole.
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(old).unwrap().folder, archive);
    assert_eq!(text_of(&s, old), was.body.text);
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let reopened = whole(&path);
    let m = reopened.message(old).unwrap();
    assert_eq!((m.folder, &m.body.text, &m.body.html), (archive, &was.body.text, &was.body.html));

    // And undone again.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(text_of(&s, old), was.body.text);
}

#[test]
fn undo_and_redo_of_rules_that_read_a_body_from_disk_keep_it_for_a_microsoft_account() {
    undo_and_redo_keep_the_body(AccountKind::Microsoft, "undo-body-ms");
}

#[test]
fn undo_and_redo_of_rules_that_read_a_body_from_disk_keep_it_for_an_exchange_account() {
    undo_and_redo_keep_the_body(AccountKind::Exchange, "undo-body-ews");
}

#[test]
fn undo_and_redo_of_rules_that_read_a_body_from_disk_keep_it_for_a_local_account() {
    undo_and_redo_keep_the_body(AccountKind::default(), "undo-body-local");
}

/// A command that flags a message and then brings its whole body in, as a command that reads a
/// message after changing it does (a list searched in its bodies worked out again, #688).
fn flag_then_load(s: &mut Session, v: &Value) -> crate::CmdResult {
    let id = v["id"].as_u64().unwrap_or(0);
    if let Ok(m) = s.mailbox.message_mut(id) {
        m.flag = FlagStatus::Flagged;
    }
    s.load_message(id)?;
    Ok(Value::Null)
}

/// The bytes of the bodies and files the session's messages hold in memory.
fn held(s: &Session) -> u64 {
    let one = |m: &Message| {
        m.body.text.len()
            + m.body.html.as_ref().map_or(0, String::len)
            + m.hidden_text.len()
            + m.attachments.iter().map(|a| a.data.len()).sum::<usize>()
    };
    s.mailbox.messages.values().map(|m| one(m) as u64).sum()
}

/// Message `id` of the mailbox saved at `path`, everything of it read back.
fn saved(path: &Path, id: Id) -> Message {
    clear_cache();
    whole(path).message(id).unwrap().clone()
}

fn a_body_brought_in_after_a_change_survives_undo_and_redo(kind: AccountKind, name: &str) {
    let tmp = Temp::new(name);
    let path = tmp.file();
    let (mut s, mb, old) = opened(&path, kind);
    let was = mb.message(old).unwrap().clone();
    let mut specs = crate::cmd::registry().all().to_vec();
    specs.push(crate::CommandSpec::new("test.flagThenLoad", "Flag Then Load", "Test", flag_then_load));
    s.registry = Arc::new(crate::Registry::new(specs));
    s.run("test.flagThenLoad", &json!({"id": old})).unwrap();
    assert!(!s.mailbox.message(old).unwrap().body_is_kept());

    // Undone: not flagged, and its text there all the same, in memory or on disk.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(old).unwrap().flag, FlagStatus::None);
    assert_eq!(text_of(&s, old), was.body.text);
    assert_eq!(s.loaded_message(old).unwrap().body.html, was.body.html);
    assert!(s.resident_bytes() <= held(&s), "{} counted, {} held", s.resident_bytes(), held(&s));
    save(&path, &s.mailbox).unwrap();
    let m = saved(&path, old);
    assert_eq!((m.flag, &m.body.text, &m.body.html), (FlagStatus::None, &was.body.text, &was.body.html));

    // Redone: flagged again, its text still whole.
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(old).unwrap().flag, FlagStatus::Flagged);
    assert_eq!(text_of(&s, old), was.body.text);
    save(&path, &s.mailbox).unwrap();
    let m = saved(&path, old);
    assert_eq!((m.flag, &m.body.text, &m.body.html), (FlagStatus::Flagged, &was.body.text, &was.body.html));

    // And undone once more.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(text_of(&s, old), was.body.text);
    save(&path, &s.mailbox).unwrap();
    assert_eq!(saved(&path, old).body.text, was.body.text);
}

#[test]
fn a_body_brought_in_after_a_change_survives_undo_and_redo_for_a_microsoft_account() {
    a_body_brought_in_after_a_change_survives_undo_and_redo(AccountKind::Microsoft, "undo-load-ms");
}

#[test]
fn a_body_brought_in_after_a_change_survives_undo_and_redo_for_an_exchange_account() {
    a_body_brought_in_after_a_change_survives_undo_and_redo(AccountKind::Exchange, "undo-load-ews");
}

#[test]
fn a_body_brought_in_after_a_change_survives_undo_and_redo_for_a_local_account() {
    a_body_brought_in_after_a_change_survives_undo_and_redo(AccountKind::default(), "undo-load-local");
}

/// A command that brings a message's body in and writes another text over it.
fn rewrite(s: &mut Session, v: &Value) -> crate::CmdResult {
    let id = v["id"].as_u64().unwrap_or(0);
    s.load_message(id)?;
    if let Ok(m) = s.mailbox.message_mut(id) {
        m.body.text = "Rewritten.".into();
        m.body.html = None;
    }
    Ok(Value::Null)
}

#[test]
fn undo_of_a_body_really_changed_puts_back_the_text_on_disk_and_redo_the_new_one() {
    let tmp = Temp::new("undo-rewrite-ms");
    let path = tmp.file();
    let (mut s, mb, old) = opened(&path, AccountKind::Microsoft);
    let was = mb.message(old).unwrap().clone();
    let mut specs = crate::cmd::registry().all().to_vec();
    specs.push(crate::CommandSpec::new("test.rewrite", "Rewrite", "Test", rewrite));
    s.registry = Arc::new(crate::Registry::new(specs));
    s.run("test.rewrite", &json!({"id": old})).unwrap();
    assert_eq!(text_of(&s, old), "Rewritten.");

    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(text_of(&s, old), was.body.text);
    assert_eq!(s.loaded_message(old).unwrap().body.html, was.body.html);
    save(&path, &s.mailbox).unwrap();
    let m = saved(&path, old);
    assert_eq!((&m.body.text, &m.body.html), (&was.body.text, &was.body.html));

    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(text_of(&s, old), "Rewritten.");
    save(&path, &s.mailbox).unwrap();
    let m = saved(&path, old);
    assert_eq!((m.body.text.as_str(), &m.body.html), ("Rewritten.", &None));
}

/// #575 review: a file removed from an invitation of a Microsoft account whose body and calendar
/// part are on disk, then the removal undone: the calendar part is read to know the invitation,
/// without bringing it or the body into the message, so the text is whole after the undo, here
/// and in the file once it is saved.
#[test]
fn a_file_removed_then_put_back_keeps_a_body_kept_on_disk() {
    let tmp = Temp::new("remove-undo-body");
    let path = tmp.file();
    let (s, _, old) = opened(&path, AccountKind::Microsoft);
    drop(s);
    let mut mb = whole(&path);
    let mut ics = b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n".to_vec();
    ics.extend("X-PAD:0123456789\r\n".repeat(200).bytes());
    ics.extend(b"END:VEVENT\r\nEND:VCALENDAR\r\n");
    let m = mb.message_mut(old).unwrap();
    m.attachments = vec![Attachment::new("agenda.pdf", "application/pdf", vec![7; 64]), Attachment::new("invite.ics", "text/calendar", ics)];
    m.server_attachments = Some(true);
    let was = m.body.text.clone();
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert!(s.mailbox.message(old).unwrap().body_is_kept());
    assert!(!s.mailbox.message(old).unwrap().attachments[1].is_loaded());

    s.run("mail.removeAttachment", &json!({"id": old, "index": 0})).unwrap();
    let m = s.mailbox.message(old).unwrap();
    assert!(m.body_is_kept() && !m.attachments[0].is_loaded(), "nothing brought in");
    assert_eq!((crate::columns::has_files(m), crate::columns::sorts_with_files(m), m.attachments_changed_here), (false, false, Some(false)));

    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(old).unwrap().attachments.len(), 2);
    assert_eq!(text_of(&s, old), was);
    save(&path, &s.mailbox).unwrap();
    clear_cache();
    let reopened = whole(&path);
    let m = reopened.message(old).unwrap();
    assert_eq!((m.attachments.len(), &m.body.text, m.attachments_changed_here), (2, &was, None));
}

/// #575 review: a PDF removed from a message whose HTML is on disk and which has a picture marked
/// inline that the HTML doesn't show: the picture is still a file, read so from the HTML on disk,
/// and the paperclip stays.
#[test]
fn a_file_removed_beside_a_picture_the_html_on_disk_does_not_show_leaves_the_paperclip() {
    let tmp = Temp::new("remove-beside-logo");
    let path = tmp.file();
    let (s, _, old) = opened(&path, AccountKind::Microsoft);
    drop(s);
    let mut mb = whole(&path);
    let m = mb.message_mut(old).unwrap();
    m.body.html = Some(format!("<p>{}</p>", "Regards. ".repeat(300)));
    m.attachments = vec![
        Attachment::new("agenda.pdf", "application/pdf", vec![7; 64]),
        Attachment { content_id: "logo@x.example".into(), inline: true, ..Attachment::new("logo.png", "image/png", vec![7; 64]) },
    ];
    m.server_attachments = Some(true);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);
    assert!(s.mailbox.message(old).unwrap().body.html.is_none(), "the HTML is on disk");

    s.run("mail.removeAttachment", &json!({"id": old, "index": 0})).unwrap();
    let m = s.mailbox.message(old).unwrap();
    assert!(m.body_is_kept() && m.body.html.is_none(), "nothing brought in");
    assert_eq!((crate::columns::has_files(m), crate::columns::sorts_with_files(m), m.attachments_changed_here), (true, true, Some(true)));
    assert!(s.load_message(old).unwrap());
    assert_eq!(s.mailbox.message(old).unwrap().listed_attachments().count(), 1, "opened, the picture is listed");
}

/// #575 review: a calendar part too large to be read isn't read to judge what is left: like any
/// that can't be read, it is taken for a file, whatever its first lines say.
#[test]
fn a_file_removed_beside_a_calendar_part_too_large_to_read_leaves_the_paperclip() {
    let tmp = Temp::new("remove-beside-huge-ics");
    let path = tmp.file();
    let (s, _, old) = opened(&path, AccountKind::Microsoft);
    drop(s);
    let mut mb = whole(&path);
    let mut ics = b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n".to_vec();
    ics.resize(crate::session::MAX_CALENDAR_PART + 1, b'x');
    let m = mb.message_mut(old).unwrap();
    m.attachments = vec![Attachment::new("agenda.pdf", "application/pdf", vec![7; 64]), Attachment::new("invite.ics", "text/calendar", ics)];
    m.server_attachments = Some(true);
    save(&path, &mb).unwrap();
    clear_cache();
    let mut s = session_without_bodies(&path);

    s.run("mail.removeAttachment", &json!({"id": old, "index": 0})).unwrap();
    let m = s.mailbox.message(old).unwrap();
    assert!(m.body_is_kept() && !m.attachments[0].is_loaded(), "nothing brought in");
    assert_eq!((crate::columns::has_files(m), m.attachments_changed_here), (true, Some(true)));
}
