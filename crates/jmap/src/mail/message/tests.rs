// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An email of a JMAP answer as a message: its marks, whose it is, and what the raw header fields make of it.
//! (The header fields are read by the program's MIME reader; what these tests hold is that a server's raw
//! fields, folds and all, come out as what a message list shows, and that no field can be made to say more.)

use serde_json::json;

use super::*;

/// The raw header fields of an email as a server shows them: the name, and all that follows the colon.
fn fields(list: &[(&str, &str)]) -> Value {
    Value::Array(list.iter().map(|(name, value)| json!({"name": name, "value": value})).collect())
}

/// An email with these header fields, that arrived at 07:30 UTC on 15 October 2026.
fn read(headers: &[(&str, &str)]) -> GraphMessage {
    whole("E1", "M1", &json!({"headers": fields(headers), "receivedAt": "2026-10-15T07:30:00Z"}), Marks::default())
}

fn time(iso: &str) -> i64 {
    mailcraft_time::iso_to_timestamp(iso, 0).unwrap()
}

fn named(name: &str, email: &str) -> Address {
    Address { name: name.into(), email: email.into() }
}

#[test]
fn the_marks_are_five_keywords_in_any_case_and_no_other() {
    let all = json!({"$seen": true, "$FLAGGED": true, "$Answered": true, "$forwarded": true, "$draft": true, "$junk": true, "work": true});
    assert_eq!(Marks::of(Some(&all)), Marks { read: true, flagged: true, answered: true, forwarded: true, draft: true });
    assert_eq!(Marks::of(Some(&json!({"$seen": true, "work": true}))), Marks { read: true, ..Default::default() });
    for none in [
        json!({"$seen": false, "$flagged": null, "$answered": "true", "$draft": 1}),
        json!({}),
        json!([]),
        json!(["$seen"]),
        json!("$seen"),
        json!(null),
    ] {
        assert_eq!(Marks::of(Some(&none)), Marks::default(), "{none}");
    }
    assert_eq!(Marks::of(None), Marks::default());
    // Every keyword is looked at: names that sort before `$seen` do not push it out of sight.
    let mut many: serde_json::Map<String, Value> = (0..5_000).map(|n| (format!("!{n:05}"), json!(true))).collect();
    many.insert("$seen".into(), json!(true));
    many.insert("$Draft".into(), json!(true));
    assert_eq!(Marks::of(Some(&Value::Object(many))), Marks { read: true, draft: true, ..Default::default() });
}

#[test]
fn without_a_list_an_email_is_listed_by_the_smallest_of_the_mailboxes_it_lies_in() {
    let lies_in = |ids: Value| claims("M2", Some(&ids), None);
    assert!(lies_in(json!({"M2": true})));
    assert!(lies_in(json!({"M2": true, "M3": true, "M9": true})));
    assert!(!lies_in(json!({"M1": true, "M2": true})));
    assert!(!lies_in(json!({"M2": true, "A": true})));
    // Bytes, not numbers: M10 is before M2.
    assert!(claims("M10", Some(&json!({"M10": true, "M2": true})), None) && !claims("M2", Some(&json!({"M10": true, "M2": true})), None));
    // A mailbox it is said not to lie in, and ids that are none, count for nothing.
    assert!(lies_in(json!({"M1": false, "M2": true})));
    assert!(lies_in(json!({"a b": true, "M2": true, "": true, "é": true})));
    // It says nothing of where it lies, or nothing that means anything: it is in the mailbox it was found in.
    for unsaid in [json!({}), json!({"M1": false}), json!({"a b": true}), json!([]), json!("M1"), json!(null)] {
        assert!(lies_in(unsaid.clone()), "{unsaid}");
    }
    assert!(claims("M2", None, None));
    // Where the server's list of mailboxes is known, only those count: an id that is no mailbox of it (a view,
    // one it left out) claims nothing.
    let list: HashMap<String, usize> = [("M2", 6), ("M3", 6)].into_iter().map(|(id, rank)| (id.to_string(), rank)).collect();
    assert!(claims("M2", Some(&json!({"A0": true, "M2": true})), Some(&list)));
    assert!(claims("M2", Some(&json!({"M1": true, "M2": true, "M3": true})), Some(&list)));
    assert!(!claims("M3", Some(&json!({"M1": true, "M2": true, "M3": true})), Some(&list)));
    assert!(claims("M3", Some(&json!({"M1": true})), Some(&list)), "none of its ids is a mailbox of the list: it stays where it was found");
}

