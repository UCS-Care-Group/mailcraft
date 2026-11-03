// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The headers that go with a message forwarded as an attachment (#315): its own fields, and of
//! the other headers kept with it only those known to come from the sender or the servers on the
//! way. A resend's Bcc, the address it was delivered to, a request for a receipt, whatever a mail
//! program or the owner's server wrote for the owner (`X-…` of any kind) and the sender's
//! organisation and mail program stay here.

use mailcraft_model::{Address, FolderKind, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::{Session, sample};

/// Headers known to come from the sender or the servers on the way: they go.
const KEPT: [(&str, &str); 19] = [
    ("Received", "from mx.lumen.example by mail.example.org for <kit@example.org>; Wed, 14 Oct 2026 00:42:00 +0000"),
    ("Return-Path", "<avery@lumen.example>"),
    ("Sender", "Avery Quinn <avery@lumen.example>"),
    ("Comments", "Figures for the third quarter"),
    ("Resent-Date", "Wed, 14 Oct 2026 01:00:00 +0000"),
    ("Resent-From", "Avery Quinn <avery@lumen.example>"),
    ("Resent-Sender", "avery@lumen.example"),
    ("Resent-To", "kit@example.org"),
    ("Resent-Cc", "sam@example.org"),
    ("Resent-Message-ID", "<resent.1@lumen.example>"),
    ("Authentication-Results", "mail.example.org; dkim=pass header.d=lumen.example; spf=pass"),
    ("DKIM-Signature", "v=1; a=rsa-sha256; d=lumen.example; s=s1; b=AbC"),
    ("ARC-Seal", "i=1; a=rsa-sha256; d=example.org; s=arc; cv=none; b=DeF"),
    ("ARC-Message-Signature", "i=1; a=rsa-sha256; d=example.org; s=arc; b=GhI"),
    ("ARC-Authentication-Results", "i=1; mail.example.org; dkim=pass"),
    ("X-Spam-Status", "No, score=-0.1"),
    ("X-Spam-Score", "-0.1"),
    ("List-Id", "Budget <budget.lumen.example>"),
    ("List-Unsubscribe", "<mailto:leave@lumen.example>"),
];

/// Headers that say something of the owner, ask something of whoever opens the message, or aren't
/// needed to tell where it came from: they stay here. The first six are those #434's review found
/// going with it.
const DROPPED: [(&str, &str); 21] = [
    ("Resent-Bcc", "hidden@example.org"),
    ("X-Original-To", "alias+budget@example.org"),
    ("X-Evolution", "00000003-0010"),
    ("X-Apple-Flags", "flagged"),
    ("X-Folder", "Private/Taxes 2025"),
    ("Return-Receipt-To", "avery@lumen.example"),
    ("resent-bcc", "lower@example.org"),
    ("Delivered-To", "alias+budget@example.org"),
    ("Envelope-To", "alias+budget@example.org"),
    ("Received-SPF", "pass (mail.example.org) receiver=alias+budget@example.org; client-ip=192.0.2.1"),
    ("Disposition-Notification-To", "avery@lumen.example"),
    ("X-MailCraft-Meeting-Response", "accepted 1791993600 00ff"),
    ("X-Mozilla-Status", "0001"),
    ("X-Gmail-Labels", "Important,Taxes 2025"),
    ("X-Forefront-Antispam-Report", "CIP:192.0.2.1;CTRY:;SFV:NSPM"),
    ("Thread-Index", "AQHZ"),
    ("Precedence", "bulk"),
    ("X-Anything", "made up by some mail program"),
    ("Organization", "Lumen"),
    ("User-Agent", "Invented Mail 2.0"),
    ("X-Mailer", "Invented Mailer 1.4"),
];

/// The message's own fields, written from what it is.
const OWN: [&str; 12] =
    ["date", "from", "to", "cc", "reply-to", "subject", "message-id", "in-reply-to", "references", "importance", "x-priority", "mime-version"];

/// A message in Sent Items with every one of those headers, a Bcc line and replies' fields.
fn sent_with_headers(s: &mut Session) -> mailcraft_model::Id {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let m = Message {
        account,
        folder,
        message_id: "budget.3@lumen.example".into(),
        in_reply_to: "budget.2@lumen.example".into(),
        references: vec!["budget.1@lumen.example".into(), "budget.2@lumen.example".into()],
        from: Address::new("Avery Quinn", "avery@lumen.example"),
        to: vec![Address::new("Kit", "kit@example.org")],
        cc: vec![Address::new("Sam", "sam@example.org")],
        bcc: vec![Address::new("Quiet", "quiet@example.org")],
        reply_to: vec![Address::new("Budget desk", "budget@lumen.example")],
        subject: "Budget".into(),
        date: 1_791_000_000,
        importance: mailcraft_model::Importance::High,
        body: mailcraft_model::Body { text: "The numbers.".into(), html: None },
        headers: KEPT.iter().chain(&DROPPED).map(|(name, value)| (name.to_string(), value.to_string())).collect(),
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn only_headers_known_to_come_from_the_sender_or_the_servers_go_with_it() {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let id = sent_with_headers(&mut s);
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let bytes = match s.compose.as_ref().unwrap().attachments.as_slice() {
        [eml] => eml.data.to_vec(),
        other => panic!("{} attachments", other.len()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let header = text.split("\r\n\r\n").next().unwrap();
    // Each field once, unfolded: its name in lower case, and its value.
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in header.split("\r\n") {
        match (line.starts_with([' ', '\t']), fields.last_mut()) {
            (true, Some((_, value))) => value.push_str(line),
            _ => {
                let (name, value) = line.split_once(':').unwrap();
                fields.push((name.to_ascii_lowercase(), value.trim().to_string()));
            }
        }
    }

    for (name, value) in KEPT {
        let lower = name.to_ascii_lowercase();
        assert!(fields.iter().any(|(n, v)| *n == lower && v == value), "{name}: {value} not in\n{header}");
    }
    for (name, _) in DROPPED {
        let lower = name.to_ascii_lowercase();
        assert!(!fields.iter().any(|(n, _)| *n == lower), "{name} in\n{header}");
    }
    assert!(!text.contains("hidden@example.org") && !text.contains("quiet@example.org") && !text.contains("alias+budget"), "{header}");
    // Nothing else but the message's own fields: no Bcc, and none of the owner's marks.
    let kept: Vec<String> = KEPT.iter().map(|(name, _)| name.to_ascii_lowercase()).collect();
    for (name, _) in &fields {
        assert!(OWN.contains(&name.as_str()) || name.starts_with("content-") || kept.contains(name), "{name} in\n{header}");
    }
    for own in ["date", "from", "to", "cc", "reply-to", "subject", "message-id", "in-reply-to", "references", "importance"] {
        assert!(fields.iter().any(|(n, _)| n == own), "no {own} in\n{header}");
    }
}
