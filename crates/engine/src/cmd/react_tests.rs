// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): a message answered with one emoji, as a short mail (RFC 9078) through the
//! account's normal sending.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::agent::{Class, Verdict, check, describe};
use crate::cmd::account::sync_request;
use crate::{CmdError, Session};

const THUMBS_UP: &str = "\u{1F44D}";

/// An IMAP account with one received message in its Inbox: the session, the account, the message.
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let received = Message {
        uid: 5,
        message_id: "plans@example.org".into(),
        subject: "Plans".into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@mail.example")],
        date: 100,
        body: Body { text: "Lunch?".into(), html: None },
        ..Default::default()
    };
    let delta = FolderDelta {
        id: None,
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen: 5,
        new: vec![received],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": SyncResult { account: a, folders: vec![delta], ..Default::default() }})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    (s, a, id)
}

fn react(s: &mut Session, id: Id, emoji: &str) -> Result<Value, CmdError> {
    s.run("mail.react", &json!({"id": id, "emoji": emoji}))
}

/// The first message of an Inbox in the sample mailbox that isn't from the owner.
fn first_received(s: &Session) -> Id {
    let from_owner = |m: &Message| s.mailbox.accounts.iter().any(|a| a.email.eq_ignore_ascii_case(&m.from.email));
    s.mailbox
        .messages
        .values()
        .filter(|m| !m.draft && !from_owner(m) && s.mailbox.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Inbox))
        .map(|m| m.id)
        .min()
        .expect("a received message in the sample")
}

/// The bytes that go out for message `id`.
fn written(s: &Session, id: Id) -> String {
    let m = s.mailbox.message(id).unwrap();
    String::from_utf8(mailcraft_mime_model::write(m, &mailcraft_mime_model::WriteOptions::default())).unwrap()
}

#[test]
fn a_reaction_is_queued_as_a_small_reply_with_a_reaction_part() {
    let (mut s, a, original) = imap();
    let r = react(&mut s, original, THUMBS_UP).unwrap();
    assert_eq!(r["queued"], true);
    let id = r["id"].as_u64().unwrap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.folder, outbox);
    assert!(m.reaction);
    assert_eq!(m.to, vec![Address::new("Bo", "bo@example.org")]);
    assert_eq!(m.subject, "RE: Plans");
    assert_eq!(m.in_reply_to, "plans@example.org");
    assert_eq!(m.references, vec!["plans@example.org".to_string()]);
    assert!(sync_request(&s, a).unwrap().outbox.iter().any(|m| m.id == id), "the next Send/Receive sends it");
    let bytes = written(&s, id);
    assert!(bytes.contains("In-Reply-To: <plans@example.org>\r\n"), "{bytes}");
    assert!(bytes.contains("References: <plans@example.org>\r\n"), "{bytes}");
    assert!(bytes.contains("Content-Type: multipart/alternative"), "{bytes}");
    assert!(bytes.contains("Content-Disposition: reaction\r\n"), "{bytes}");
    assert_eq!(bytes.matches("Content-Disposition: reaction").count(), 1);
    // The emoji is in the reaction part and again as plain text after it, as UTF-8.
    assert!(bytes.contains("Content-Type: text/plain; charset=utf-8"), "{bytes}");
    assert_eq!(bytes.matches("=F0=9F=91=8D").count(), 2, "{bytes}");
    // Nothing of the original is quoted, and the original is not marked answered by it.
    assert!(!bytes.contains("Lunch?"));
    assert!(!s.mailbox.message(original).unwrap().answered);
}

#[test]
fn only_the_six_emoji_are_sent_and_the_heart_in_one_form() {
    let (mut s, _, original) = imap();
    let before = s.mailbox.messages.len();
    for bad in [
        "",
        " ",
        "x",
        "\u{1F44E}",
        "\u{1F44D}\u{1F44D}",
        "\u{1F44D} \u{2764}",
        "\u{1F44D}<script>",
        "\u{1F600}",
        "thumbs up",
        "\u{1F44D}\r\nBcc: x@example.org",
    ] {
        let r = react(&mut s, original, bad);
        assert!(matches!(r, Err(CmdError::Params(_))), "{bad:?}: {r:?}");
    }
    for bad in [
        json!({"id": original}),
        json!({"id": original, "emoji": 7}),
        json!({"id": original, "emoji": null}),
        json!({"id": original, "emoji": THUMBS_UP, "to": "x@example.org"}),
    ] {
        assert!(matches!(s.run("mail.react", &bad), Err(CmdError::Params(_))), "{bad}");
    }
    assert_eq!(s.mailbox.messages.len(), before, "nothing was made");
    // A heart with or without its variation selector is the one heart.
    let heart = react(&mut s, original, "\u{2764}").unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(heart).unwrap().body.text, "\u{2764}\u{FE0F}");
    for (emoji, _) in crate::cmd::react::REACTIONS.iter().skip(2) {
        react(&mut s, original, emoji).unwrap();
    }
}

