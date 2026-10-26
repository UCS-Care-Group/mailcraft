// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Forward and Forward as Attachment with several messages (#360): one new message with each
//! attached whole as an `.eml`, in the message list's order, from the account of the one in focus;
//! refused, naming the message, when one of them can't go whole; at most 200 messages and 64 MB
//! together, judged before any is read in; and each original marked forwarded once it has gone.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::{Answers, Attachment, Body, ContentRef, Id, KeptBody, Message, NOT_DOWNLOADED};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::{ComposeMode, Session, sample};

const MOCKUPS: &str = "Homepage mockups, round three";
const ROADMAP: &str = "Q4 roadmap review moved to Thursday";
const INVOICE: &str = "Your October invoice is ready";

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn by_subject(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().id
}

/// The three Inbox messages, in the order the list shows them (newest first).
fn three(s: &mut Session) -> [Id; 3] {
    let ids = [MOCKUPS, ROADMAP, INVOICE].map(|subject| by_subject(s, subject));
    let rows = s.rows();
    let at = |id: Id| crate::list::position(&rows, id).unwrap();
    assert!(at(ids[0]) < at(ids[1]) && at(ids[1]) < at(ids[2]), "the sample lists them in this order");
    ids
}

/// The message being written's attachments: their names and types, and each parsed back.
fn attached(s: &Session) -> Vec<(String, String, Message)> {
    s.compose.as_ref().unwrap().attachments.iter().map(|a| (a.name.clone(), a.mime.clone(), mailcraft_mime::parse(&a.data))).collect()
}

/// Why the command is greyed, if it is.
fn greyed(s: &Session, id: &str) -> Option<&'static str> {
    (s.registry.get(id).unwrap().enabled)(s)
}

/// The selection as clicks make it: the last one is the one in focus.
fn select(s: &mut Session, ids: &[Id]) {
    s.run("mail.select", &json!({"ids": ids})).unwrap();
    assert_eq!(s.sel.focus(), ids.last().copied());
}

#[test]
fn several_go_each_whole_as_an_eml_in_the_lists_order() {
    for command in ["mail.forwardAsAttachment", "mail.forward"] {
        let mut s = session();
        let [a, b, c] = three(&mut s);
        // Clicked in another order than the list's, the focus on the middle one.
        select(&mut s, &[c, a, b]);
        let r = s.run(command, &json!({})).unwrap();
        assert_eq!(r, json!({"composing": true, "mode": "forward"}), "{command}");
        let compose = s.compose.as_ref().unwrap();
        assert_eq!((compose.mode, compose.source, compose.also.as_slice()), (ComposeMode::Forward, Some(b), &[a, c][..]), "{command}");
        assert!(compose.as_attachment, "{command}");
        assert_eq!(compose.account, s.mailbox.message(b).unwrap().account);
        assert!(compose.to.is_empty() && compose.cc.is_empty() && compose.bcc.is_empty());
        assert!(compose.quote.is_none() && compose.in_reply_to.is_empty() && compose.references.is_empty(), "{command}: nothing quoted");
        // No subject is chosen for them: the owner writes it.
        assert_eq!(compose.subject, "", "{command}");
        // A line to write on, then the signature for replies and forwards, and nothing of them.
        let signature = s.mailbox.accounts[0].reply_signature().to_string();
        assert_eq!(compose.signature.as_ref().map(|sig| sig.name.as_str()), Some(signature.as_str()));
        assert!(compose.body.plain().starts_with('\n') && !compose.body.plain().contains("Round three is attached"));
        // Each whole, as it came, in the list's order.
        let files = attached(&s);
        assert_eq!(files.len(), 3, "{command}");
        for ((name, mime, back), id) in files.iter().zip([a, b, c]) {
            let original = s.loaded_message(id).unwrap();
            assert_eq!(mime, "message/rfc822");
            assert_eq!(name, &crate::cmd::file::eml_name(&original));
            assert_eq!((&back.subject, &back.message_id, back.date), (&original.subject, &original.message_id, original.date));
            assert_eq!((&back.from, &back.to), (&original.from, &original.to));
            assert_eq!(back.body.text.trim_end(), original.body.text.trim_end());
            let parts = |m: &Message| m.attachments.iter().map(|a| (a.name.clone(), a.data.to_vec())).collect::<Vec<_>>();
            assert_eq!(parts(back), parts(&original));
        }
        // Nothing is marked yet: nothing has gone.
        assert!([a, b, c].iter().all(|id| !s.mailbox.message(*id).unwrap().forwarded));
    }
}

