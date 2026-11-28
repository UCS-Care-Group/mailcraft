// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local Sieve check: valid scripts pass, each kind of mistake is found where it is, a block
//! for MailCraft's script says what it needs, and nothing a script holds makes it panic.

use proptest::prelude::*;

use crate::sieve::LexFault;
use crate::sieve_check::*;

/// The errors of a script that should have some.
fn errors(script: &str) -> Vec<Problem> {
    match check(script, None) {
        Ok(c) => panic!("no error in {script:?}: {c:?}"),
        Err(p) => p.into_iter().filter(|p| !p.warning).collect(),
    }
}

/// The first error's fault, line, column and bytes.
fn first(script: &str) -> (Fault, usize, usize, std::ops::Range<usize>) {
    let p = errors(script).into_iter().next().unwrap();
    (p.fault, p.line, p.column, p.at)
}

fn s(x: &str) -> String {
    x.to_string()
}

#[test]
fn valid_scripts_pass() {
    let scripts: &[(&str, &[&str])] = &[
        ("", &[]),
        ("keep;", &[]),
        ("# just a comment\r\n/* and\r\nanother */\r\nstop;", &[]),
        ("require \"fileinto\";\r\nif header :contains \"subject\" \"report\" { fileinto \"Reports\"; stop; }", &["fileinto"]),
        (
            "require [\"fileinto\", \"copy\", \"imap4flags\"];\nif anyof (address :domain :is \"from\" \"example.org\", header :is [\"X-Spam-Flag\"] \"YES\") {\n  fileinto :copy :flags \"\\\\Seen\" \"Junk\";\n} elsif not exists \"date\" {\n  addflag \"\\\\Flagged\";\n} else {\n  keep;\n}\n",
            &["fileinto", "copy", "imap4flags"],
        ),
        (
            "require [\"vacation\", \"date\", \"relational\"];\nif allof (currentdate :zone \"+0000\" :value \"ge\" \"iso8601\" \"2026-11-24T08:00:00\", true) {\n  vacation :days 7 :addresses [\"ada@example.org\"] :handle \"h\" text:\nAway.\n..dot\n.\n;\n}\n",
            &["date", "relational", "vacation"],
        ),
        ("require [\"body\", \"variables\"];\nif body :text :contains \"invoice\" { set :lower \"x\" \"${1}\"; }", &["body", "variables"]),
        ("if size :over 100K { discard; }", &[]),
        (
            "require \"comparator-i;ascii-numeric\";\nif header :comparator \"i;ascii-numeric\" :is \"x-n\" \"1\" { keep; }",
            &["comparator-i;ascii-numeric"],
        ),
        ("require \"editheader\";\ndeleteheader :index 1 \"X-A\";\ndeleteheader \"X-B\" [\"a\", \"b\"];", &["editheader"]),
        ("require \"imap4flags\";\nsetflag \"var\" [\"\\\\Seen\"];\nsetflag \"\\\\Seen\";", &["imap4flags"]),
        (
            "require [\"spamtestplus\", \"relational\"];\nif spamtest :percent :value \"gt\" :comparator \"i;ascii-casemap\" \"50\" { discard; }",
            &["spamtest", "spamtestplus", "relational"],
        ),
    ];
    for (script, needs) in scripts {
        let checked = check(script, None).unwrap_or_else(|p| panic!("{script:?}: {p:?}"));
        assert!(checked.warnings.is_empty(), "{script:?}: {:?}", checked.warnings);
        assert_eq!(checked.needs, needs.iter().map(|n| s(n)).collect::<Vec<_>>(), "{script:?}");
    }
}

