// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `file.importPst` (#815): synthetic .pst files made by the reader's test writer, of invented
//! people at `example.org`, never a real archive. Counts, fields, the report, caps, the
//! password, importing again, cancelling and going on, a dry run, ANSI text in its code page,
//! and hostile bytes.

use std::collections::HashSet;
use std::io::Cursor;

use mailcraft_model::{FlagStatus, Importance, Message};
use mailcraft_pst::test_writer::{self as w, AttachSpec, FileSpec, FolderSpec, Layout, MessageSpec, Value, one_off};
use mailcraft_pst::{Encryption, Format, Limits, PS_PUBLIC_STRINGS};
use mailcraft_time::{Date, DateTime, Timestamp};
use proptest::prelude::*;
use serde_json::{Value as Json, json};

use crate::Session;
use crate::cmd::base64_encode;
use crate::import_pst::item::PSETID_TASK;
use crate::import_pst::{Batch, Job, PST_ITEM_HEADER, Place, Progress, apply, categories, finish, folder_name, read};

/// When the writer's messages were delivered, as a FILETIME and as seconds.
const DELIVERED: u64 = 134_067_498_000_000_000;
const DELIVERED_AT: i64 = (DELIVERED / 10_000_000) as i64 - 11_644_473_600;
/// The ids the writer gives the named properties `spec` lists: PidLidTaskDueDate, then Keywords.
const DUE: u16 = 0x8000;
const KEYWORDS: u16 = 0x8001;

fn now() -> Timestamp {
    DateTime::new(Date::new(2026, 11, 5).unwrap(), 10, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(mailcraft_model::Mailbox::with_local_account("", ""), now(), 0)
}

fn note(subject: &str) -> MessageSpec {
    MessageSpec {
        class: "IPM.Note".into(),
        subject: subject.into(),
        body: format!("{subject}\r\n\r\nWritten for the tests."),
        read: false,
        delivered: DELIVERED,
        ..Default::default()
    }
}

fn person(name: &str, user: &str, kind: i32) -> (String, String, i32) {
    (name.to_string(), format!("{user}@example.org"), kind)
}

fn folder(name: &str, class: &str, messages: Vec<MessageSpec>, children: Vec<FolderSpec>) -> FolderSpec {
    FolderSpec { name: name.into(), class: Some(class.into()), messages, children }
}

fn spec(layout: Layout, folders: Vec<FolderSpec>) -> FileSpec {
    FileSpec {
        layout,
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders,
        named: vec![(PSETID_TASK, 0x8105)],
        named_strings: vec![(PS_PUBLIC_STRINGS, "Keywords".into())],
    }
}

fn unicode_layout() -> Layout {
    Layout::new(Format::Unicode, Encryption::Permute)
}

fn multi_unicode(items: &[&str]) -> Value {
    let mut table = (items.len() as u32).to_le_bytes().to_vec();
    let mut data = Vec::new();
    let base = 4 + 4 * items.len();
    for i in items {
        table.extend_from_slice(&((base + data.len()) as u32).to_le_bytes());
        data.extend(i.encode_utf16().flat_map(u16::to_le_bytes));
    }
    table.extend(data);
    Value { ty: 0x101F, bytes: table }
}

/// A file with mail in a few folders, a calendar, Deleted Items and Junk.
fn mailbox_file() -> FileSpec {
    let mut report = note("Quarterly figures");
    report.recipients = vec![person("Ada Fernsby", "ada", 1), person("Bram Okonjo", "bram", 2), person("Cleo Varga", "cleo", 3)];
    report.read = true;
    report.extra = vec![
        (0x0C1A, w::unicode("Dee Halloran")),
        (0x0C1F, w::unicode("dee@example.org")),
        (0x0042, w::unicode("Dee Halloran")),
        (0x0065, w::unicode("dee@example.org")),
        (0x1035, w::unicode("<figures-1@example.org>")),
        (0x1042, w::unicode("<agenda-7@example.org>")),
        (0x1039, w::unicode("<start-1@example.org> <agenda-7@example.org>")),
        (0x0039, w::time(DELIVERED - 600_000_000)),
        (0x1090, w::int32(2)),
        (DUE, w::time(DELIVERED + 864_000_000_000)),
        (0x0036, w::int32(2)),
        (KEYWORDS, multi_unicode(&["Finance", "Board"])),
        (0x1081, w::int32(102)),
    ];
    let mut forwarded = note("Minutes of the garden committee");
    forwarded.recipients = vec![person("Eli Marsh", "eli", 1)];
    forwarded.attachments = vec![AttachSpec::File { name: "minutes.txt".into(), mime: "text/plain".into(), data: b"Present: everyone.".to_vec() }];
    report.attachments = vec![
        AttachSpec::File { name: "figures.csv".into(), mime: "text/csv".into(), data: b"quarter,total\nQ3,12\n".to_vec() },
        AttachSpec::Message(Box::new(forwarded)),
    ];
    let mut appointment = note("Dentist");
    appointment.class = "IPM.StickyNote".into();
    let mut contact = note("Fern Ochoa");
    contact.class = "IPM.Contact".into();
    let mut stray = note("A message among contacts");
    stray.extra = vec![(0x0017, w::int32(2))];
    folder_list(vec![
        folder("Inbox", "IPF.Note", vec![report, note("Lunch on Friday?")], vec![folder("Projects", "IPF.Note", vec![note("Shed roof")], vec![])]),
        folder("Calendar", "IPF.Appointment", vec![appointment], vec![]),
        folder("Contacts", "IPF.Contact", vec![contact, stray], vec![]),
        folder("Deleted Items", "IPF.Note", vec![note("Old news")], vec![]),
        folder("Junk E-mail", "IPF.Note", vec![note("You have won")], vec![]),
        folder("Empty", "IPF.Note", vec![], vec![]),
    ])
}

fn folder_list(folders: Vec<FolderSpec>) -> FileSpec {
    spec(unicode_layout(), folders)
}

fn job(dry_run: bool) -> Job {
    Job {
        password: None,
        dry_run,
        file: "old-mail.pst".into(),
        name: "old-mail".into(),
        known: HashSet::new(),
        room: 31,
        known_contacts: HashSet::new(),
        known_plans: HashSet::new(),
        kept_room: crate::import_pst::item::KEPT_TEXT_BUDGET,
        keep_at: None,
        calendar: None,
        here: crate::import_pst::calendar::Here { offset: Some(0), zone: mailcraft_time::Zone::UTC },
    }
}

/// Run the command over `bytes` handed over as data.
fn import(s: &mut Session, bytes: &[u8], extra: Json) -> Result<Json, crate::CmdError> {
    let mut params = json!({"name": "old-mail.pst", "data": base64_encode(bytes)});
    if let (Some(p), Some(e)) = (params.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            p.insert(k.clone(), v.clone());
        }
    }
    s.run("file.importPst", &params)
}

fn folder_id(s: &Session, path: &[&str]) -> Option<u64> {
    let mut parent = None;
    let mut at = None;
    for name in path {
        at = s.mailbox.folders.iter().find(|f| f.parent == parent && f.name == *name).map(|f| f.id);
        parent = at;
        at?;
    }
    at
}

fn messages_in<'a>(s: &'a Session, path: &[&str]) -> Vec<&'a Message> {
    let Some(id) = folder_id(s, path) else { return Vec::new() };
    s.mailbox.messages_in(id).map(|m| m.as_ref()).collect()
}

fn row<'a>(answer: &'a Json, path: &str) -> &'a Json {
    answer["folders"].as_array().unwrap().iter().find(|r| r["path"] == path).unwrap_or_else(|| panic!("no row {path}: {answer}"))
}

fn adds_up(answer: &Json) {
    for r in answer["folders"].as_array().unwrap() {
        let sum: u64 = ["added", "already", "leftOut"].iter().map(|k| r[*k].as_u64().unwrap()).sum();
        assert_eq!(sum, r["found"].as_u64().unwrap(), "{r}");
    }
}

#[test]
fn the_mail_of_every_folder_comes_in_and_every_item_is_accounted_for() {
    let mut s = session();
    let bytes = w::build(&mailbox_file());
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["found"], 8, "{answer}");
    assert_eq!(answer["added"], 8, "{answer}");
    assert_eq!((answer["messages"].clone(), answer["contacts"].clone(), answer["lists"].clone()), (json!(6), json!(1), json!(0)));
    assert_eq!(answer["stickyNotes"], 1, "{answer}");
    assert_eq!(answer["leftOut"], 0, "{answer}");
    assert_eq!(row(&answer, "Inbox")["added"], 2);
    assert_eq!(row(&answer, "Inbox/Projects")["added"], 1);
    assert_eq!(row(&answer, "Calendar")["added"], 1);
    assert_eq!(row(&answer, "Contacts")["added"], 2);
    assert_eq!(s.mailbox.contacts.len(), 1);
    // The file's own folders are named, not brought.
    let not: Vec<&str> = answer["notBrought"].as_array().unwrap().iter().map(|e| e["what"].as_str().unwrap()).collect();
    assert!(not.contains(&"Search Root"), "{not:?}");
    // Into folders of their own under a new one named after the file, on this computer.
    let top = folder_id(&s, &["old-mail"]).unwrap();
    assert_eq!(answer["folder"], top);
    assert_eq!(s.view.folder, top);
    for path in [
        &["old-mail", "Inbox"][..],
        &["old-mail", "Inbox", "Projects"],
        &["old-mail", "Deleted Items"],
        &["old-mail", "Junk E-mail"],
        &["old-mail", "Contacts"],
    ] {
        assert!(!messages_in(&s, path).is_empty(), "{path:?}");
    }
    // An empty folder of mail comes too; a calendar with no mail in it doesn't.
    assert!(folder_id(&s, &["old-mail", "Empty"]).is_some());
    assert!(folder_id(&s, &["old-mail", "Calendar"]).is_none());
    let local = s.mailbox.accounts.iter().find(|a| a.kind == mailcraft_model::AccountKind::Local).unwrap().id;
    assert!(s.mailbox.messages.values().all(|m| m.account == local));
    // Every message brought in remembers where in the file it was.
    assert!(s.mailbox.messages.values().all(|m| m.headers.iter().any(|(k, _)| k == PST_ITEM_HEADER)));
}

#[test]
fn a_message_keeps_its_people_dates_state_and_attachments() {
    let mut s = session();
    import(&mut s, &w::build(&mailbox_file()), json!({})).unwrap();
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let m = inbox.iter().find(|m| m.subject == "Quarterly figures").unwrap();
    assert_eq!(m.from.email, "dee@example.org");
    assert_eq!(m.from.name, "Dee Halloran");
    assert_eq!(m.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ada@example.org"]);
    assert_eq!(m.cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["bram@example.org"]);
    assert_eq!(m.bcc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["cleo@example.org"]);
    assert_eq!(m.message_id, "figures-1@example.org");
    assert_eq!(m.in_reply_to, "agenda-7@example.org");
    assert_eq!(m.references, ["start-1@example.org", "agenda-7@example.org"]);
    assert_eq!(m.received, DELIVERED_AT);
    assert_eq!(m.date, DELIVERED_AT - 60);
    assert!(m.read && m.answered && !m.forwarded);
    assert_eq!(m.flag, FlagStatus::Flagged);
    assert_eq!(m.flag_due, Some(DELIVERED_AT + 86_400));
    assert_eq!(m.categories, ["Finance", "Board"]);
    assert!(m.headers.iter().any(|(k, v)| k == "Sensitivity" && v == "Private"));
    assert!(m.body.text.starts_with("Quarterly figures"), "{:?}", m.body.text);
    assert_eq!(m.attachments.len(), 2);
    assert_eq!(m.attachments[0].name, "figures.csv");
    assert_eq!(m.attachments[0].mime, "text/csv");
    assert_eq!(m.attachments[0].data.as_slice(), b"quarter,total\nQ3,12\n");
    // An attached message is a message/rfc822 file, with its own attachment inside.
    let attached = &m.attachments[1];
    assert_eq!(attached.mime, "message/rfc822");
    assert!(attached.name.ends_with(".eml"), "{}", attached.name);
    let inner = mailcraft_mime_model::parse(&attached.data);
    assert_eq!(inner.subject, "Minutes of the garden committee");
    assert_eq!(inner.to[0].email, "eli@example.org");
    assert_eq!(inner.attachments[0].name, "minutes.txt");
    assert_eq!(inner.attachments[0].data.as_slice(), b"Present: everyone.");
    let stray = messages_in(&s, &["old-mail", "Contacts"]);
    assert_eq!(stray[0].importance, Importance::High);
}

#[test]
fn the_headers_it_was_sent_with_come_first() {
    let mut m = note("Ignored subject");
    let date = mailcraft_time::format_rfc5322(DELIVERED_AT - 600, 0);
    m.extra = vec![(
        0x007D,
        w::unicode(&format!(
            "Received: from mx.example.org\r\nFrom: Gus Pardo <gus@example.org>\r\nTo: Hana Ito <hana@example.org>, ivo@example.org\r\nSubject: =?utf-8?q?Gr=C3=BC=C3=9Fe?=\r\nDate: {date}\r\nMessage-ID: <hdr-1@example.org>\r\nX-Mailer: Test\r\n",
        )),
    )];
    let mut s = session();
    import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m], vec![])])), json!({})).unwrap();
    let m = messages_in(&s, &["old-mail", "Inbox"])[0];
    assert_eq!(m.from.email, "gus@example.org");
    assert_eq!(m.to.len(), 2);
    assert_eq!(m.subject, "Grüße");
    assert_eq!(m.message_id, "hdr-1@example.org");
    assert_eq!(m.date, DELIVERED_AT - 600);
    assert_eq!(m.received, DELIVERED_AT);
    assert!(m.headers.iter().any(|(k, v)| k == "X-Mailer" && v == "Test"));
    assert!(m.headers.iter().any(|(k, _)| k == "Received"));
}