#[test]
fn drafts_and_the_owners_own_mail_are_refused() {
    let (mut s, a, _) = imap();
    let folder = |s: &Session, kind| s.mailbox.folder_of_kind(a, kind).unwrap();
    let make = |s: &mut Session, folder: Id, from: &str, draft: bool| {
        let m = Message {
            folder,
            message_id: format!("own{folder}@mail.example"),
            from: Address::new("", from),
            to: vec![Address::new("", "bo@example.org")],
            draft,
            ..Default::default()
        };
        s.mailbox.add_message(m).unwrap()
    };
    let sent = folder(&s, FolderKind::Sent);
    let drafts = folder(&s, FolderKind::Drafts);
    let inbox = folder(&s, FolderKind::Inbox);
    let own_sent = make(&mut s, sent, "ada@mail.example", false);
    let draft = make(&mut s, drafts, "ada@mail.example", true);
    // Mail from the owner's own address that sits in the Inbox (a note to self) is the owner's too.
    let to_self = make(&mut s, inbox, "ADA@mail.example", false);
    let before = s.mailbox.messages.len();
    for id in [own_sent, draft, to_self] {
        assert!(matches!(react(&mut s, id, THUMBS_UP), Err(CmdError::Failed(_))), "{id}");
    }
    assert_eq!(s.mailbox.messages.len(), before);
}

#[test]
fn junk_mail_mail_without_a_sender_or_an_id_is_refused() {
    let (mut s, a, _) = imap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let junk = s.mailbox.folder_of_kind(a, FolderKind::Junk).unwrap();
    let make = |s: &mut Session, folder: Id, from: &str, message_id: &str| {
        let m = Message { folder, message_id: message_id.into(), from: Address::new("", from), ..Default::default() };
        s.mailbox.add_message(m).unwrap()
    };
    let spam = make(&mut s, junk, "spam@example.org", "spam@example.org");
    let nobody = make(&mut s, inbox, "", "nobody@example.org");
    let broken = make(&mut s, inbox, "not an address", "broken@example.org");
    let no_id = make(&mut s, inbox, "bo@example.org", "");
    let before = s.mailbox.messages.len();
    for id in [spam, nobody, broken, no_id] {
        assert!(matches!(react(&mut s, id, THUMBS_UP), Err(CmdError::Failed(_))), "{id}");
    }
    assert!(matches!(react(&mut s, 999_999, THUMBS_UP), Err(CmdError::Said(_)) | Err(CmdError::Failed(_))));
    assert_eq!(s.mailbox.messages.len(), before);
}

/// Nothing Exchange-specific: Microsoft and Exchange server accounts (and one of a kind this version
/// doesn't know) are refused; IMAP, JMAP and local accounts work.
#[test]
fn microsoft_and_exchange_accounts_are_refused() {
    use mailcraft_model::AccountKind;
    for kind in [AccountKind::Microsoft, AccountKind::Exchange, AccountKind::Unknown] {
        let (mut s, a, id) = imap();
        s.mailbox.account_mut(a).unwrap().kind = kind;
        let before = s.mailbox.messages.len();
        assert!(matches!(react(&mut s, id, THUMBS_UP), Err(CmdError::Failed(_))), "{kind:?}");
        assert_eq!(s.mailbox.messages.len(), before, "{kind:?}");
        // The button is greyed by the same word.
        s.sel.messages = vec![id];
        assert!(!s.enabled("mail.react"), "{kind:?}");
    }
    for kind in [AccountKind::Imap, AccountKind::Jmap] {
        let (mut s, a, id) = imap();
        s.mailbox.account_mut(a).unwrap().kind = kind;
        s.sel.messages = vec![id];
        assert!(s.enabled("mail.react"), "{kind:?}");
        assert_eq!(react(&mut s, id, THUMBS_UP).unwrap()["queued"], true, "{kind:?}");
    }
}

