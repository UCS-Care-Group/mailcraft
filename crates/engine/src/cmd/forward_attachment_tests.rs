// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forward as Attachment (#315): a new message with nothing quoted and the message attached whole
//! as an `.eml`, without a Bcc or the owner's own marks; never what only stands in for a message
//! here; and the original marked forwarded once it has gone.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{Address, Attachment, Body, ContentRef, FlagStatus, FolderKind, Id, KeptBody, Message, NOT_DOWNLOADED};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::{ComposeMode, Session, sample};

const MOCKUPS: &str = "Homepage mockups, round three";

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn by_subject(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().id
}

/// The one attachment of the message being written: its name, its type and its bytes.
fn attached(s: &Session) -> (String, String, Vec<u8>) {
    match s.compose.as_ref().unwrap().attachments.as_slice() {
        [one] => (one.name.clone(), one.mime.clone(), one.data.to_vec()),
        other => panic!("{} attachments", other.len()),
    }
}

/// A message of the sample's account in its folder of `kind`, with a Bcc line.
fn with_bcc(s: &mut Session, kind: FolderKind, draft: bool) -> Id {
    let account = s.mailbox.accounts[0].id;
    let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
    let m = Message {
        account,
        folder,
        draft,
        message_id: format!("bcc.{draft}@lumen.example"),
        from: Address::new("Avery Quinn", "avery@lumen.example"),
        to: vec![Address::new("Kit", "kit@example.org")],
        bcc: vec![Address::new("Quiet", "quiet@example.org")],
        subject: "Budget".into(),
        date: 1_791_000_000,
        body: Body { text: "The numbers.".into(), html: None },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn the_message_goes_whole_as_an_eml_with_nothing_quoted() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    let original = s.loaded_message(id).unwrap();
    assert!(!original.attachments.is_empty() && !original.body.text.is_empty());

    let r = s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    assert_eq!(r, json!({"composing": true, "mode": "forward"}));
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.mode, c.source, c.account), (ComposeMode::Forward, Some(id), original.account));
    assert!(c.as_attachment);
    assert_eq!(c.subject, format!("FW: {MOCKUPS}"));
    assert!(c.to.is_empty() && c.cc.is_empty() && c.bcc.is_empty());
    assert!(c.quote.is_none() && c.in_reply_to.is_empty() && c.references.is_empty(), "nothing quoted, not an answer");
    // A line to write on, then the signature for replies and forwards, and nothing of the original.
    let signature = s.mailbox.account(original.account).unwrap().reply_signature().to_string();
    assert_eq!(c.signature.as_ref().map(|sig| sig.name.as_str()), Some(signature.as_str()));
    let text = c.body.plain();
    assert!(text.starts_with('\n') && !text.contains("Round three is attached"), "{text:?}");

    // The message itself, as it came: its headers, its text and its files.
    let (name, mime, bytes) = attached(&s);
    assert_eq!((name.as_str(), mime.as_str()), ("Homepage mockups, round three.eml", "message/rfc822"));
    let back = mailcraft_mime::parse(&bytes);
    assert_eq!((&back.from, &back.to, &back.cc), (&original.from, &original.to, &original.cc));
    assert_eq!((back.subject.as_str(), back.message_id.as_str(), back.date), (MOCKUPS, original.message_id.as_str(), original.date));
    assert_eq!(back.body.text.trim_end(), original.body.text.trim_end());
    let files = |m: &Message| m.attachments.iter().map(|a| (a.name.clone(), a.data.to_vec())).collect::<Vec<_>>();
    assert_eq!(files(&back), files(&original));
    // Not yet marked: nothing has gone.
    assert!(!s.mailbox.message(id).unwrap().forwarded);
}

