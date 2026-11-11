// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The rest of Outlook's conditions, exceptions and actions (#566): what `rules.add` takes and
//! refuses, and what a rule does with the mail it finds.

use mailcraft_model::{Action, Address, Condition, FlagStatus, FolderKind, FollowUp, Id, Importance, Message, Sensitivity};
use serde_json::{Value, json};

use crate::Session;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const ME: &str = "me@example.org";

/// A session on this computer at [`NOW`], with an account of this computer; its id, its Inbox's
/// and the id of a folder "Kept" of its own.
fn session() -> (Session, Id, Id, Id) {
    let mut s = Session::at(Default::default(), NOW, 0);
    let account = s.run("account.add", &json!({"email": ME})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let kept = s.mailbox.add_folder(account, None, "Kept").unwrap();
    (s, account, inbox, kept)
}

fn arriving(s: &mut Session, inbox: Id, from: &str, subject: &str) -> Id {
    let to = vec![Address::new("Me", ME)];
    let categories = vec!["Red category".to_string()];
    let m = Message { folder: inbox, from: Address::new("", from), to, subject: subject.into(), categories, ..Default::default() };
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn every_new_part_is_taken_and_one_of_the_wrong_type_is_refused() {
    let (mut s, account, _, kept) = session();
    let outbox = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let rule = |conditions: Value, exceptions: Value, actions: Value| json!({"name": "Club", "account": account, "conditions": conditions, "exceptions": exceptions, "actions": actions});
    let conditions = json!([
        {"type": "from", "value": ["Ada@Example.org", "ada@example.org", "noor@example.org"]},
        {"type": "sentTo", "value": "team@example.org"},
        {"type": "subjectOrBodyContains", "value": "agenda"},
        {"type": "headerContains", "value": "X-Club: yes"},
        {"type": "sizeBetween", "value": {"min": 1, "max": 100}},
        {"type": "receivedBetween", "value": {"to": "2026-10-31"}},
        {"type": "sensitivity", "value": "company-confidential"},
        {"type": "category", "value": ["Red category"]},
        {"type": "sentOnlyToMe"}, {"type": "toMe"}, {"type": "ccMe"}, {"type": "toOrCcMe"}, {"type": "notToMe"},
        {"type": "meetingRequest"}, {"type": "automaticReply"}, {"type": "flagged"},
    ]);
    // Exceptions of one kind are one, as a server keeps them.
    let exceptions = json!([
        {"type": "subjectContains", "value": "opt out"}, {"type": "subjectContains", "value": ["Unsubscribe", "OPT OUT"]},
        {"type": "hasAttachment"}, {"type": "hasAttachment"}, {"type": "importance", "value": "low"},
    ]);
    let actions = json!([
        {"type": "copyTo", "value": kept}, {"type": "flagFor", "value": "nextWeek"}, {"type": "clearFlag"},
        {"type": "clearCategories"}, {"type": "deleteForGood"},
    ]);
    let id = s.run("rules.add", &rule(conditions.clone(), exceptions, actions.clone())).unwrap()["id"].as_u64().unwrap();
    let r = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
    assert_eq!(r.conditions.len(), 16);
    // The same address once, as it was first written.
    assert_eq!(r.conditions[0], Condition::From(vec!["Ada@Example.org".into(), "noor@example.org".into()]));
    assert_eq!(r.conditions[6], Condition::Sensitivity(Sensitivity::Confidential));
    assert_eq!(
        r.exceptions,
        [Condition::SubjectContains(vec!["opt out".into(), "Unsubscribe".into()]), Condition::HasAttachment, Condition::Importance(Importance::Low)]
    );
    assert_eq!(
        r.actions,
        [Action::CopyTo(kept), Action::FlagFor(FollowUp::NextWeek), Action::ClearFlag, Action::ClearCategories, Action::DeleteForGood]
    );
    // A desktop alert is something to do by itself; nothing at all is not.
    let alert = json!({"name": "Boss", "conditions": [{"type": "from", "value": "boss@example.org"}], "actions": [], "alert": true});
    let boss = s.run("rules.add", &alert).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.rules.iter().any(|r| r.id == boss && r.alert && r.actions.is_empty()));
    let before = serde_json::to_value(&s.mailbox).unwrap();
    let mut wrong: Vec<Value> = [
        json!({"type": "from", "value": "Noor"}),
        json!({"type": "from", "value": "<ada@example.org>"}),
        json!({"type": "sentTo", "value": ["ada@example.org", "Noor Smith"]}),
        json!({"type": "subjectContains", "value": 7}),
        json!({"type": "subjectContains", "value": []}),
        json!({"type": "subjectContains", "value": ["ok", 3]}),
        json!({"type": "subjectContains", "value": "  "}),
        json!({"type": "sizeBetween", "value": {"min": 5, "max": 1}}),
        json!({"type": "sizeBetween", "value": {"min": "5", "max": 10}}),
        json!({"type": "sizeBetween", "value": {"min": -1, "max": 10}}),
        // More than a Microsoft account's server keeps.
        json!({"type": "sizeBetween", "value": {"min": 1, "max": 2_147_483_648_u64}}),
        json!({"type": "sizeBetween"}),
        json!({"type": "receivedBetween", "value": {}}),
        json!({"type": "receivedBetween", "value": {"from": "2026-11-02", "to": "2026-11-01"}}),
        json!({"type": "receivedBetween", "value": {"from": "yesterday"}}),
        json!({"type": "receivedBetween", "value": "2026-11-01"}),
        json!({"type": "sensitivity", "value": "secret"}),
        json!({"type": "category", "value": ""}),
        json!({"type": "category", "value": 1}),
    ]
    .into_iter()
    .map(|c| rule(json!([c]), json!([]), actions.clone()))
    .collect();
    wrong.extend([
        rule(conditions.clone(), json!("not a list"), actions.clone()),
        rule(conditions.clone(), json!([{"type": "from", "value": "Noor"}]), actions.clone()),
        rule(conditions.clone(), json!([]), json!([{"type": "flagFor", "value": "someday"}])),
        rule(conditions.clone(), json!([]), json!([{"type": "flagFor"}])),
        rule(conditions.clone(), json!([]), json!([{"type": "copyTo", "value": "1"}])),
        rule(conditions.clone(), json!([]), json!([{"type": "copyTo", "value": outbox}])),
        rule(conditions.clone(), json!([]), json!([{"type": "copyTo", "value": 999_999}])),
        rule(conditions.clone(), json!([]), json!([])),
        json!({"name": "Boss", "conditions": [{"type": "flagged"}], "actions": [], "alert": "yes"}),
        json!({"name": "Boss", "conditions": [{"type": "flagged"}], "actions": [], "alert": false}),
    ]);
    for params in wrong {
        assert!(s.run("rules.add", &params).is_err(), "{params}");
    }
    assert_eq!(serde_json::to_value(&s.mailbox).unwrap(), before);
}

#[test]
fn more_words_or_exceptions_than_a_rule_keeps_are_refused_not_cut() {
    let (mut s, account, _, _) = session();
    let many = |n: usize| (0..n).map(|i| format!("w{i}@example.org")).collect::<Vec<_>>();
    let rule = |conditions: Value, exceptions: Value| json!({"name": "Club", "account": account, "conditions": conditions, "exceptions": exceptions, "actions": [{"type": "deleteForGood"}]});
    let plain = json!([{"type": "toMe"}]);
    let tried = [
        rule(json!([{"type": "from", "value": many(21)}]), json!([])),
        rule(plain.clone(), json!([{"type": "from", "value": many(21)}])),
        // Each is short enough; together, as a server keeps them, they aren't.
        rule(plain.clone(), json!([{"type": "from", "value": many(15)}, {"type": "from", "value": many(30)[15..].to_vec()}])),
        rule(plain.clone(), Value::Array((0..21).map(|i| json!({"type": "subjectContains", "value": format!("w{i}")})).collect())),
        // Nor more conditions or actions: one without a condition acts on more mail.
        rule(Value::Array((0..21).map(|i| json!({"type": "subjectContains", "value": format!("w{i}")})).collect()), json!([])),
        json!({"name": "Club", "account": account, "conditions": plain, "actions": vec![json!({"type": "markRead"}); 21]}),
    ];
    let before = s.mailbox.rules.len();
    for params in tried {
        assert!(s.run("rules.add", &params).is_err(), "{params}");
    }
    assert_eq!(s.mailbox.rules.len(), before);
    // As many as it keeps go in.
    s.run("rules.add", &rule(plain.clone(), json!([{"type": "from", "value": many(20)}]))).unwrap();
    let twenty = Value::Array((0..20).map(|i| json!({"type": "subjectContains", "value": format!("w{i}")})).collect());
    let id = s.run("rules.add", &rule(twenty, json!([]))).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.rules.iter().find(|r| r.id == id).map(|r| r.conditions.len()), Some(20));
}