/// A local account has nowhere to send: the reaction is filed in Sent Items.
#[test]
fn a_local_account_files_it_in_sent_items() {
    let mut s = Session::new(crate::sample::mailbox(1_791_993_600, 0));
    let m = first_received(&s);
    s.mailbox.message_mut(m).unwrap().message_id = "sample-1544@example.org".into();
    let account = s.mailbox.message(m).unwrap().account;
    let r = react(&mut s, m, "\u{1F389}").unwrap();
    assert_eq!(r["queued"], false);
    let sent = s.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    assert_eq!(s.mailbox.message(r["id"].as_u64().unwrap()).unwrap().folder, sent);
}

#[test]
fn the_same_reaction_twice_is_refused_another_is_not() {
    let (mut s, _, original) = imap();
    react(&mut s, original, THUMBS_UP).unwrap();
    let count = s.mailbox.messages.len();
    assert!(matches!(react(&mut s, original, THUMBS_UP), Err(CmdError::Said(_))));
    assert_eq!(s.mailbox.messages.len(), count);
    react(&mut s, original, "\u{2764}\u{FE0F}").unwrap();
    assert_eq!(s.mailbox.messages.len(), count + 1);
}

/// A Reply-To is never used: a message from the boss that names another domain there must not
/// tell that domain the address is read (the owner is not shown it).
#[test]
fn it_goes_to_the_sender_alone_whatever_the_reply_to_says() {
    let (mut s, _, original) = imap();
    let reply_to = vec![Address::new("", "ADA@mail.example"), Address::new("", "x@attacker.example"), Address::new("", "other@example.net")];
    s.mailbox.message_mut(original).unwrap().reply_to = reply_to;
    let id = react(&mut s, original, THUMBS_UP).unwrap()["id"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(m.to, vec![Address::new("Bo", "bo@example.org")], "the sender, not the Reply-To");
    assert!(m.cc.is_empty() && m.bcc.is_empty());
    assert!(!written(&s, id).contains("attacker.example"));
    // What the window shows beside the emoji is the same address.
    let shown = super::react::recipient(&s, s.mailbox.message(original).unwrap());
    assert_eq!(shown, Some(Address::new("Bo", "bo@example.org")));
}

/// A name that shows another address, a lookalike domain: a reaction would confirm to whoever is
/// behind it that the address is read (as `mail.unsubscribe` leaves it out).
#[test]
fn a_message_with_a_sender_warning_is_refused() {
    let (mut s, a, original) = imap();
    s.mailbox.message_mut(original).unwrap().from.name = "billing@bank.example".into();
    let m = s.mailbox.message(original).cloned().unwrap();
    assert_eq!(super::react::refusal(&mut s, &m), Some(super::react::Why::Warned));
    let err = react(&mut s, original, THUMBS_UP).unwrap_err();
    assert!(err.to_string().contains("warning about its sender"), "{err}");
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0, "nothing queued");
    // A warning about a link in the body, or the External tag, is not about the sender: an
    // ordinary HTML mail from a colleague is not refused for them.
    let (mut s, _, original) = imap();
    s.mailbox.message_mut(original).unwrap().body.html = Some(r#"<p><a href="https://collect.example/x">https://example.org/account</a></p>"#.into());
    let m = s.mailbox.message(original).cloned().unwrap();
    assert_eq!(super::react::refusal(&mut s, &m), None, "a link that goes elsewhere is no sender warning");
    assert!(react(&mut s, original, THUMBS_UP).is_ok());
    // The same message without the warning goes out.
    let (mut s, _, original) = imap();
    let m = s.mailbox.message(original).cloned().unwrap();
    assert_eq!(super::react::refusal(&mut s, &m), None);
}

#[test]
fn mail_from_a_list_goes_to_its_sender_not_to_the_list() {
    let (mut s, _, original) = imap();
    let m = s.mailbox.message_mut(original).unwrap();
    m.reply_to = vec![Address::new("", "list@example.org")];
    m.headers.push(("List-Id".into(), "<list.example.org>".into()));
    let id = react(&mut s, original, THUMBS_UP).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().to, vec![Address::new("Bo", "bo@example.org")]);
}