#[test]
fn html_inline_pictures_ole_links_and_smime_are_kept_or_named() {
    let mut m = note("Pictures");
    m.class = "IPM.Note.SMIME.MultipartSigned".into();
    m.extra = vec![(0x1013, w::binary(b"<html><body><p>See <img src=\"cid:logo-1\"></p></body></html>"))];
    m.attachments = vec![
        AttachSpec::Props(vec![
            (0x3705, w::int32(1)),
            (0x3707, w::unicode("logo.png")),
            (0x370E, w::unicode("image/png")),
            (0x3712, w::unicode("<logo-1>")),
            (0x3701, w::binary(b"\x89PNG\r\n\x1a\n")),
        ]),
        AttachSpec::Props(vec![(0x3705, w::int32(4)), (0x3707, w::unicode("plan.docx")), (0x370D, w::unicode("\\\\server\\share\\plan.docx"))]),
        AttachSpec::Ole(vec![1, 2, 3, 4, 5]),
    ];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m], vec![])])), json!({})).unwrap();
    let m = messages_in(&s, &["old-mail", "Inbox"])[0];
    assert!(m.body.html.as_deref().unwrap().contains("cid:logo-1"));
    let logo = m.attachments.iter().find(|a| a.name == "logo.png").unwrap();
    assert_eq!((logo.content_id.as_str(), logo.inline, logo.mime.as_str()), ("logo-1", true, "image/png"));
    let ole = m.attachments.iter().find(|a| a.name == "embedded object.bin").unwrap();
    assert_eq!(ole.data.as_slice(), &[1, 2, 3, 4, 5]);
    assert!(!m.attachments.iter().any(|a| a.name == "plan.docx"));
    let notes: Vec<&str> = answer["notes"].as_array().unwrap().iter().map(|n| n["why"].as_str().unwrap()).collect();
    assert!(notes.iter().any(|n| n.contains("plan.docx") && n.contains("not fetched")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("kept as it was, can't be read yet")), "{notes:?}");
}

#[test]
fn rich_text_alone_becomes_the_body_and_is_kept_as_an_attachment() {
    let rtf = b"{\\rtf1 Hello}";
    let mut stored = Vec::new();
    stored.extend_from_slice(&((rtf.len() + 12) as u32).to_le_bytes());
    stored.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
    stored.extend_from_slice(b"MELA");
    stored.extend_from_slice(&0u32.to_le_bytes());
    stored.extend_from_slice(rtf);
    // Rich text alone (no text, no HTML): kept, and said.
    let mut m = note("Rich");
    m.body = String::new();
    m.extra = vec![(0x1009, w::binary(&stored))];
    // Rich text beside a text, as Outlook keeps most mail: the text, and no file.
    let mut plain = note("Plain");
    plain.extra = vec![(0x1009, w::binary(&stored))];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m, plain], vec![])])), json!({})).unwrap();
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let rich = inbox.iter().find(|m| m.subject == "Rich").unwrap();
    let file = rich.attachments.iter().find(|a| a.name == "message body.rtf").unwrap();
    assert_eq!(file.data.as_slice(), rtf);
    assert_eq!(file.mime, "application/rtf");
    assert_eq!(rich.body.text, "Hello");
    let plain = inbox.iter().find(|m| m.subject == "Plain").unwrap();
    assert!(plain.body.text.starts_with("Plain") && plain.attachments.is_empty(), "{:?}", plain.attachments);
    let notes = answer["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{answer}");
    assert!(notes[0]["why"].as_str().unwrap().contains("rich text") && notes[0]["what"].as_str().unwrap().contains("Rich"), "{answer}");
}

#[test]
fn ansi_text_is_read_in_the_items_code_page() {
    // "Привет" in Windows-1251, written as the writer writes 8-bit text: a character a byte.
    let cyrillic: String = [0xCFu8, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2].iter().map(|&b| char::from(b)).collect();
    let mut m = note(&cyrillic);
    m.body = cyrillic.clone();
    m.recipients = vec![(cyrillic.clone(), "olga@example.org".into(), 1)];
    m.extra = vec![(0x3FFD, w::int32(1251))];
    // And one without a code page: Windows-1252, where 0x80 is the euro sign.
    let euro = note("\u{80}5 lunch");
    let file = spec(Layout::new(Format::Ansi, Encryption::Cyclic), vec![folder("Inbox", "IPF.Note", vec![m, euro], vec![])]);
    let mut s = session();
    let answer = import(&mut s, &w::build(&file), json!({})).unwrap();
    assert_eq!(answer["format"], "ANSI");
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let m = inbox.iter().find(|m| m.to.first().is_some_and(|a| a.email == "olga@example.org")).unwrap();
    assert_eq!(m.subject, "Привет");
    assert_eq!(m.body.text, "Привет");
    assert_eq!(m.to[0].name, "Привет");
    assert!(inbox.iter().any(|m| m.subject == "€5 lunch"), "{:?}", inbox.iter().map(|m| &m.subject).collect::<Vec<_>>());
}

#[test]
fn a_file_with_a_password_imports_nothing_without_it() {
    let mut file = mailbox_file();
    file.password = Some("garden-shed".into());
    let bytes = w::build(&file);
    let mut s = session();
    let before = s.mailbox.messages.len();
    let none = import(&mut s, &bytes, json!({})).unwrap_err().to_string();
    assert!(none.contains("password"), "{none}");
    let wrong = import(&mut s, &bytes, json!({"password": "garden-gate"})).unwrap_err().to_string();
    assert!(wrong.contains("isn't the password") && !wrong.contains("garden"), "{wrong}");
    assert_eq!(s.mailbox.messages.len(), before);
    assert!(folder_id(&s, &["old-mail"]).is_none());
    let answer = import(&mut s, &bytes, json!({"password": "garden-shed"})).unwrap();
    assert_eq!(answer["added"], 8);
    assert!(!answer.to_string().contains("garden-shed"));
}

#[test]
fn importing_again_skips_what_is_already_here() {
    let mut s = session();
    let bytes = w::build(&mailbox_file());
    import(&mut s, &bytes, json!({})).unwrap();
    let count = s.mailbox.messages.len();
    let folders = s.mailbox.folders.len();
    let again = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&again);
    assert_eq!((again["added"].as_u64(), again["already"].as_u64()), (Some(0), Some(8)), "{again}");
    assert_eq!(s.mailbox.messages.len(), count);
    assert_eq!(s.mailbox.folders.len(), folders, "the same folders are used again");
}

#[test]
fn a_dry_run_counts_and_changes_nothing() {
    let mut s = session();
    let before = (s.mailbox.messages.len(), s.mailbox.folders.len());
    let answer = import(&mut s, &w::build(&mailbox_file()), json!({"dryRun": true})).unwrap();
    adds_up(&answer);
    assert_eq!((answer["dryRun"].as_bool(), answer["added"].as_u64()), (Some(true), Some(8)), "{answer}");
    assert_eq!((s.mailbox.messages.len(), s.mailbox.folders.len()), before);
    assert!(answer["report"].is_null());
}

#[test]
fn mail_can_go_under_a_folder_on_this_computer_but_never_a_servers() {
    let mut s = session();
    let account = s.mailbox.local_account();
    let parent = s.mailbox.add_folder(account, None, "Archives").unwrap();
    let answer = import(&mut s, &w::build(&mailbox_file()), json!({"places": {"mail": parent}})).unwrap();
    assert_eq!(answer["folder"], parent);
    assert!(!messages_in(&s, &["Archives", "Inbox"]).is_empty());
    let server = s.run("account.add", &json!({"email": "kit@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let theirs = s.mailbox.folders.iter().find(|f| f.account == server).unwrap().id;
    let refused = import(&mut s, &w::build(&mailbox_file()), json!({"places": {"mail": theirs}})).unwrap_err().to_string();
    assert!(refused.contains("on this computer only"), "{refused}");
    assert!(import(&mut s, &w::build(&mailbox_file()), json!({"places": {"mail": "elsewhere"}})).is_err());
}

#[test]
fn cancelling_keeps_what_is_in_and_another_run_brings_the_rest() {
    let messages: Vec<MessageSpec> = (0..10).map(|i| note(&format!("Letter {i}"))).collect();
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", messages, vec![])]));
    let mut s = session();
    let place = Place::New("old-mail".into());
    let progress = Progress::default();
    let mut sink = |batch: Batch| -> Result<(), crate::CmdError> {
        apply(&mut s, &place, batch)?;
        // The owner presses Cancel once the first batch is in.
        progress.cancel();
        Ok(())
    };
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &progress, &mut sink).unwrap();
    assert!(outcome.report.cancelled);
    let answer = finish(&mut s, &place, outcome, false).unwrap();
    assert_eq!(answer["cancelled"], true);
    let first = messages_in(&s, &["old-mail", "Inbox"]).len();
    assert!(first > 0 && first < 10, "{first}");
    // Again: the rest comes in, and what came before is already here.
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["already"].as_u64(), Some(first as u64));
    assert_eq!(answer["added"].as_u64(), Some(10 - first as u64));
    assert_eq!(messages_in(&s, &["old-mail", "Inbox"]).len(), 10);
    let subjects: HashSet<&str> = messages_in(&s, &["old-mail", "Inbox"]).iter().map(|m| m.subject.as_str()).collect();
    assert_eq!(subjects.len(), 10);
}

#[test]
fn progress_says_the_folder_and_how_far() {
    let bytes = w::build(&mailbox_file());
    let progress = Progress::default();
    let mut batches = 0;
    let mut sink = |_: Batch| -> Result<(), crate::CmdError> {
        batches += 1;
        Ok(())
    };
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &progress, &mut sink).unwrap();
    assert!(batches >= 2, "batches of {}", crate::import_pst::BATCH_MESSAGES);
    assert_eq!(progress.total.load(std::sync::atomic::Ordering::Relaxed), 8);
    assert_eq!(progress.done.load(std::sync::atomic::Ordering::Relaxed), 8);
    assert_eq!(progress.folders_total.load(std::sync::atomic::Ordering::Relaxed), outcome.report.rows.len());
    assert!(!progress.folder().is_empty());
}

#[test]
fn caps_leave_items_out_and_name_them() {
    let mut big = note("Big attachment");
    big.attachments = vec![AttachSpec::File { name: "big.bin".into(), mime: "application/octet-stream".into(), data: vec![7; 40_000] }];
    let messages: Vec<MessageSpec> = std::iter::once(big).chain((0..5).map(|i| note(&format!("Note {i}")))).collect();
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", messages, vec![])]));
    let limits = Limits { max_items: 4, max_attachment_bytes: 10_000, ..Limits::default() };
    let mut sink = |_: Batch| Ok(());
    let outcome = read(Cursor::new(&bytes), &job(false), limits, &Progress::default(), &mut sink).unwrap();
    let r = &outcome.report;
    assert!(r.rows.iter().all(|row| row.adds_up()), "{:?}", r.rows);
    let total = r.total();
    assert_eq!(total.found, 6, "{:?}", r.rows);
    assert_eq!(total.left_out, 2, "{:?}", r.rows);
    assert!(r.left_out.iter().any(|e| e.why.contains("number of items")), "{:?}", r.left_out);
    assert!(r.notes.iter().any(|n| n.why.contains("big.bin")), "{:?}", r.notes);
    // An item larger than the reader takes is left out, with why; the rest come in.
    let mut long = note("Long");
    long.body = "words ".repeat(4_000);
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![long, note("Short")], vec![])]));
    let limits = Limits { max_item_bytes: 10_000, ..Limits::default() };
    let outcome = read(Cursor::new(&bytes), &job(false), limits, &Progress::default(), &mut sink).unwrap();
    let t = outcome.report.total();
    assert_eq!((t.found, t.added, t.left_out), (2, 1, 1), "{:?}", outcome.report.rows);
    assert!(outcome.report.left_out[0].why.contains("larger than the cap"), "{:?}", outcome.report.left_out);
    assert!(outcome.report.rows.iter().all(|row| row.adds_up()));
}

