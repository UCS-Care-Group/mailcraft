// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #301: `from:`, `to:` and `cc:` with a whole address.

use crate::search::{Query, ServerField, ServerTerm, is_address, keyword_pills, keyword_spans, keyword_value};
use crate::words::{ALL, FROM};
use crate::{Address, Message};

fn msg() -> Message {
    Message {
        from: Address::new("Ada Lovelace", "ada@example.org"),
        to: vec![Address::new("Charles", "charles@example.org")],
        cc: vec![Address::new("", "team@example.org")],
        subject: "Budget review: Q4".into(),
        ..Default::default()
    }
}

fn from(email: &str) -> Message {
    Message { from: Address::new("Ada", email), ..msg() }
}

fn finds(q: &str, m: &Message) -> bool {
    Query::parse(q).matches(m, 0)
}

/// A whole address finds that address and no other, ignoring case; anything else after the
/// keyword matches words as before.
#[test]
fn a_whole_address_finds_that_address_alone() {
    for q in ["from:ada@example.org", "from:\"ada@example.org\"", "from: ADA@Example.ORG", "FROM:ada@example.org"] {
        // As kept, with the spaces a message may have round it.
        let spaced = Message { from: Address { name: "Ada".into(), email: " Ada@Example.org ".into() }, ..msg() };
        assert!(finds(q, &from("ada@example.org")) && finds(q, &spaced), "{q}");
        for near in ["adam@example.org", "ada.smith@example.org", "ada@example.org.example", "ada@example.com", "xada@example.org"] {
            assert!(!finds(q, &from(near)), "{q} found {near}");
        }
    }
    // Not the sender: in To or Cc, by their own keyword.
    assert!(!finds("from:charles@example.org", &msg()) && finds("to:charles@example.org", &msg()) && !finds("to:charl@example.org", &msg()));
    let copied = Message { cc: vec![Address::new("Kit", "kit@example.org")], ..msg() };
    assert!(finds("cc:KIT@example.org", &copied) && !finds("to:kit@example.org", &copied));
    assert!(!finds("-from:ada@example.org", &msg()) && finds("-from:adam@example.org", &msg()));
    // A part of one is words, as before: a word starts a word.
    assert!(finds("from:ada", &from("adam@example.org")) && finds("from:example.org", &msg()) && finds("from:\"ada love\"", &msg()));
    // Accents written either way, and a letter whose small form is longer (İ), compare as the
    // same address in either case.
    let decomposed = "u\u{0308}nal@bu\u{0308}cher.example";
    assert!(finds(&format!("from:{decomposed}"), &from(decomposed)) && !finds(&format!("from:{decomposed}"), &from("unal@bucher.example")));
    assert!(finds("from:İpek@example.org", &from("i̇pek@example.org")) && finds("from:i̇pek@example.org", &from("İPEK@EXAMPLE.ORG")));
    // A final sigma is a sigma: ΟΣ and οσ are one address, either way round.
    assert!(finds("from:οσ@example.org", &from("ΟΣ@example.org")) && finds("from:ΟΣ@example.org", &from("οσ@example.org")));
    // An international domain in `xn--` form and in its own letters are two addresses.
    assert!(
        !finds("from:ada@xn--bcher-kva.example", &from("ada@bücher.example"))
            && !finds("from:ada@bücher.example", &from("ada@xn--bcher-kva.example"))
    );
    // A message with no sender address is found by no address.
    assert!(!finds("from:ada@example.org", &from("")) && !finds("from:ada@example.org", &Message::default()));
}