/// The copy a server files in Sent Items has lost the `reaction` mark: the same reaction is still one.
#[test]
fn the_same_reaction_twice_is_refused_after_a_sync_gave_back_a_copy_without_the_mark() {
    let (mut s, a, original) = imap();
    let id = react(&mut s, original, "\u{2764}\u{FE0F}").unwrap()["id"].as_u64().unwrap();
    // The server's copy, as it comes back from Sent Items: no mark, the text as it was read.
    let sent = s.mailbox.folder_of_kind(a, FolderKind::Sent).unwrap();
    let mut copy = s.mailbox.message(id).unwrap().clone();
    (copy.id, copy.folder, copy.reaction, copy.body.text) = (0, sent, false, "\u{2764}\n".into());
    s.mailbox.add_message(copy).unwrap();
    s.mailbox.remove_message(id).unwrap();
    assert!(!s.mailbox.messages.values().any(|m| m.reaction));
    let count = s.mailbox.messages.len();
    for same in ["\u{2764}\u{FE0F}", "\u{2764}"] {
        assert!(matches!(react(&mut s, original, same), Err(CmdError::Said(_))), "{same}");
    }
    assert_eq!(s.mailbox.messages.len(), count);
    // Another emoji is not held back by it.
    react(&mut s, original, THUMBS_UP).unwrap();
}

/// Mail a program wrote is not answered.
#[test]
fn automated_mail_is_refused() {
    let cases: [(&str, &[(&str, &str)], &str); 12] = [
        ("auto-generated", &[("Auto-Submitted", "auto-generated")], "bo@example.org"),
        ("auto-replied", &[("auto-submitted", "Auto-Replied; comment=x")], "bo@example.org"),
        ("bulk", &[("Precedence", "bulk")], "bo@example.org"),
        ("list", &[("Precedence", " List ")], "bo@example.org"),
        ("junk", &[("Precedence", "JUNK")], "bo@example.org"),
        ("noreply", &[], "noreply@example.org"),
        ("no-reply", &[], "No-Reply@shop.example.org"),
        ("no_reply plus", &[], "no_reply+abc@example.org"),
        ("do not reply", &[], "do-not-reply@example.org"),
        ("daemon", &[], "MAILER-DAEMON@example.org"),
        ("list address", &[("List-Post", "<mailto:Team@Lists.example.org>"), ("List-Id", "<team.lists.example.org>")], "team@lists.example.org"),
        ("list address with a subject", &[("List-Post", "<mailto:team@lists.example.org?subject=x>")], "team@lists.example.org"),
    ];
    for (name, headers, from) in cases {
        let (mut s, _, id) = imap();
        let m = s.mailbox.message_mut(id).unwrap();
        m.from = Address::new("X", from);
        m.headers.extend(headers.iter().map(|(n, v)| (n.to_string(), v.to_string())));
        let before = s.mailbox.messages.len();
        assert!(matches!(react(&mut s, id, THUMBS_UP), Err(CmdError::Failed(_))), "{name}");
        assert_eq!(s.mailbox.messages.len(), before, "{name}");
        s.sel.messages = vec![id];
        assert!(!s.enabled("mail.react"), "{name}: the button is greyed by the same rule");
    }
}

/// A person's mail that merely looks near those rules goes through.
#[test]
fn mail_from_a_person_is_not_taken_for_automated_mail() {
    let cases: [(&str, &[(&str, &str)], &str); 6] = [
        ("auto-submitted no", &[("Auto-Submitted", "no")], "bo@example.org"),
        ("empty", &[("Auto-Submitted", "")], "bo@example.org"),
        ("first-class", &[("Precedence", "first-class")], "bo@example.org"),
        ("a name with reply in it", &[], "reply.bo@example.org"),
        (
            "a list's mail from a member",
            &[("List-Id", "<team.lists.example.org>"), ("List-Post", "<mailto:team@lists.example.org>")],
            "bo@example.org",
        ),
        ("noted", &[], "nora@example.org"),
    ];
    for (name, headers, from) in cases {
        let (mut s, _, id) = imap();
        let m = s.mailbox.message_mut(id).unwrap();
        m.from = Address::new("X", from);
        m.headers.extend(headers.iter().map(|(n, v)| (n.to_string(), v.to_string())));
        assert!(react(&mut s, id, THUMBS_UP).is_ok(), "{name}");
    }
}