#[test]
fn the_report_is_written_beside_the_mailbox() {
    let dir = std::env::temp_dir().join(format!("mc-pst-report-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = session();
    s.path = Some(dir.join("mail.mailcraft"));
    let answer = import(&mut s, &w::build(&mailbox_file()), json!({})).unwrap();
    let path = answer["report"].as_str().unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert!(path.contains("old-mail import report"), "{path}");
    assert!(text.contains("Found 8 items: 8 brought in, 0 already here, 0 left out."), "{text}");
    assert!(text.contains("Inbox/Projects: 1 / 1 / 0 / 0"), "{text}");
    assert!(text.contains("Search Root"), "{text}");
}

#[test]
fn the_files_list_of_categories_is_read_with_its_colours() {
    let xml = r#"<?xml version="1.0"?><categories default="Red"><category name="Finance &amp; Board" color="7" keyboardShortcut="0"/><category name="Garden" color="-1"/><category  name='Quiet' color="99"/></categories>"#;
    let list = categories(xml);
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].0, "Finance & Board");
    assert_eq!(Some(list[0].1), mailcraft_model::preset_category_color(7));
    assert_eq!(list[1].1, mailcraft_model::GREY_CATEGORY);
    assert_eq!(list[2].0, "Quiet");
    assert!(categories("<categories><category name=\"\u{0}\"/><categoryx name=\"x\"/>").is_empty());
}

#[test]
fn the_folder_is_named_after_the_file() {
    assert_eq!(folder_name("/home/x/Old Mail.PST"), "Old Mail");
    assert_eq!(folder_name("C:\\Users\\x\\archive.pst"), "archive");
    assert_eq!(folder_name("../.."), "Folder");
    assert_eq!(folder_name("noext"), "noext");
}

#[test]
fn a_staged_batch_and_the_report_come_in_through_the_command() {
    let bytes = w::build(&mailbox_file());
    let mut s = session();
    let mut batches = Vec::new();
    let mut sink = |b: Batch| -> Result<(), crate::CmdError> {
        batches.push(b);
        Ok(())
    };
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &Progress::default(), &mut sink).unwrap();
    for b in batches {
        s.staged_pst = Some(crate::import_pst::Staged::Batch(b));
        s.run("file.importPst", &json!({"staged": true, "name": "old-mail"})).unwrap();
    }
    s.staged_pst = Some(crate::import_pst::Staged::Done(Box::new(outcome)));
    let answer = s.run("file.importPst", &json!({"staged": true, "name": "old-mail"})).unwrap();
    assert_eq!(answer["added"], 8);
    assert_eq!(s.mailbox.messages.len(), 6);
    assert!(s.run("file.importPst", &json!({"staged": true})).is_err());
}

#[test]
fn not_a_pst_file_and_junk_params_say_so() {
    let mut s = session();
    let e = import(&mut s, b"From x@example.org\nSubject: no\n\n", json!({})).unwrap_err().to_string();
    assert!(e.contains("isn't a .pst file"), "{e}");
    assert!(s.run("file.importPst", &json!({})).is_err());
    assert!(s.run("file.importPst", &json!({"path": "/nonexistent/x.pst"})).is_err());
    assert!(s.run("file.importPst", &json!({"name": "x.pst", "data": "!!!"})).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A file damaged anywhere never makes the import panic, and what it brings is accounted
    /// for item by item.
    #[test]
    fn a_damaged_file_never_panics_and_adds_up(flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..24)) {
        let mut bytes = w::build(&mailbox_file());
        for (at, b) in flips {
            let i = at.index(bytes.len());
            bytes[i] ^= b | 1;
        }
        let mut sink = |_: Batch| Ok(());
        if let Ok(outcome) = read(Cursor::new(&bytes), &job(false), Limits::default(), &Progress::default(), &mut sink) {
            prop_assert!(outcome.report.rows.iter().all(|r| r.adds_up()));
        }
    }

    /// And a file cut short anywhere.
    #[test]
    fn a_file_cut_short_never_panics(cut in any::<prop::sample::Index>()) {
        let bytes = w::build(&mailbox_file());
        let bytes = &bytes[..cut.index(bytes.len())];
        let mut sink = |_: Batch| Ok(());
        let _ = read(Cursor::new(bytes), &job(true), Limits::default(), &Progress::default(), &mut sink);
    }
}

/// A chain of `depth` folders, `Level 1` to `Level n`, with one message in the deepest.
fn chain(depth: usize) -> FolderSpec {
    let mut f = folder(&format!("Level {depth}"), "IPF.Note", vec![note("At the bottom")], vec![]);
    for n in (1..depth).rev() {
        f = folder(&format!("Level {n}"), "IPF.Note", vec![], vec![f]);
    }
    f
}

/// Folders nested deeper than a mailbox here may be (P2-1): the deep ones' mail goes into the
/// deepest that fits, and their rows say so; under a deep folder of the owner's too.
#[test]
fn folders_nested_too_deeply_go_into_the_deepest_that_fits() {
    let bytes = w::build(&folder_list(vec![chain(40)]));
    let mut s = session();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["added"], 1, "{answer}");
    assert_eq!(s.mailbox.messages.len(), 1);
    let m = s.mailbox.messages.values().next().unwrap();
    assert_eq!(s.mailbox.depth(m.folder), mailcraft_model::MAX_FOLDER_DEPTH);
    let deep = answer["folders"].as_array().unwrap().iter().find(|r| r["path"].as_str().unwrap().ends_with("Level 40")).unwrap();
    assert!(deep["into"].as_str().unwrap().ends_with("Level 31"), "{deep}");

    // Under a folder of the owner's that is already 30 deep: two levels fit below it.
    let mut s = session();
    let account = s.mailbox.local_account();
    let mut parent = None;
    for n in 0..30 {
        parent = Some(s.mailbox.add_folder(account, parent, &format!("Mine {n}")).unwrap());
    }
    let answer = import(&mut s, &bytes, json!({"places": {"mail": parent.unwrap()}})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["added"], 1, "{answer}");
    let m = s.mailbox.messages.values().next().unwrap();
    assert_eq!(s.mailbox.depth(m.folder), mailcraft_model::MAX_FOLDER_DEPTH);
}

/// The batches of an import the window hands over are one step to undo (P2-2), which takes the
/// whole import out; Undo waits while it runs. One command is one step too.
#[test]
fn an_import_is_one_step_to_undo() {
    let messages: Vec<MessageSpec> = (0..10).map(|i| note(&format!("Letter {i}"))).collect();
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", messages, vec![])]));
    let mut s = session();
    s.run("folder.new", &json!({"name": "Mine"})).unwrap();
    let steps = s.history.len();
    let mut batches = Vec::new();
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &Progress::default(), &mut |b: Batch| {
        batches.push(b);
        Ok(())
    })
    .unwrap();
    assert!(batches.len() >= 3, "{}", batches.len());
    for b in batches {
        s.staged_pst = Some(crate::import_pst::Staged::Batch(b));
        s.run("file.importPst", &json!({"staged": true, "name": "old-mail"})).unwrap();
        assert_eq!(s.history.len(), steps, "a batch is no step of its own");
        assert!(s.run("edit.undo", &json!({})).is_err(), "Undo waits while it runs");
    }
    s.staged_pst = Some(crate::import_pst::Staged::Done(Box::new(outcome)));
    s.run("file.importPst", &json!({"staged": true, "name": "old-mail"})).unwrap();
    assert_eq!(s.history.len(), steps + 1);
    assert_eq!(s.undo_label(), Some("Import a .pst File"));
    assert_eq!(s.mailbox.messages.len(), 10);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.messages.is_empty());
    assert!(folder_id(&s, &["old-mail"]).is_none());
    assert!(s.mailbox.folders.iter().any(|f| f.name == "Mine"), "what came before stays");

    // One command: one step.
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((s.history.len(), s.undo_label()), (1, Some("Import a .pst File")));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.messages.is_empty());
}

/// The batches and the outcome of reading `bytes`.
fn staged(bytes: &[u8]) -> (Vec<Batch>, crate::import_pst::Outcome) {
    let mut batches = Vec::new();
    let outcome = read(Cursor::new(bytes), &job(false), Limits::default(), &Progress::default(), &mut |b: Batch| {
        batches.push(b);
        Ok(())
    })
    .unwrap();
    (batches, outcome)
}

fn hand_over(s: &mut Session, what: crate::import_pst::Staged, params: &Json) -> Result<Json, crate::CmdError> {
    s.staged_pst = Some(what);
    s.run("file.importPst", params)
}

fn letters(n: usize) -> Vec<u8> {
    w::build(&folder_list(vec![folder("Inbox", "IPF.Note", (0..n).map(|i| note(&format!("N {i}"))).collect(), vec![])]))
}

/// The place goes for good while the window's import runs: its last word fails, and what came
/// in is still one step to undo, with Undo free again.
#[test]
fn an_import_whose_last_word_fails_is_one_step_too() {
    let mut s = session();
    let account = s.mailbox.local_account();
    let under = s.mailbox.add_folder(account, None, "Archives").unwrap();
    let (mut batches, outcome) = staged(&letters(6));
    let params = json!({"staged": true, "name": "old-mail", "places": {"mail": under}});
    hand_over(&mut s, crate::import_pst::Staged::Batch(batches.remove(0)), &params).unwrap();
    s.mailbox.remove_folders(&[under]);
    assert!(hand_over(&mut s, crate::import_pst::Staged::Batch(batches.remove(0)), &params).is_err());
    assert!(hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).is_err());
    assert!(!s.importing_pst(), "the import is over");
    assert_eq!(s.undo_label(), Some("Import a .pst File"));
    s.run("edit.undo", &json!({})).unwrap();
}

/// Undoing an import takes out its mail alone: what the owner put in its folders meanwhile, a
/// message or a folder, stays where it was put; Redo brings the import back.
#[test]
fn undoing_an_import_keeps_what_the_owner_put_in_its_folders() {
    let mut s = session();
    let account = s.mailbox.local_account();
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let own = s.mailbox.add_message(Message { folder: inbox, subject: "the owner's".into(), ..Default::default() }).unwrap();
    let (batches, outcome) = staged(&letters(8));
    let params = json!({"staged": true, "name": "old-mail"});
    for (i, b) in batches.into_iter().enumerate() {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
        if i == 0 {
            let imported = folder_id(&s, &["old-mail", "Inbox"]).unwrap();
            s.mailbox.message_mut(own).unwrap().folder = imported;
            s.mailbox.add_folder(account, Some(imported), "Sub").unwrap();
        }
    }
    hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).unwrap();
    assert_eq!(s.mailbox.messages.len(), 9);
    s.run("edit.undo", &json!({})).unwrap();
    let imported = folder_id(&s, &["old-mail", "Inbox"]).expect("the folder with the owner's things stays");
    assert_eq!(s.mailbox.messages.len(), 1);
    assert_eq!(s.mailbox.message(own).unwrap().folder, imported);
    assert!(folder_id(&s, &["old-mail", "Inbox", "Sub"]).is_some(), "the owner's folder stays where it was");
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.len(), 9);
}

/// Another import (an agent's, a script's) waits while the window's runs, which keeps its one
/// step and its Undo held; a batch of another file is refused too.
#[test]
fn another_import_waits_while_one_runs() {
    let mut s = session();
    let bytes = letters(6);
    let (mut batches, outcome) = staged(&bytes);
    let params = json!({"staged": true, "name": "old-mail"});
    hand_over(&mut s, crate::import_pst::Staged::Batch(batches.remove(0)), &params).unwrap();
    let e = import(&mut s, &bytes, json!({})).unwrap_err().to_string();
    assert!(e.contains("being imported"), "{e}");
    let mut other = Batch::default();
    other.identity = "another file".into();
    assert!(hand_over(&mut s, crate::import_pst::Staged::Batch(other), &params).unwrap_err().to_string().contains("being imported"));
    assert!(s.importing_pst());
    for b in batches {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
    }
    hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.messages.is_empty(), "the first batch is in the step too");
}

/// A batch is measured by all a message holds (P2-3): recipients and headers too.
#[test]
fn a_message_is_measured_by_all_it_holds() {
    let mut m = Message::default();
    let small = crate::import_pst::item::held(&m);
    m.to = (0..1000).map(|i| mailcraft_model::Address { name: "x".repeat(100), email: format!("p{i}@example.org") }).collect();
    m.headers = (0..1000).map(|i| (format!("X-H{i}"), "v".repeat(100))).collect();
    m.references = vec!["<r@example.org>".repeat(10); 100];
    assert!(crate::import_pst::item::held(&m) > small + 200_000, "{}", crate::import_pst::item::held(&m));
}

/// The report's name keeps "import report" and the time whatever the file is called, and an
/// existing file (or a link) beside the mailbox is never written over (P2-4).
#[test]
fn the_report_never_writes_over_a_file() {
    let dir = std::env::temp_dir().join(format!("mc-pst-report-name-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = session();
    s.path = Some(dir.join("mail.mailcraft"));
    let long = format!("{}.pst", "a".repeat(120));
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![note("One")], vec![])]));
    let mut params = json!({"name": long, "data": base64_encode(&bytes)});
    let first = s.run("file.importPst", &params).unwrap()["report"].as_str().unwrap().to_string();
    assert!(first.contains("import report 2026-11-05T10-00-00"), "{first}");
    // A link where the next report would go: not followed, the next free name is taken.
    params["dryRun"] = json!(false);
    let target = dir.join("target.txt");
    std::fs::write(&target, "the owner's").unwrap();
    // Only Unix makes the link: a symbolic link needs extra rights on Windows.
    #[cfg(unix)]
    {
        let name = std::path::Path::new(&first).file_name().unwrap().to_string_lossy().replace(".txt", " (2).txt");
        std::os::unix::fs::symlink(&target, dir.join(&name)).unwrap();
    }
    let second = s.run("file.importPst", &params).unwrap()["report"].as_str().unwrap().to_string();
    let third = s.run("file.importPst", &params).unwrap()["report"].as_str().unwrap().to_string();
    assert_ne!(first, second);
    assert_ne!(second, third);
    assert!(std::fs::read_to_string(&first).unwrap().contains("Found 1 items: 1 brought in"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "the owner's");
    std::fs::remove_dir_all(&dir).ok();
}

