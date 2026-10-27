// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the system hands MailCraft (#505): the command line, a later start, the Finder. A
//! `mailto:` link starts a message with what it says and nothing else; a message file opens in a
//! window of its own; a calendar or contact file is imported only once the owner says so, one
//! question at a time.

use std::path::PathBuf;

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-system-open-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nBEGIN:VEVENT\r\nUID:handed-1@lumen.example\r\nDTSTAMP:20261014T100000Z\r\nDTSTART:20261020T090000Z\r\nDTEND:20261020T100000Z\r\nSUMMARY:Handed over meeting\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
const VCF: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Robin Handed\r\nN:Handed;Robin;;;\r\nEMAIL:robin@lumen.example\r\nEND:VCARD\r\n";

/// RFC 6068: the recipients (to, cc, bcc), the subject and the body, and nothing that can carry a
/// line break into a header.
#[test]
fn a_mailto_link_from_the_system_starts_a_message_with_what_it_says() {
    let mut a = app();
    a.open_from_system("mailto:maya@lumen.example,sam@lumen.example?cc=lee@lumen.example&bcc=kim@lumen.example&subject=Lunch%20on%20Friday&body=Shall%20we%3F%0D%0ASee%20you");
    let c = a.session.compose.clone().unwrap();
    assert_eq!(c.to, "maya@lumen.example; sam@lumen.example");
    assert_eq!((c.cc.as_str(), c.bcc.as_str(), c.subject.as_str()), ("lee@lumen.example", "kim@lumen.example", "Lunch on Friday"));
    assert!(c.body.plain().starts_with("Shall we?\nSee you"), "{:?}", c.body.plain());

    // A header of the link's own choosing, and line breaks in the subject and the addresses, go
    // nowhere.
    let mut b = app();
    b.open_from_system(
        "MAILTO:x@lumen.example%0D%0ABcc:%20spy@evil.example?subject=Hi%0D%0ABcc:%20spy@evil.example&from=boss@evil.example&in-reply-to=%3Cx%3E",
    );
    let c = b.session.compose.clone().unwrap();
    for field in [&c.to, &c.cc, &c.bcc, &c.subject] {
        assert!(!field.contains('\r') && !field.contains('\n'), "{field:?}");
    }
    assert!(c.bcc.is_empty(), "{:?}", c.bcc);
    assert!(!c.to.contains("boss@") && !c.cc.contains("boss@"));
}

#[test]
fn calendar_and_contact_files_from_the_system_are_imported_only_on_the_owners_word() {
    let dir = scratch("ask");
    let (ics, vcf) = (dir.join("meeting.ics"), dir.join("robin.vcf"));
    std::fs::write(&ics, ICS).unwrap();
    std::fs::write(&vcf, VCF).unwrap();
    let mut a = app();
    let (events, contacts) = (a.session.mailbox.events.len(), a.session.mailbox.contacts.len());
    a.open_from_system(&ics.to_string_lossy());
    // The same file twice is asked about once; the contact file waits its turn.
    a.open_from_system(&ics.to_string_lossy());
    a.open_from_system(&format!("file://{}", vcf.to_string_lossy().replace(' ', "%20")));
    assert_eq!(a.session.mailbox.events.len(), events, "nothing is imported before the owner says so");
    let Some(Dialog::Confirm { command, params, ask }) = &a.dialog else { panic!("asked first") };
    assert_eq!(command, "file.import");
    assert_eq!(params["path"], json!(ics.to_string_lossy()));
    assert!(ask.title == "Import Calendar File" && ask.text.contains("meeting.ics") && ask.action == "Import", "{}", ask.text);
    assert_eq!(a.opened_waiting.len(), 1);

    // Import: what the question's button does.
    let (command, params) = (command.clone(), crate::dialogs::answered(params, &Value::Null, None));
    a.dialog = None;
    crate::dialogs::go_ahead(&mut a, &command, params);
    assert_eq!(a.session.mailbox.events.len(), events + 1);
    assert!(a.session.mailbox.events.values().any(|e| e.title == "Handed over meeting"));

    // The next one is asked once nothing else is; Cancel leaves it.
    super::ask_next(&mut a);
    let Some(Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("the contact file is asked about next") };
    assert!(ask.title == "Import Contacts" && ask.text.contains("robin.vcf"), "{}", ask.text);
    a.dialog = None;
    super::ask_next(&mut a);
    assert!(a.dialog.is_none() && a.opened_waiting.is_empty());
    assert_eq!(a.session.mailbox.contacts.len(), contacts);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn files_waiting_to_be_asked_about_are_bounded() {
    let mut a = app();
    for i in 0..super::MOST_WAITING + 20 {
        a.open_from_system(&format!("/nowhere/{i}.ics"));
    }
    // One on screen, the rest waiting, no more than the bound.
    assert!(a.dialog.is_some());
    assert!(a.opened_waiting.len() <= super::MOST_WAITING);
}

/// A drop on the window is the owner's own act: it is imported at once, as before.
#[test]
fn a_calendar_file_dropped_on_the_window_is_imported_without_a_question() {
    let dir = scratch("drop");
    let ics = dir.join("dropped.ics");
    std::fs::write(&ics, ICS).unwrap();
    let mut a = app();
    let events = a.session.mailbox.events.len();
    a.open_path(&ics.to_string_lossy());
    assert!(a.dialog.is_none());
    assert_eq!(a.session.mailbox.events.len(), events + 1);
    let _ = std::fs::remove_dir_all(dir);
}