/// The owner's Undo Send wait applies as it does to any sent message.
#[test]
fn it_waits_as_long_as_undo_send_says_and_can_be_taken_back() {
    const T0: i64 = 1_800_000_000;
    let (mut s, a, original) = imap();
    s.clock.now = Some(T0);
    s.run("sync.waitBeforeSending", &json!({"seconds": 10})).unwrap();
    let r = react(&mut s, original, THUMBS_UP).unwrap();
    let id = r["id"].as_u64().unwrap();
    assert_eq!((r["wait"].clone(), r["sendAt"].clone()), (json!(true), json!(T0 + 10)));
    let m = s.mailbox.message(id).unwrap();
    assert_eq!((m.send_at, m.date, m.received), (Some(T0 + 10), T0 + 10, T0 + 10));
    assert!(m.reaction);
    assert!(sync_request(&s, a).unwrap().outbox.iter().all(|m| m.id != id), "not before the wait is over");
    assert_eq!(s.undo_send_wait().map(|(i, _)| i), Some(id), "the Undo bar offers it");
    // Taken back it is an ordinary draft in a composer.
    let back = s.run("compose.undoSend", &json!({})).unwrap();
    assert_eq!(back["composing"], true);
    let drafts = s.mailbox.folder_of_kind(a, FolderKind::Drafts).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(m.folder == drafts && m.draft && !m.reaction && m.send_at.is_none());
    s.clock.now = Some(T0 + 100);
    assert!(sync_request(&s, a).unwrap().outbox.iter().all(|m| m.id != id), "it never goes");
    // Another one, left alone, goes when the wait is over.
    s.compose = None;
    let id = react(&mut s, original, THUMBS_UP).unwrap()["id"].as_u64().unwrap();
    s.clock.now = Some(T0 + 111);
    assert!(sync_request(&s, a).unwrap().outbox.iter().any(|m| m.id == id));
    assert_eq!(s.undo_send_wait(), None);
}

#[test]
fn with_no_wait_set_it_goes_at_once_and_a_local_account_never_waits() {
    let (mut s, a, original) = imap();
    let r = react(&mut s, original, THUMBS_UP).unwrap();
    assert_eq!((r["wait"].clone(), r["sendAt"].clone()), (json!(false), Value::Null));
    assert!(s.undo_send_wait().is_none());
    assert!(sync_request(&s, a).unwrap().outbox.iter().any(|m| m.id == r["id"].as_u64().unwrap()));
    let mut local = Session::new(crate::sample::mailbox(1_791_993_600, 0));
    local.run("sync.waitBeforeSending", &json!({"seconds": 10})).unwrap();
    let m = first_received(&local);
    local.mailbox.message_mut(m).unwrap().message_id = "sample-1544@example.org".into();
    let r = react(&mut local, m, THUMBS_UP).unwrap();
    assert_eq!((r["queued"].clone(), r["wait"].clone()), (json!(false), json!(false)));
    assert!(local.undo_send_wait().is_none());
}

/// A guard only: composing is separate state that React doesn't read or write. It passes without much
/// of the change; the tests above are what prove the behaviour.
#[test]
fn a_message_being_written_is_left_alone() {
    let (mut s, _, original) = imap();
    s.run("mail.new", &json!({"to": "cy@example.org", "subject": "Draft in progress", "body": "Hello"})).unwrap();
    let before = s.compose.clone().unwrap();
    react(&mut s, original, THUMBS_UP).unwrap();
    let after = s.compose.clone().unwrap();
    assert_eq!((after.to, after.subject, after.dirty), (before.to, before.subject, before.dirty));
}

#[test]
fn a_reaction_taken_back_to_drafts_is_an_ordinary_message() {
    let (mut s, _, original) = imap();
    let id = react(&mut s, original, THUMBS_UP).unwrap()["id"].as_u64().unwrap();
    s.run("mail.cancelSend", &json!({"id": id})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert!(!m.reaction && m.draft);
    assert!(!written(&s, id).contains("Content-Disposition: reaction"));
}

/// An agent sends it only when the owner says so, and names the message.
#[test]
fn an_agent_is_asked_before_it_reacts_and_must_name_the_message() {
    let (mut s, _, original) = imap();
    let params = json!({"id": original, "emoji": THUMBS_UP});
    assert!(matches!(check(&s, "mail.react", &params, &[]), Verdict::Ask(Class::Send, _)));
    assert_eq!(check(&s, "mail.react", &params, &[Class::Send]), Verdict::Run);
    s.sel.messages = vec![original];
    assert!(matches!(check(&s, "mail.react", &json!({"emoji": THUMBS_UP}), &[Class::Send]), Verdict::Refuse(_)));
    let said = describe(&s, "mail.react", &params);
    assert!(said.contains(THUMBS_UP) && said.contains("Plans"), "{said}");
}