#[test]
fn the_owners_own_marks_on_the_message_stay_here() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    let m = s.mailbox.message_mut(id).unwrap();
    (m.read, m.answered, m.flag, m.categories) = (true, true, FlagStatus::Flagged, vec!["Blue category".into(), "Private".into()]);
    // Marks a mail program keeps as headers: an answer to a meeting request; Thunderbird's own, its
    // account and a POP message's; Gmail's from a Takeout; a server's mailbox file's; and the
    // address it was delivered to. And what the sender and the servers on the way wrote, which
    // goes (every header: `forward_attachment_headers_tests`).
    for (name, value) in [
        ("X-MailCraft-Meeting-Response", "accepted 1791993600 00ff"),
        ("X-Mozilla-Status", "0001"),
        ("X-Mozilla-Keys", "$label1"),
        ("X-Mozilla-Draft-Info", "internal/draft; vcard=0"),
        ("X-Account-Key", "account3"),
        ("X-Identity-Key", "id2"),
        ("X-UIDL", "GmailId18f2"),
        ("X-Gmail-Labels", "Important,Taxes 2025"),
        ("X-GM-THRID", "1791993600123"),
        ("X-UID", "4182"),
        ("X-Keywords", "$Forwarded"),
        ("Received", "from mx.lumen.example by mail.example; Wed, 14 Oct 2026 00:42:00 +0000"),
        ("Authentication-Results", "mail.example; dkim=pass header.d=lumen.example"),
        ("Delivered-To", "you@example.org"),
        ("X-Spam-Status", "No, score=-0.1"),
    ] {
        m.headers.push((name.into(), value.into()));
    }
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let (_, _, bytes) = attached(&s);
    let text = String::from_utf8_lossy(&bytes);
    let header = text.split("\r\n\r\n").next().unwrap();
    let names: Vec<String> = header
        .split("\r\n")
        .filter(|line| !line.starts_with([' ', '\t']))
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.to_ascii_lowercase())
        .collect();
    for field in [
        "keywords",
        "status",
        "x-status",
        "x-account-key",
        "x-identity-key",
        "x-uidl",
        "x-gmail-labels",
        "x-gm-thrid",
        "x-uid",
        "x-keywords",
        "delivered-to",
    ] {
        assert!(!names.iter().any(|n| n == field), "{field} in {header}");
    }
    for prefix in ["x-mailcraft-", "x-mozilla-"] {
        assert!(!names.iter().any(|n| n.starts_with(prefix)), "{prefix} in {header}");
    }
    for field in ["Received: from mx.lumen.example", "Authentication-Results: mail.example; dkim=pass", "X-Spam-Status: No"] {
        assert!(header.contains(field), "{field} not in {header}");
    }
    // Finished, the flag doesn't go either.
    s.mailbox.message_mut(id).unwrap().flag = FlagStatus::Complete;
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let (_, _, bytes) = attached(&s);
    assert!(!String::from_utf8_lossy(&bytes).contains("X-MailCraft-Flag:"));
    // And the message here keeps them.
    let m = s.mailbox.message(id).unwrap();
    assert!(m.read && m.answered && m.flag == FlagStatus::Complete && m.categories.len() == 2 && m.headers.len() >= 5);
}

#[test]
fn a_bcc_never_goes_with_it_not_even_a_drafts() {
    let mut s = session();
    for (kind, draft) in [(FolderKind::Sent, false), (FolderKind::Drafts, true)] {
        let id = with_bcc(&mut s, kind, draft);
        s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
        let (name, _, bytes) = attached(&s);
        assert_eq!(name, "Budget.eml");
        let back = mailcraft_mime::parse(&bytes);
        assert!(back.bcc.is_empty() && !String::from_utf8_lossy(&bytes).contains("quiet@example.org"), "{kind:?}");
        assert_eq!(back.to, [Address::new("Kit", "kit@example.org")]);
        // Save Message As keeps a draft's, for the owner's own file.
        let saved = s.run("file.exportMessage", &json!({"id": id})).unwrap();
        let saved = crate::cmd::base64_decode(saved["data"].as_str().unwrap()).unwrap();
        assert_eq!(String::from_utf8_lossy(&saved).contains("quiet@example.org"), draft);
    }
}