#[test]
fn one_message_is_forwarded_as_before() {
    let mut s = session();
    let id = by_subject(&s, MOCKUPS);
    select(&mut s, &[id]);
    s.run("mail.forward", &json!({})).unwrap();
    let c = s.compose.as_ref().unwrap();
    assert!(!c.as_attachment && c.also.is_empty());
    assert!(c.body.plain().contains("Round three is attached"), "quoted");
    assert_eq!(c.subject, format!("FW: {MOCKUPS}"));
    let files = |m: &Message| m.attachments.iter().map(|a| a.name.clone()).collect::<Vec<_>>();
    assert_eq!(
        files(&Message { attachments: c.attachments.clone(), ..Default::default() }),
        files(s.mailbox.message(id).unwrap()),
        "its files carried"
    );
    // Named by `id`, it is that one whatever is selected; by `ids`, one is one too.
    let [a, b, _] = three(&mut s);
    select(&mut s, &[a, b]);
    for params in [json!({"id": a}), json!({"ids": [a]}), json!({"ids": [a, a]})] {
        s.run("mail.forward", &params).unwrap();
        let c = s.compose.as_ref().unwrap();
        assert_eq!((c.source, c.as_attachment), (Some(a), false), "{params}");
        s.run("mail.forwardAsAttachment", &params).unwrap();
        assert_eq!(attached(&s).len(), 1, "{params}");
    }
}

#[test]
fn named_messages_go_in_the_lists_order_from_the_last_ones_account() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    // Another account's message, which the list in view doesn't show, and the focus.
    let other = s.run("account.add", &json!({"email": "kit@example.org", "kind": "local"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(other, mailcraft_model::FolderKind::Inbox).unwrap();
    let elsewhere = s
        .mailbox
        .add_message(Message {
            account: other,
            folder: inbox,
            subject: "Elsewhere".into(),
            message_id: "elsewhere@example.org".into(),
            body: Body { text: "Kept apart.".into(), html: None },
            ..Default::default()
        })
        .unwrap();
    s.run("mail.forwardAsAttachment", &json!({"ids": [elsewhere, c, a, b, c]})).unwrap();
    let compose = s.compose.as_ref().unwrap();
    // Each once; `c` named last of them all, but `b` is the last of the list's after it.
    assert_eq!(compose.source, Some(c));
    let subjects: Vec<String> = attached(&s).into_iter().map(|(_, _, m)| m.subject).collect();
    assert_eq!(subjects, [MOCKUPS, ROADMAP, INVOICE, "Elsewhere"], "the list's order, then what it doesn't show");
    assert_eq!(compose.also, [a, b, elsewhere]);
    // Written from the account of the one named last.
    s.run("mail.forward", &json!({"ids": [a, elsewhere]})).unwrap();
    let compose = s.compose.as_ref().unwrap();
    assert_eq!((compose.source, compose.account), (Some(elsewhere), other));
}

#[test]
fn ids_named_wrongly_are_refused_whatever_is_selected() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    select(&mut s, &[a, c]);
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        // One entry that isn't an id refuses them all: the selected ones aren't taken instead.
        for junk in [json!([b, "7", a]), json!([b, 0]), json!([b, -1]), json!([b, 1.5]), json!([b, null]), json!([b, {"id": c}]), json!([b, [a]])] {
            let e = s.run(command, &json!({"ids": junk})).unwrap_err().to_string();
            assert!(e.contains("`ids` is a list of"), "{command} {junk}: {e}");
            assert!(s.compose.is_none(), "{command} {junk}");
        }
        // More than a command takes is refused before any is looked at.
        let e = s.run(command, &json!({"ids": vec![b; 100_001]})).unwrap_err().to_string();
        assert!(e.contains("`ids` is a list of"), "{command}: {e}");
        // One that is no message is refused, and nothing is written.
        let e = s.run(command, &json!({"ids": [a, 999_999, b]})).unwrap_err().to_string();
        assert!(e.contains("no message with id 999999"), "{command}: {e}");
        assert!(s.compose.is_none(), "{command}");
        // Named rightly, they go, whatever is selected.
        s.run(command, &json!({"ids": [b, a]})).unwrap();
        let compose = s.compose.as_ref().unwrap();
        assert_eq!((compose.source, compose.also.as_slice()), (Some(a), &[b][..]), "{command}");
        s.compose = None;
    }
}