#[test]
fn each_mistake_is_found_where_it_is() {
    use Fault::*;
    let cases: Vec<(&str, Fault, usize, usize, std::ops::Range<usize>)> = vec![
        ("keep;\n\"open", Lex(LexFault::String), 2, 1, 6..7),
        ("keep; /* never closed", Lex(LexFault::Comment), 1, 7, 6..8),
        ("vacation text:\nno end\n", Lex(LexFault::Text), 1, 10, 9..14),
        ("vacation text: x\n.\n", Lex(LexFault::TextStart), 1, 10, 9..14),
        ("if size :over 99999999999999999999 {}", Lex(LexFault::Number), 1, 15, 14..34),
        ("keep; : x;", Lex(LexFault::Tag), 1, 7, 6..7),
        ("keep;\n  é;", Lex(LexFault::Char), 2, 3, 8..10),
        ("éé fileinto", Lex(LexFault::Char), 1, 1, 0..2),
        ("\"x\";", ExpectedCommand, 1, 1, 0..3),
        ("keep", MissingSemicolon, 1, 5, 4..4),
        ("discard\nstop;", MissingSemicolon, 1, 8, 7..7),
        ("if true {\n  keep\n}", MissingSemicolon, 2, 7, 16..16),
        ("if true {\n  keep;\n", UnclosedBlock, 1, 9, 8..9),
        ("keep;\n}", StrayClose, 2, 1, 6..7),
        ("if anyof (true false) {}", NoTest(s("true")), 1, 11, 10..14),
        ("if anyof (true; ) {}", TestListEnd, 1, 15, 14..15),
        ("if anyof () {}", ExpectedTest, 1, 11, 10..11),
        ("if header :is [\"a\" \"b\"] \"c\" {}", ListEnd, 1, 20, 19..22),
        ("if header :is [1] \"c\" {}", ExpectedString, 1, 16, 15..16),
        ("frobnicate;", UnknownCommand(s("frobnicate")), 1, 1, 0..10),
        ("if frob { keep; }", UnknownTest(s("frob")), 1, 4, 3..7),
        ("keep;\nfileinto \"x\";", NeedsRequire { name: s("fileinto"), ext: s("fileinto") }, 2, 1, 6..14),
        ("redirect :copy \"a@example.org\";", NeedsRequire { name: s(":copy"), ext: s("copy") }, 1, 10, 9..14),
        ("keep;\nelsif true {}", ElseWithoutIf(s("elsif")), 2, 1, 6..11),
        ("else { keep; }", ElseWithoutIf(s("else")), 1, 1, 0..4),
        ("if true {} keep; else {}", ElseWithoutIf(s("else")), 1, 18, 17..21),
        ("header :is \"a\" \"b\";", TestAsCommand(s("header")), 1, 1, 0..6),
        ("if keep { }", CommandAsTest(s("keep")), 1, 4, 3..7),
        ("keep;\nrequire \"fileinto\";", RequireNotFirst, 2, 1, 6..13),
        ("if { keep; }", NeedsTest(s("if")), 1, 1, 0..2),
        ("if true;", NeedsBlock(s("if")), 1, 1, 0..2),
        ("keep { }", NoBlock(s("keep")), 1, 1, 0..4),
        ("if not (true) {}", OneTest(s("not")), 1, 4, 3..6),
        ("if allof true {}", NeedsTestList(s("allof")), 1, 4, 3..8),
        ("if header :is :is \"a\" \"b\" {}", TagTwice(s("is")), 1, 15, 14..17),
        ("if header :is :contains \"a\" \"b\" {}", TagClash { tag: s("contains"), other: s("is") }, 1, 15, 14..23),
        ("if header \"a\" :is \"b\" {}", TagAfterPositional(s("is")), 1, 15, 14..17),
        ("if header :frob \"a\" \"b\" {}", UnknownTag { tag: s("frob"), name: s("header") }, 1, 11, 10..15),
        ("if size :over \"1\" {}", WrongArg { name: s("size"), wants: Wants::Num }, 1, 15, 14..17),
        ("require \"vacation\";\nvacation :days \"7\" \"Away\";", TagValue { tag: s("days"), wants: Wants::Num }, 2, 16, 35..38),
        ("if header :is \"a\" {}", MissingArg(s("header")), 1, 4, 3..9),
        ("redirect \"a@example.org\" \"b@example.org\";", ExtraArg(s("redirect")), 1, 26, 25..40),
        ("if size 100 {}", NeedsTag { name: s("size"), tags: s(":over, :under") }, 1, 4, 3..7),
        ("require \"relational\";\nif header :value \"gx\" \"a\" \"1\" {}", BadRelation(s("gx")), 2, 18, 39..43),
        ("if header :comparator \"i;nope\" :is \"a\" \"b\" {}", UnknownComparator(s("i;nope")), 1, 23, 22..30),
        (
            "if header :comparator \"i;ascii-numeric\" :is \"a\" \"1\" {}",
            NeedsRequire { name: s(":comparator \"i;ascii-numeric\""), ext: s("comparator-i;ascii-numeric") },
            1,
            23,
            22..39,
        ),
    ];
    for (script, fault, line, column, at) in cases {
        assert_eq!(first(script), (fault, line, column, at), "{script:?}");
    }
}