#[test]
fn a_message_with_no_subject_goes_as_message_eml() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    s.mailbox.message_mut(id).unwrap().subject = "  ".into();
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    assert_eq!(attached(&s).0, "message.eml");
    assert_eq!(s.compose.as_ref().unwrap().subject, "FW: ");
    // Nor when nothing of it is left in a file name: only characters a name can't keep, or none
    // that can be seen.
    for subject in ["???", "\u{200b}"] {
        s.mailbox.message_mut(id).unwrap().subject = subject.into();
        s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
        assert_eq!(attached(&s).0, "message.eml", "{subject:?}");
    }
    // A subject that names folders or devices is only a name.
    s.mailbox.message_mut(id).unwrap().subject = "../Q3: plan\\draft?".into();
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let name = attached(&s).0;
    assert!(name.ends_with(".eml") && !name.contains(['/', '\\', ':', '?']) && !name.starts_with('.'), "{name}");
}

#[test]
fn what_only_stands_in_for_a_message_is_never_attached() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    let whole = s.mailbox.message(id).unwrap().clone();
    let refused = |s: &mut Session, change: &dyn Fn(&mut Message), says: &str| {
        *s.mailbox.message_mut(id).unwrap() = whole.clone();
        change(s.mailbox.message_mut(id).unwrap());
        let before = s.mailbox.message(id).unwrap().clone();
        let e = s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains(says), "{e}");
        assert!(s.compose.is_none(), "{says}: nothing is being written");
        assert_eq!(s.mailbox.message(id).unwrap(), &before, "{says}: the message is as it was");
    };
    // Of a Microsoft account, only its headers and a preview, waiting to be fetched.
    refused(
        &mut s,
        &|m| {
            (m.unfetched, m.server) = (true, Some(ServerMessage { id: "AAMk1".into(), ..Default::default() }));
            m.attachments.clear();
            m.body = Body { text: "Hi both, Round three".into(), html: None };
        },
        "isn't downloaded yet",
    );
    // The server's copy went before it could be fetched: the headers are all there will be.
    refused(&mut s, &|m| (m.unfetched, m.incomplete) = (true, true), "only the headers");
    // Its text couldn't be read from where the mailbox keeps it: what is here says so instead.
    refused(
        &mut s,
        &|m| {
            m.body = Body { text: "[This message's text is unavailable]".into(), html: None };
            m.kept = Some(Box::new(KeptBody { text: Some(ContentRef { sha256: [7; 32], len: 900 }), failed: true, ..Default::default() }));
        },
        "can't be read on this computer",
    );
    // Too large to download (IMAP): its header and a note in place of its text.
    refused(
        &mut s,
        &|m| {
            m.attachments.clear();
            m.body = Body { text: "This message is too large to download: it is 61 MB on the server.".into(), html: None };
            m.headers.push((NOT_DOWNLOADED.into(), "62914561".into()));
        },
        "too large to download",
    );

    // A sender can write that mark into a message of their own: one with a file, with HTML, or
    // plain text that isn't the note, is no such note, and goes, without the mark.
    *s.mailbox.message_mut(id).unwrap() = whole.clone();
    s.mailbox.message_mut(id).unwrap().headers.push(("x-mailcraft-not-downloaded".into(), "1".into()));
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let m = s.mailbox.message_mut(id).unwrap();
    m.attachments.clear();
    m.body.html = Some("<p>Round three</p>".into());
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    s.mailbox.message_mut(id).unwrap().body = Body { text: "Your account is locked. Sign in here.".into(), html: None };
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    let (_, mime, bytes) = attached(&s);
    assert_eq!(mime, "message/rfc822");
    assert!(!String::from_utf8_lossy(&bytes).to_ascii_lowercase().contains("x-mailcraft-not-downloaded"));
}

#[test]
fn a_message_too_large_to_attach_is_refused_before_it_is_read_in() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    // A file of 80 MB kept beside the mailbox, which isn't there to read: never asked for.
    let big = Attachment { kept: Some(ContentRef { sha256: [9; 32], len: 80 << 20 }), ..Attachment::new("scan.tiff", "image/tiff", Vec::new()) };
    s.mailbox.message_mut(id).unwrap().attachments.push(big);
    let e = s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("Homepage mockups, round three.eml is larger than 64 MB"), "{e}");
    assert!(s.compose.is_none());
}