/// A new folder for the file is never one of the owner's that only shares its name (P2-6): the
/// import makes `name (2)`, and a second run of the same file finds that one again.
#[test]
fn the_new_folder_is_never_somebody_elses() {
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![note("Theirs")], vec![])]));
    let mut s = session();
    let account = s.mailbox.local_account();
    let own = s.mailbox.add_folder(account, None, "OLD-MAIL").unwrap();
    s.mailbox.add_message(Message { folder: own, subject: "the owner's".into(), ..Default::default() }).unwrap();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!(answer["name"], "old-mail (2)", "{answer}");
    assert_eq!(s.mailbox.messages_in(own).count(), 1);
    let again = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((again["name"].as_str(), again["already"].as_u64()), (Some("old-mail (2)"), Some(1)), "{again}");
    // An empty folder this session's import made is its own too.
    let empty = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![], vec![])]));
    let mut s = session();
    let first = import(&mut s, &empty, json!({})).unwrap();
    let second = import(&mut s, &empty, json!({})).unwrap();
    assert_eq!(first["name"], second["name"]);
}

/// The report has a row for each of at most so many folders and adds up the rest (P2-7); items
/// of no folder are one row.
#[test]
fn the_reports_rows_are_capped_and_still_add_up() {
    let mut r = crate::import_pst::report::Report::default();
    for i in 0..2500 {
        r.push_row(crate::import_pst::report::Row { path: format!("F{i}{}", "x".repeat(300)), found: 2, added: 2, ..Default::default() });
    }
    assert_eq!(r.rows.len(), crate::import_pst::report::MAX_ROWS);
    assert_eq!(r.folders_more, 500);
    assert!(r.rows.iter().all(|row| row.path.chars().count() <= 201 && row.path.starts_with('…')));
    assert_eq!((r.total().found, r.total().added), (5000, 5000));
    assert!(r.first_short().is_none());
    assert_eq!(r.to_json()["foldersMore"], 500);
    assert!(r.to_text("now").contains("and 500 more folders"));
}

/// Thousands of folders of one name side by side stay apart, each once.
#[test]
fn many_folders_of_one_name_stay_apart() {
    let children: Vec<FolderSpec> = (0..300).map(|_| folder("Same", "IPF.Note", vec![], vec![])).collect();
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![], children)]));
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    let inbox = folder_id(&s, &["old-mail", "Inbox"]).unwrap();
    let names: HashSet<String> = s.mailbox.folders.iter().filter(|f| f.parent == Some(inbox)).map(|f| f.name.to_lowercase()).collect();
    assert_eq!(names.len(), 300);
    assert!(names.contains("same (300)"));
}

/// A folder the import puts mail under that is moved to Deleted Items meanwhile stops it.
#[test]
fn a_place_moved_to_deleted_items_stops_the_import() {
    let bytes = w::build(&folder_list(vec![folder("Inbox", "IPF.Note", (0..6).map(|i| note(&format!("N {i}"))).collect(), vec![])]));
    let mut s = session();
    let account = s.mailbox.local_account();
    let under = s.mailbox.add_folder(account, None, "Archives").unwrap();
    let mut batches = Vec::new();
    read(Cursor::new(&bytes), &job(false), Limits::default(), &Progress::default(), &mut |b: Batch| {
        batches.push(b);
        Ok(())
    })
    .unwrap();
    let params = json!({"staged": true, "name": "old-mail", "places": {"mail": under}});
    s.staged_pst = Some(crate::import_pst::Staged::Batch(batches.remove(0)));
    s.run("file.importPst", &params).unwrap();
    let before = s.mailbox.messages.len();
    let deleted = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Deleted).unwrap();
    s.mailbox.folders.iter_mut().find(|f| f.id == under).unwrap().parent = Some(deleted);
    s.staged_pst = Some(crate::import_pst::Staged::Batch(batches.remove(0)));
    let e = s.run("file.importPst", &params).unwrap_err().to_string();
    assert!(e.contains("Deleted Items"), "{e}");
    assert_eq!(s.mailbox.messages.len(), before);
}

/// A dry run reads every item, so that one that can't be read is not said to come in.
#[test]
fn a_dry_run_reads_what_it_counts() {
    let bytes = w::build(&mailbox_file());
    let mut s = session();
    let dry = import(&mut s, &bytes, json!({"dryRun": true})).unwrap();
    let real = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((dry["added"].clone(), dry["leftOut"].clone()), (real["added"].clone(), real["leftOut"].clone()));
}

// ---- contacts and distribution lists ----

/// The named contact properties the writer is given, in the order of their ids (0x8000 on).
const ADDRESS_LIDS: [u32; 23] = [
    0x8005, 0x8082, 0x8083, 0x8084, 0x8092, 0x8093, 0x8094, 0x80A2, 0x80A3, 0x80A4, 0x802B, 0x8045, 0x8046, 0x8047, 0x8048, 0x8049, 0x80DE, 0x80DF,
    0x8053, 0x8054, 0x8055, 0x8062, 0x8064,
];

/// The id the writer gives a named contact property.
fn lid(l: u32) -> u16 {
    0x8000 + ADDRESS_LIDS.iter().position(|x| *x == l).unwrap() as u16
}

fn contacts_file(folders: Vec<FolderSpec>) -> FileSpec {
    let mut f = spec(unicode_layout(), folders);
    f.named = ADDRESS_LIDS.iter().map(|l| (crate::import_pst::contact::PSETID_ADDRESS, *l)).collect();
    f
}

/// The id of Keywords in a contacts file.
const CONTACT_KEYWORDS: u16 = 0x8000 + ADDRESS_LIDS.len() as u16;

fn multi_binary(items: &[Vec<u8>]) -> Value {
    let mut table = (items.len() as u32).to_le_bytes().to_vec();
    let mut data = Vec::new();
    let base = 4 + 4 * items.len();
    for i in items {
        table.extend_from_slice(&((base + data.len()) as u32).to_le_bytes());
        data.extend_from_slice(i);
    }
    table.extend(data);
    Value { ty: 0x1102, bytes: table }
}

fn contact_item(name: &str, extra: Vec<(u16, Value)>) -> MessageSpec {
    MessageSpec { class: "IPM.Contact".into(), subject: name.into(), body: String::new(), delivered: DELIVERED, extra, ..Default::default() }
}

/// A FILETIME for midnight of a day.
fn filetime_of(year: i32, month: u8, day: u8) -> u64 {
    let ts = DateTime::new(Date::new(year, month, day).unwrap(), 0, 0, 0).unwrap().to_timestamp(0);
    ((ts + 11_644_473_600) * 10_000_000) as u64
}

fn ada() -> MessageSpec {
    contact_item(
        "Ada Fernsby",
        vec![
            (0x3001, w::unicode("Ada Fernsby")),
            (0x3A06, w::unicode("Ada")),
            (0x3A44, w::unicode("Maud")),
            (0x3A11, w::unicode("Fernsby")),
            (0x3A4F, w::unicode("Addie")),
            (0x3A16, w::unicode("Fernsby Gardens Ltd")),
            (0x3A17, w::unicode("Head gardener")),
            (0x3A18, w::unicode("Roses")),
            (lid(0x8082), w::unicode("SMTP")),
            (lid(0x8083), w::unicode("ada@example.org")),
            (lid(0x8092), w::unicode("SMTP")),
            (lid(0x8093), w::unicode("ada.home@example.org")),
            (lid(0x80A2), w::unicode("SMTP")),
            (lid(0x80A3), w::unicode("ADA@example.org")),
            (0x3A08, w::unicode("+44 20 7946 0001")),
            (0x3A09, w::unicode("+44 20 7946 0002")),
            (0x3A1C, w::unicode("+44 7700 900003")),
            (0x3A24, w::unicode("+44 20 7946 0004")),
            (0x3A1B, multi_unicode(&["+44 20 7946 0005", "+44 20 7946 0006"])),
            (lid(0x8045), w::unicode("1 Garden Row\r\nGate B")),
            (lid(0x8046), w::unicode("Kew")),
            (lid(0x8047), w::unicode("Surrey")),
            (lid(0x8048), w::unicode("TW9 3AB")),
            (lid(0x8049), w::unicode("United Kingdom")),
            (0x3A5D, w::unicode("2 Hill Close")),
            (0x3A59, w::unicode("Richmond")),
            (0x3A5B, w::unicode("TW10 1AA")),
            (0x3A63, w::unicode("3 Mill Lane")),
            (0x3A5F, w::unicode("Ham")),
            (lid(0x802B), w::unicode("https://fernsby.example.org")),
            (0x3A50, w::unicode("https://ada.example.org")),
            (lid(0x80DE), w::time(filetime_of(1960, 3, 5))),
            (0x1000, w::unicode("Prefers morning calls.\r\nAllergic to lilies.")),
            (CONTACT_KEYWORDS, multi_unicode(&["Garden", "Friends"])),
            // What the model has no place for.
            (0x3A19, w::unicode("Greenhouse 4")),
            (lid(0x8062), w::unicode("ada@chat.example.org")),
        ],
    )
}

fn people_file(contacts: Vec<MessageSpec>) -> Vec<u8> {
    w::build(&contacts_file(vec![folder("Contacts", "IPF.Contact", contacts, vec![])]))
}

fn texts(answer: &Json, key: &str, field: &str) -> Vec<String> {
    answer[key].as_array().unwrap().iter().map(|n| n[field].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_contact_keeps_every_field_the_model_has() {
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![ada()]), json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(
        (answer["found"].clone(), answer["added"].clone(), answer["contacts"].clone(), answer["messages"].clone()),
        (json!(1), json!(1), json!(1), json!(0))
    );
    assert_eq!(row(&answer, "Contacts")["added"], 1);
    let c = s.mailbox.contacts.values().next().unwrap().as_ref().clone();
    assert_eq!((c.display_name.as_str(), c.first.as_str(), c.middle.as_str(), c.last.as_str()), ("Ada Fernsby", "Ada", "Maud", "Fernsby"));
    assert_eq!(
        (c.nickname.as_str(), c.company.as_str(), c.title.as_str(), c.department.as_str()),
        ("Addie", "Fernsby Gardens Ltd", "Head gardener", "Roses")
    );
    // The third address is the first again, in capitals: once.
    let emails: Vec<&str> = c.emails.iter().map(|e| e.value.as_str()).collect();
    assert_eq!(emails, ["ada@example.org", "ada.home@example.org"]);
    let phones: Vec<(&str, &str)> = c.phones.iter().map(|p| (p.label.as_str(), p.value.as_str())).collect();
    assert_eq!(
        phones,
        [
            ("work", "+44 20 7946 0001"),
            ("work", "+44 20 7946 0005"),
            ("work", "+44 20 7946 0006"),
            ("home", "+44 20 7946 0002"),
            ("mobile", "+44 7700 900003"),
            ("work fax", "+44 20 7946 0004"),
        ]
    );
    assert_eq!(c.addresses.len(), 3);
    let work = &c.addresses[0];
    assert_eq!(
        (work.label.as_str(), work.street.as_str(), work.city.as_str(), work.region.as_str(), work.postal_code.as_str(), work.country.as_str()),
        ("work", "1 Garden Row, Gate B", "Kew", "Surrey", "TW9 3AB", "United Kingdom")
    );
    assert_eq!((c.addresses[1].label.as_str(), c.addresses[1].street.as_str(), c.addresses[1].city.as_str()), ("home", "2 Hill Close", "Richmond"));
    assert_eq!((c.addresses[2].label.as_str(), c.addresses[2].street.as_str()), ("other", "3 Mill Lane"));
    assert_eq!(c.websites, ["https://fernsby.example.org", "https://ada.example.org"]);
    // (Before 1970 too.)
    assert_eq!(c.birthday, Date::new(1960, 3, 5));
    assert_eq!(c.notes, "Prefers morning calls.\nAllergic to lilies.");
    // The categories it had, and the mark that it was imported, and from what.
    assert_eq!(c.categories, ["Garden", "Friends"]);
    assert_eq!(c.imported_from, "old-mail.pst");
    assert!(c.origin.is_local() && c.uid.is_empty());
    assert!(!c.imported.is_empty());
    // What there is no place for is named; the contact is in all the same.
    let why = texts(&answer, "notes", "why").join("|");
    assert!(why.contains("office") && why.contains("instant messaging address"), "{why}");
    // The private key is in nothing that is shown or exported.
    let shown = s.run("people.get", &json!({"id": c.id})).unwrap().to_string();
    assert!(!shown.contains(&c.imported));
    assert!(!mailcraft_vcard::write(&[&c]).contains(&c.imported));
}

#[test]
fn imported_contacts_are_marked_found_and_keep_the_mark() {
    let mut s = session();
    let own = s.mailbox.add_contact(mailcraft_model::Contact { first: "Hal".into(), ..Default::default() });
    import(&mut s, &people_file(vec![ada(), contact_item("Eli", vec![(0x3001, w::unicode("Eli Marsh"))])]), json!({})).unwrap();
    let ids: Vec<u64> = s.mailbox.contacts.values().filter(|c| !c.imported_from.is_empty()).map(|c| c.id).collect();
    assert_eq!(ids.len(), 2);
    // Shown by the commands, found by a search, never set by params.
    let got = s.run("people.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(got["importedFrom"], "old-mail.pst");
    assert_eq!(s.run("people.get", &json!({"id": own})).unwrap()["importedFrom"], "");
    let found = s.run("people.search", &json!({"query": "is:imported"})).unwrap();
    assert_eq!(found["found"], 2);
    assert!(s.mailbox.contact(own).is_some_and(|c| !c.matches("is:imported")));
    s.run("people.edit", &json!({"id": own, "importedFrom": "forged.pst", "imported": "x:1"})).unwrap();
    s.run("people.new", &json!({"first": "Zed", "importedFrom": "forged.pst"})).unwrap();
    assert!(s.mailbox.contacts.values().filter(|c| c.imported_from == "forged.pst").count() == 0);
    // An edit and a merge keep it; a merge with an own contact too.
    s.run("people.edit", &json!({"id": ids[1], "title": "Checked"})).unwrap();
    assert_eq!(s.mailbox.contact(ids[1]).unwrap().imported_from, "old-mail.pst");
    s.run("people.merge", &json!({"keep": own, "others": [ids[0]]})).unwrap();
    assert_eq!(s.mailbox.contact(own).unwrap().imported_from, "old-mail.pst");
    // Not written to a vCard.
    assert!(!mailcraft_vcard::write(&[s.mailbox.contact(own).unwrap()]).contains("old-mail"));
    // No place for contacts to choose: they all go to the one list.
    let answer = import(&mut session(), &people_file(vec![ada()]), json!({"places": {"contacts": "elsewhere"}})).unwrap();
    assert_eq!(answer["contacts"], 1);
}

#[test]
fn an_exchange_address_with_no_internet_address_is_named_and_left_out() {
    let c = contact_item(
        "Bram Okonjo",
        vec![
            (0x3001, w::unicode("Bram Okonjo")),
            (lid(0x8082), w::unicode("EX")),
            (lid(0x8083), w::unicode("/O=EXAMPLE/OU=FIRST/CN=RECIPIENTS/CN=BRAM")),
            (lid(0x8092), w::unicode("EX")),
            (lid(0x8093), w::unicode("/O=EXAMPLE/CN=BRAM2")),
            (lid(0x8094), w::unicode("bram@example.org")),
        ],
    );
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![c]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.emails.iter().map(|e| e.value.as_str()).collect::<Vec<_>>(), ["bram@example.org"]);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("Exchange")), "{why:?}");
}

