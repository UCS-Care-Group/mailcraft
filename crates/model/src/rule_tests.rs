// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::*;

#[test]
fn conditions_all_must_hold() {
    let m = Message {
        from: Address::new("Weekly Digest", "news@example.org"),
        to: vec![Address::new("", "me@example.org")],
        subject: "Your October digest".into(),
        attachments: vec![Attachment::new("a.pdf", "application/pdf", vec![1])],
        ..Default::default()
    };
    let mut rule = Rule {
        enabled: true,
        conditions: vec![Condition::FromContains(vec!["DIGEST".into()]), Condition::HasAttachment],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    assert!(rule.matches(&m));
    rule.conditions.push(Condition::SubjectContains(vec!["invoice".into()]));
    assert!(!rule.matches(&m));
    rule.conditions.pop();
    rule.enabled = false;
    assert!(!rule.matches(&m));
    // No conditions: matches nothing, not everything.
    assert!(!Rule { enabled: true, ..Default::default() }.matches(&m));
    assert!(Condition::ToContains(vec!["me@".into()]).matches(&m) && !Condition::Importance(Importance::High).matches(&m));
    assert_eq!(
        Rule { conditions: vec![Condition::HasAttachment], actions: vec![Action::Flag, Action::MarkRead], ..Default::default() }.describe(),
        "When has an attachment: flag it, mark it as read"
    );
}

#[test]
fn rules_round_trip_as_json() {
    let rule = Rule {
        id: 4,
        name: "News".into(),
        enabled: true,
        conditions: vec![Condition::FromContains(vec!["news".into()])],
        actions: vec![Action::MoveTo(9), Action::Categorize("Blue".into())],
        stop: true,
        ..Default::default()
    };
    let json = serde_json::to_string(&rule).unwrap();
    assert!(json.contains(r#"{"type":"moveTo","value":9}"#), "{json}");
    assert_eq!(serde_json::from_str::<Rule>(&json).unwrap(), rule);
}

fn words(w: &[&str]) -> Vec<String> {
    w.iter().map(|w| w.to_string()).collect()
}

#[test]
fn one_word_is_written_as_before_and_several_as_a_list() {
    // As a rule of #566's first part has it, and as it is still written with one word: an
    // earlier MailCraft reads it.
    let before = r#"{"type":"subjectContains","value":"invoice"}"#;
    let one: Condition = serde_json::from_str(before).unwrap();
    assert_eq!(one, Condition::SubjectContains(words(&["invoice"])));
    assert_eq!(serde_json::to_string(&one).unwrap(), before);
    let several = Condition::SubjectContains(words(&["invoice", "receipt"]));
    let json = serde_json::to_string(&several).unwrap();
    assert_eq!(json, r#"{"type":"subjectContains","value":["invoice","receipt"]}"#);
    assert_eq!(serde_json::from_str::<Condition>(&json).unwrap(), several);
    // A rule from before exceptions and alerts reads as one without them, and is written as it was.
    let old =
        r#"{"id":3,"name":"Bills","enabled":true,"conditions":[{"type":"fromContains","value":"billing"}],"actions":[{"type":"flag"}],"stop":false}"#;
    let rule: Rule = serde_json::from_str(old).unwrap();
    assert!(rule.exceptions.is_empty() && !rule.alert);
    assert_eq!(serde_json::to_string(&rule).unwrap(), old);
}

#[test]
fn any_word_of_a_condition_is_enough() {
    let m = Message { subject: "Your receipt for October".into(), ..Default::default() };
    assert!(Condition::SubjectContains(words(&["invoice", "RECEIPT"])).matches(&m));
    assert!(!Condition::SubjectContains(words(&["invoice", "bill"])).matches(&m));
    // An empty word matches nothing, not everything.
    assert!(!Condition::SubjectContains(words(&[""])).matches(&m));
    assert!(!Condition::SubjectContains(Vec::new()).matches(&m));
    assert_eq!(Condition::SubjectContains(words(&["a", "b"])).describe(), r#"subject contains "a" or "b""#);
}

#[test]
fn an_exception_that_holds_leaves_the_message_alone() {
    let me = "me@example.org";
    let m = Message {
        from: Address::new("Billing", "billing@example.org"),
        to: vec![Address::new("", "team@example.org")],
        cc: vec![Address::new("Me", me)],
        subject: "Invoice 12".into(),
        ..Default::default()
    };
    let how = Matching { me, ..Default::default() };
    let mut rule = Rule {
        enabled: true,
        conditions: vec![Condition::SubjectContains(words(&["invoice"]))],
        exceptions: vec![Condition::Flagged, Condition::CcMe],
        actions: vec![Action::MarkRead],
        alert: true,
        ..Default::default()
    };
    assert!(!rule.matches_in(&m, &how), "I'm in Cc");
    rule.exceptions = vec![Condition::Flagged, Condition::ToMe];
    assert!(rule.matches_in(&m, &how), "no exception holds");
    assert_eq!(
        rule.describe(),
        r#"When subject contains "invoice", except if is flagged or my address is in To: mark it as read, show a desktop alert"#
    );
}

#[test]
fn the_conditions_about_me_read_the_accounts_address() {
    let me = "Me@Example.org";
    let to = |to: &[&str], cc: &[&str]| Message {
        to: to.iter().map(|a| Address::new("", *a)).collect(),
        cc: cc.iter().map(|a| Address::new("", *a)).collect(),
        ..Default::default()
    };
    let how = Matching { me, ..Default::default() };
    let on = |c: Condition, m: &Message| c.matches_in(m, &how);
    let only = to(&["me@example.org"], &[]);
    let with_others = to(&["me@example.org", "ann@example.org"], &[]);
    let copied = to(&["ann@example.org"], &["me@example.org"]);
    assert!(on(Condition::SentOnlyToMe, &only) && !on(Condition::SentOnlyToMe, &with_others) && !on(Condition::SentOnlyToMe, &copied));
    assert!(on(Condition::ToMe, &with_others) && !on(Condition::ToMe, &copied));
    assert!(on(Condition::CcMe, &copied) && !on(Condition::CcMe, &only));
    assert!(on(Condition::ToOrCcMe, &copied) && on(Condition::ToOrCcMe, &only));
    assert!(on(Condition::NotToMe, &copied) && !on(Condition::NotToMe, &only));
    // Without an address of the account's, none of them holds.
    let nobody = Matching::default();
    for c in [Condition::SentOnlyToMe, Condition::ToMe, Condition::CcMe, Condition::ToOrCcMe, Condition::NotToMe] {
        assert!(!c.matches_in(&copied, &nobody) && !c.matches_in(&only, &nobody), "{c:?}");
    }
}

#[test]
fn people_are_matched_by_their_whole_address() {
    let m = Message {
        from: Address::new("Ann", "Ann@Example.org"),
        to: vec![Address::new("", "team@example.org")],
        cc: vec![Address::new("", "bob@example.org")],
        ..Default::default()
    };
    assert!(Condition::From(words(&["ann@example.org"])).matches(&m));
    assert!(!Condition::From(words(&["ann@example"])).matches(&m), "not a part of it");
    assert!(Condition::SentTo(words(&["carol@example.org", "bob@example.org"])).matches(&m));
    assert!(!Condition::SentTo(words(&["ann@example.org"])).matches(&m));
}

#[test]
fn the_header_sensitivity_size_and_kind_of_a_message_are_read() {
    let mut m = Message {
        from: Address::new("Ann", "ann@example.org"),
        subject: "Out of office".into(),
        headers: vec![
            ("Sensitivity".into(), "Company-Confidential".into()),
            ("Auto-Submitted".into(), "auto-replied".into()),
            ("X-Mailer".into(), "Harbor Mail 3".into()),
        ],
        body: Body { text: "x".repeat(3000), ..Default::default() },
        ..Default::default()
    };
    assert!(Condition::HeaderContains(words(&["harbor mail"])).matches(&m));
    assert!(Condition::HeaderContains(words(&["from: ann"])).matches(&m), "the modelled lines too");
    assert!(!Condition::HeaderContains(words(&["lumen"])).matches(&m));
    assert!(Condition::Sensitivity(Sensitivity::Confidential).matches(&m));
    assert!(Condition::AutomaticReply.matches(&m));
    assert!(Condition::SizeBetween { min: 2, max: 3 }.matches(&m) && !Condition::SizeBetween { min: 10, max: 100 }.matches(&m));
    m.headers.clear();
    assert!(Condition::Sensitivity(Sensitivity::Normal).matches(&m) && !Condition::AutomaticReply.matches(&m));
    assert!(!Condition::MeetingRequest.matches(&m));
    m.attachments.push(Attachment::new("invite.ics", "text/calendar", b"BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n".to_vec()));
    assert!(Condition::MeetingRequest.matches(&m));
    // An answer to an invitation isn't one.
    m.attachments = vec![Attachment::new("reply.ics", "text/calendar", b"BEGIN:VCALENDAR\r\nMETHOD:REPLY\r\nEND:VCALENDAR\r\n".to_vec())];
    assert!(!Condition::MeetingRequest.matches(&m));
}

#[test]
fn a_header_rule_reads_the_headers_mail_came_with_and_not_mailcrafts_own() {
    // As a .pst import and a copy put on a server leave them: lines the mail wasn't sent with,
    // which the server's own rule never sees.
    let m = Message {
        headers: vec![
            ("X-MailCraft-Pst-Item".into(), "5a110e93c0ffee00c0ffee00c0ffee00:00200024".into()),
            ("x-mailcraft-place".into(), "pst-marker".into()),
            ("X-Mailer".into(), "Harbor Mail 3".into()),
        ],
        ..Default::default()
    };
    for word in ["pst", "mailcraft", "00200024", "marker"] {
        assert!(!Condition::HeaderContains(words(&[word])).matches(&m), "{word}");
        assert!(!m.header_contains(word), "{word}");
    }
    assert!(Condition::HeaderContains(words(&["harbor"])).matches(&m));
    // A name too short to be one of ours, or with a letter outside ASCII where ours has its
    // dash, is read as it came.
    let odd = Message { headers: vec![("X-MailCraft".into(), "plain".into()), ("X-MailCraft\u{e9}x".into(), "accent".into())], ..Default::default() };
    assert!(odd.header_contains("plain") && odd.header_contains("accent"));
}

#[test]
fn a_text_rule_doesnt_read_mailcrafts_note_in_place_of_a_message_too_large_to_download() {
    // As `mailcraft-net` keeps such a message: its header, and a note in place of its text.
    let note = format!(
        "{NOT_DOWNLOADED_NOTE} it is 60 MB on the server. The message itself, with anything attached to it, is still on the server. Read it there on the web.\n"
    );
    let big = Message {
        subject: "Holiday photos".into(),
        body: Body { text: note.clone(), html: None },
        headers: vec![(NOT_DOWNLOADED.into(), "62914560".into())],
        not_downloaded: 62_914_560,
        ..Default::default()
    };
    for word in ["attached", "on the web", "too large"] {
        let rule =
            Rule { enabled: true, conditions: vec![Condition::BodyContains(words(&[word]))], actions: vec![Action::Delete], ..Default::default() };
        assert!(!rule.matches(&big), "{word}");
        assert!(!Condition::SubjectOrBodyContains(words(&[word])).matches(&big), "{word}");
        // Nor can it be told: as an exception it is not here.
        assert!(Condition::BodyContains(words(&[word])).unread_in(&big), "{word}");
    }
    assert!(Condition::SubjectOrBodyContains(words(&["photos"])).matches(&big), "the subject is its own");
    // A sender can write the note and the header into a message of their own, not the size the
    // server gave: its text is read as its own.
    let written = Message { not_downloaded: 0, ..big };
    assert!(Condition::BodyContains(words(&["on the web"])).matches(&written));
}

#[test]
fn received_between_reads_the_day_by_the_calendars_clock() {
    use crate::time::{Date, DateTime, Zone};
    let berlin = Zone::named("Europe/Berlin").unwrap();
    // 23:30 on 31 October in UTC is 00:30 on 1 November in Berlin.
    let at = DateTime::new(Date::new(2026, 10, 31).unwrap(), 23, 30, 0).unwrap().to_timestamp(0);
    let m = Message { received: at, ..Default::default() };
    let november = Condition::ReceivedBetween { from: Date::new(2026, 11, 1), to: Date::new(2026, 11, 30) };
    assert!(november.matches_in(&m, &Matching { zone: berlin, ..Default::default() }));
    assert!(!november.matches_in(&m, &Matching::default()), "still October in UTC");
    assert!(Condition::ReceivedBetween { from: None, to: Date::new(2026, 10, 31) }.matches(&m));
    assert!(!Condition::ReceivedBetween { from: None, to: None }.matches(&m), "no day at all is no condition");
}

#[test]
fn a_flag_for_this_week_falls_due_on_its_friday_at_the_end_of_the_day() {
    use crate::time::{Date, DateTime, Zone};
    let zone = Zone::fixed(120).unwrap();
    // Tuesday 3 November 2026, 09:00 there.
    let now = DateTime::new(Date::new(2026, 11, 3).unwrap(), 9, 0, 0).unwrap().to_timestamp(120);
    let clock = RuleClock { now, zone, due_hour: 17 };
    let due = |f: FollowUp| zone.wall(f.due(&clock).unwrap());
    assert_eq!(due(FollowUp::Today), DateTime::new(Date::new(2026, 11, 3).unwrap(), 17, 0, 0).unwrap());
    assert_eq!(due(FollowUp::Tomorrow).date, Date::new(2026, 11, 4).unwrap());
    assert_eq!(due(FollowUp::ThisWeek).date, Date::new(2026, 11, 6).unwrap());
    assert_eq!(due(FollowUp::NextWeek).date, Date::new(2026, 11, 13).unwrap());
    // At the weekend, as Follow Up's menu: this week is today, next week the coming Friday.
    for (saturday_or_sunday, friday) in [(7, 13), (8, 13)] {
        let today = Date::new(2026, 11, saturday_or_sunday).unwrap();
        assert_eq!(FollowUp::ThisWeek.day(today), today);
        assert_eq!(FollowUp::NextWeek.day(today), Date::new(2026, 11, friday).unwrap(), "{saturday_or_sunday}");
    }
}

#[test]
fn exceptions_of_a_kind_are_one_as_a_server_keeps_them() {
    use crate::fold_exceptions;
    let words = |w: &[&str]| w.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    let folded = fold_exceptions(vec![
        Condition::SubjectContains(words(&["opt out"])),
        Condition::HasAttachment,
        Condition::SubjectContains(words(&["Unsubscribe", "OPT OUT"])),
        Condition::HasAttachment,
        Condition::Importance(Importance::Low),
        Condition::Importance(Importance::High),
        Condition::From(words(&["ada@example.org"])),
    ])
    .unwrap();
    assert_eq!(
        folded,
        [
            Condition::SubjectContains(words(&["opt out", "Unsubscribe"])),
            Condition::HasAttachment,
            Condition::Importance(Importance::Low),
            Condition::Importance(Importance::High),
            Condition::From(words(&["ada@example.org"])),
        ]
    );
    // Never more words than a condition keeps: an exception cut short would let mail through
    // that it was to leave alone.
    let many: Vec<Condition> = (0..30).map(|i| Condition::BodyContains(vec![format!("w{i}")])).collect();
    assert!(fold_exceptions(many).is_err());
    let enough: Vec<Condition> = (0..crate::MAX_WORDS_EACH).map(|i| Condition::BodyContains(vec![format!("w{i}")])).collect();
    assert!(matches!(&fold_exceptions(enough).unwrap()[..], [Condition::BodyContains(w)] if w.len() == crate::MAX_WORDS_EACH));
}

#[test]
fn an_exception_that_reads_what_isnt_here_yet_leaves_the_mail_alone() {
    let words = |t: &str| vec![t.to_string()];
    let mut rule = Rule {
        enabled: true,
        conditions: vec![Condition::SubjectContains(words("report"))],
        actions: vec![Action::DeleteForGood],
        ..Default::default()
    };
    let whole = Message { subject: "Report".into(), ..Default::default() };
    let stub = Message { unfetched: true, ..whole.clone() };
    let headers_only = Message { incomplete: true, ..whole.clone() };
    let not_all = Message { not_downloaded: 2048, ..whole.clone() };
    let unreadable = Message { kept: Some(Box::new(KeptBody { failed: true, ..Default::default() })), ..whole.clone() };
    for exception in [Condition::BodyContains(words("keep")), Condition::SubjectOrBodyContains(words("keep")), Condition::MeetingRequest] {
        rule.exceptions = vec![exception];
        assert!(rule.matches(&whole), "{:?}", rule.exceptions);
        for part in [&stub, &headers_only, &not_all, &unreadable] {
            assert!(!rule.matches(part), "{:?}", rule.exceptions);
        }
    }
    // Headers are there as soon as a message is, unless a server's list gave none.
    for exception in [Condition::HeaderContains(words("X-Keep")), Condition::Sensitivity(Sensitivity::Private), Condition::AutomaticReply] {
        rule.exceptions = vec![exception];
        for here in [&whole, &not_all, &unreadable] {
            assert!(rule.matches(here), "{:?}", rule.exceptions);
        }
        for part in [&stub, &headers_only] {
            assert!(!rule.matches(part), "{:?}", rule.exceptions);
        }
    }
    // What a message has from the start is read as ever.
    rule.exceptions = vec![Condition::From(words("ada@example.org")), Condition::Flagged, Condition::HasAttachment];
    assert!(rule.matches(&stub));
    // A server's list says whether there are attachments; the headers of a message too large to
    // download don't.
    rule.exceptions = vec![Condition::HasAttachment];
    assert!(!rule.matches(&not_all) && !rule.matches(&headers_only));
    // A list needn't say how big it is, and what is here isn't.
    rule.exceptions = vec![Condition::SizeBetween { min: 1000, max: 999_999 }];
    assert!(!rule.matches(&stub));
    let sized = Message { server: Some(crate::sync::ServerMessage { size: 2048, ..Default::default() }), ..stub.clone() };
    assert!(rule.matches(&sized));
    rule.exceptions.clear();
    rule.conditions.push(Condition::SizeBetween { min: 0, max: 100 });
    assert!(!rule.matches(&stub) && rule.matches(&sized));
    // An exception that can't be told doesn't count as holding, whatever a guess at it says: no
    // header reads as normal sensitivity.
    rule.conditions.pop();
    rule.exceptions = vec![Condition::Sensitivity(Sensitivity::Normal)];
    assert!(rule.undecided_in(&stub, &Matching::default()));
    // A condition that can't be read yet doesn't hold, as before, and may yet.
    rule.exceptions.clear();
    rule.conditions = vec![Condition::BodyContains(words("report"))];
    assert!(!rule.matches(&stub));
    assert!(rule.undecided_in(&stub, &Matching::default()));
    // Nor does one on a guess: no header reads as normal sensitivity.
    rule.conditions = vec![Condition::Sensitivity(Sensitivity::Normal)];
    assert!(rule.matches(&whole) && !rule.matches(&stub));
    assert!(rule.undecided_in(&stub, &Matching::default()));
    // A word found in what is here holds.
    rule.conditions = vec![Condition::SubjectOrBodyContains(words("report"))];
    assert!(rule.matches(&stub) && !rule.undecided_in(&stub, &Matching::default()));
}

/// A From or Sent to address of 201 to 254 characters, which `rules.add` keeps whole and so does
/// a Microsoft account's server, is still whole and known when the server's rules are read back
/// ([`crate::remote_rules::RemoteRule::tidy`]), not cut to 200 and greyed. Words looked for are
/// still cut at 200.
#[test]
fn a_long_address_read_back_from_the_server_stays_whole() {
    let long = format!("a@{}.{}.{}.{}.example", "b".repeat(60), "c".repeat(60), "d".repeat(60), "e".repeat(30));
    assert!(long.len() > 200 && long.len() <= 254);
    let mut remote = crate::remote_rules::RemoteRule {
        id: "A1".into(),
        name: "Long".into(),
        conditions: vec![Condition::From(vec![long.clone()]), Condition::SentTo(vec![long.clone()])],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    remote.tidy();
    assert!(remote.unknown.is_empty(), "{:?}", remote.unknown);
    assert_eq!(remote.conditions, vec![Condition::From(vec![long.clone()]), Condition::SentTo(vec![long.clone()])]);
    let mut words =
        crate::remote_rules::RemoteRule { id: "A2".into(), conditions: vec![Condition::SubjectContains(vec![long])], ..Default::default() };
    words.tidy();
    assert_eq!(words.unknown, ["(words longer than are kept)"]);
}
