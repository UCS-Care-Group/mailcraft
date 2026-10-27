// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Advanced Search's fields, read out of the search box's query and written back into it (#329).

use mailcraft_time::Date;

use crate::Importance;
use crate::search::{Advanced, Query, Reading, full_date};

fn day(s: &str) -> Date {
    Date::parse_iso(s).unwrap()
}

fn read(q: &str) -> Advanced {
    Reading::of(q).fields
}

/// `q` with its fields changed by `change`.
fn written(q: &str, change: impl FnOnce(&mut Advanced)) -> String {
    let r = Reading::of(q);
    let mut fields = r.fields.clone();
    change(&mut fields);
    r.write(&fields)
}

#[test]
fn each_field_is_read_out_of_the_box() {
    let a = read(
        r#"lunch from:ada to:bob cc:carol subject:"q4 plan" body:numbers category:Red attachment:xlsx has:attachment is:unread importance:high received:2026-10-01..2026-10-22"#,
    );
    let want = Advanced {
        from: "ada".into(),
        to: "bob".into(),
        cc: "carol".into(),
        subject: "q4 plan".into(),
        body: "numbers".into(),
        category: "Red".into(),
        attachment: "xlsx".into(),
        attachments: Some(true),
        received_from: Some(day("2026-10-01")),
        received_to: Some(day("2026-10-22")),
        read: Some(false),
        importance: Some(Importance::High),
    };
    assert_eq!(a, want);
    // The other names a keyword is written by, a space after its colon, any case.
    let b = read("FROM: ada Categories:Red attachments:xlsx hasattachments:yes is:read importance:Low");
    assert_eq!((b.from.as_str(), b.category.as_str(), b.attachment.as_str()), ("ada", "Red", "xlsx"));
    assert_eq!((b.attachments, b.read, b.importance), (Some(true), Some(true), Some(Importance::Low)));
    // A whole address, a quote and a backslash in a value.
    let c = read(r#"from:"\"ada smith\"@example.org" subject:"say \"hi\" \\ bye""#);
    assert_eq!((c.from.as_str(), c.subject.as_str()), (r#""ada smith"@example.org"#, r#"say "hi" \ bye"#));
}

#[test]
fn what_the_panel_cant_show_stays_as_typed() {
    for q in [
        "budget",
        "\"from:ada\"",
        "-from:ada",
        "-is:unread",
        "is:flagged",
        "is:important",
        "is:banana",
        "subject:",
        "from:@@",
        "-category:red",
        "received:10/22",
        "received:today",
        "received:\"last week\"",
        "on:2026-10-22",
        "before:2026-10-22",
        "after:2026-10-22",
        "sent:2026-10-22",
        "-received:2026-10-22",
        "nosuch:thing",
        "re: budget",
    ] {
        assert!(read(q).is_empty(), "{q}");
        // Search with nothing changed leaves it as it is.
        assert_eq!(written(q, |_| {}), q, "{q}");
    }
}

#[test]
fn the_first_keyword_of_a_field_is_read_and_the_rest_left() {
    let q = "from:ada from:bob has:attachment -has:attachment is:read is:unread";
    let a = read(q);
    assert_eq!((a.from.as_str(), a.attachments, a.read), ("ada", Some(true), Some(true)));
    let w = written(q, |a| a.from = "carol".into());
    assert_eq!(w, "from:carol from:bob has:attachment -has:attachment is:read is:unread");
}

#[test]
fn without_attachments_is_any_way_the_search_reads_it() {
    for q in ["-has:attachment", "-hasattachments:yes", "-has:attachments", "-hasattachment:true"] {
        assert_eq!(read(q).attachments, Some(false), "{q}");
    }
}

#[test]
fn days_received_are_read_in_every_whole_form() {
    let both = |q: &str| {
        let a = read(q);
        (a.received_from.map(Date::iso), a.received_to.map(Date::iso))
    };
    let s = |d: &str| Some(d.to_string());
    assert_eq!(both("received:2026-10-22"), (s("2026-10-22"), s("2026-10-22")));
    assert_eq!(both("received:10/1/2026..10/22/2026"), (s("2026-10-01"), s("2026-10-22")));
    assert_eq!(both("received:>=2026-10-01"), (s("2026-10-01"), None));
    assert_eq!(both("received:<=2026-10-22"), (None, s("2026-10-22")));
    assert_eq!(both("received:>2026-10-01"), (s("2026-10-02"), None));
    assert_eq!(both("received:<2026-10-22"), (None, s("2026-10-21")));
    // Two keywords, one for each side, are one field; a third is left.
    assert_eq!(both("received:>=2026-10-01 x received:<=2026-10-22 received:<=2026-12-01"), (s("2026-10-01"), s("2026-10-22")));
    assert_eq!(both("received:2026-10-01 received:>=2026-10-05"), (s("2026-10-01"), s("2026-10-01")));
    // A side the calendar has no day past is left as typed.
    let last = Date::from_days(i64::MAX);
    assert!(read(&format!("received:>{}", last.iso())).is_empty());
    let first = Date::from_days(i64::MIN);
    assert!(read(&format!("received:<{}", first.iso())).is_empty());
}

#[test]
fn a_changed_field_is_written_where_it_stood() {
    assert_eq!(written("lunch  FROM: ada  soon", |a| a.from = "bob smith".into()), "lunch  from:\"bob smith\"  soon");
    // Emptied: taken out, with the spaces after it.
    assert_eq!(written("lunch from:ada soon", |a| a.from.clear()), "lunch soon");
    assert_eq!(written("lunch from:ada", |a| a.from.clear()), "lunch");
    assert_eq!(written("from:ada", |a| a.from.clear()), "");
    // Days received in two keywords: the first takes the new range, the second goes.
    assert_eq!(
        written("received:>=2026-10-01 x received:<=2026-10-22 y", |a| a.received_to = Some(day("2026-10-23"))),
        "received:2026-10-01..2026-10-23 x y"
    );
    // A field that was empty is added at the end, in the panel's order.
    assert_eq!(
        written("lunch ", |a| {
            a.body = "x".into();
            a.from = "ada".into();
            a.received_from = Some(day("2026-10-01"));
            a.read = Some(false);
        }),
        "lunch from:ada received:>=2026-10-01 is:unread body:x"
    );
    // Only what changed is written: the others keep their spelling.
    assert_eq!(written("Categories:Red  FROM: ada", |a| a.subject = "q4".into()), "Categories:Red  FROM: ada subject:q4");
}

#[test]
fn every_value_is_written_as_the_search_reads_it() {
    let w = written("", |a| {
        a.from = "  Ada Smith ".into();
        a.subject = r#"say "hi" \ bye"#.into();
        a.attachments = Some(false);
        a.importance = Some(Importance::Normal);
        a.received_from = Some(day("2026-10-22"));
        a.received_to = Some(day("2026-10-22"));
    });
    assert_eq!(w, r#"from:"Ada Smith" subject:"say \"hi\" \\ bye" received:2026-10-22 -has:attachment importance:normal"#);
    assert_eq!(read(&w).subject, r#"say "hi" \ bye"#);
    assert_eq!(written("", |a| a.received_to = Some(day("2026-10-22"))), "received:<=2026-10-22");
    // A field of spaces asks for nothing, nor one of text the search passes over: neither is
    // written, and the keyword it replaces goes.
    assert_eq!(written("x", |a| a.cc = "   ".into()), "x");
    assert_eq!(written("x", |a| a.from = "@@".into()), "x");
    assert_eq!(written("from:ada x", |a| a.subject = "💰".into()), "from:ada x");
    assert_eq!(written("from:ada x", |a| a.from = "-".into()), "x");
}

#[test]
fn keywords_that_would_not_be_read_at_the_end_go_at_the_start() {
    // After a quote left open, the rest of the query is in it.
    assert_eq!(written("\"open quote", |a| a.from = "ada".into()), "from:ada \"open quote");
    // Past the terms the search keeps.
    let many = "w ".repeat(70);
    let w = written(&many, |a| a.from = "ada".into());
    assert_eq!(w, format!("from:ada {many}"));
    assert_eq!(read(&w).from, "ada");
}

/// A keyword with nothing after its colon asks for nothing while a keyword follows it: taking that
/// one out takes it too, so that it doesn't take the next word as its value.
#[test]
fn a_keyword_waiting_for_its_value_goes_with_the_one_taken_out() {
    assert_eq!(written("subject: is:read budget", |a| a.read = None), "budget");
    assert_eq!(written("from: -subject: is:read budget", |a| a.read = None), "budget");
    assert_eq!(written("x subject: is:read has:attachment budget", |a| (a.read, a.attachments) = (None, None)), "x budget");
    // Followed by a keyword, or by nothing, it is left.
    assert_eq!(written("subject: is:read from:ada", |a| a.read = None), "subject: from:ada");
    assert_eq!(written("subject: is:read", |a| a.read = None), "subject:");
}

#[test]
fn unicode_before_a_field_is_kept() {
    assert_eq!(written("Grüße Đorđe from:ada 日本", |a| a.from = "bob".into()), "Grüße Đorđe from:bob 日本");
    assert_eq!(written("Grüße from:ada", |a| a.from.clear()), "Grüße");
}

#[test]
fn a_full_date_is_one_with_a_year() {
    assert_eq!(full_date(" 2026-10-22 "), Some(day("2026-10-22")));
    assert_eq!(full_date("10/22/2026"), Some(day("2026-10-22")));
    for s in ["10/22", "today", "", "2026-13-01", "10/22/2026/1", "2/30/2026"] {
        assert_eq!(full_date(s), None, "{s}");
    }
}

/// What the panel writes, the search reads as the panel's fields.
#[test]
fn what_is_written_searches_for_the_fields() {
    let w = written("budget", |a| {
        a.from = "ada".into();
        a.attachments = Some(true);
        a.received_from = Some(day("2026-10-01"));
    });
    assert_eq!(w, "budget from:ada received:>=2026-10-01 has:attachment");
    assert_eq!(Query::parse(&w).len(), 4);
}

fn text() -> impl proptest::strategy::Strategy<Value = String> {
    proptest::string::string_regex(r#"([a-zA-Z0-9Đđ][a-zA-Z0-9 "\\Đđ.@]{0,8})?"#).unwrap()
}

fn date() -> impl proptest::strategy::Strategy<Value = Option<Date>> {
    use proptest::strategy::Strategy;
    proptest::option::of((1900i32..2200, 1u8..=12, 1u8..=28).prop_map(|(y, m, d)| Date::new(y, m, d).unwrap()))
}

/// A query of pieces in any order: words, keywords the panel reads (each field's once, so that
/// emptying it leaves no second one to be read in its place), keywords it leaves, quotes.
fn query() -> impl proptest::strategy::Strategy<Value = String> {
    use proptest::strategy::Strategy;
    const PIECES: &[&str] = &[
        "budget",
        "-x",
        "\"two words\"",
        "\"open",
        "from:ada",
        "-from:zed",
        "\"cc:q\"",
        "subject:",
        "is:read",
        "is:flagged",
        "-has:attachment",
        "received:10/22",
        "Categories:Red",
        "re:",
        "日本",
        "\\",
        "--",
        "-",
        "  ",
    ];
    proptest::sample::subsequence(PIECES, 0..PIECES.len()).prop_shuffle().prop_map(|p| p.join(" "))
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]

    /// Any query and any fields: no panic, the fields are read back as written, and what the
    /// panel didn't touch is still in the query.
    #[test]
    fn fields_written_into_any_query_are_read_back(
        query in query(),
        from in text(), subject in text(), category in text(), cc in text(),
        attachments in proptest::option::of(proptest::bool::ANY),
        read_status in proptest::option::of(proptest::bool::ANY),
        from_day in date(), to_day in date(),
    ) {
        let r = Reading::of(&query);
        let fields = Advanced {
            from: from.trim().to_string(),
            subject: subject.trim().to_string(),
            category: category.trim().to_string(),
            cc: cc.trim().to_string(),
            attachments,
            read: read_status,
            received_from: from_day,
            received_to: to_day,
            ..r.fields.clone()
        };
        let w = r.write(&fields);
        let back = Reading::of(&w).fields;
        proptest::prop_assert_eq!(&back, &fields, "{:?} → {:?}", query, w);
        // Unchanged, it stays as it is.
        proptest::prop_assert_eq!(r.write(&r.fields), query);
    }
}