#[test]
fn with_a_list_an_email_is_listed_by_the_inbox_then_the_special_mailboxes_then_the_smallest_id() {
    // The ranks a run gives: the Inbox 0, Sent 2, Junk 5, and the folders of the owner's own 6.
    let list: HashMap<String, usize> = [("inbox-id-z", 0), ("sent-y", 2), ("junk-x", 5), ("label-a", 6), ("label-b", 6)]
        .into_iter()
        .map(|(id, rank)| (id.to_string(), rank))
        .collect();
    let lies_in = |mailbox: &str, ids: Value| claims(mailbox, Some(&ids), Some(&list));
    // In the Inbox and a label whose id sorts first: the Inbox's.
    assert!(lies_in("inbox-id-z", json!({"inbox-id-z": true, "label-a": true})));
    assert!(!lies_in("label-a", json!({"inbox-id-z": true, "label-a": true})));
    // A special mailbox before a label, and the first special mailbox of the pane's order before another.
    assert!(lies_in("sent-y", json!({"label-a": true, "sent-y": true})));
    assert!(lies_in("sent-y", json!({"junk-x": true, "sent-y": true})) && !lies_in("junk-x", json!({"junk-x": true, "sent-y": true})));
    // Two of one rank: the smallest id.
    assert!(lies_in("label-a", json!({"label-b": true, "label-a": true})) && !lies_in("label-b", json!({"label-b": true, "label-a": true})));
    // The Inbox it is said not to lie in counts for nothing.
    assert!(lies_in("label-a", json!({"inbox-id-z": false, "label-a": true})));
}

#[test]
fn a_line_is_one_line_of_at_most_so_many_characters() {
    assert_eq!(line("  a \t b\r\n\r\n c  ", 100), "a b c");
    assert_eq!(line("", 5), "");
    assert_eq!(line(" \n\t ", 5), "");
    assert_eq!(line("a\u{0}b\u{7f}c\u{85}d", 10), "a b c d");
    assert_eq!(line("é\u{a0}è", 10), "é\u{a0}è", "no-break spaces are the sender's");
    // The cut is in characters and never leaves a blank at the end.
    assert_eq!(line("abc def", 3), "abc");
    assert_eq!(line("abc def", 4), "abc");
    assert_eq!(line("abc def", 5), "abc d");
    assert_eq!(line(&"é".repeat(600), 256).chars().count(), 256);
    assert_eq!(line("abc", 0), "");
    assert_eq!(line("a b", 2), "a");
}

#[test]
fn folded_lines_and_encoded_words_come_out_as_text() {
    let m = read(&[
        ("Subject", " one\r\n two\r\n\tthree"),
        ("From", " =?UTF-8?Q?Zo=C3=AB_Park?= <zoe@example.org>"),
        ("To", " =?ISO-8859-1?Q?Andr=E9?= <andre@example.org>,\r\n \"Park, Bob\" <bob@example.org>"),
    ]);
    assert_eq!(m.subject, "one two three");
    assert_eq!(m.from, named("Zoë Park", "zoe@example.org"));
    assert_eq!(m.to, [named("André", "andre@example.org"), named("Park, Bob", "bob@example.org")]);
    for (raw, text) in [
        (" =?UTF-8?B?Q2Fmw6k=?=", "Café"),
        (" =?utf-8?q?caf=C3=A9?= au lait", "café au lait"),
        // Adjacent encoded words, folded between: the blank between them is not part of the text.
        (" =?UTF-8?B?Q2Fm?=\r\n =?UTF-8?B?w6k=?=", "Café"),
        (" Re: =?UTF-8?Q?=E2=9C=93?= done", "Re: ✓ done"),
        (" plain, with a colon: and \"quotes\"", "plain, with a colon: and \"quotes\""),
        (" raw 8-bit é and 日本語", "raw 8-bit é and 日本語"),
        ("", ""),
        (" \r\n \r\n", ""),
    ] {
        assert_eq!(read(&[("Subject", raw)]).subject, text, "{raw:?}");
    }
}

