// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `file.importPst` (#815): synthetic .pst files made by the reader's test writer, of invented
//! people at `example.org`, never a real archive. Counts, fields, the report, caps, the
//! password, importing again, cancelling and going on, a dry run, ANSI text in its code page,
//! and hostile bytes.

use std::collections::HashSet;
use std::io::Cursor;

use mailcraft_model::{FlagStatus, Importance, Message};
use mailcraft_pst::test_writer::{self as w, AttachSpec, FileSpec, FolderSpec, Layout, MessageSpec, Value};
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
    appointment.class = "IPM.Appointment".into();
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
    Job { password: None, dry_run, file: "old-mail.pst".into(), name: "old-mail".into(), known: HashSet::new() }
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
        let sum: u64 = ["added", "already", "leftOut", "later"].iter().map(|k| r[*k].as_u64().unwrap()).sum();
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
    assert_eq!(answer["added"], 6, "{answer}");
    assert_eq!(answer["later"], 2, "{answer}");
    assert_eq!(answer["leftOut"], 0, "{answer}");
    assert_eq!(answer["preview"], true);
    assert_eq!(row(&answer, "Inbox")["added"], 2);
    assert_eq!(row(&answer, "Inbox/Projects")["added"], 1);
    assert_eq!(row(&answer, "Calendar")["later"], 1);
    assert_eq!(row(&answer, "Contacts")["later"], 1);
    assert_eq!(row(&answer, "Contacts")["added"], 1);
    let kinds: Vec<&str> = answer["laterKinds"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
    assert!(kinds.contains(&"appointment") && kinds.contains(&"contact"), "{kinds:?}");
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
    let inner = mailcraft_mime::parse(&attached.data);
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
            (0x3701, w::binary(&[0x89, b'P', b'N', b'G'])),
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
fn rich_text_without_html_is_kept_as_an_attachment_and_named() {
    let rtf = b"{\\rtf1 Hello}";
    let mut stored = Vec::new();
    stored.extend_from_slice(&((rtf.len() + 12) as u32).to_le_bytes());
    stored.extend_from_slice(&(rtf.len() as u32).to_le_bytes());
    stored.extend_from_slice(b"MELA");
    stored.extend_from_slice(&0u32.to_le_bytes());
    stored.extend_from_slice(rtf);
    let mut m = note("Rich");
    m.extra = vec![(0x1009, w::binary(&stored))];
    let mut s = session();
    let answer = import(&mut s, &w::build(&folder_list(vec![folder("Inbox", "IPF.Note", vec![m], vec![])])), json!({})).unwrap();
    let m = messages_in(&s, &["old-mail", "Inbox"])[0];
    assert!(m.body.text.starts_with("Rich"));
    let file = m.attachments.iter().find(|a| a.name == "message body.rtf").unwrap();
    assert_eq!(file.data.as_slice(), rtf);
    assert!(answer["notes"][0]["why"].as_str().unwrap().contains("rich text"), "{answer}");
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
    assert_eq!(answer["added"], 6);
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
    assert_eq!((again["added"].as_u64(), again["already"].as_u64()), (Some(0), Some(6)), "{again}");
    assert_eq!(s.mailbox.messages.len(), count);
    assert_eq!(s.mailbox.folders.len(), folders, "the same folders are used again");
}

#[test]
fn a_dry_run_counts_and_changes_nothing() {
    let mut s = session();
    let before = (s.mailbox.messages.len(), s.mailbox.folders.len());
    let answer = import(&mut s, &w::build(&mailbox_file()), json!({"dryRun": true})).unwrap();
    adds_up(&answer);
    assert_eq!((answer["dryRun"].as_bool(), answer["added"].as_u64(), answer["later"].as_u64()), (Some(true), Some(6), Some(2)), "{answer}");
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
    let answer = finish(&mut s, &place, outcome).unwrap();
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
    assert!(text.contains("Found 8 items: 6 brought in, 0 already here, 0 left out, 2 not yet brought in."), "{text}");
    assert!(text.contains("Inbox/Projects: 1 / 1 / 0 / 0 / 0"), "{text}");
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
    assert_eq!(answer["added"], 6);
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
