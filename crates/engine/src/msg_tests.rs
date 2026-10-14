//! Outlook `.msg` files through `file.import`. The files are made in code (`mailcraft_msg::writer`)
//! from synthetic content.

use mailcraft_model::{FolderKind, Importance};
use mailcraft_msg::writer::Props;
use serde_json::json;

use crate::cmd::base64_encode;
use crate::io::Format;
use crate::*;

fn session() -> Session {
    let now = 1_792_000_000;
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn message() -> Vec<u8> {
    Props::new()
        .unicode(0x0037, "Venue options for the offsite")
        .unicode(0x0C1A, "Rowan Ellis")
        .unicode(0x5D01, "rowan@example.org")
        .time(0x0039, 1_700_000_000)
        .unicode(0x1035, "<venues.7@example.org>")
        .int(0x0017, 2)
        .unicode(0x1000, "Three venues, details attached.")
        .recipient(Props::new().int(0x0C15, 1).unicode(0x3001, "Avery Quinn").unicode(0x3002, "SMTP").unicode(0x3003, "avery@example.org"))
        .attachment(Props::new().int(0x3705, 1).unicode(0x3707, "venues.txt").binary(0x3701, b"Hall A\nHall B\nHall C\n"))
        .msg()
}

#[test]
fn formats_by_name_and_by_signature() {
    let bytes = message();
    assert_eq!(Format::from_extension("Venues.MSG"), Some(Format::Msg));
    assert_eq!(Format::detect("venues.msg", &bytes), Some(Format::Msg));
    // Whatever it is called, a compound file is read as one.
    assert_eq!(Format::detect("download", &bytes), Some(Format::Msg));
    assert_eq!(Format::detect("venues.eml", &bytes), Some(Format::Msg));
    assert_eq!(Format::detect("note.eml", b"Subject: hi\r\n\r\n"), Some(Format::Eml));
    // A `.msg` that is really an Internet message (some programs save them so) is read as one;
    // one that is nothing is still called what its name says, for the error to say why.
    assert_eq!(Format::detect("note.msg", b"Subject: hi\r\n\r\n"), Some(Format::Eml));
    assert_eq!(Format::detect("note.msg", b"\x00\x01"), Some(Format::Msg));
}

#[test]
fn a_msg_file_comes_in_to_the_folder_in_view() {
    let mut s = session();
    let archive = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Archive).unwrap_or(0);
    s.run("folder.select", &json!({"id": archive})).unwrap();
    let count = s.mailbox.messages.len();
    let r = s.run("file.import", &json!({"name": "Venue options.msg", "data": base64_encode(&message())})).unwrap();
    assert_eq!((r["format"].as_str(), r["messages"].as_u64(), r["folder"].as_u64()), (Some("msg"), Some(1), Some(archive)));
    let m = s.mailbox.message(r["ids"][0].as_u64().unwrap()).unwrap();
    assert_eq!(m.folder, archive);
    assert_eq!(m.subject, "Venue options for the offsite");
    assert_eq!((m.from.name.as_str(), m.from.email.as_str()), ("Rowan Ellis", "rowan@example.org"));
    assert_eq!(m.to[0].email, "avery@example.org");
    assert_eq!((m.date, m.received), (1_700_000_000, 1_700_000_000));
    assert_eq!(m.importance, Importance::High);
    assert_eq!(m.attachments[0].name, "venues.txt");
    assert_eq!(s.sel.messages, vec![m.id]);
    // One undo takes it back out.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.len(), count);
}

#[test]
fn a_msg_file_without_a_name_and_a_damaged_one() {
    let mut s = session();
    let inbox = s.mailbox.inbox().unwrap_or(0);
    let r = s.run("file.import", &json!({"name": "attachment", "data": base64_encode(&message()), "folder": inbox})).unwrap();
    assert_eq!(r["format"], "msg");

    let before = s.mailbox.messages.len();
    let mut damaged = message();
    damaged.truncate(600);
    assert!(s.run("file.import", &json!({"name": "broken.msg", "data": base64_encode(&damaged)})).is_err());
    let word = mailcraft_msg::writer::compound(&[mailcraft_msg::writer::Node::Stream("WordDocument".into(), vec![0; 100])], false);
    let e = s.run("file.import", &json!({"name": "letter.doc", "data": base64_encode(&word)})).unwrap_err();
    assert!(e.to_string().contains("not an Outlook message"), "{e}");
    assert_eq!(s.mailbox.messages.len(), before);
}