#[test]
fn the_people_of_a_message_are_read_as_an_address_list_is() {
    let m = read(&[
        ("To", " Ada <ada@example.org>, bob@example.org (Bob), Team: a@example.org, b@example.org;"),
        ("To", " carol@example.org"),
        ("Cc", " "),
        ("From", " \"Last, First\" <first@example.org>"),
    ]);
    assert_eq!(
        m.to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(),
        ["ada@example.org", "bob@example.org", "a@example.org", "b@example.org", "carol@example.org"]
    );
    assert_eq!((m.to[0].name.as_str(), m.to[1].name.as_str()), ("Ada", "Bob"));
    assert!(m.cc.is_empty());
    assert_eq!(m.from, named("Last, First", "first@example.org"));
    let cc = read(&[("CC", " x@example.org"), ("cc", " y@example.org")]);
    assert_eq!(cc.cc, [named("", "x@example.org"), named("", "y@example.org")], "a name is matched in any case, and a Cc may repeat");
    // None of it.
    let none = read(&[]);
    assert_eq!(
        (none.from.clone(), none.to.len(), none.cc.len(), none.subject.as_str(), none.message_id.as_str()),
        (Address::default(), 0, 0, "", "")
    );
}

#[test]
fn the_dates_are_the_headers_and_the_servers() {
    let sent = time("2026-10-15T07:30:00Z");
    let m = read(&[("Date", " Thu, 15 Oct 2026 09:30:00 +0200")]);
    assert_eq!((m.sent, m.received), (sent, sent));
    // When it was sent and when it arrived are not the same.
    let arrived = whole(
        "E1",
        "M1",
        &json!({"headers": fields(&[("Date", " Thu, 15 Oct 2026 09:30:00 +0200")]), "receivedAt": "2026-10-16T01:00:00Z"}),
        Marks::default(),
    );
    assert_eq!((arrived.sent, arrived.received), (sent, time("2026-10-16T01:00:00Z")));
    // A date that makes no sense, or none: the arrival stands for it.
    for bad in [fields(&[("Date", " yesterday")]), fields(&[("Date", "")]), fields(&[]), json!(null)] {
        let m = whole("E1", "M1", &json!({"headers": bad, "receivedAt": "2026-10-15T07:30:00Z"}), Marks::default());
        assert_eq!((m.sent, m.received), (sent, sent), "{bad}");
    }
    // The server gave no arrival (null: it could not read one): the date stands for it. Neither: zero.
    let m = whole("E1", "M1", &json!({"headers": fields(&[("Date", " Thu, 15 Oct 2026 09:30:00 +0200")]), "receivedAt": null}), Marks::default());
    assert_eq!((m.sent, m.received), (sent, sent));
    let m = whole("E1", "M1", &json!({"headers": fields(&[]), "receivedAt": null}), Marks::default());
    assert_eq!((m.sent, m.received), (0, 0));
    // The arrival in the forms an RFC 3339 date has, and not when it is none or too long to be one.
    for (arrival, expected) in [
        ("2026-10-15T07:30:00Z", Some(sent)),
        ("2026-10-15T09:30:00+02:00", Some(sent)),
        ("2026-10-15T07:30:00.250Z", Some(sent)),
        ("2026-10-15T07:30:00", Some(sent)),
        ("not a date", None),
        ("", None),
        ("2026-10-15T07:30:00Z                                                  ", None),
    ] {
        let m = whole("E1", "M1", &json!({"receivedAt": arrival}), Marks::default());
        assert_eq!(Some(m.received).filter(|r| *r != 0), expected, "{arrival:?}");
    }
    for odd in [json!(5), json!({}), json!([]), json!(true)] {
        assert_eq!(whole("E1", "M1", &json!({"receivedAt": odd}), Marks::default()).received, 0, "{odd}");
    }
    // Text that is not ASCII is no date, wherever it is cut.
    for text in [
        "2026-10-15T07:30:00é",
        "２０２６-10-15T07:30:00Z",
        "日本語",
        "2026-10-15T07:30:00+０２:００",
        "2026-10-1\u{301}5T07:30:00Z",
        "é",
        "\u{1F600}T\u{1F600}",
    ] {
        let m = whole("E1", "M1", &json!({"receivedAt": text}), Marks::default());
        assert!(m.received == 0 || m.received == sent, "{text:?} {}", m.received);
    }
}

#[test]
fn the_message_id_is_without_its_brackets_and_a_header_that_is_twice_there_is_read_once() {
    let m = read(&[("Message-ID", " <abc.def@example.org>"), ("Message-Id", " <second@example.org>"), ("Subject", " first"), ("subject", " second")]);
    assert_eq!((m.message_id.as_str(), m.subject.as_str()), ("abc.def@example.org", "first"));
    for (raw, id) in [
        (" abc@example.org", "abc@example.org"),
        (" <a@example.org> (comment)", "a@example.org"),
        (" <spaced out@example.org>", "spacedout@example.org"),
        (" <>", ""),
        ("", ""),
    ] {
        assert_eq!(read(&[("Message-ID", raw)]).message_id, id, "{raw:?}");
    }
    // A second field makes up for a first that said nothing.
    let m = read(&[("Subject", ""), ("Subject", " second")]);
    assert_eq!(m.subject, "second");
}