#[test]
fn what_is_a_whole_address() {
    let decomposed = "u\u{0308}nal@bu\u{0308}cher.example";
    for v in [
        "ada@example.org",
        "a@b",
        "ada+news@example.org",
        "Ada@Example.ORG",
        "o'neil@mail.example",
        "ünal@bücher.example",
        decomposed,
        "\"ada\"@example.org",
        "\"ada smith\"@example.org",
        "\"a@b\"@example.org",
        "\"say \\\"hi\\\"\"@example.org",
        "\"back\\\\slash\"@example.org",
    ] {
        assert!(is_address(v), "{v}");
    }
    for v in [
        "ada",
        "@example.org",
        "ada@",
        "ada@@example.org",
        "a@b@c",
        "ada @example.org",
        "\"ada@example.org",
        "\"ada\"x@example.org",
        "\"a\tb\"@example.org",
        "\"ada\\\"@example.org",
        "\"a\"@b@example.org",
        "@",
        "-@-",
        "<ada@example.org>",
        "ada@example.org,",
        "ada@example.org)",
        "(ada@example.org",
        "Ada<ada@example.org>",
        "ada@example.org.",
        "ada@-example.org",
        ".ada@example.org",
        "ada.@example.org",
        "ada..smith@example.org",
        "ada@example..org",
        "ada@.example.org",
    ] {
        assert!(!is_address(v), "{v}");
    }
    // More round an address is words, as before: it still finds it.
    for q in ["from:<ada@example.org>", "from:ada@example.org,", "from:(ada@example.org)"] {
        assert!(finds(q, &msg()) && !Query::parse(q).is_address_term(0), "{q}");
    }
}

/// The server is given the address whole, and what it finds is looked at here too, so that both
/// searches give the same answer.
#[test]
fn a_server_is_given_the_address_whole_and_its_results_are_checked() {
    let split = Query::parse("from:Ada@Example.org budget").for_server();
    assert_eq!(
        split.terms,
        [
            ServerTerm::Words { field: Some(ServerField::From), words: vec!["ada@example.org".into()] },
            ServerTerm::Words { field: None, words: vec!["budget".into()] },
        ]
    );
    assert_eq!(split.local.len(), 1);
    assert!(split.local.matches(&msg(), 0) && !split.local.matches(&from("adam@example.org"), 0));
    // More round an address is words here, and a server would read the address in it as that
    // address: it stays here alone, and still finds the words.
    let words = Query::parse("from:<ada@example.org> budget").for_server();
    assert_eq!(words.terms, [ServerTerm::Words { field: None, words: vec!["budget".into()] }]);
    assert!(words.local.len() == 1 && words.local.matches(&from("adam@example.org"), 0));
    // One too long for a server's word, or with what a server's search can't take, stays here
    // alone, and still finds it.
    let long = format!("{}@example.org", "a".repeat(70));
    for address in [long.as_str(), "ada!news@example.org"] {
        let split = Query::parse(&format!("from:{address} budget")).for_server();
        assert_eq!(split.terms, [ServerTerm::Words { field: None, words: vec!["budget".into()] }], "{address}");
        assert!(split.local.matches(&from(address), 0) && !split.local.matches(&msg(), 0), "{address}");
    }
}

/// A phrase longer than a server is given is looked at here too, so the server's answer is
/// checked for what it wasn't asked.
#[test]
fn what_a_server_is_given_less_of_is_looked_at_here() {
    let q = "from:ada@example.org subject:\"one two three four five six seven eight nine\"";
    let split = Query::parse(q).for_server();
    assert_eq!(split.terms.len(), 2);
    assert_eq!(split.local.len(), 2);
    let eight = Message { subject: "one two three four five six seven eight".into(), ..msg() };
    let nine = Message { subject: "one two three four five six seven eight nine".into(), ..msg() };
    assert!(!split.local.matches(&eight, 0) && split.local.matches(&nine, 0));
    // In the body or an attachment's name, the server's alone: a message found there has only a
    // preview of its body here, and no attachments.
    for q in [
        "body:\"one two three four five six seven eight nine\"",
        "attachment:\"one two three four five six seven eight\"",
        "\"one two three four five six seven eight nine\"",
    ] {
        let split = Query::parse(q).for_server();
        assert!(split.terms.len() == 1 && split.local.is_empty(), "{q}");
    }
    // A phrase a server is given whole is the server's alone, as before.
    assert_eq!(Query::parse("from:ada@example.org subject:\"budget review\"").for_server().local.len(), 1);
}

