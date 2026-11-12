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
    // A copy into an Outbox is refused as a copy.
    let said = s.run("rules.add", &rule(conditions, json!([]), json!([{"type": "copyTo", "value": outbox}]))).unwrap_err().to_string();
    assert!(said.contains("move or copy mail to an Outbox"), "{said}");
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

/// A rule for `account`'s mail with "news" in the subject that deletes it for good.
fn purge(s: &mut Session, account: Id) {
    let purge = json!({"name": "Purge", "account": account, "conditions": [{"type": "subjectContains", "value": "news"}], "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &purge).unwrap();
}

/// Run Rules Now over a big Inbox of a Microsoft account with a rule that deletes for good, which
/// queues a delete for its server of each: in proportion to the messages, as
/// `mail.deletePermanently` of them is (#566).
#[test]
fn run_rules_now_deleting_for_good_on_microsoft_takes_time_in_proportion() {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, ServerMessage};
    let n = 20_000u64;
    let mut s = Session::at(Default::default(), NOW, 0);
    let ms = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let listing = GraphMailResult { account: ms, email: ME.into(), listing: Some(FolderListing { folders, complete: true }), ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, listing).unwrap();
    let inbox = s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap();
    for i in 0..n {
        let server = Some(ServerMessage { id: format!("M{i}"), folder: "in".into(), ..Default::default() });
        s.mailbox.add_message(Message { folder: inbox, account: ms, subject: format!("news {i}"), server, ..Default::default() }).unwrap();
    }
    purge(&mut s, ms);
    let started = std::time::Instant::now();
    let matched = s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"].as_u64().unwrap();
    let took = started.elapsed();
    assert_eq!(matched, n);
    assert_eq!(s.mailbox.messages_in(inbox).count(), 0);
    let deletes = s.mailbox.queued.iter().filter(|q| matches!(q, mailcraft_model::Queued::DeleteMessage { .. })).count();
    assert_eq!(deletes, n as usize, "one delete for the server of each");
    assert!(took.as_millis() < 1_500, "{took:?}");
}

/// A JMAP account: mail its server has isn't deleted here yet, so a rule that deletes it for good
/// does nothing, and Run Rules Now doesn't count it as changed.
#[test]
fn run_rules_now_doesnt_count_what_a_rule_couldnt_do() {
    let mut s = Session::at(Default::default(), NOW, 0);
    let jmap = json!({"email": ME, "kind": "jmap", "jmap": {"session": "https://jmap.example.org/jmap/session", "bearer": true}});
    let a = s.run("account.add", &jmap).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap_or_else(|| s.mailbox.add_folder(a, None, "Inbox").unwrap());
    let server = Some(mailcraft_model::sync::ServerMessage { id: "E1".into(), folder: "M1".into(), ..Default::default() });
    let m = s.mailbox.add_message(Message { folder: inbox, account: a, subject: "news".into(), server, ..Default::default() }).unwrap();
    purge(&mut s, a);
    let matched = s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"].clone();
    assert!(s.mailbox.message(m).is_some(), "stays: the server has it");
    assert_eq!(matched, 0, "nothing was done to it");
}

/// Run Rules Now acts on mail that is already here: a rule's desktop alert is for mail arriving,
/// and none is shown for it.
#[test]
fn run_rules_now_shows_no_alert() {
    let (mut s, account, inbox, _) = session();
    let m = arriving(&mut s, inbox, "boss@example.org", "Urgent");
    s.mailbox.message_mut(m).unwrap().rule_alert = false;
    let urgent = json!({"name": "Urgent", "account": account, "conditions": [{"type": "subjectContains", "value": "urgent"}], "actions": [{"type": "markRead"}], "alert": true});
    s.run("rules.add", &urgent).unwrap();
    assert_eq!(s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"], 1);
    let m = s.mailbox.message(m).unwrap();
    assert!(m.read && !m.rule_alert);
}

/// An exception for "Ünal@example.org; ünal@example.org" leaves both alone: addresses are kept
/// once only as a person's address is compared, A to Z in either case, whether they come in one
/// exception or in two.
#[test]
fn an_exception_for_an_address_in_two_cases_leaves_both_alone() {
    let one = json!([{"type": "from", "value": ["Ünal@example.org", "ünal@example.org"]}]);
    let two = json!([{"type": "from", "value": "Ünal@example.org"}, {"type": "from", "value": "ünal@example.org"}]);
    for exceptions in [one, two] {
        let (mut s, _, inbox, _) = session();
        let rule = json!({
            "name": "Clean up",
            "conditions": [{"type": "subjectContains", "value": "report"}],
            "exceptions": exceptions,
            "actions": [{"type": "deleteForGood"}],
        });
        s.run("rules.add", &rule).unwrap();
        let upper = arriving(&mut s, inbox, "Ünal@example.org", "Report one");
        let lower = arriving(&mut s, inbox, "ünal@example.org", "Report two");
        let same = arriving(&mut s, inbox, "üNAL@EXAMPLE.ORG", "Report three");
        let other = arriving(&mut s, inbox, "ada@example.org", "Report four");
        s.run("rules.run", &json!({"folder": inbox})).unwrap();
        assert!(s.mailbox.message(other).is_none(), "the rule acts on the rest");
        assert!(s.mailbox.message(upper).is_some() && s.mailbox.message(lower).is_some(), "{exceptions}");
        assert!(s.mailbox.message(same).is_some(), "the second, in other capitals A to Z");
        assert_eq!(s.mailbox.rules[0].exceptions, [Condition::From(vec!["Ünal@example.org".into(), "ünal@example.org".into()])]);
    }
}

/// An address's length is counted in characters, as a server rule's is when read back: one of
/// 152 characters in more than 254 bytes is an address a rule looks for whole.
#[test]
fn an_address_is_measured_in_characters() {
    let (mut s, _, _, _) = session();
    let long = format!("{}@example.org", "ü".repeat(140));
    assert!(long.chars().count() <= 254 && long.len() > 254);
    let rule = json!({"name": "Long", "conditions": [{"type": "from", "value": long}], "actions": [{"type": "markRead"}]});
    s.run("rules.add", &rule).unwrap();
    assert_eq!(s.mailbox.rules[0].conditions, [Condition::From(vec![long])]);
}

/// More than 20 exceptions that aren't one list of words (three importances, four sensitivities,
/// fourteen sizes) are refused, not cut: a rule without one of them would act on mail it was to
/// leave alone. Twenty are taken.
#[test]
fn more_than_twenty_exceptions_are_refused() {
    let (mut s, _, _, _) = session();
    let mut exceptions: Vec<serde_json::Value> = ["high", "normal", "low"].iter().map(|i| json!({"type": "importance", "value": i})).collect();
    exceptions.extend(["normal", "personal", "private", "confidential"].iter().map(|v| json!({"type": "sensitivity", "value": v})));
    exceptions.extend((0..14).map(|n| json!({"type": "sizeBetween", "value": {"min": n * 10, "max": n * 10 + 5}})));
    assert_eq!(exceptions.len(), 21);
    let rule = |exceptions: &[serde_json::Value]| json!({"name": "Many", "conditions": [{"type": "subjectContains", "value": "x"}], "exceptions": exceptions, "actions": [{"type": "deleteForGood"}]});
    let refused = s.run("rules.add", &rule(&exceptions)).unwrap_err();
    assert!(refused.to_string().contains("`exceptions` has more than 20"), "{refused}");
    assert!(s.mailbox.rules.is_empty());
    s.run("rules.add", &rule(&exceptions[..20])).unwrap();
    assert_eq!(s.mailbox.rules[0].exceptions.len(), 20);
}

/// Run Rules Now counts the messages the rules changed: run again over mail the first run left as
/// the rule would (read, with its category), and over a folder its rule moves mail to, nothing
/// changes, and it says 0.
#[test]
fn run_rules_now_counts_only_the_messages_it_changed() {
    let (mut s, account, inbox, kept) = session();
    for n in 0..3 {
        arriving(&mut s, inbox, "club@example.org", &format!("Club news {n}"));
    }
    let rule = json!({"name": "Club", "account": account, "conditions": [{"type": "subjectContains", "value": "club"}],
        "actions": [{"type": "markRead"}, {"type": "categorize", "value": "Red category"}]});
    s.run("rules.add", &rule).unwrap();
    assert_eq!(s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"], 3, "the first run marks three read");
    let before = serde_json::to_value(&s.mailbox.messages).unwrap();
    let again = s.run("rules.run", &json!({"folder": inbox})).unwrap()["matched"].clone();
    assert_eq!(serde_json::to_value(&s.mailbox.messages).unwrap(), before, "the second run changes no message");
    assert_eq!(again, 0);
    for n in 0..2 {
        let m = arriving(&mut s, inbox, "news@example.org", &format!("Filed {n}"));
        s.mailbox.move_message(m, kept).unwrap();
    }
    let file = json!({"name": "File", "account": account, "conditions": [{"type": "subjectContains", "value": "filed"}],
        "actions": [{"type": "moveTo", "value": kept}]});
    s.run("rules.add", &file).unwrap();
    let id = s.mailbox.rules.iter().find(|r| r.name == "File").unwrap().id;
    assert_eq!(s.run("rules.run", &json!({"folder": kept, "id": id})).unwrap()["matched"], 0, "nothing moved in Kept");
}

/// Run Rules Now over a large Inbox of a mailbox with many folders, some of them under Deleted
/// Items (deleted folders go there): whether each message lies under Deleted Items is looked for
/// up from its folder, so the run doesn't walk every folder under Deleted Items for each message.
#[test]
fn run_rules_now_stays_quick_with_folders_under_deleted_items() {
    let (mut s, account, inbox, _) = session();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    for i in 0..1_000usize {
        let parent = (i < 200).then_some(deleted);
        s.mailbox.add_folder(account, parent, &format!("F{i}")).unwrap();
    }
    for i in 0..20_000 {
        arriving(&mut s, inbox, "zoe@example.org", &format!("Plan {i}"));
    }
    let rule = json!({"name": "Nothing", "account": account, "conditions": [{"type": "subjectContains", "value": "zzzz"}], "actions": [{"type": "markRead"}]});
    s.run("rules.add", &rule).unwrap();
    let started = std::time::Instant::now();
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    let took = started.elapsed();
    assert!(took.as_millis() < 1_000, "{took:?}");
}

/// A part of a rule named wrongly (a misspelt `exceptions`, a `form` for a `from` day, a key a
/// condition or an action doesn't take) is refused, not left out: without it, the rule would act
/// on more mail than it was to (#566).
#[test]
fn a_rule_with_a_part_named_wrongly_is_refused_not_made_without_it() {
    let (mut s, account, _, _) = session();
    let rule = |extra: Value| {
        let mut v = json!({"name": "Odd", "account": account, "conditions": [{"type": "subjectContains", "value": "offer"}], "actions": [{"type": "deleteForGood"}]});
        if let (Some(v), Some(extra)) = (v.as_object_mut(), extra.as_object()) {
            v.extend(extra.clone());
        }
        v
    };
    let wrong = [
        rule(json!({"exception": [{"type": "fromContains", "value": "boss"}]})),
        rule(json!({"conditions": [{"type": "receivedBetween", "value": {"form": "2026-01-01", "to": "2026-02-01"}}]})),
        rule(json!({"conditions": [{"type": "subjectContains", "value": "offer", "except": true}]})),
        rule(json!({"actions": [{"type": "deleteForGood", "folder": 1}]})),
    ];
    for v in wrong {
        let before = s.mailbox.rules.len();
        assert!(s.run("rules.add", &v).is_err(), "{v}");
        assert_eq!(s.mailbox.rules.len(), before, "{v}");
    }
    assert!(s.run("rules.add", &rule(json!({"exceptions": [{"type": "fromContains", "value": "boss"}]}))).is_ok());
}

/// A key a condition doesn't take (a `min` beside words, a unit in a size, a second `max` beside
/// the value) and a `value` given to a part that is a yes or no (`false` on Flagged would be read
/// as Flagged; a day on Flag isn't kept) are refused, not left out: the rule would act on other
/// mail than was asked, and delete it for good (#566).
#[test]
fn a_key_or_a_value_a_part_doesnt_take_is_refused() {
    let (mut s, account, _, kept) = session();
    let with = |condition: Value, action: Value| json!({"name": "Odd", "account": account, "conditions": [condition], "actions": [action]});
    let delete = json!({"type": "deleteForGood"});
    let wrong = [
        with(json!({"type": "subjectContains", "value": "invoice", "min": 500}), json!({"type": "moveTo", "value": kept})),
        with(json!({"type": "sizeBetween", "value": {"min": 10, "max": 20, "unit": "MB"}}), delete.clone()),
        with(json!({"type": "sizeBetween", "value": {"min": 1, "max": 5}, "max": 999}), delete.clone()),
        with(json!({"type": "sizeBetween", "value": 5}), delete.clone()),
        with(json!({"type": "flagged", "value": false}), delete.clone()),
        with(json!({"type": "meetingRequest", "value": false}), delete.clone()),
        with(json!({"type": "toMe", "value": false}), delete.clone()),
        with(json!({"type": "hasAttachment", "value": false}), delete.clone()),
        with(json!({"type": "subjectContains", "value": "offer"}), json!({"type": "flag", "value": "tomorrow"})),
        with(json!({"type": "subjectContains", "value": "offer"}), json!({"type": "deleteForGood", "value": true})),
    ];
    let mut taken = Vec::new();
    for v in wrong {
        if s.run("rules.add", &v).is_ok() {
            let r = s.mailbox.rules.last();
            taken.push(format!("{v} made {:?} {:?}", r.map(|r| &r.conditions), r.map(|r| &r.actions)));
        }
    }
    assert!(taken.is_empty(), "taken:\n{}", taken.join("\n"));
    // Without one, or with none at all, they are taken.
    for v in [
        with(json!({"type": "flagged"}), delete.clone()),
        with(json!({"type": "hasAttachment", "value": null}), json!({"type": "flag"})),
        with(json!({"type": "sizeBetween", "value": {"min": 1, "max": 5}}), delete.clone()),
    ] {
        assert!(s.run("rules.add", &v).is_ok(), "{v}");
    }
}

/// A category name longer than a category keeps (120 characters; a Microsoft account's mail
/// carries up to 255) is refused, not cut: an exception for it would never hold, and a rule that
/// deletes for good would take the message it was to leave alone (#566).
#[test]
fn a_long_category_exception_is_refused_not_cut() {
    let (mut s, account, inbox, _) = session();
    let long = format!("Project {}", "x".repeat(130));
    let rule = json!({
        "name": "Clean up", "account": account,
        "conditions": [{"type": "subjectContains", "value": "report"}],
        "exceptions": [{"type": "category", "value": long}],
        "actions": [{"type": "deleteForGood"}],
    });
    let made = s.run("rules.add", &rule);
    let m = arriving(&mut s, inbox, "ada@example.org", "Report");
    s.mailbox.message_mut(m).unwrap().categories = vec![long.clone()];
    if made.is_ok() {
        let kept = s.mailbox.rules.last().map(|r| r.exceptions.clone());
        s.run("rules.run", &json!({"folder": inbox})).unwrap();
        assert!(s.mailbox.message(m).is_some(), "deleted for good past its exception; the rule kept {kept:?}");
    }
    assert!(made.is_err());
    // Nor is a category a rule gives cut to another name; one of 120 characters is taken.
    let categorize = |name: &str| json!({"name": "Tag", "account": account, "conditions": [{"type": "subjectContains", "value": "report"}], "actions": [{"type": "categorize", "value": name}]});
    assert!(s.run("rules.add", &categorize(&long)).is_err());
    assert!(s.run("rules.add", &categorize(&"y".repeat(120))).is_ok());
}

/// A rule deletes Ann's mail for good, except if its header holds "multipart/signed" (her signed
/// mail) or "Highest" (`X-Priority: 1 (Highest)`): both words are in lines MailCraft reads into the
/// message (the Content-Type, X-Priority), and neither message is deleted (#566).
#[test]
fn an_exception_for_a_word_in_a_header_line_read_into_the_message_leaves_it_alone() {
    let (mut s, account, inbox, _) = session();
    let rule = json!({
        "name": "Ann's, but signed or urgent", "account": account,
        "conditions": [{"type": "from", "value": "ann@example.org"}],
        "exceptions": [{"type": "headerContains", "value": ["multipart/signed", "Highest"]}],
        "actions": [{"type": "deleteForGood"}],
    });
    s.run("rules.add", &rule).unwrap();
    let mut add = |bytes: Vec<u8>| {
        let mut m = mailcraft_mime::parse_fetched(&bytes);
        m.folder = inbox;
        s.mailbox.add_message(m).unwrap()
    };
    let signed = add(report("detached", 1024));
    let urgent = add(
        format!("From: Ann <ann@example.org>\r\nTo: Me <{ME}>\r\nSubject: Now\r\nMessage-ID: <urgent@example.org>\r\nX-Priority: 1 (Highest)\r\n\r\nCall me.\r\n")
            .into_bytes(),
    );
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    let gone: Vec<&str> =
        [("signed", signed), ("urgent", urgent)].into_iter().filter(|(_, id)| s.mailbox.message(*id).is_none()).map(|(k, _)| k).collect();
    assert!(gone.is_empty(), "deleted for good past their exception: {gone:?}");
}

/// A word longer than a rule keeps is refused, not cut, as a long address or category is: cut, the
/// condition looks for less of it, and a rule that deletes for good takes mail it was to leave
/// alone.
#[test]
fn a_long_word_is_refused_not_cut() {
    assert_eq!(mailcraft_model::MAX_WORD, 200, "the Rules dialog's reason says 200");
    let word = "x".repeat(200);
    let asked = format!("{word}-2026");
    for kind in ["subjectOrBodyContains", "subjectContains", "headerContains", "fromContains"] {
        let (mut s, account, inbox, _) = session();
        let rule = |value: &str| json!({"name": "Old reports", "account": account, "conditions": [{"type": kind, "value": value}], "actions": [{"type": "deleteForGood"}]});
        let made = s.run("rules.add", &rule(&asked));
        let other = arriving(&mut s, inbox, "ada@example.org", &format!("{word}-2025"));
        if made.is_ok() {
            s.run("rules.run", &json!({"folder": inbox})).unwrap();
            assert!(s.mailbox.message(other).is_some(), "{kind}: deleted for good by a word cut short");
        }
        assert!(made.is_err(), "{kind}: a word of {} characters was taken", asked.chars().count());
        assert!(s.run("rules.add", &rule(&word)).is_ok(), "{kind}: 200 characters are taken");
    }
    // An address longer than one can be is refused too.
    let (mut s, account, _, _) = session();
    let long = format!("{}@example.org", "a".repeat(mailcraft_model::MAX_ADDRESS));
    let rule = json!({"name": "Ada", "account": account, "conditions": [{"type": "from", "value": long}], "actions": [{"type": "markRead"}]});
    assert!(s.run("rules.add", &rule).is_err());
}

/// A rule deletes newsletters permanently, except those the owner put a category on to keep
/// them. The owner renames that category (Categorize › All Categories › Rename): the mail has the
/// new name, and so has the rule's exception, which still leaves that mail alone. A condition and
/// a Categorize follow too, Undo takes the rename back from the rules as well, and a rule its
/// account's server has keeps the name the server gave it.
#[test]
fn renaming_the_category_an_exception_names_doesnt_let_the_rule_delete_that_mail_for_good() {
    let (mut s, account, inbox, _) = session();
    let rule = json!({"name": "Newsletters", "account": account,
        "conditions": [{"type": "fromContains", "value": "newsletter"}],
        "exceptions": [{"type": "category", "value": ["Red category"]}],
        "actions": [{"type": "deleteForGood"}]});
    s.run("rules.add", &rule).unwrap();
    let kept = arriving(&mut s, inbox, "newsletter@example.org", "Issue 12, keep this one");
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    assert!(s.mailbox.message(kept).is_some(), "the exception keeps it");
    s.run("category.rename", &json!({"account": account, "from": "Red category", "to": "Keep"})).unwrap();
    assert_eq!(s.mailbox.message(kept).unwrap().categories, ["Keep"]);
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    let exceptions = &s.mailbox.rules[0].exceptions;
    assert!(s.mailbox.message(kept).is_some(), "deleted for good after the rename; the rule's exception is now {exceptions:?}");
    assert_eq!(s.mailbox.rules[0].exceptions, [Condition::Category(vec!["Keep".into()])]);
    // A condition and a Categorize follow the rename too, each name once.
    s.run("category.add", &json!({"account": account, "name": "Blue category"})).ok();
    let tag = json!({"name": "Tag", "account": account,
        "conditions": [{"type": "category", "value": ["Keep", "Blue category"]}],
        "actions": [{"type": "categorize", "value": "Blue category"}]});
    let tag = s.run("rules.add", &tag).unwrap()["id"].as_u64().unwrap();
    s.run("category.rename", &json!({"account": account, "from": "blue category", "to": "Sky"})).unwrap();
    let rule = s.mailbox.rules.iter().find(|r| r.id == tag).unwrap();
    assert_eq!(rule.conditions, [Condition::Category(vec!["Keep".into(), "Sky".into()])]);
    assert_eq!(rule.actions, [Action::Categorize("Sky".into())]);
    // Undo takes the rename back from the rules with the mail.
    s.run("edit.undo", &json!({})).unwrap();
    let rule = s.mailbox.rules.iter().find(|r| r.id == tag).unwrap();
    assert_eq!(rule.actions, [Action::Categorize("Blue category".into())]);
    while s.mailbox.message(kept).is_some_and(|m| m.categories != ["Red category"]) {
        s.run("edit.undo", &json!({})).unwrap();
    }
    assert_eq!(s.mailbox.rules[0].exceptions, [Condition::Category(vec!["Red category".into()])]);
    // A rule its server has is the server's: it keeps the name the server gave it.
    s.mailbox.rules[0].server = Some(Default::default());
    s.run("category.rename", &json!({"account": account, "from": "Red category", "to": "Keep"})).unwrap();
    assert_eq!(s.mailbox.rules[0].exceptions, [Condition::Category(vec!["Red category".into()])]);
}