#[test]
fn several_have_no_subject_for_the_owner_to_write_one() {
    let mut s = session();
    let inbox = by_subject(&s, "Release checklist");
    let sent = by_subject(&s, "RE: Release checklist");
    // Not even when they are all about the same.
    for command in ["mail.forwardAsAttachment", "mail.forward"] {
        select(&mut s, &[sent, inbox]);
        s.run(command, &json!({})).unwrap();
        assert_eq!(s.compose.as_ref().unwrap().subject, "", "{command}");
        s.run(command, &json!({"ids": [inbox, by_subject(&s, MOCKUPS), sent]})).unwrap();
        assert_eq!(s.compose.as_ref().unwrap().subject, "", "{command}");
    }
    // One alone has its own, as before.
    s.run("mail.forwardAsAttachment", &json!({"ids": [inbox]})).unwrap();
    assert_eq!(s.compose.as_ref().unwrap().subject, "FW: Release checklist");
}

#[test]
fn two_of_one_subject_are_two_files_of_their_own() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    for id in [a, b, c] {
        s.mailbox.message_mut(id).unwrap().subject = "Budget".into();
    }
    s.mailbox.message_mut(c).unwrap().subject = "budget".into();
    s.run("mail.forwardAsAttachment", &json!({"ids": [a, b, c]})).unwrap();
    let names: Vec<String> = attached(&s).into_iter().map(|(name, ..)| name).collect();
    assert_eq!(names, ["Budget.eml", "Budget (2).eml", "budget (3).eml"]);
}

#[test]
fn one_that_cant_go_whole_stops_them_all_and_is_named() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    let whole = s.mailbox.message(b).unwrap().clone();
    let refused = |s: &mut Session, change: &dyn Fn(&mut Message), says: &str| {
        *s.mailbox.message_mut(b).unwrap() = whole.clone();
        change(s.mailbox.message_mut(b).unwrap());
        let before = s.mailbox.clone();
        for command in ["mail.forward", "mail.forwardAsAttachment"] {
            let e = s.run(command, &json!({"ids": [a, b, c]})).unwrap_err().to_string();
            assert!(e.contains(says) && e.contains(&format!("“{ROADMAP}”")), "{command}: {e}");
            assert!(s.compose.is_none(), "{command} {says}: nothing is being written");
            assert!(s.mailbox == before, "{command} {says}: the mailbox is as it was");
        }
    };
    // Of a Microsoft account, only its headers and a preview, waiting to be fetched.
    refused(
        &mut s,
        &|m| {
            (m.unfetched, m.server) = (true, Some(ServerMessage { id: "AAMk1".into(), ..Default::default() }));
            m.body = Body { text: "Moved to Thursday".into(), html: None };
        },
        "isn't downloaded yet",
    );
    // Its server's copy went before it could be fetched.
    refused(&mut s, &|m| (m.unfetched, m.incomplete) = (true, true), "only the headers");
    // Its text can't be read from where the mailbox keeps it.
    refused(
        &mut s,
        &|m| {
            m.body = Body { text: "[This message's text is unavailable]".into(), html: None };
            m.kept = Some(Box::new(KeptBody { text: Some(ContentRef { sha256: [7; 32], len: 900 }), failed: true, ..Default::default() }));
        },
        "can't be read on this computer",
    );
    // Too large to download (IMAP), as a file from before #300 has it and as one since does.
    refused(
        &mut s,
        &|m| {
            m.body = Body { text: "This message is too large to download: it is 61 MB on the server.".into(), html: None };
            m.headers.push((NOT_DOWNLOADED.into(), "62914561".into()));
        },
        "too large to download",
    );
    refused(&mut s, &|m| m.not_downloaded = 30 << 20, "too large to download");
    // A subject far too long to say, or none, is named as far as it can be.
    *s.mailbox.message_mut(b).unwrap() = Message { incomplete: true, subject: format!("Line one\r\n{}", "x".repeat(500)), ..whole.clone() };
    let e = s.run("mail.forward", &json!({"ids": [a, b]})).unwrap_err().to_string();
    assert!(e.contains(&format!("“Line one {}…”", "x".repeat(71))), "{e}");
    s.mailbox.message_mut(b).unwrap().subject = " \t".into();
    let e = s.run("mail.forward", &json!({"ids": [a, b]})).unwrap_err().to_string();
    assert!(e.contains("“(no subject)”"), "{e}");
}