#[test]
fn a_distribution_list_becomes_a_contact_listing_its_members() {
    let members = vec![
        one_off("Cleo Varga", "SMTP", "cleo@example.org", true),
        one_off("Dee Halloran", "SMTP", "dee@example.org", false),
        one_off("Cleo again", "SMTP", "CLEO@example.org", true),
        one_off("Somebody", "EX", "/O=EXAMPLE/CN=SOME", true),
        vec![1, 2, 3],
    ];
    let list = MessageSpec {
        class: "IPM.DistList".into(),
        subject: "Garden committee".into(),
        extra: vec![
            (0x3001, w::unicode("Garden committee")),
            (lid(0x8053), w::unicode("Garden committee")),
            (lid(0x8054), multi_binary(&members)),
            (lid(0x8055), multi_binary(&members)),
        ],
        ..Default::default()
    };
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![list, ada()]), json!({})).unwrap();
    adds_up(&answer);
    assert_eq!((answer["added"].clone(), answer["contacts"].clone(), answer["lists"].clone()), (json!(2), json!(1), json!(1)));
    let c = s.mailbox.contacts.values().find(|c| c.display_name == "Garden committee").unwrap();
    assert_eq!(c.notes, "Members:\nCleo Varga <cleo@example.org>\nDee Halloran <dee@example.org>");
    assert!(c.emails.is_empty());
    let notes: Vec<(String, String)> = texts(&answer, "notes", "what").into_iter().zip(texts(&answer, "notes", "why")).collect();
    assert!(
        notes.iter().any(|(what, why)| what.contains("Garden committee") && why.contains("kept as a contact listing its members (2)")),
        "{notes:?}"
    );
    assert!(notes.iter().any(|(_, why)| why.contains("2 member(s) of the list name no internet address")), "{notes:?}");
}

#[test]
fn importing_contacts_again_adds_nothing() {
    let bytes = people_file(vec![ada(), contact_item("Eli Marsh", vec![(0x3001, w::unicode("Eli Marsh"))])]);
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 2);
    // The owner changed one; it is still known by where it came from.
    let id = *s.mailbox.contacts.keys().next().unwrap();
    s.mailbox.contact_mut(id).unwrap().first = "Changed".into();
    let again = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&again);
    assert_eq!((again["added"].clone(), again["already"].clone(), again["contacts"].clone()), (json!(0), json!(2), json!(0)));
    assert_eq!(s.mailbox.contacts.len(), 2);
    // Saved and opened again, the key is still there.
    let saved = serde_json::to_string(&*s.mailbox.contacts[&id]).unwrap();
    let back: mailcraft_model::Contact = serde_json::from_str(&saved).unwrap();
    assert_eq!(back.imported, s.mailbox.contacts[&id].imported);
}

#[test]
fn a_dry_run_counts_contacts_and_adds_none() {
    let bytes = people_file(vec![ada(), contact_item("Eli Marsh", vec![])]);
    let mut s = session();
    let dry = import(&mut s, &bytes, json!({"dryRun": true})).unwrap();
    assert!(s.mailbox.contacts.is_empty());
    assert_eq!((dry["added"].clone(), dry["contacts"].clone()), (json!(2), json!(2)));
    let real = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((real["added"].clone(), real["contacts"].clone()), (json!(2), json!(2)));
}

#[test]
fn subclasses_and_contacts_among_mail_come_in_and_the_rest_is_still_counted() {
    let mut sub = contact_item("Fay Quinn", vec![(0x3001, w::unicode("Fay Quinn"))]);
    sub.class = "IPM.Contact.Customer".into();
    let mut task = note("Water the ferns");
    task.class = "IPM.Task".into();
    let bytes = w::build(&contacts_file(vec![
        folder("Inbox", "IPF.Note", vec![note("Hello"), contact_item("Gus Lane", vec![(0x3001, w::unicode("Gus Lane"))])], vec![]),
        folder("Contacts", "IPF.Contact", vec![sub, task], vec![]),
    ]));
    let mut s = session();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&answer);
    assert_eq!((answer["found"].clone(), answer["added"].clone(), answer["contacts"].clone()), (json!(4), json!(4), json!(2)));
    assert_eq!(answer["leftOut"], 0);
    assert_eq!(s.mailbox.contacts.len(), 2);
    assert_eq!(answer["tasks"], 1);
}

#[test]
fn hostile_contacts_are_cut_and_capped() {
    let many: Vec<String> = (0..500).map(|i| format!("+44 20 7946 {i:04}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let categories: Vec<String> = (0..300).map(|i| format!("c{i}")).collect();
    let categories: Vec<&str> = categories.iter().map(String::as_str).collect();
    let members: Vec<Vec<u8>> = (0..1200).map(|i| one_off(&format!("Member {i}"), "SMTP", &format!("m{i}@example.org"), true)).collect();
    let list = MessageSpec {
        class: "IPM.DistList".into(),
        extra: vec![(0x3001, w::unicode("Everyone")), (lid(0x8054), multi_binary(&members)), (lid(0x8055), multi_binary(&members))],
        ..Default::default()
    };
    let big = contact_item(
        "Hugo",
        vec![
            (0x3001, w::unicode(&format!("Hugo\u{7}\n{}", "é".repeat(10_000)))),
            (0x3A1B, multi_unicode(&many)),
            (0x1000, w::unicode(&"n".repeat(2 << 20))),
            (CONTACT_KEYWORDS, multi_unicode(&categories)),
            (lid(0x802B), w::unicode(&"w".repeat(50_000))),
            (lid(0x80DE), w::time(u64::MAX)),
        ],
    );
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![big, list]), json!({})).unwrap();
    adds_up(&answer);
    let h = s.mailbox.contacts.values().find(|c| c.display_name.starts_with("Hugo")).unwrap();
    assert!(h.display_name.chars().count() <= 4096 && !h.display_name.contains(['\u{7}', '\n']));
    assert_eq!(h.phones.len(), 100);
    assert!(h.notes.len() <= 1 << 20);
    assert!(h.categories.len() <= 101);
    assert!(h.websites[0].chars().count() <= 4096);
    assert_eq!(h.birthday, None);
    let all = s.mailbox.contacts.values().find(|c| c.display_name == "Everyone").unwrap();
    assert_eq!(all.notes.lines().count(), 1 + crate::import_pst::contact::MAX_MEMBERS);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("past the limit")), "{why:?}");
}

#[test]
fn undoing_an_import_keeps_the_contacts_the_owner_made_meanwhile() {
    let names = ["Ida", "Jon", "Kim", "Lou", "Mae", "Ned", "Ora"];
    let bytes = people_file(names.iter().map(|n| contact_item(n, vec![(0x3001, w::unicode(n))])).collect());
    let mut s = session();
    let before = s.mailbox.add_contact(mailcraft_model::Contact { first: "Hal".into(), last: "Ortiz".into(), ..Default::default() });
    let (batches, outcome) = staged(&bytes);
    assert!(batches.len() >= 2, "{}", batches.len());
    let params = json!({"staged": true, "name": "old-mail"});
    let mut later = None;
    for (i, b) in batches.into_iter().enumerate() {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
        if i == 0 {
            later = Some(s.mailbox.add_contact(mailcraft_model::Contact { first: "Pia".into(), ..Default::default() }));
        }
    }
    hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 9);
    assert_eq!(s.undo_label(), Some("Import a .pst File"));
    s.run("edit.undo", &json!({})).unwrap();
    let left: Vec<mailcraft_model::Id> = s.mailbox.contacts.keys().copied().collect();
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(s.mailbox.contact(before).is_some() && s.mailbox.contact(later.unwrap()).is_some());
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 9);

    // One command: one step, and undo takes the contacts out.
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 7);
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.mailbox.contacts.is_empty());
}

#[test]
fn cancelling_keeps_the_contacts_in_and_another_run_brings_the_rest() {
    let names = ["Ida", "Jon", "Kim", "Lou", "Mae", "Ned", "Ora"];
    let bytes = people_file(names.iter().map(|n| contact_item(n, vec![(0x3001, w::unicode(n))])).collect());
    let mut s = session();
    let progress = Progress::default();
    let place = Place::New("old-mail".into());
    let mut seen = 0;
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &progress, &mut |b: Batch| {
        seen += b.contacts.len();
        apply(&mut s, &place, b)?;
        progress.cancel();
        Ok(())
    })
    .unwrap();
    assert!(outcome.report.cancelled);
    finish(&mut s, &place, outcome, false).unwrap();
    assert_eq!(s.mailbox.contacts.len(), seen);
    assert!(seen > 0 && seen < 7, "{seen}");
    let again = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&again);
    assert_eq!(again["already"], seen);
    assert_eq!(s.mailbox.contacts.len(), 7);
}

#[test]
fn junk_in_the_contact_properties_never_panics() {
    // Numbers where strings go, binary where times go, a string where a list goes.
    let junk = contact_item(
        "Junk",
        vec![
            (0x3001, w::int32(5)),
            (0x3A06, w::binary(&[0xFF, 0xFE, 0x00])),
            (0x3A42, w::unicode("not a time")),
            (lid(0x80DE), w::int32(-1)),
            (lid(0x8083), w::binary(b"\x00\x01")),
            (lid(0x8054), w::unicode("not a list")),
            (0x1000, w::boolean(true)),
            (0x3A1B, w::int32(1)),
        ],
    );
    let mut list = junk.clone();
    list.class = "IPM.DistList".into();
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![junk, list]), json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["added"], 2);
}

// ---- review fixes ----

fn list_item(name: &str, members: &[Vec<u8>]) -> MessageSpec {
    MessageSpec {
        class: "IPM.DistList".into(),
        subject: name.into(),
        extra: vec![(0x3001, w::unicode(name)), (lid(0x8054), multi_binary(members)), (lid(0x8055), multi_binary(members))],
        ..Default::default()
    }
}

/// Undoing a later step must not bring back contacts an import put in and a later step's
/// snapshot still holds.
#[test]
fn undoing_a_later_step_does_not_bring_back_the_imported_contacts() {
    let names = ["Ida", "Jon", "Kim", "Lou", "Mae", "Ned", "Ora"];
    let bytes = people_file(names.iter().map(|n| contact_item(n, vec![(0x3001, w::unicode(n))])).collect());
    let mut s = session();
    let (batches, outcome) = staged(&bytes);
    assert!(batches.len() >= 2);
    let params = json!({"staged": true, "name": "old-mail"});
    for (i, b) in batches.into_iter().enumerate() {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
        if i == 0 {
            // The owner's own step, while the import runs: its snapshot holds the first batch.
            s.run("people.new", &json!({"first": "Pia"})).unwrap();
        }
    }
    hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 8);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 1);
    s.run("edit.undo", &json!({})).unwrap();
    let imported = s.mailbox.contacts.values().filter(|c| !c.imported.is_empty()).count();
    assert_eq!(imported, 0, "the import is undone, and stays so");
    assert!(s.mailbox.contacts.is_empty());
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 1);
}