#[test]
fn what_an_ihave_guards_is_only_a_warning() {
    let checked = check("require \"ihave\";\nif ihave \"vnd.example.thing\" { thing \"x\"; }\n", None).unwrap();
    let w: Vec<(Fault, bool)> = checked.warnings.iter().map(|w| (w.fault.clone(), w.warning)).collect();
    assert_eq!(w, vec![(Fault::UnknownCommand(s("thing")), true)]);
    // Outside it, and in a block, it is an error.
    assert_eq!(first("require \"ihave\";\nif ihave \"vnd.example.thing\" { keep; }\nthing;").0, Fault::UnknownCommand(s("thing")));
    assert!(check_block("if ihave \"vnd.example.thing\" { thing; }", None).is_err());
}

#[test]
fn every_error_of_a_script_is_said_in_order() {
    let p = errors("fileinto \"a\";\nfrob;\nif header :is \"a\" {}");
    let faults: Vec<(usize, Fault)> = p.into_iter().map(|p| (p.line, p.fault)).collect();
    assert_eq!(
        faults,
        vec![
            (1, Fault::NeedsRequire { name: s("fileinto"), ext: s("fileinto") }),
            (2, Fault::UnknownCommand(s("frob"))),
            (3, Fault::MissingArg(s("header"))),
        ]
    );
}

#[test]
fn columns_count_characters_and_lines_count_line_feeds() {
    // "ü" is two bytes, one character; a CR before the LF ends no line of its own.
    let (_, line, column, at) = first("# über\r\nkeep; # ü\r\n  frob;");
    assert_eq!((line, column, at), (3, 3, 23..27));
}

#[test]
fn an_unknown_extension_is_a_warning_and_what_it_may_add_too() {
    let checked = check("require [\"vnd.example.x\"];\nfrobnicate :loud;\nif header :wild \"a\" \"b\" {}", None).unwrap();
    let faults: Vec<Fault> = checked.warnings.iter().map(|w| w.fault.clone()).collect();
    assert_eq!(
        faults,
        vec![
            Fault::UnknownExtension(s("vnd.example.x")),
            Fault::UnknownCommand(s("frobnicate")),
            Fault::UnknownTag { tag: s("wild"), name: s("header") },
        ]
    );
    assert_eq!(checked.requires, vec![s("vnd.example.x")]);
    // A server that says it has it knows what it adds: no warning for the require itself.
    let server = vec![s("vnd.example.x")];
    let checked = check("require \"vnd.example.x\";\nfrobnicate;", Some(&server)).unwrap();
    assert_eq!(checked.warnings.iter().map(|w| w.fault.clone()).collect::<Vec<_>>(), vec![Fault::UnknownCommand(s("frobnicate"))]);
    // The words of each are said.
    assert!(checked.warnings.first().unwrap().what().to_string().contains("frobnicate"));
}