#[test]
fn more_than_200_are_refused_before_any_is_read_in() {
    let mut s = session();
    let [a, b, _] = three(&mut s);
    let mut copies = Vec::new();
    for k in 0..200 {
        let m = Message { id: 0, subject: format!("Copy {k}"), ..s.mailbox.message(a).unwrap().clone() };
        copies.push(s.mailbox.add_message(m).unwrap());
    }
    // One whose file is kept beside the mailbox and isn't there: read in, it would say so.
    let unread = Attachment { kept: Some(ContentRef { sha256: [9; 32], len: 1_000 }), ..Attachment::new("scan.tiff", "image/tiff", Vec::new()) };
    s.mailbox.message_mut(b).unwrap().attachments.push(unread);
    let mut many = copies.clone();
    many.push(b);
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        let e = s.run(command, &json!({"ids": many})).unwrap_err().to_string();
        assert!(e.contains("at most 200 messages"), "{command}: {e}");
        // 200 are as many as one message carries: those are read in.
        let e = s.run(command, &json!({"ids": many.get(1..).unwrap()})).unwrap_err().to_string();
        assert!(!e.contains("at most 200"), "{command}: {e}");
        assert!(s.compose.is_none());
    }
    // A message named twice is one of them.
    let mut twice = copies.clone();
    twice.push(copies[0]);
    s.run("mail.forwardAsAttachment", &json!({"ids": twice})).unwrap();
    assert_eq!(attached(&s).len(), 200);
    // Selected, the commands are greyed, saying why, and refused all the same.
    s.compose = None;
    s.sel.messages = many;
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        assert_eq!(greyed(&s, command), Some("at most 200 messages can be forwarded together"), "{command}");
        let e = s.run(command, &json!({})).unwrap_err().to_string();
        assert!(e.contains("at most 200 messages"), "{command}: {e}");
    }
    s.sel.messages.pop();
    assert_eq!(greyed(&s, "mail.forwardAsAttachment"), None);
    s.run("mail.forwardAsAttachment", &json!({})).unwrap();
    assert_eq!(attached(&s).len(), 200);
}

#[test]
fn over_64_mb_together_is_refused_before_any_is_read_in() {
    let mut s = session();
    let [a, b, c] = three(&mut s);
    // Files of 30 MB kept beside the mailbox, which aren't there to read: never asked for.
    for id in [a, b, c] {
        let big =
            Attachment { kept: Some(ContentRef { sha256: [id as u8; 32], len: 30 << 20 }), ..Attachment::new("scan.tiff", "image/tiff", Vec::new()) };
        s.mailbox.message_mut(id).unwrap().attachments.push(big);
    }
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        let e = s.run(command, &json!({"ids": [a, b, c]})).unwrap_err().to_string();
        assert!(e.contains("larger than 64 MB together"), "{command}: {e}");
        assert!(s.compose.is_none());
    }
    // Two of them are under it, so they are read in: the files that aren't there say so.
    let e = s.run("mail.forward", &json!({"ids": [a, b]})).unwrap_err().to_string();
    assert!(!e.contains("64 MB"), "{e}");
    // What the server said of one not fetched yet counts, before it is fetched.
    assert!(super::compose::forward_check(&s, &[a, b], true).is_ok());
    let m = s.mailbox.message_mut(b).unwrap();
    (m.unfetched, m.server) = (true, Some(ServerMessage { id: "AAMk2".into(), size: 40 << 20, ..Default::default() }));
    m.attachments.clear();
    let e = super::compose::forward_check(&s, &[a, b], true).unwrap_err().to_string();
    assert!(e.contains("larger than 64 MB together"), "{e}");
    // Waiting to be fetched, it is refused for that unless the window fetches it first.
    let e = super::compose::forward_check(&s, &[c, b], false).unwrap_err().to_string();
    assert!(e.contains("isn't downloaded yet"), "{e}");
}