#[test]
fn a_lists_notes_fit_a_vcard_line_and_the_cut_is_named() {
    let commas = ",".repeat(800);
    let members: Vec<Vec<u8>> = (0..1000).map(|i| one_off(&format!("{commas}{i}"), "SMTP", &format!("m{i}@example.org"), true)).collect();
    let mut list = list_item("Big", &members);
    list.extra.push((0x1000, w::unicode(&"\nx".repeat(450_000))));
    let plain = contact_item("Plain", vec![(0x3001, w::unicode("Plain")), (0x1000, w::unicode(&"\nx".repeat(1 << 19)))]);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![list, plain]), json!({})).unwrap();
    adds_up(&answer);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("notes") && w.contains("cut")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("past the limit")), "{why:?}");
    for c in s.mailbox.contacts.values() {
        assert!(crate::cmd::people_merge::notes_line(&c.notes) <= crate::cmd::people_merge::MAX_NOTES, "{}", c.display_name);
        // Exported and read back, nothing is lost.
        let back = mailcraft_vcard::parse_bytes(mailcraft_vcard::write(&[c.as_ref()]).as_bytes());
        assert_eq!(back[0].notes, c.notes, "{}", c.display_name);
    }
}

#[test]
fn deleted_contacts_are_left_out_in_one_line_and_every_other_folders_come_in() {
    let c = |n: &str| contact_item(n, vec![(0x3001, w::unicode(n))]);
    let classless = FolderSpec { name: "Loose".into(), class: None, messages: vec![c("Loose one")], children: vec![] };
    let bytes = w::build(&contacts_file(vec![
        folder("Contacts", "IPF.Contact", vec![c("Kept")], vec![]),
        folder(
            "Deleted Items",
            "IPF.Note",
            vec![c("Binned"), c("Binned 2"), c("Binned 3")],
            vec![folder("Old", "IPF.Contact", vec![c("Binned too")], vec![])],
        ),
        folder("Recipient Cache", "hidden:IPF.Contact", vec![c("Cached")], vec![]),
        folder("Suggested", "IPF.Contact.RecipientCache", vec![c("Suggested one")], vec![]),
        folder("Inbox", "IPF.Note", vec![c("Among mail")], vec![]),
        classless,
    ]));
    let mut s = session();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    adds_up(&answer);
    let mut names: Vec<String> = s.mailbox.contacts.values().map(|c| c.display_name.clone()).collect();
    names.sort();
    assert_eq!(names, ["Among mail", "Cached", "Kept", "Loose one", "Suggested one"]);
    assert!(s.mailbox.contacts.values().all(|c| c.imported_from == "old-mail.pst"));
    assert_eq!((answer["found"].clone(), answer["added"].clone(), answer["leftOut"].clone()), (json!(9), json!(5), json!(4)));
    // One line for each folder in Deleted Items, not one for each contact.
    let left = texts(&answer, "leftOutItems", "what");
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.iter().any(|w| w.ends_with("tasks (3)")) && left.iter().any(|w| w.ends_with("tasks (1)")), "{left:?}");
    assert!(texts(&answer, "leftOutItems", "why").iter().all(|w| w == "in Deleted Items, left out"));
    // The report names the automatic and the hidden folders the contacts came from.
    let folders = texts(&answer, "notes", "folder");
    assert!(folders.iter().any(|f| f == "Recipient Cache") && folders.iter().any(|f| f == "Suggested"), "{folders:?}");
    // Mail in Deleted Items is still mail.
    let mut mail = folder("Deleted Items", "IPF.Note", vec![note("Old news")], vec![]);
    mail.messages.push(c("Binned"));
    let bytes = w::build(&contacts_file(vec![mail]));
    let mut s = session();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((answer["messages"].clone(), answer["contacts"].clone()), (json!(1), json!(0)));
}

#[test]
fn a_file_of_contacts_alone_makes_no_mail_folder() {
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![ada()]), json!({})).unwrap();
    assert!(folder_id(&s, &["old-mail"]).is_none());
    assert!(answer["folder"].is_null() && answer["name"] == "");
    assert_eq!(s.view.module, crate::Module::People);
    assert!(s.mailbox.folders.iter().all(|f| f.name != "old-mail"));
    // The same through batches.
    let mut s = session();
    let (batches, outcome) = staged(&people_file(vec![ada()]));
    let params = json!({"staged": true, "name": "old-mail"});
    for b in batches {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
    }
    hand_over(&mut s, crate::import_pst::Staged::Done(Box::new(outcome)), &params).unwrap();
    assert!(folder_id(&s, &["old-mail"]).is_none());
    assert_eq!(s.mailbox.contacts.len(), 1);
    // With mail, the folder is made.
    let bytes =
        w::build(&contacts_file(vec![folder("Inbox", "IPF.Note", vec![note("Hi")], vec![]), folder("Contacts", "IPF.Contact", vec![ada()], vec![])]));
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    assert!(folder_id(&s, &["old-mail", "Inbox"]).is_some());
}

#[test]
fn a_merged_away_contact_is_not_brought_back_by_importing_again() {
    let bytes =
        people_file(vec![contact_item("Eli", vec![(0x3001, w::unicode("Eli Marsh"))]), contact_item("Eli", vec![(0x3001, w::unicode("Eli Marsh"))])]);
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    let ids: Vec<u64> = s.mailbox.contacts.keys().copied().collect();
    s.run("people.merge", &json!({"keep": ids[0], "others": [ids[1]]})).unwrap();
    assert_eq!(s.mailbox.contacts.len(), 1);
    let again = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((again["added"].clone(), again["already"].clone()), (json!(0), json!(2)));
    assert_eq!(s.mailbox.contacts.len(), 1);
}

#[test]
fn contacts_get_names_and_addresses_must_have_two_sides() {
    let nameless = contact_item("", vec![(0x3A08, w::unicode("+44 20 7946 0100"))]);
    let empty = contact_item("", vec![]);
    let odd = contact_item(
        "Odd",
        vec![
            (0x3001, w::unicode("Odd")),
            (lid(0x8082), w::unicode("SMTP")),
            (lid(0x8083), w::unicode("x@")),
            (CONTACT_KEYWORDS, multi_unicode(&["Garden", "GARDEN", "garden"])),
        ],
    );
    let list =
        list_item("Odd list", &[one_off("X", "SMTP", "x@", true), one_off("Y", "SMTP", "@y", true), one_off("Z", "SMTP", "z@example.org", true)]);
    let mut s = session();
    import(&mut s, &people_file(vec![nameless, empty, odd, list]), json!({"places": {"contacts": "here"}})).unwrap();
    assert!(s.mailbox.contacts.values().all(|c| !c.name().is_empty()));
    let by = |n: &str| s.mailbox.contacts.values().find(|c| c.name() == n).unwrap();
    assert_eq!(by("+44 20 7946 0100").display_name, "+44 20 7946 0100");
    assert!(s.mailbox.contacts.values().any(|c| c.display_name == "(no name)"));
    let odd = by("Odd");
    assert!(odd.emails.is_empty());
    assert_eq!(odd.categories, ["Garden"]);
    assert_eq!(by("Odd list").notes, "Members:\nZ <z@example.org>");
    // The nameless ones can be edited.
    for id in s.mailbox.contacts.keys().copied().collect::<Vec<_>>() {
        s.run("people.edit", &json!({"id": id, "title": "Checked"})).unwrap();
    }
}

#[test]
fn a_dry_run_reads_each_contact_so_its_notes_show() {
    let mut s = session();
    let dry = import(&mut s, &people_file(vec![ada()]), json!({"dryRun": true})).unwrap();
    assert!(s.mailbox.contacts.is_empty());
    assert!(texts(&dry, "notes", "why").iter().any(|w| w.contains("office")), "{dry}");
}

#[test]
fn a_contacts_only_file_makes_no_mail_folder_however_often_or_empty_it_is() {
    // Outlook's own shape: Contacts, an empty Inbox and an empty Deleted Items.
    let bytes = w::build(&contacts_file(vec![
        folder("Contacts", "IPF.Contact", vec![ada()], vec![]),
        folder("Inbox", "IPF.Note", vec![], vec![]),
        folder("Deleted Items", "IPF.Note", vec![], vec![]),
    ]));
    let mut s = session();
    let answer = import(&mut s, &bytes, json!({})).unwrap();
    assert!(folder_id(&s, &["old-mail"]).is_none());
    assert!(s.mailbox.folders.iter().all(|f| f.name != "old-mail"));
    assert!(answer["folder"].is_null() && answer["name"] == "");
    // Again: the contacts are already here, and still no folder, still People.
    s.view.module = crate::Module::Mail;
    let again = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!((again["added"].clone(), again["already"].clone()), (json!(0), json!(1)));
    assert!(folder_id(&s, &["old-mail"]).is_none());
    assert!(again["folder"].is_null());
    assert_eq!(s.view.module, crate::Module::People);
    // With one message, the empty folders come too.
    let bytes = w::build(&contacts_file(vec![
        folder("Contacts", "IPF.Contact", vec![ada()], vec![]),
        folder("Inbox", "IPF.Note", vec![note("Hi")], vec![]),
        folder("Empty", "IPF.Note", vec![], vec![]),
    ]));
    let mut s = session();
    import(&mut s, &bytes, json!({})).unwrap();
    assert!(folder_id(&s, &["old-mail", "Empty"]).is_some());
}

#[test]
fn a_list_with_no_members_keeps_all_the_room_for_its_notes() {
    let body = "n".repeat(700_000);
    let empty = MessageSpec {
        class: "IPM.DistList".into(),
        extra: vec![(0x3001, w::unicode("Empty list")), (0x1000, w::unicode(&body))],
        ..Default::default()
    };
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![empty]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes.len(), 700_000);
    assert!(!texts(&answer, "notes", "why").iter().any(|w| w.contains("are cut")));
}

#[test]
fn undo_does_not_warn_about_contacts_it_would_leave_alone() {
    let names = ["Ida", "Jon", "Kim", "Lou", "Mae", "Ned", "Ora"];
    let bytes = people_file(names.iter().map(|n| contact_item(n, vec![(0x3001, w::unicode(n))])).collect());
    let mut s = session();
    let (batches, _outcome) = staged(&bytes);
    let params = json!({"staged": true, "name": "old-mail"});
    let mut made = None;
    for (i, b) in batches.into_iter().enumerate() {
        hand_over(&mut s, crate::import_pst::Staged::Batch(b), &params).unwrap();
        if i == 0 {
            s.run("people.new", &json!({"first": "Pia"})).unwrap();
            made = s.mailbox.contacts.values().find(|c| c.first == "Pia").map(|c| c.id);
        }
    }
    // The last step is the owner's New Contact: it takes Pia away and nothing else.
    let later = s.mailbox.contacts.values().filter(|c| !c.imported.is_empty()).map(|c| c.id).max().unwrap();
    assert!(s.undo_removes(false, made.unwrap()));
    assert!(!s.undo_removes(false, later));
}

#[test]
fn the_file_a_contact_came_from_is_not_free_text_but_is_imported_finds_it() {
    let mut s = session();
    let own = s.mailbox.add_contact(mailcraft_model::Contact {
        first: "Martin".into(),
        emails: vec![mailcraft_model::Labeled::new("work", "martin@example.org")],
        ..Default::default()
    });
    let bytes = people_file(vec![contact_item(
        "Sam Smith",
        vec![(0x3001, w::unicode("Sam Smith")), (lid(0x8082), w::unicode("SMTP")), (lid(0x8083), w::unicode("sam@example.org"))],
    )]);
    let mut params = json!({"name": "martin@example.org.pst", "data": base64_encode(&bytes)});
    params["places"] = json!({});
    s.run("file.importPst", &params).unwrap();
    let imported = s.mailbox.contacts.values().find(|c| !c.imported_from.is_empty()).unwrap().id;
    assert_eq!(s.mailbox.contact(imported).unwrap().imported_from, "martin@example.org.pst");
    // Searching for the owner's name finds the owner's contact alone.
    let found: Vec<u64> = s.mailbox.contacts.values().filter(|c| c.matches("martin")).map(|c| c.id).collect();
    assert_eq!(found, [own]);
    // Check Names still resolves "Martin" to the owner's contact.
    s.run("mail.new", &json!({"to": "Martin", "subject": "Hello"})).unwrap();
    assert_eq!(s.run("compose.checkNames", &json!({})).unwrap()["unresolved"], json!([]));
    assert_eq!(s.compose.as_ref().unwrap().to, "Martin <martin@example.org>");
    // `is:imported` finds the imported ones, alone or with other words.
    let hits = |q: &str| s.mailbox.contacts.values().filter(|c| c.matches(q)).count();
    assert_eq!((hits("is:imported"), hits("IS:IMPORTED smith"), hits("is:imported nobody"), hits("is:importedx")), (1, 1, 0, 0));
    // The term many times over is read in a loop: no stack is used up.
    let long = "is:imported ".repeat(200_000);
    assert_eq!(hits(&long), 1);
}