/// A plus-address stays in one piece, for the server too; elsewhere `+` parts words.
#[test]
fn a_plus_address_stays_whole() {
    let split = Query::parse("from:ada+news@example.org budget").for_server();
    assert_eq!(split.terms[0], ServerTerm::Words { field: Some(ServerField::From), words: vec!["ada+news@example.org".into()] });
    assert!(split.local.matches(&from("ada+news@example.org"), 0) && !split.local.matches(&msg(), 0));
    assert!(!finds("from:ada+news@example.org", &from("ada+other@example.org")));
    // Typed as a word of its own, or among others: one word for the server.
    let bare = Query::parse("ada+news@example.org").for_server();
    assert_eq!(bare.terms, [ServerTerm::Words { field: None, words: vec!["ada+news@example.org".into()] }]);
    let among = Query::parse("subject:\"from ada+news@example.org today\"").for_server();
    assert_eq!(
        among.terms,
        [ServerTerm::Words { field: Some(ServerField::Subject), words: vec!["from".into(), "ada+news@example.org".into(), "today".into()] }]
    );
    // Not an address: `+` parts it, as every other mark does.
    let words = Query::parse("c+plus +budget+").for_server();
    assert_eq!(words.terms[0], ServerTerm::Words { field: None, words: vec!["c".into(), "plus".into()] });
    assert_eq!(words.terms[1], ServerTerm::Words { field: None, words: vec!["budget".into()] });
}

/// A word index narrows the messages by an address's words, and the address is looked at on
/// each; the search box draws its keyword as a pill like any other.
#[test]
fn an_address_term_is_a_term_like_the_others() {
    let q = Query::parse("from:ada@example.org");
    assert!(q.is_address_term(0) && !Query::parse("from:ada").is_address_term(0));
    assert_eq!(q.words_of(0), Some((FROM, &["ada".to_string(), "example".into(), "org".into()][..])));
    assert!(!q.reads_bodies() && !q.is_empty());
    assert_eq!(Query::parse("ada@example.org").words_of(0).map(|(fields, _)| fields), Some(ALL));
    for s in ["from:ada@example.org", "-to:ada+news@example.org", "cc: ada@example.org"] {
        let spans = keyword_spans(s);
        assert_eq!(spans.len(), 1, "{s}");
        assert_eq!(spans.first().map(|k| k.range.end), Some(s.len()), "{s}");
        assert_eq!(keyword_pills(s, None).len(), 1, "{s}");
    }
}

/// A value written by [`keyword_value`] after a keyword is read back as it is, whatever is in it,
/// and nothing of it becomes another term.
#[test]
fn a_quoted_address_is_read_back_whole() {
    for address in [
        "ada@example.org",
        "\"ada smith\"@example.org",
        "\"say \\\"hi\\\"\"@example.org",
        "\"back\\\\slash\"@example.org",
        "\"x\\\" from:eve@example.org -is:read \"@example.org",
        "\"-from:\\\"eve\\\" is:unread\"@example.org",
    ] {
        let written = format!("from:{}", keyword_value(address));
        let q = Query::parse(&written);
        assert!(q.len() == 1 && q.is_address_term(0), "{address}: {written}");
        assert!(q.matches(&from(address), 0) && !q.matches(&from("eve@example.org"), 0), "{address}");
        assert_eq!(keyword_spans(&written).len(), 1, "{written}");
    }
    assert_eq!(keyword_value("ada@example.org"), "ada@example.org");
    assert_eq!(keyword_value("\"a b\"@example.org"), r#""\"a b\"@example.org""#);
    assert_eq!(keyword_value(""), r#""""#);
    // Outside quotes a backslash is a backslash, and inside them before anything else too.
    assert!(finds(r"from:a\b", &from(r"a\b")) && finds(r#"subject:"q4\plan""#, &Message { subject: r"Q4\plan".into(), ..msg() }));
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    /// Whatever a sender's address is, written by [`keyword_value`] after `from:` it is one term,
    /// and as an address it finds itself.
    #[test]
    fn whatever_the_address_it_is_read_back_as_written(name in "[ -~]{0,30}", domain in "[a-z]{1,8}\\.[a-z]{2,4}") {
        let address = format!("\"{}\"@{domain}", name.replace('\\', "\\\\").replace('"', "\\\""));
        let q = Query::parse(&format!("from:{}", keyword_value(&address)));
        proptest::prop_assert!(q.len() <= 1);
        if is_address(&address) {
            proptest::prop_assert!(q.is_address_term(0) && q.matches(&from(&address), 0));
        }
    }
}
