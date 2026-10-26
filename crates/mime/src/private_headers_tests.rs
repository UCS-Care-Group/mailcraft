// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's own notes in a message's kept headers stay in the mailbox (#505): what was
//! answered to a meeting and the file a message was opened from are never written into a file,
//! an mbox, a server's copy or what is sent, and a message can't bring the second in. What tells
//! another program a message is only its header (`X-MailCraft-Not-Downloaded`) still goes, and the
//! flag's own field is written from the flag.

use mailcraft_model::{Address, FlagStatus, Message};

use crate::{OPENED_FILE_HEADER, WriteOptions, parse, write, write_mbox};

fn message() -> Message {
    Message {
        from: Address { name: "Maya Lin".into(), email: "maya@lumen.example".into() },
        subject: "Notes".into(),
        flag: FlagStatus::Complete,
        headers: vec![
            ("X-MailCraft-Meeting-Response".into(), "accepted 1791993600 9f2c0000aaaa1111".into()),
            (OPENED_FILE_HEADER.into(), "ab".repeat(32)),
            ("x-mailcraft-anything-else".into(), "kept here".into()),
            ("X-MailCraft-Not-Downloaded".into(), "123456789".into()),
            ("X-Other".into(), "kept".into()),
        ],
        ..Default::default()
    }
}

#[test]
fn mailcrafts_own_notes_are_never_written_out() {
    let m = message();
    for out in [
        String::from_utf8(write(&m, &WriteOptions::default())).unwrap(),
        String::from_utf8(write(&m, &WriteOptions { include_bcc: true, offset_minutes: 60 })).unwrap(),
        String::from_utf8(write_mbox(&[&m], &WriteOptions::default())).unwrap(),
    ] {
        let lower = out.to_ascii_lowercase();
        assert!(!lower.contains("meeting-response") && !lower.contains("opened-file") && !lower.contains("anything-else"), "{out}");
        assert!(out.contains("X-MailCraft-Not-Downloaded: 123456789\r\n"), "{out}");
        assert!(out.contains("X-MailCraft-Flag: complete\r\n") && out.contains("X-Other: kept\r\n"), "{out}");
    }
}

#[test]
fn a_message_cant_bring_in_the_opened_file_note() {
    let text = format!(
        "From: a@lumen.example\r\n{OPENED_FILE_HEADER}: {}\r\nX-MAILCRAFT-OPENED-FILE-SHA256: x\r\nX-Other: kept\r\n\r\nHi\r\n",
        "cd".repeat(32)
    );
    let m = parse(text.as_bytes());
    assert!(!m.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(OPENED_FILE_HEADER)), "{:?}", m.headers);
    assert!(m.headers.iter().any(|(k, v)| k == "X-Other" && v == "kept"));
}