/// An address up to the 254 characters an address can have is looked for whole, never cut into
/// somebody else's; a longer one is refused (#566).
#[test]
fn a_long_address_is_looked_for_whole_and_a_longer_one_is_refused() {
    let (mut s, account, _, _) = session();
    let long = format!("a@{}.{}.{}.{}.example", "b".repeat(60), "c".repeat(60), "d".repeat(60), "e".repeat(30));
    assert!(long.len() > 200 && long.len() <= 254);
    let longer = format!("a@{}.{}", "f".repeat(40), long.trim_start_matches("a@"));
    assert!(longer.len() > 254);
    for kind in ["from", "sentTo"] {
        let rule = |value: &str| json!({"name": "Long", "account": account, "conditions": [{"type": kind, "value": value}], "actions": [{"type": "markRead"}]});
        let id = s.run("rules.add", &rule(&long)).unwrap()["id"].as_u64().unwrap();
        let made = s.mailbox.rules.iter().find(|r| r.id == id).unwrap();
        let whole = vec![long.clone()];
        assert!(
            made.conditions == [Condition::From(whole.clone())] || made.conditions == [Condition::SentTo(whole)],
            "{kind}: {:?}",
            made.conditions
        );
        let before = s.mailbox.rules.len();
        assert!(s.run("rules.add", &rule(&longer)).is_err(), "{kind}");
        assert_eq!(s.mailbox.rules.len(), before);
    }
}

