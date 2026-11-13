// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Attachments: the opening verdict, hostile names and sizes, the caps, the picture told from its
//! own bytes, and the calendar import. Invented people and `example.org` addresses only.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::{Attachment, FolderKind, Message};

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::{Core, CoreError, OpenVerdict, attachment_check};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nBEGIN:VEVENT\r\nUID:one@example.org\r\nDTSTAMP:20261014T160000Z\r\nDTSTART:20261020T100000Z\r\nDTEND:20261020T110000Z\r\nSUMMARY:Planning\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

fn files() -> Vec<Attachment> {
    vec![
        Attachment::new("report.pdf", "application/pdf", b"%PDF-1.4".to_vec()),
        Attachment::new("budget.xlsm", "application/vnd.ms-excel.sheet.macroEnabled.12", vec![1, 2, 3]),
        Attachment::new("page.html", "text/html", b"<script>1</script>".to_vec()),
        Attachment::new("setup.apk", "application/vnd.android.package-archive", vec![9; 4]),
        Attachment::new("photo.png", "image/png", PNG.to_vec()),
        Attachment::new("disguised.jpg", "image/jpeg", b"MZ not a picture".to_vec()),
        Attachment::new("invite.ics", "text/calendar", ICS.as_bytes().to_vec()),
        Attachment::new("../../etc/readme.txt", "text/plain", b"x".to_vec()),
        Attachment::new("big.bin.pdf", "application/pdf", vec![7u8; 9 << 20]),
    ]
}

fn core_with(attachments: Vec<Attachment>, platform: Arc<FakePlatform>) -> (Arc<Core>, u64) {
    let core = Core::start(platform, move || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let folder = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder, subject: "Files".into(), ..Message::default() };
        m.body.text = "See the files".into();
        m.attachments = attachments;
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let folders = block_on(core.folders(0)).unwrap();
    let inbox = folders.folders.iter().find(|f| f.kind == "inbox").unwrap().id;
    let id = block_on(core.messages(inbox, "all".into(), 0, 5)).unwrap().rows[0].id;
    (core, id)
}

#[test]
fn the_verdict_is_the_desktops() {
    assert_eq!(attachment_check("report.pdf".into()).verdict, OpenVerdict::Open);
    assert_eq!(attachment_check("budget.XLSM".into()).verdict, OpenVerdict::Macros);
    assert_eq!(attachment_check("page.html".into()).verdict, OpenVerdict::Scripts);
    for blocked in ["setup.apk", "a.apks", "a.xapk", "a.aab", "a.ipa", "a.mobileconfig", "run.exe", "noextension", ""] {
        let check = attachment_check(blocked.into());
        assert_eq!(check.verdict, OpenVerdict::Blocked, "{blocked}");
        assert!(!check.blocked.is_empty(), "{blocked}");
    }
    // A kind that is hidden behind another is judged by what the name is written as.
    assert_eq!(attachment_check("invoice.pdf.apk".into()).verdict, OpenVerdict::Blocked);
    assert_eq!(attachment_check("x.pdf\u{202e}kpa".into()).verdict, attachment_check("x.pdfkpa".into()).verdict);
}

#[test]
fn a_name_from_a_message_is_made_safe() {
    for hostile in
        ["../../etc/passwd", "..\\..\\x.pdf", "/abs/path.pdf", "a\0b.pdf", "con.pdf", ".hidden", "-rf.pdf", "gpj\u{202e}fdp.exe", "a:b*c?.pdf"]
    {
        let check = attachment_check(hostile.into());
        for name in [&check.name, &check.save_name] {
            assert!(!name.is_empty(), "{hostile:?}");
            assert!(!name.contains(['/', '\\', '\0', ':', '*', '?']), "{hostile:?} -> {name:?}");
            assert!(!name.starts_with(['.', '-']), "{hostile:?} -> {name:?}");
            assert!(!name.contains('\u{202e}'), "{hostile:?} -> {name:?}");
        }
    }
    let long = attachment_check(format!("{}.pdf", "a".repeat(100_000)));
    assert!(long.name.chars().count() <= 120 && long.name.ends_with(".pdf"));
    assert_eq!(attachment_check("noext".into()).save_name, "noext.bin");
    assert!(attachment_check("invite.ICS".into()).calendar);
    assert!(!attachment_check("report.pdf".into()).calendar);
}