#[test]
fn an_extension_the_server_lacks_is_a_warning() {
    let server = vec![s("fileinto"), s("envelope")];
    let checked = check("require [\"fileinto\", \"vacation\"];\nfileinto \"x\";\nvacation \"Away\";", Some(&server)).unwrap();
    assert_eq!(checked.warnings.len(), 1);
    let w = checked.warnings.first().unwrap();
    assert_eq!((w.fault.clone(), w.line, w.column, w.warning), (Fault::ServerLacks(s("vacation")), 1, 9, true));
    assert_eq!(w.what().to_string(), "The server doesn't have the extension vacation.");
    // Errors and warnings come together, in order.
    let p = check("require \"vacation\";\nfrob;", Some(&server)).unwrap_err();
    assert_eq!(p.iter().map(|p| p.warning).collect::<Vec<_>>(), vec![true, false]);
}

#[test]
fn a_block_says_what_it_needs_and_may_not_require() {
    let block = "if header :contains \"subject\" \"report\" {\r\n  fileinto :copy \"Reports\";\r\n  addflag \"\\\\Seen\";\r\n}\r\n";
    let checked = check_block(block, None).unwrap();
    assert_eq!(checked.needs, vec![s("fileinto"), s("copy"), s("imap4flags")]);
    assert!(checked.requires.is_empty());
    assert!(checked.warnings.is_empty());
    let p = check_block("require \"fileinto\";\nfileinto \"x\";", None).unwrap_err();
    assert_eq!((p[0].fault.clone(), p[0].line, p[0].column), (Fault::RequireInBlock, 1, 1));
    // What isn't known is refused in a block, required or not.
    let p = check_block("frobnicate;", None).unwrap_err();
    assert_eq!(p[0].fault, Fault::UnknownCommand(s("frobnicate")));
    let p = check_block("if header :comparator \"i;nope\" :is \"a\" \"b\" { keep; }", None).unwrap_err();
    assert_eq!(p[0].fault, Fault::UnknownComparator(s("i;nope")));
    // Nothing that reaches out of it: an else joining the rule before, a } too many, a comment or
    // a string left open.
    for (bad, fault) in [
        ("else { discard; }", Fault::ElseWithoutIf(s("else"))),
        ("elsif true { discard; }", Fault::ElseWithoutIf(s("elsif"))),
        ("keep; }", Fault::StrayClose),
        ("keep; /* the rest", Fault::Lex(LexFault::Comment)),
        ("fileinto \"x;", Fault::Lex(LexFault::String)),
        ("if true { keep;", Fault::UnclosedBlock),
    ] {
        assert_eq!(check_block(bad, None).unwrap_err()[0].fault, fault, "{bad:?}");
    }
    // The server's extensions: one the block needs that it lacks is a warning, at its first use.
    let server = vec![s("fileinto")];
    let checked = check_block("fileinto :copy \"x\";", Some(&server)).unwrap();
    let w: Vec<(Fault, usize)> = checked.warnings.iter().map(|w| (w.fault.clone(), w.column)).collect();
    assert_eq!(w, vec![(Fault::ServerLacks(s("copy")), 10)]);
}

#[test]
fn the_table_says_what_each_name_needs() {
    assert_eq!(needs_of("fileinto", None), Some(&["fileinto"][..]));
    assert_eq!(needs_of("keep", None), Some(&[][..]));
    assert_eq!(needs_of("fileinto", Some("copy")), Some(&["copy"][..]));
    assert_eq!(needs_of("keep", Some("flags")), Some(&["imap4flags"][..]));
    assert_eq!(needs_of("header", Some("last")), Some(&["index"][..]));
    assert_eq!(needs_of("deleteheader", Some("last")), Some(&[][..]));
    assert_eq!(needs_of("global", None), Some(&["include", "variables"][..]));
    assert_eq!(needs_of("frobnicate", None), None);
    assert_eq!(needs_of("keep", Some("copy")), None);
    // Every extension the table names is one known.
    for name in ["fileinto", "vacation", "set", "date", "notify", "addheader", "foreverypart", "spamtest", "duplicate"] {
        for ext in needs_of(name, None).unwrap() {
            assert!(EXTENSIONS.contains(ext), "{ext}");
        }
    }
}