#[test]
fn over_64_mb_together_as_written_out_is_refused() {
    let mut s = session();
    let [a, b, _] = three(&mut s);
    // 49 MB of files as kept, under the limit; written out (base64) they come to more than 64 MB.
    for id in [a, b] {
        let scan = Attachment::new("scan.tiff", "image/tiff", vec![7; 49 << 19]);
        s.mailbox.message_mut(id).unwrap().attachments.push(scan);
    }
    assert!(super::compose::forward_check(&s, &[a, b], false).is_ok(), "under it as they are kept");
    let e = s.run("mail.forwardAsAttachment", &json!({"ids": [a, b]})).unwrap_err().to_string();
    assert!(e.contains("larger than 64 MB together"), "{e}");
    assert!(s.compose.is_none());
}

#[test]
fn each_original_is_marked_forwarded_once_it_has_gone() {
    for command in ["mail.forward", "mail.forwardAsAttachment"] {
        let mut s = session();
        let [a, b, c] = three(&mut s);
        for id in [a, b, c] {
            let m = s.mailbox.message_mut(id).unwrap();
            (m.answered, m.forwarded) = (false, false);
        }
        select(&mut s, &[a, c]);
        s.run(command, &json!({})).unwrap();
        s.run("compose.set", &json!({"to": "help@example.org", "subject": "Two of them"})).unwrap();
        // Kept as a draft first: it still knows them all.
        let draft = s.run("compose.saveDraft", &json!({})).unwrap()["draft"].as_u64().unwrap();
        let answers = s.mailbox.message(draft).unwrap().answers.clone().unwrap();
        assert_eq!((answers.message, answers.forward), (c, true), "{command}");
        assert_eq!(answers.also.iter().map(|o| (o.message, o.forward)).collect::<Vec<_>>(), [(a, true)], "{command}");
        // Closed and opened again, it is sent from the draft, which says what it forwards.
        s.run("compose.close", &json!({})).unwrap();
        s.run("mail.editDraft", &json!({"id": draft})).unwrap();
        assert!(s.compose.as_ref().is_some_and(|c| c.source.is_none()), "{command}");
        // The sample's account is on this computer: what is sent is filed as sent at once.
        let r = s.run("compose.send", &json!({})).unwrap();
        assert_eq!(r["queued"], false);
        for (id, forwarded) in [(a, true), (b, false), (c, true)] {
            let m = s.mailbox.message(id).unwrap();
            assert_eq!((m.answered, m.forwarded), (false, forwarded), "{command}: {}", m.subject);
        }
        let sent = s.mailbox.message(r["id"].as_u64().unwrap()).unwrap();
        assert!(sent.answers.is_none(), "{command}: marked, so done with");
        // As it goes out, both are in it whole.
        let out = mailcraft_mime::write(&s.loaded_message(sent.id).unwrap(), &mailcraft_mime::WriteOptions::default());
        let subjects: Vec<String> = mailcraft_mime::parse(&out).attachments.iter().map(|eml| mailcraft_mime::parse(&eml.data).subject).collect();
        assert_eq!(subjects, [MOCKUPS, INVOICE], "{command}");
    }
}

#[test]
fn a_forward_of_several_is_kept_in_the_mailbox_file_and_an_older_one_still_reads() {
    let answers = Answers {
        message: 7,
        message_id: "seven@example.org".into(),
        forward: true,
        also: vec![Answers { message: 9, message_id: "nine@example.org".into(), forward: true, also: Vec::new() }],
    };
    let m = Message { answers: Some(answers.clone()), ..Default::default() };
    let text = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<Message>(&text).unwrap().answers, Some(answers));
    // What a build from before wrote, and wrong entries among the others, read.
    let old: Message = serde_json::from_str(r#"{"answers": {"message": 7, "messageId": "seven@example.org", "forward": true}}"#).unwrap();
    assert_eq!(old.answers.as_ref().map(|a| (a.message, a.also.len())), Some((7, 0)));
    let odd: Message =
        serde_json::from_str(r#"{"answers": {"message": 7, "forward": true, "also": [{"message": 9, "forward": true}, "x", 5]}}"#).unwrap();
    assert_eq!(odd.answers.unwrap().also.iter().map(|a| a.message).collect::<Vec<_>>(), [9]);
    let worse: Message = serde_json::from_str(r#"{"answers": {"message": 7, "also": "x"}}"#).unwrap();
    assert!(worse.answers.unwrap().also.is_empty());
    // A reply or a forward of one writes nothing new.
    let one = Message { answers: Some(Answers { message: 7, forward: true, ..Default::default() }), ..Default::default() };
    assert!(!serde_json::to_string(&one).unwrap().contains("also"));
}