#[test]
fn the_new_parts_act_on_arriving_mail_and_on_mail_already_there() {
    let (mut s, account, inbox, kept) = session();
    let rule = json!({
        "name": "Club",
        "conditions": [{"type": "from", "value": "ada@example.org"}, {"type": "toOrCcMe"}],
        "exceptions": [{"type": "subjectContains", "value": ["unsubscribe", "opt out"]}],
        "actions": [{"type": "copyTo", "value": kept}, {"type": "flagFor", "value": "tomorrow"}, {"type": "clearCategories"}],
    });
    s.run("rules.add", &rule).unwrap();
    // Due tomorrow, Thursday, at the end of the working day, as Follow Up's Tomorrow is.
    let due = NOW + 8 * 3600 + i64::from(s.prefs.work_end) * 3600;
    let m = arriving(&mut s, inbox, "Ada@Example.org", "Agenda");
    s.mailbox.apply_rules(m, &s.rule_clock());
    let got = s.mailbox.message(m).unwrap();
    assert_eq!((got.flag, got.flag_due, got.categories.len(), got.folder), (FlagStatus::Flagged, Some(due), 0, inbox));
    assert_eq!(s.mailbox.messages_in(kept).map(|c| c.subject.as_str()).collect::<Vec<_>>(), ["Agenda"]);
    // An exception leaves it alone; so does another sender, or mail not to me.
    let unsubscribe = arriving(&mut s, inbox, "ada@example.org", "Opt out of the club");
    let other = arriving(&mut s, inbox, "ada.lovelace@example.org", "Agenda");
    let bcc = s
        .mailbox
        .add_message(Message { folder: inbox, from: Address::new("", "ada@example.org"), subject: "Bcc".into(), ..Default::default() })
        .unwrap();
    for id in [unsubscribe, other, bcc] {
        s.mailbox.apply_rules(id, &s.rule_clock());
        let got = s.mailbox.message(id).unwrap();
        assert!(got.flag == FlagStatus::None && got.flag_due.is_none(), "{}", got.subject);
    }
    assert_eq!(s.mailbox.messages_in(kept).count(), 1);
    // Run Rules Now does the same with mail already there.
    let later = s
        .mailbox
        .add_message(Message {
            folder: inbox,
            from: Address::new("", "ada@example.org"),
            to: vec![Address::new("", ME)],
            subject: "Later".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"], 2);
    assert_eq!(s.mailbox.message(later).unwrap().flag_due, Some(due));
    // Clearing the flag, and deleting for good.
    let clear = json!({"name": "Unflag", "conditions": [{"type": "subjectContains", "value": "later"}], "actions": [{"type": "clearFlag"}]});
    s.run("rules.add", &clear).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    let got = s.mailbox.message(later).unwrap();
    assert_eq!((got.flag, got.flag_due), (FlagStatus::None, None));
    let gone = json!({"name": "Gone", "conditions": [{"type": "subjectContains", "value": "opt out"}], "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &gone).unwrap();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    // Gone, not in Deleted Items.
    assert!(s.mailbox.message(unsubscribe).is_none());
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    assert!(!s.mailbox.messages_in(deleted).any(|m| m.subject.starts_with("Opt out")));
}

/// "Has the category" reads a category's name as category names are read everywhere (in any
/// script's case): an exception for "Büro" leaves alone a message whose category the server
/// wrote "BÜRO", and a rule that deletes for good doesn't take it.
#[test]
fn a_category_exception_holds_for_the_name_in_any_scripts_case() {
    let (mut s, _, inbox, _) = session();
    let rule = json!({
        "name": "Clean up",
        "conditions": [{"type": "subjectContains", "value": "report"}],
        "exceptions": [{"type": "category", "value": "Büro"}],
        "actions": [{"type": "deleteForGood"}],
    });
    s.run("rules.add", &rule).unwrap();
    let same = arriving(&mut s, inbox, "ada@example.org", "Report one");
    s.mailbox.message_mut(same).unwrap().categories = vec!["Büro".into()];
    let capitals = arriving(&mut s, inbox, "ada@example.org", "Report two");
    s.mailbox.message_mut(capitals).unwrap().categories = vec!["BÜRO".into()];
    let other = arriving(&mut s, inbox, "ada@example.org", "Report three");
    s.run("rules.run", &serde_json::json!({"folder": inbox})).unwrap();
    assert!(s.mailbox.message(same).is_some(), "the exception holds for the same spelling");
    assert!(s.mailbox.message(capitals).is_some(), "and for the name in capitals");
    assert!(s.mailbox.message(other).is_none(), "the rule still acts on the rest");
}

#[test]
fn always_move_messages_from_a_sender_without_a_whole_address_looks_for_its_words() {
    let (mut s, _, inbox, kept) = session();
    for (from, made) in [
        ("digest@news.example", Condition::From(vec!["digest@news.example".into()])),
        ("noreply", Condition::FromContains(vec!["noreply".into()])),
        ("a@b@news.example", Condition::FromContains(vec!["a@b@news.example".into()])),
    ] {
        let m = arriving(&mut s, inbox, from, "x");
        let id = s.run("rules.fromMessage", &json!({"folder": kept, "id": m})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(s.mailbox.rules.iter().find(|r| r.id == id).unwrap().conditions, [made], "{from}");
    }
}

/// A DER element of `tag` holding `value`.
fn der(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = value.len().to_be_bytes();
    let len: Vec<u8> = len.iter().copied().skip_while(|b| *b == 0).collect();
    if value.len() < 0x80 {
        out.push(value.len() as u8);
    } else {
        out.push(0x80 | len.len() as u8);
        out.extend(len);
    }
    out.extend_from_slice(value);
    out
}

fn base64_lines(bytes: &[u8]) -> String {
    let text = mailcraft_charset::base64::STANDARD.encode(bytes);
    text.as_bytes().chunks(76).map(|line| format!("{}\r\n", std::str::from_utf8(line).unwrap())).collect()
}

/// A short text and a file of `size` bytes, as a MIME entity.
fn text_and_file(size: usize) -> String {
    let file: Vec<u8> = (0..size).map(|i| (i * 7 + 13) as u8).collect();
    format!(
        "Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain\r\n\r\nThe report.\r\n--b1\r\nContent-Type: application/pdf; name=\"report.pdf\"\r\nContent-Disposition: attachment; filename=\"report.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}--b1--\r\n",
        base64_lines(&file)
    )
}

/// A message from example.org holding [`text_and_file`]: as it is, signed as S/MIME's opaque
/// signed data (no signer), or signed beside it (`multipart/signed`); or a short text and a
/// `.p7m` file of about `size` bytes.
fn report(kind: &str, size: usize) -> Vec<u8> {
    let head =
        format!("From: Ann <ann@example.org>\r\nTo: Me <{ME}>\r\nSubject: Report\r\nMessage-ID: <{kind}@example.org>\r\nMIME-Version: 1.0\r\n");
    let inner = text_and_file(size);
    let body = match kind {
        "opaque" => {
            let oid = |last: u8| [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, last];
            let content = der(0x30, &[oid(1).to_vec(), der(0xa0, &der(0x04, inner.as_bytes()))].concat());
            let signed = der(0x30, &[vec![0x02, 0x01, 0x01], vec![0x31, 0x00], content].concat());
            let data = der(0x30, &[oid(2).to_vec(), der(0xa0, &signed)].concat());
            format!(
                "Content-Type: application/pkcs7-mime; smime-type=signed-data; name=\"smime.p7m\"\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
                base64_lines(&data)
            )
        }
        "detached" => format!(
            "Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\"; micalg=sha-256; boundary=\"s1\"\r\n\r\n--s1\r\n{inner}\r\n--s1\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}--s1--\r\n",
            base64_lines(&[0x30, 0x03, 0x02, 0x01, 0x01])
        ),
        // A `.p7m` sent as a file (an e-invoice as signed data, or encrypted data), not opened.
        "p7m file" | "encrypted file" => {
            let payload: Vec<u8> = (0..size).map(|i| (i * 7 + 13) as u8).collect();
            let oid = |last: u8| [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, last];
            let data = der(0x30, &[oid(if kind == "p7m file" { 2 } else { 3 }).to_vec(), der(0xa0, &der(0x04, &payload))].concat());
            format!(
                "Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain\r\n\r\nThe report.\r\n--b1\r\nContent-Type: application/pkcs7-mime; name=\"invoice.xml.p7m\"\r\nContent-Disposition: attachment; filename=\"invoice.xml.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{}--b1--\r\n",
                base64_lines(&data)
            )
        }
        _ => inner,
    };
    format!("{head}{body}").into_bytes()
}

/// To a size condition, a message is as big as the message list shows it: one signed beside its
/// text (#30), or carrying a `.p7m` file (signed or encrypted data sent as a file, which isn't
/// opened), measures as an ordinary one with the same text and a file of that size. A rule for
/// smaller mail, and one that a size exception keeps off it, both deleting for good, leave them
/// alone, and one for that size finds them.
#[test]
fn a_size_rule_measures_a_message_as_the_list_shows_it() {
    let (mut s, _, inbox, kept) = session();
    let ids: Vec<(&str, Id)> = ["ordinary", "detached", "p7m file", "encrypted file"]
        .into_iter()
        .map(|kind| {
            let mut m = mailcraft_mime::parse_fetched(&report(kind, 256 << 10));
            m.folder = inbox;
            (kind, s.mailbox.add_message(m).unwrap())
        })
        .collect();
    let kb = s.mailbox.message(ids[0].1).unwrap().list_size().unwrap() / 1024;
    let size = |min: u64, max: u64| json!([{"type": "sizeBetween", "value": {"min": min, "max": max}}]);
    let from_ann = json!([{"type": "from", "value": ["ann@example.org"]}]);
    let gone = json!([{"type": "deleteForGood"}]);
    let but_big = json!({"name": "Ann's, but big", "conditions": from_ann, "exceptions": size(kb, 1_000_000), "actions": gone});
    let smaller = json!({"name": "Smaller", "conditions": size(0, kb - 1), "actions": gone});
    let that_size = json!({"name": "That size", "conditions": size(kb, kb), "actions": [{"type": "moveTo", "value": kept}]});
    for rule in [but_big, smaller, that_size] {
        s.run("rules.add", &rule).unwrap();
    }
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    for (kind, id) in ids {
        let here = s.mailbox.message(id).map(|m| (m.folder, m.list_size().unwrap() / 1024));
        assert_eq!(here, Some((kept, kb)), "{kind}");
    }
}