#[test]
fn the_caps_hold() {
    // Nesting.
    let deep = format!("{}keep;{}", "if true {".repeat(MAX_DEPTH + 2), "}".repeat(MAX_DEPTH + 2));
    assert_eq!(first(&deep).0, Fault::TooDeep);
    let deep_test = format!("if {}true {{}}", "not ".repeat(MAX_DEPTH + 2));
    assert_eq!(first(&deep_test).0, Fault::TooDeep);
    // Commands.
    assert_eq!(first(&"keep;".repeat(MAX_COMMANDS + 1)).0, Fault::TooManyCommands);
    assert!(check(&"keep;".repeat(MAX_COMMANDS), None).is_ok());
    // Arguments and tests.
    assert_eq!(first(&format!("keep {};", "1 ".repeat(MAX_ARGS + 1))).0, Fault::TooManyArgs);
    assert_eq!(first(&format!("if anyof ({}) {{}}", vec!["true"; MAX_ARGS + 1].join(","))).0, Fault::TooManyArgs);
    // Problems.
    let many = errors(&"frob;".repeat(MAX_PROBLEMS * 2));
    assert_eq!(many.len(), MAX_PROBLEMS);
    // A long name is cut in what is said.
    let long = "x".repeat(10_000);
    match first(&format!("{long};")).0 {
        Fault::UnknownCommand(n) => assert!(n.len() <= 64),
        f => panic!("{f:?}"),
    }
}

#[test]
fn every_problem_says_something() {
    let scripts = [
        "\"x",
        "/*",
        "text: x\n.\n",
        ":",
        "9999999999999999999999;",
        "é",
        "\"x\";",
        "keep",
        "{",
        "}",
        "if anyof (true true) {}",
        "if anyof () {}",
        "if header :is [\"a\" \"b\"] \"c\" {}",
        "if header :is [1] \"c\" {}",
        "frob;",
        "if frob {}",
        "fileinto \"x\";",
        "else {}",
        "header \"a\" \"b\";",
        "if keep {}",
        "keep; require \"x\";",
        "if {}",
        "if true;",
        "keep {}",
        "if not (true) {}",
        "if allof true {}",
        "if header :is :is \"a\" \"b\" {}",
        "if header :is :contains \"a\" \"b\" {}",
        "if header \"a\" :is \"b\" {}",
        "if header :x \"a\" \"b\" {}",
        "if size :over \"1\" {}",
        "redirect :copy;",
        "if header :is \"a\" {}",
        "redirect \"a\" \"b\";",
        "if size 1 {}",
        "if header :value \"x\" \"a\" \"b\" {}",
        "if header :comparator \"x\" \"a\" \"b\" {}",
    ];
    for script in scripts {
        for p in check(script, None).unwrap_err() {
            let said = p.what().to_string();
            assert!(!said.is_empty() && !["{n}", "{name}", "{tag}", "{ext}", "{mb}"].iter().any(|v| said.contains(v)), "{script:?}: {said}");
        }
    }
}

/// Every problem's bytes are within the script, on character boundaries, and its line and column
/// are where those bytes begin.
fn sound(script: &str, problems: &[Problem]) {
    for p in problems {
        assert!(p.at.start <= p.at.end && p.at.end <= script.len(), "{p:?}");
        assert!(script.is_char_boundary(p.at.start) && script.is_char_boundary(p.at.end), "{p:?}");
        let before = &script[..p.at.start];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().unwrap().chars().count() + 1;
        assert_eq!((p.line, p.column), (line, column), "{p:?}");
        let _ = p.what().to_string();
    }
}