#[test]
fn a_dry_run_of_mail_and_contacts_answers_as_the_real_one() {
    let bytes =
        w::build(&contacts_file(vec![folder("Inbox", "IPF.Note", vec![note("Hi")], vec![]), folder("Contacts", "IPF.Contact", vec![ada()], vec![])]));
    let mut s = session();
    let dry = import(&mut s, &bytes, json!({"dryRun": true})).unwrap();
    let real = import(&mut s, &bytes, json!({})).unwrap();
    assert_eq!(dry["name"], real["name"]);
    assert_eq!(real["name"], "old-mail");
}

#[test]
fn cancelling_while_in_the_contacts_folder_still_makes_the_mail_folders() {
    let names = ["Ida", "Jon", "Kim", "Lou", "Mae", "Ned", "Ora"];
    let bytes = w::build(&contacts_file(vec![
        folder("Contacts", "IPF.Contact", names.iter().map(|n| contact_item(n, vec![(0x3001, w::unicode(n))])).collect(), vec![]),
        folder("Inbox", "IPF.Note", vec![note("Hi")], vec![]),
    ]));
    let mut s = session();
    let progress = Progress::default();
    let place = Place::New("old-mail".into());
    let outcome = read(Cursor::new(&bytes), &job(false), Limits::default(), &progress, &mut |b: Batch| {
        apply(&mut s, &place, b)?;
        progress.cancel();
        Ok(())
    })
    .unwrap();
    assert!(outcome.report.cancelled);
    let answer = finish(&mut s, &place, outcome, false).unwrap();
    assert_eq!(answer["name"], "old-mail");
    assert!(folder_id(&s, &["old-mail"]).is_some());
}

/// [MS-OXRTFCP] 3.1.1: compressed RTF for `{\rtf1\ansi\ansicpg1252\pard hello world}`.
const COMPRESSED_HELLO: [u8; 49] = [
    0x2d, 0x00, 0x00, 0x00, 0x2b, 0x00, 0x00, 0x00, 0x4c, 0x5a, 0x46, 0x75, 0xf1, 0xc5, 0xc7, 0xa7, 0x03, 0x00, 0x0a, 0x00, 0x72, 0x63, 0x70, 0x67,
    0x31, 0x32, 0x35, 0x42, 0x32, 0x0a, 0xf3, 0x20, 0x68, 0x65, 0x6c, 0x09, 0x00, 0x20, 0x62, 0x77, 0x05, 0xb0, 0x6c, 0x64, 0x7d, 0x0a, 0x80, 0x0f,
    0xa0,
];

/// Rich text kept uncompressed (`MELA`).
fn uncompressed_rtf(rtf: &str) -> Vec<u8> {
    let mut v = ((rtf.len() + 12) as u32).to_le_bytes().to_vec();
    v.extend((rtf.len() as u32).to_le_bytes());
    v.extend(b"MELA");
    v.extend([0u8; 4]);
    v.extend(rtf.bytes());
    v
}

#[test]
fn html_kept_in_rich_text_comes_in_as_html() {
    let mut m = note("Encapsulated");
    m.body = String::new();
    m.extra = vec![(0x1009, w::binary(&uncompressed_rtf("{\\rtf1\\fromhtml1 {\\*\\htmltag <p>}Hello{\\*\\htmltag </p>}}")))];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m], vec![])])), json!({})).unwrap();
    adds_up(&answer);
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let got = inbox.iter().find(|m| m.subject == "Encapsulated").unwrap();
    assert_eq!((got.body.html.as_deref(), got.body.text.as_str()), (Some("<p>Hello</p>"), "Hello"));
    assert!(got.attachments.is_empty(), "nothing of the rich text is left to keep");
}

#[test]
fn what_html_in_rich_text_hides_is_kept_apart_from_its_text() {
    let mut m = note("Hiding");
    m.body = String::new();
    let rtf = "{\\rtf1\\fromhtml1 {\\*\\htmltag <div style=\"display:none\">}Preheader{\\*\\htmltag </div>}\\htmlrtf Only in rich text\\htmlrtf0 {\\*\\htmltag <p>}Hello{\\*\\htmltag </p>}}";
    m.extra = vec![(0x1009, w::binary(&uncompressed_rtf(rtf)))];
    let mut s = session();
    import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m], vec![])])), json!({})).unwrap();
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let got = inbox.iter().find(|m| m.subject == "Hiding").unwrap();
    assert_eq!((got.body.text.trim(), got.hidden_text.trim()), ("Hello", "Preheader"));
}

#[test]
fn compressed_rich_text_alone_is_read_and_broken_rich_text_is_kept_as_it_was() {
    let rich = |subject: &str, stored: &[u8]| {
        let mut m = note(subject);
        m.body = String::new();
        m.extra = vec![(0x1009, w::binary(stored))];
        m
    };
    let items = vec![
        rich("Packed", &COMPRESSED_HELLO),
        rich("Broken", &COMPRESSED_HELLO[..30]),
        rich("Not rich", &uncompressed_rtf("no rich text here")),
        rich("Lines", &uncompressed_rtf("{\\rtf1\\ansi First\\par Caf\\'e9 \\u8364?}")),
    ];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", items, vec![])])), json!({})).unwrap();
    adds_up(&answer);
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let by = |subject: &str| *inbox.iter().find(|m| m.subject == subject).unwrap();
    let packed = by("Packed");
    assert_eq!(packed.body.text, "hello world");
    let file = packed.attachments.iter().find(|a| a.name == "message body.rtf").unwrap();
    assert_eq!(file.data.as_slice(), b"{\\rtf1\\ansi\\ansicpg1252\\pard hello world}\r\n".as_slice());
    assert_eq!(by("Lines").body.text, "First\nCaf\u{e9} \u{20ac}");
    // What can't be read stays whole, and is named.
    let broken = by("Broken");
    assert_eq!(broken.body.text, "");
    let kept = broken.attachments.iter().find(|a| a.name == "message body (compressed rich text).bin").unwrap();
    assert_eq!(kept.data.as_slice(), &COMPRESSED_HELLO[..30]);
    let not_rich = by("Not rich");
    assert_eq!(not_rich.body.text, "");
    assert_eq!(not_rich.attachments.iter().find(|a| a.name == "message body.rtf").unwrap().data.as_slice(), b"no rich text here".as_slice());
    let why = texts(&answer, "notes", "why");
    assert_eq!(why.iter().filter(|w| w.contains("could not be read")).count(), 2, "{why:?}");
}

#[test]
fn large_rich_text_damaged_in_the_file_is_named() {
    // Too large to be read with the item: a damaged block shows only when it is read for itself.
    let stored = uncompressed_rtf(&format!("{{\\rtf1 {}}}", "q".repeat(5 << 20)));
    let mut m = note("Torn");
    m.body = String::new();
    m.extra = vec![(0x1009, w::binary(&stored))];
    let mut bytes = w::build(&FileSpec {
        layout: Layout::new(Format::Unicode, Encryption::None),
        password: None,
        password_ansi: false,
        name: "Personal Folders".into(),
        folders: vec![folder("Inbox", "IPF.Note", vec![m], vec![])],
        named: vec![],
        named_strings: vec![(PS_PUBLIC_STRINGS, "Keywords".into())],
    });
    let at = bytes.windows(64).position(|w| w.iter().all(|b| *b == b'q')).unwrap();
    bytes[at + (2 << 20)] = b'z';
    let dir = crate::import_pst_memory_tests::Temp::new("rich-text-torn");
    let mut s = crate::import_pst_memory_tests::kept_session(&dir);
    let pst = dir.pst(&bytes);
    let answer = crate::import_pst_memory_tests::import(&mut s, &pst).unwrap();
    adds_up(&answer);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("rich text is damaged")), "{answer}");
}

#[test]
fn empty_rich_text_brings_no_attachment_and_no_note() {
    let mut m = note("Stub");
    m.body = "\r\n".into();
    m.extra = vec![(0x1009, w::binary(&uncompressed_rtf("{\\rtf1\\ansi\\par}")))];
    let mut pic = note("Picture");
    pic.body = String::new();
    pic.extra = vec![(0x1009, w::binary(&uncompressed_rtf("{\\rtf1{\\pict 00}}")))];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m, pic], vec![])])), json!({})).unwrap();
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let by = |subject: &str| *inbox.iter().find(|m| m.subject == subject).unwrap();
    assert!(by("Stub").attachments.is_empty(), "{:?}", by("Stub").attachments);
    // A picture and no words is still something: kept, and said.
    assert_eq!(by("Picture").attachments[0].name, "message body.rtf");
    let why = texts(&answer, "notes", "why");
    assert_eq!(why.len(), 1, "{why:?}");
    assert!(why[0].contains("no text of its own"), "{why:?}");
}

#[test]
fn a_small_file_of_rich_text_can_not_bring_more_than_the_cap() {
    // Megabytes that open out past what rich text may be: kept as they are, and said.
    let head = b"{\\rtf1 x\\par y ";
    let bomb = repeating_rtf(head, 6, 4_200_000);
    assert!(bomb.len() < 10 << 20);
    // Past what a body may be, but within what rich text may: the text is cut, and said.
    let long = repeating_rtf(head, 6, 2_400_000);
    let rich = |subject: &str, stored: &[u8]| {
        let mut m = note(subject);
        m.body = String::new();
        m.extra = vec![(0x1009, w::binary(stored))];
        m
    };
    let dir = crate::import_pst_memory_tests::Temp::new("rich-text-cap");
    let mut s = crate::import_pst_memory_tests::kept_session(&dir);
    // ("Long" spends most of what one import may take; the one after it is kept as it is.)
    let items = vec![rich("Bomb", &bomb), rich("Long", &long), rich("Long again", &long)];
    let pst = dir.pst(&w::build(&folder_list(vec![folder("Inbox", "IPF.Note", items, vec![])])));
    let answer = crate::import_pst_memory_tests::import(&mut s, &pst).unwrap();
    adds_up(&answer);
    let inbox = messages_in(&s, &["old-mail", "Inbox"]);
    let by = |subject: &str| *inbox.iter().find(|m| m.subject == subject).unwrap();
    assert_eq!(by("Bomb").body.text, "");
    assert_eq!(by("Bomb").attachments[0].name, "message body (compressed rich text).bin");
    assert!(by("Long").body.text.len() <= 32 << 20 && by("Long").body.text.len() > 30 << 20);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("could not be read")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("text is cut")), "{why:?}");
    assert!(by("Long").attachments.iter().any(|a| a.name == "message body.rtf"));
    // Together they stay within what one import may take from rich text.
    let total: usize = inbox.iter().map(|m| m.body.text.len()).sum();
    assert!(total as u64 <= crate::import_pst::item::RTF_TEXT_BUDGET + 4096, "{total}");
}

#[test]
fn notes_kept_only_as_rich_text_come_across() {
    // Outlook writes a contact's notes as PidTagRtfCompressed alone: no PidTagBody.
    let rtf = |name: &str, bytes: Vec<u8>| contact_item(name, vec![(0x3001, w::unicode(name)), (0x1009, w::binary(&bytes))]);
    let contacts = vec![
        rtf("Compressed Cora", COMPRESSED_HELLO.to_vec()),
        rtf("Plain Pia", uncompressed_rtf("{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}\\pard Call after 5\\par Caf\\'e9 \\u8364?\\par}")),
        rtf("Broken Bea", COMPRESSED_HELLO[..30].to_vec()),
        // Plain text wins when there is some.
        contact_item("Both Bo", vec![(0x3001, w::unicode("Both Bo")), (0x1000, w::unicode("The text")), (0x1009, w::binary(&COMPRESSED_HELLO))]),
    ];
    let mut s = session();
    let answer = import(&mut s, &people_file(contacts), json!({})).unwrap();
    adds_up(&answer);
    let notes = |name: &str| s.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().notes.clone();
    assert_eq!(notes("Compressed Cora"), "hello world");
    assert_eq!(notes("Plain Pia"), "Call after 5\nCaf\u{e9} \u{20ac}");
    assert_eq!(notes("Broken Bea"), "");
    assert_eq!(notes("Both Bo"), "The text");
    // The one that couldn't be read is named, with why.
    let why = texts(&answer, "notes", "why");
    assert_eq!(why.iter().filter(|w| w.contains("notes (rich text) could not be read")).count(), 1, "{why:?}");
    let what = texts(&answer, "notes", "what");
    assert!(what.iter().any(|w| w.contains("Broken Bea")), "{what:?}");
}