#[test]
fn a_field_cannot_be_made_to_say_more_than_it_does() {
    // A value with a line break and another field in it is one line of its own field's text.
    let m = read(&[
        ("To", " a@example.org\r\nSubject: evil\r\nFrom: eve@example.org"),
        ("Subject", " real\r\n\r\nFrom: eve@example.org"),
        ("From", " Zoe <zoe@example.org>"),
    ]);
    assert_eq!(m.subject, "real From: eve@example.org");
    assert_eq!(m.from, named("Zoe", "zoe@example.org"));
    assert!(m.to.iter().all(|a| a.email.contains("example.org")));
    // The name of a field that is almost one is none; nor is the name of one that is not wanted.
    let m = read(&[
        ("Subject ", " one"),
        ("X-Subject", " two"),
        ("Subject:", " three"),
        ("Sub ject", " four"),
        ("From", " f@example.org"),
        ("subjectx", "x"),
    ]);
    assert_eq!((m.subject.as_str(), m.from.email.as_str()), ("", "f@example.org"));
    // Controls of every kind are blanks.
    let m = read(&[("Subject", " a\u{0}b\u{85}c\u{7f}d\u{1b}[31me")]);
    assert_eq!(m.subject, "a b c d [31me");
    // Not a list of fields, fields that are not objects, names and values that are not text.
    for odd in [
        json!(null),
        json!("Subject: x"),
        json!({"name": "Subject", "value": "x"}),
        json!([1, "a", null, [], {"name": 1, "value": "x"}, {"name": "Subject"}, {"value": "x"}, {"name": "Subject", "value": 5}]),
    ] {
        let m = whole("E1", "M1", &json!({"headers": odd}), Marks::default());
        assert_eq!((m.subject.as_str(), m.message_id.as_str(), m.to.len()), ("", "", 0), "{odd}");
    }
}

#[test]
fn every_field_a_list_shows_is_cut_to_a_size() {
    let m = read(&[("Subject", &format!(" {}", "x".repeat(20_000)))]);
    assert_eq!(m.subject.chars().count(), 998);
    assert_eq!(read(&[("Message-ID", &format!(" <{}@example.org>", "i".repeat(1_500)))]).message_id.chars().count(), 998);
    // (One that does not fit in its field is cut before its closing bracket, and is none.)
    assert_eq!(read(&[("Message-ID", &format!(" <{}@example.org>", "i".repeat(5_000)))]).message_id, "");
    // 600 people are 500; each name and address is cut.
    let people: Vec<String> = (0..600).map(|n| format!("p{n}@example.org")).collect();
    let m = read(&[("To", &format!(" {}", people.join(", "))), ("Cc", &format!(" {}", people.join(", ")))]);
    assert_eq!((m.to.len(), m.cc.len()), (500, 500));
    let m = read(&[("From", &format!(" \"{}\" <{}@example.org>", "n".repeat(400), "e".repeat(400)))]);
    assert_eq!((m.from.name.chars().count(), m.from.email.chars().count()), (256, 320));
    // More than a field has room for is cut at the last address it holds whole, never in the middle of one;
    // a first address longer than all the room is nothing.
    let long = format!(" {}@example.org, ada@example.org", "a".repeat(30_000));
    assert_eq!(read(&[("To", &long), ("Cc", " cy@example.org")]).to, Vec::<Address>::new());
    assert_eq!(read(&[("To", &long), ("Cc", " cy@example.org")]).cc.len(), 1);
    let huge: Vec<String> = (0..6_000).map(|n| format!("person{n}@example.org")).collect();
    let m = read(&[("To", &format!(" {}", huge.join(", ")))]);
    assert_eq!(m.to.len(), 500);
    assert!(m.to.iter().all(|a| a.email.starts_with("person") && a.email.ends_with("@example.org")), "{:?}", m.to.last());
    // A field that comes again may add to its room and no further.
    let page = format!(" {}", huge[..1_000].join(", "));
    let m = read(&[("To", &page), ("To", &page), ("To", &page), ("Subject", " still read")]);
    assert_eq!((m.to.len(), m.subject.as_str()), (500, "still read"));
    // A message has 500 header fields at most in a server's answer, and no more are looked at.
    let mut many: Vec<(String, String)> = (0..500).map(|n| (format!("X-Field-{n}"), " v".to_string())).collect();
    many[499] = ("Subject".into(), " the last that is looked at".into());
    let refs: Vec<(&str, &str)> = many.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    assert_eq!(read(&refs).subject, "the last that is looked at");
    many.push(("From".into(), " beyond@example.org".into()));
    let refs: Vec<(&str, &str)> = many.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    assert_eq!(read(&refs).from, Address::default(), "the 501st field is not read");
}