#[test]
fn a_message_named_is_that_one_whatever_is_selected() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    let other = by_subject(&s, "Q4 roadmap review moved to Thursday");
    // Named, it is that one, whatever is selected.
    s.sel.messages = vec![id];
    for params in [json!({"id": other}), json!({"ids": [other]}), json!({"id": other as f64})] {
        s.run("mail.forwardAsAttachment", &params).unwrap();
        assert_eq!(s.compose.as_ref().unwrap().source, Some(other), "{params}");
        assert_eq!(s.compose.as_ref().unwrap().attachments.len(), 1, "{params}");
    }
    // Named wrongly, it is refused, never the selected message taken for it (#360): a forward goes
    // to other people.
    s.compose = None;
    for command in ["mail.forwardAsAttachment", "mail.forward"] {
        for junk in [
            json!({"id": other.to_string()}),
            json!({"id": 0}),
            json!({"id": -1}),
            json!({"id": 1.5}),
            json!({"id": [other]}),
            json!({"ids": other}),
            json!({"ids": format!("{other}")}),
            json!({"ids": {"0": other}}),
            json!({"ids": []}),
            json!({"ids": [other, "7"]}),
            json!({"id": other, "ids": [other]}),
            json!({"message": other}),
            json!({"id": other, "to": "help@example.org"}),
            json!([other]),
        ] {
            let e = s.run(command, &junk).unwrap_err().to_string();
            assert!(e.starts_with("bad parameters"), "{command} {junk}: {e}");
            assert!(s.compose.is_none(), "{command} {junk}: nothing written");
        }
    }
    // A message being written may be let go by the caller.
    s.run("mail.forward", &json!({"id": other, "discard": true})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().source, Some(other));
}

#[test]
fn it_needs_a_message() {
    let mut s = session();
    s.sel.messages.clear();
    let e = s.run("mail.forwardAsAttachment", &json!({})).unwrap_err().to_string();
    assert!(e.contains("select a message first"), "{e}");
    let e = s.run("mail.forwardAsAttachment", &json!({"id": 999_999})).unwrap_err().to_string();
    assert!(e.contains("no message with id 999999"), "{e}");
    assert!(s.compose.is_none());
    // The selected one, else.
    let id = by_subject(&s, MOCKUPS);
    s.run("mail.select", &json!({"id": id})).unwrap();
    s.run("mail.forwardAsAttachment", &json!({})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().source, Some(id));
}

#[test]
fn the_original_is_marked_forwarded_once_it_has_gone() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    let m = s.mailbox.message_mut(id).unwrap();
    (m.answered, m.forwarded) = (false, false);
    s.run("mail.forwardAsAttachment", &json!({"id": id})).unwrap();
    s.run("compose.set", &json!({"to": "help@example.org"})).unwrap();
    // The sample's account is on this computer: what is sent is filed as sent at once.
    let r = s.run("compose.send", &json!({})).unwrap();
    assert_eq!(r["queued"], false);
    let original = s.mailbox.message(id).unwrap();
    assert_eq!((original.answered, original.forwarded), (false, true));
    let sent = s.mailbox.message(r["id"].as_u64().unwrap()).unwrap();
    assert!(sent.answers.is_none(), "marked, so done with");
    assert_eq!(sent.subject, format!("FW: {MOCKUPS}"));
    assert!(matches!(sent.attachments.as_slice(), [eml] if eml.mime == "message/rfc822" && eml.name.ends_with(".eml")));
    // As it goes out, the attached message is in it whole.
    let out = mailcraft_mime::write(&s.loaded_message(sent.id).unwrap(), &mailcraft_mime::WriteOptions::default());
    let back = mailcraft_mime::parse(&out);
    assert!(matches!(back.attachments.as_slice(), [eml] if mailcraft_mime::parse(&eml.data).subject == MOCKUPS));
}