/// Compressed rich text ([MS-OXRTFCP]) of `head` and then `repeats` copies of its last `period`
/// bytes (17 at a time), made of dictionary references alone: a small input that unpacks to a big one.
fn repeating_rtf(head: &[u8], period: usize, repeats: usize) -> Vec<u8> {
    let mut body = Vec::new();
    let mut write = 207usize;
    // Some(byte) is a literal, None a reference; the end mark follows the last.
    let mut tokens: Vec<Option<u8>> = head.iter().map(|b| Some(*b)).collect();
    tokens.extend(std::iter::repeat_n(None, repeats));
    let total = tokens.len() + 1;
    for chunk in (0..total).collect::<Vec<_>>().chunks(8) {
        let at = body.len();
        body.push(0);
        for (bit, i) in chunk.iter().enumerate() {
            match tokens.get(*i) {
                Some(Some(b)) => {
                    body.push(*b);
                    write = (write + 1) % 4096;
                }
                Some(None) => {
                    body[at] |= 1 << bit;
                    let offset = (write + 4096 - period) % 4096;
                    body.extend((((offset << 4) | (15)) as u16).to_be_bytes());
                    write = (write + 17) % 4096;
                }
                // The end mark: a reference to the write position.
                None => {
                    body[at] |= 1 << bit;
                    body.extend(((write << 4) as u16).to_be_bytes());
                }
            }
        }
    }
    let mut v = ((body.len() + 12) as u32).to_le_bytes().to_vec();
    v.extend(0u32.to_le_bytes());
    v.extend(b"LZFu");
    v.extend(mailcraft_pst::crc32(&body).to_le_bytes());
    v.extend(body);
    v
}

#[test]
fn a_whitespace_only_text_does_not_hide_the_rich_text_notes() {
    let both = contact_item(
        "Blank Bo",
        vec![(0x3001, w::unicode("Blank Bo")), (0x1000, w::unicode("\r\n")), (0x1009, w::binary(&uncompressed_rtf("{\\rtf1 The real notes}")))],
    );
    let mut s = session();
    import(&mut s, &people_file(vec![both]), json!({})).unwrap();
    assert_eq!(s.mailbox.contacts.values().next().unwrap().notes, "The real notes");
}

#[test]
fn rich_text_notes_cut_at_the_cap_are_named_even_if_blank_lines_fold_them_short() {
    // "x" and five paragraph ends: 6 bytes of text that fold to 3 and cost 5.
    let head = b"{\\rtf1 x\\par\\par\\par\\par\\par ";
    let rtf = repeating_rtf(head, 22, 230_000);
    assert!(rtf.len() < 1 << 20);
    let big = contact_item("Big Bo", vec![(0x3001, w::unicode("Big Bo")), (0x1009, w::binary(&rtf))]);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![big]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert!(c.notes.starts_with("x\n\nx\n\n") && c.notes.len() > 1000, "{} {:?}", c.notes.len(), texts(&answer, "notes", "why"));
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("notes") && w.contains("cut")), "{why:?}");
}

// ---- a list's stream (#843) ----

fn streamed_list(name: &str, stream: Value, extra: Vec<(u16, Value)>) -> MessageSpec {
    let mut e = vec![(0x3001, w::unicode(name)), (lid(0x8064), stream)];
    e.extend(extra);
    MessageSpec { class: "IPM.DistList".into(), subject: name.into(), extra: e, ..Default::default() }
}

fn smtp(i: usize) -> Vec<u8> {
    one_off(&format!("Member {i}"), "SMTP", &format!("m{i}@example.org"), true)
}

#[test]
fn a_big_list_kept_in_a_stream_comes_in_with_every_member() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..400).map(|i| (smtp(i), Vec::new())).collect();
    let stream = w::list_stream(&members);
    // Past 15,000 bytes, as Outlook writes them, and so past what the heap holds (3,580 bytes):
    // the writer puts it in a subnode, and it is read on its own.
    assert!(stream.bytes.len() > 15_000);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![streamed_list("Neighbours", stream, vec![])]), json!({})).unwrap();
    adds_up(&answer);
    let c = s.mailbox.contacts.values().next().unwrap();
    let lines: Vec<&str> = c.notes.lines().collect();
    assert_eq!(lines.len(), 401, "{}", c.notes);
    assert_eq!((lines[0], lines[1], lines[400]), ("Members:", "Member 0 <m0@example.org>", "Member 399 <m399@example.org>"));
    let notes: Vec<(String, String)> = texts(&answer, "notes", "what").into_iter().zip(texts(&answer, "notes", "why")).collect();
    assert!(notes.iter().any(|(what, why)| what.contains("Neighbours") && why.contains("listing its members (400)")), "{notes:?}");
    // Nothing is said to be kept in a form this version doesn't read, nor left out.
    assert!(!notes.iter().any(|(_, why)| why.contains("stream") || why.contains("doesn't read") || why.contains("left out")), "{notes:?}");
    assert!(!c.notes.contains("doesn't read"));
}

#[test]
fn a_stream_with_more_members_than_the_cap_keeps_the_cap_and_names_the_rest() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..1200).map(|i| (smtp(i), Vec::new())).collect();
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![streamed_list("Everyone", w::list_stream(&members), vec![])]), json!({})).unwrap();
    adds_up(&answer);
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes.lines().count(), 1001);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("200 member(s) past the limit of 1000")), "{why:?}");
}

#[test]
fn contact_linked_and_one_off_members_of_a_stream_come_in() {
    let members = vec![
        // A one-off address.
        (one_off("Cleo Varga", "SMTP", "cleo@example.org", true), Vec::new()),
        // A contact's first email address: its address is in the one-off beside it.
        (w::wrapped_entry_id(0xC3, &[7; 46]), one_off("Dee Halloran", "SMTP", "dee@example.org", false)),
        // A one-off wrapped, with no one-off beside it.
        (w::wrapped_entry_id(0x00, &one_off("Eli Marsh", "SMTP", "eli@example.org", true)), Vec::new()),
        // A contact with no one-off, a list and a directory entry: no internet address.
        (w::wrapped_entry_id(0xC3, &[7; 46]), Vec::new()),
        (w::wrapped_entry_id(0xB4, &[8; 46]), Vec::new()),
        (one_off("Somebody", "EX", "/O=EXAMPLE/CN=SOME", true), Vec::new()),
    ];
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![streamed_list("Garden", w::list_stream(&members), vec![])]), json!({})).unwrap();
    adds_up(&answer);
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes, "Members:\nCleo Varga <cleo@example.org>\nDee Halloran <dee@example.org>\nEli Marsh <eli@example.org>");
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("3 member(s) of the list name no internet address")), "{why:?}");
}

#[test]
fn the_stream_is_read_and_the_member_lists_beside_it_are_not() {
    // [MS-OXOCNTC] 2.2.2.2.4: with a stream, the two lists are to be ignored.
    let old = vec![one_off("Old", "SMTP", "old@example.org", true)];
    let list =
        streamed_list("Both", w::list_stream(&[(smtp(1), Vec::new())]), vec![(lid(0x8054), multi_binary(&old)), (lid(0x8055), multi_binary(&old))]);
    let mut s = session();
    import(&mut s, &people_file(vec![list]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes, "Members:\nMember 1 <m1@example.org>");
}

#[test]
fn a_stream_cut_short_keeps_what_comes_before_the_cut_and_says_so() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..5).map(|i| (smtp(i), Vec::new())).collect();
    let mut bytes = w::list_stream_bytes(&members);
    let one = 12 + members[0].0.len();
    bytes.truncate(28 + 3 * one + 10);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![streamed_list("Cut", w::binary(&bytes), vec![])]), json!({})).unwrap();
    adds_up(&answer);
    let c = s.mailbox.contacts.values().next().unwrap();
    let lines: Vec<&str> = c.notes.lines().collect();
    assert_eq!(lines.len(), 5, "{}", c.notes);
    assert_eq!(
        (lines[3], lines[4]),
        ("Member 2 <m2@example.org>", "Some members may be missing: the list's stream of members couldn't be read whole.")
    );
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("2 member(s) of the list's stream couldn't be read")), "{why:?}");
}

#[test]
fn members_with_no_address_do_not_use_up_the_cap() {
    let mut members: Vec<(Vec<u8>, Vec<u8>)> = (0..300).map(|_| (w::wrapped_entry_id(0xB5, &[5; 30]), Vec::new())).collect();
    members.extend((0..900).map(|i| (smtp(i), Vec::new())));
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![streamed_list("Mixed", w::list_stream(&members), vec![])]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes.lines().count(), 901);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("300 member(s) of the list name no internet address")), "{why:?}");
    assert!(!why.iter().any(|w| w.contains("past the limit")), "{why:?}");
}

#[test]
fn a_stream_broken_at_its_first_member_gives_way_to_the_member_lists() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..3).map(|i| (smtp(i), Vec::new())).collect();
    let mut bytes = w::list_stream_bytes(&members);
    // The first entry id says it is longer than an entry may be.
    bytes[28..32].copy_from_slice(&0x0010_0000u32.to_le_bytes());
    let old = vec![one_off("Old", "SMTP", "old@example.org", true)];
    let list = streamed_list("Broken", w::binary(&bytes), vec![(lid(0x8054), multi_binary(&old)), (lid(0x8055), multi_binary(&old))]);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![list]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert_eq!(c.notes, "Members:\nOld <old@example.org>\nSome members may be missing: the list's stream of members couldn't be read whole.");
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("ends before its first member")), "{why:?}");
}

#[test]
fn a_stream_larger_than_the_cap_is_left_out_and_named() {
    // 4 MB and a little more, of members that fit what an entry may be.
    let one = smtp(1);
    let n = (4 << 20) / (one.len() + 12) + 10;
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..n).map(|_| (one.clone(), Vec::new())).collect();
    let stream = w::list_stream(&members);
    assert!(stream.bytes.len() > 4 << 20);
    let old = vec![one_off("Old", "SMTP", "old@example.org", true)];
    let list = streamed_list("Huge", stream, vec![(lid(0x8054), multi_binary(&old)), (lid(0x8055), multi_binary(&old))]);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![list]), json!({})).unwrap();
    adds_up(&answer);
    let c = s.mailbox.contacts.values().next().unwrap();
    assert!(c.notes.starts_with("Members:\nOld <old@example.org>\nSome members may be missing"), "{}", c.notes);
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("list's stream of members is larger than the limit")), "{why:?}");
}

#[test]
fn a_stream_that_lies_is_refused_and_the_member_lists_are_read_instead() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..3).map(|i| (smtp(i), Vec::new())).collect();
    let mut lying = w::list_stream_bytes(&members);
    // More members than the bytes could hold.
    lying[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    let old = vec![one_off("Old", "SMTP", "old@example.org", true)];
    let fallback = streamed_list("Lying", w::binary(&lying), vec![(lid(0x8054), multi_binary(&old)), (lid(0x8055), multi_binary(&old))]);
    // Of another version, with nothing beside it.
    let mut v2 = w::list_stream_bytes(&members);
    v2[0] = 9;
    let alone = streamed_list("Other version", w::binary(&v2), vec![]);
    // Not even a header.
    let tiny = streamed_list("Tiny", w::binary(&[1, 2, 3, 4]), vec![]);
    let mut s = session();
    let answer = import(&mut s, &people_file(vec![fallback, alone, tiny]), json!({})).unwrap();
    adds_up(&answer);
    let by = |n: &str| s.mailbox.contacts.values().find(|c| c.display_name == n).unwrap().notes.clone();
    assert_eq!(by("Lying"), "Members:\nOld <old@example.org>\nSome members may be missing: the list's stream of members couldn't be read whole.");
    assert_eq!(by("Other version"), "Members: the list's stream of members couldn't be read.");
    assert_eq!(by("Tiny"), "Members: the list's stream of members couldn't be read.");
    let why = texts(&answer, "notes", "why");
    assert!(why.iter().any(|w| w.contains("counts more members than it has room for")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("of a version this reader doesn't know")), "{why:?}");
    assert!(why.iter().any(|w| w.contains("shorter than its header")), "{why:?}");
}

#[test]
fn a_stream_with_lying_lengths_never_panics_and_stays_within_the_caps() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..50).map(|i| (smtp(i), smtp(i + 50))).collect();
    let good = w::list_stream_bytes(&members);
    let mut lists = Vec::new();
    for at in [12usize, 16, 28, 32, 100, 500, 1000, 3000] {
        for v in [0u32, 1, 0x7FFF_FFFF, u32::MAX] {
            let mut b = good.clone();
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            lists.push(streamed_list(&format!("L{at}-{v}"), w::binary(&b), vec![]));
        }
    }
    // Junk where the stream goes.
    lists.push(streamed_list("Text", w::unicode("not a stream"), vec![]));
    lists.push(streamed_list("Many", multi_binary(std::slice::from_ref(&good)), vec![]));
    let n = lists.len();
    let mut s = session();
    let answer = import(&mut s, &people_file(lists), json!({})).unwrap();
    adds_up(&answer);
    assert_eq!(answer["added"], n);
    for c in s.mailbox.contacts.values() {
        assert!(c.notes.lines().count() <= 1001, "{}", c.display_name);
        assert!(crate::cmd::people_merge::notes_line(&c.notes) <= crate::cmd::people_merge::MAX_NOTES, "{}", c.display_name);
    }
}

#[test]
fn a_streamed_list_with_long_notes_shares_the_room_with_its_members() {
    let members: Vec<(Vec<u8>, Vec<u8>)> = (0..10).map(|i| (smtp(i), Vec::new())).collect();
    let list = streamed_list("Long", w::list_stream(&members), vec![(0x1000, w::string8(&"n".repeat(1_048_570)))]);
    let mut s = session();
    import(&mut s, &people_file(vec![list]), json!({})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap();
    assert!(c.notes.ends_with("Member 9 <m9@example.org>"), "{}", &c.notes[c.notes.len() - 200..]);
}