#[test]
fn the_header_of_a_server_that_names_by_place_is_read_whole() {
    // As such a server shows it: 500 fields, the wanted ones among them, every value with the blank after
    // its colon and its folds kept, one with an encoded word and one with a fold in the middle of a list.
    let mut list: Vec<(String, String)> =
        (0..500).map(|n| (format!("Received-{n}"), format!(" from host{n}.example.org by mx.example.org;\r\n\twith ESMTP id {n}"))).collect();
    list[3] = ("Message-ID".into(), " <519b2f6e@mail.example.org>".into());
    list[17] = ("Date".into(), " Thu, 15 Oct 2026 09:30:00 +0200".into());
    list[90] = ("From".into(), " =?UTF-8?Q?Zo=C3=AB?= Park\r\n <zoe@example.org>".into());
    list[120] = ("To".into(), " ada@example.org,\r\n \"Park, Bob\" <bob@example.org>".into());
    list[200] = ("Cc".into(), " cy@example.org".into());
    list[499] = ("Subject".into(), " =?UTF-8?B?SGVsbG8gdzDDqXJsZA==?=\r\n continued".into());
    let refs: Vec<(&str, &str)> = list.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let m = read(&refs);
    assert_eq!(m.message_id, "519b2f6e@mail.example.org");
    assert_eq!(m.sent, time("2026-10-15T07:30:00Z"));
    assert_eq!(m.from, named("Zoë Park", "zoe@example.org"));
    assert_eq!(m.to, [named("", "ada@example.org"), named("Park, Bob", "bob@example.org")]);
    assert_eq!(m.cc, [named("", "cy@example.org")]);
    assert_eq!(m.subject, "Hello w0érld continued");
}

#[test]
fn the_rest_of_an_email_is_read_as_the_server_says_it() {
    let marks = Marks { read: true, flagged: true, answered: true, forwarded: false, draft: true };
    let entry = json!({
        "preview": "  A preview\nof the\tmessage   with   gaps ", "size": 4321, "hasAttachment": true,
        "receivedAt": "2026-10-15T07:30:00Z", "headers": fields(&[("Subject", " Hi")]),
    });
    let m = whole("E1", "M7", &entry, marks);
    assert_eq!(m.preview, "A preview of the message with gaps");
    assert_eq!((m.server.size, m.has_attachments, m.draft), (4321, true, true));
    assert_eq!(m.server.id, "E1");
    assert_eq!(m.server.folder, "M7");
    assert_eq!((m.server.read, m.server.answered, m.server.forwarded, m.server.flag), (true, true, false, FlagStatus::Flagged));
    assert!(!m.server.gone && m.server.categories.is_empty() && m.server.change_key.is_empty());
    // A preview is 256 characters (not bytes), however it is written.
    let long = whole("E1", "M1", &json!({"preview": "é".repeat(600)}), Marks::default());
    assert_eq!(long.preview.chars().count(), 256);
    // What is not what it should be is not there.
    for (size, attachment, preview) in [
        (json!(-5), json!("yes"), json!(5)),
        (json!(12.5), json!(1), json!(null)),
        (json!("9"), json!(null), json!([])),
        (json!({}), json!([]), json!({})),
    ] {
        let m = whole("E1", "M1", &json!({"size": size, "hasAttachment": attachment, "preview": preview}), Marks::default());
        assert_eq!((m.server.size, m.has_attachments, m.preview.as_str()), (0, false, ""), "{size} {attachment} {preview}");
    }
    assert_eq!(whole("E1", "M1", &json!({"size": u64::MAX}), Marks::default()).server.size, u64::MAX);
    assert_eq!(whole("E1", "M1", &json!(null), Marks::default()).server.id, "E1");
    // An email the engine has comes as its marks and nothing else.
    let had = known("E2", "M3", Marks { read: true, ..Default::default() });
    assert_eq!(
        had,
        GraphMessage { server: ServerMessage { id: "E2".into(), folder: "M3".into(), read: true, ..Default::default() }, ..Default::default() }
    );
}