fn check_all(script: &str) {
    let server = vec![s("fileinto"), s("vacation")];
    for result in [check(script, None), check(script, Some(&server)), check_block(script, None), check_block(script, Some(&server))] {
        match result {
            Ok(c) => {
                assert!(c.warnings.iter().all(|w| w.warning));
                sound(script, &c.warnings);
            }
            Err(p) => {
                assert!(p.iter().any(|p| !p.warning));
                assert!(p.len() <= MAX_PROBLEMS);
                sound(script, &p);
            }
        }
    }
}

/// Words of Sieve, put together in any order.
fn soup() -> impl Strategy<Value = String> {
    let words = prop::sample::select(vec![
        "require",
        "if",
        "elsif",
        "else",
        "stop",
        "keep",
        "discard",
        "fileinto",
        "redirect",
        "header",
        "address",
        "envelope",
        "exists",
        "size",
        "anyof",
        "allof",
        "not",
        "true",
        "false",
        "vacation",
        "set",
        "string",
        "date",
        "currentdate",
        "body",
        "ihave",
        "frob",
        ":is",
        ":contains",
        ":matches",
        ":value",
        ":count",
        ":comparator",
        ":copy",
        ":days",
        ":over",
        ":under",
        ":frob",
        ":index",
        ":last",
        "\"fileinto\"",
        "\"vnd.x\"",
        "\"i;ascii-numeric\"",
        "\"ge\"",
        "\"x\"",
        "\"ü\"",
        "[",
        "]",
        "(",
        ")",
        "{",
        "}",
        ",",
        ";",
        "1",
        "10K",
        "\n",
        "# c\n",
        "/* c */",
        "text:\nhi\n.\n",
        "é",
        "\"",
        "/*",
    ]);
    prop::collection::vec(words, 0..60).prop_map(|w| w.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_text_never_panics(script in "\\PC{0,200}") {
        check_all(&script);
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        check_all(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn sieve_words_in_any_order_never_panic(script in soup()) {
        check_all(&script);
    }
}

#[test]
fn the_scripts_mailcraft_writes_pass() {
    use mailcraft_model::auto_replies::{AutoReplyStatus, ExternalAudience};

    use crate::sieve::{Owner, Vacation, vacation_script};
    let owner = Owner { domain: "example.org".into(), addresses: vec!["ada@example.org".into()] };
    for status in [AutoReplyStatus::AlwaysEnabled, AutoReplyStatus::Scheduled, AutoReplyStatus::Disabled] {
        let vacation = Vacation {
            status,
            start: Some(1_795_507_200),
            end: Some(1_796_112_000),
            internal: "In \"quotes\" \\ and\nlines.".into(),
            external: "Away.".into(),
            audience: ExternalAudience::All,
        };
        let script = vacation_script(&vacation, &owner);
        let server = vec![s("vacation"), s("date"), s("relational")];
        let checked = check(&script, Some(&server)).unwrap_or_else(|p| panic!("{script}: {p:?}"));
        assert!(checked.warnings.is_empty(), "{script}: {:?}", checked.warnings);
        let (mut requires, mut needs) = (checked.requires, checked.needs);
        requires.sort();
        needs.sort();
        assert_eq!(requires, needs, "{script}");
    }
}

/// A raw NUL is refused wherever it is, a comment too (#2058).
#[test]
fn a_raw_nul_is_refused_even_in_a_comment() {
    let (fault, line, _, at) = first("# a\0b\r\nkeep;\r\n");
    assert_eq!((fault, line, at), (Fault::Control, 1, 3..4));
    assert!(check_block("keep; # \0\r\n", None).is_err());
}

/// A script with more warnings than are kept still fails on its error (#2058).
#[test]
fn an_error_after_a_full_list_of_warnings_still_fails() {
    let names: Vec<String> = (0..MAX_PROBLEMS + 5).map(|n| format!("\"x-unknown-{n}\"")).collect();
    let script = format!("require [{}];\r\nkeep \"extra\";\r\n", names.join(", "));
    let problems = check(&script, None).expect_err("an error");
    assert!(problems.len() <= MAX_PROBLEMS);
    assert!(problems.iter().any(|p| !p.warning), "{problems:?}");
}