#[test]
fn bytes_come_with_their_kind_told_from_the_bytes() {
    let (core, id) = core_with(files(), FakePlatform::new());
    let pdf = block_on(core.attachment(id, 0)).unwrap();
    assert_eq!(pdf.bytes, b"%PDF-1.4");
    assert_eq!(pdf.check.verdict, OpenVerdict::Open);
    assert!(pdf.picture.is_empty());
    assert_eq!(block_on(core.attachment(id, 4)).unwrap().picture, "image/png");
    // A name that says picture is no picture when the bytes say otherwise.
    assert_eq!(block_on(core.attachment(id, 5)).unwrap().picture, "");
    // Blocked kinds can still be read, to be saved.
    assert_eq!(block_on(core.attachment(id, 3)).unwrap().check.verdict, OpenVerdict::Blocked);
    // The name that comes back is the safe one.
    assert_eq!(block_on(core.attachment(id, 7)).unwrap().check.name, "readme.txt");
}

#[test]
fn nothing_is_read_for_an_index_or_a_message_that_is_not_there() {
    let (core, id) = core_with(files(), FakePlatform::new());
    for (message, index) in [(id, 99), (id, u32::MAX), (0, 0), (u64::MAX, 0), (id + 1000, 1)] {
        assert!(block_on(core.attachment(message, index)).is_err(), "{message} {index}");
        assert!(block_on(core.open_attachment(message, index, true)).is_err(), "{message} {index}");
        assert!(block_on(core.import_calendar_attachment(message, index)).is_err(), "{message} {index}");
    }
}

#[test]
fn a_file_over_the_cap_is_not_read() {
    // 9 MB is within the desktop's cap, but not within a quarter of a 16 MB budget.
    let (small, id) = core_with(files(), FakePlatform::with_budget(16 << 20));
    assert!(matches!(block_on(small.attachment(id, 8)), Err(CoreError::Failed { .. })));
    assert_eq!(block_on(small.attachment(id, 0)).unwrap().bytes.len(), 8);
    let (roomy, id) = core_with(files(), FakePlatform::with_budget(2 << 30));
    assert_eq!(block_on(roomy.attachment(id, 8)).unwrap().bytes.len(), 9 << 20);
}

#[test]
fn opening_follows_the_verdict() {
    let platform = FakePlatform::new();
    let (core, id) = core_with(files(), platform.clone());
    block_on(core.open_attachment(id, 0, false)).unwrap();
    // A kind that asks is not opened until the owner said yes.
    assert!(matches!(block_on(core.open_attachment(id, 1, false)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.open_attachment(id, 2, false)), Err(CoreError::BadRequest { .. })));
    block_on(core.open_attachment(id, 1, true)).unwrap();
    // A blocked kind is never opened, said yes or not, and its bytes are not read for it.
    for yes in [false, true] {
        assert!(matches!(block_on(core.open_attachment(id, 3, yes)), Err(CoreError::Failed { .. })));
    }
    // A path in a name does not reach the other app.
    block_on(core.open_attachment(id, 7, false)).unwrap();
    let opened = platform.files.lock().unwrap().clone();
    assert_eq!(opened, vec![("report.pdf".to_string(), 8), ("budget.xlsm".to_string(), 3), ("readme.txt".to_string(), 1)]);
}

#[test]
fn a_calendar_file_goes_into_the_calendar_and_nothing_else_does() {
    let (core, id) = core_with(files(), FakePlatform::new());
    let done = block_on(core.import_calendar_attachment(id, 6)).unwrap();
    assert_eq!((done.events, done.tasks), (1, 0));
    assert!(matches!(block_on(core.import_calendar_attachment(id, 0)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.import_calendar_attachment(id, 3)), Err(CoreError::BadRequest { .. })));
}

#[test]
fn a_calendar_file_that_is_not_one_is_an_error_and_changes_nothing() {
    let junk = vec![Attachment::new("fake.ics", "text/calendar", b"\0\0 not a calendar".to_vec())];
    let (core, id) = core_with(junk, FakePlatform::new());
    let rev = block_on(core.attachment(id, 0)).unwrap().rev;
    assert!(block_on(core.import_calendar_attachment(id, 0)).is_err());
    assert_eq!(block_on(core.attachment(id, 0)).unwrap().rev, rev);
}

#[test]
fn what_was_read_is_checked_again_before_it_is_handed_over() {
    use crate::attachments::refuse_unless_openable;
    for (name, confirmed, ok) in [("a.pdf", false, true), ("a.xlsm", false, false), ("a.xlsm", true, true), ("a.exe", true, false), ("a.exe", false, false)] {
        assert_eq!(refuse_unless_openable(&attachment_check(name.to_string()), confirmed).is_ok(), ok, "{name} {confirmed}");
    }
}
