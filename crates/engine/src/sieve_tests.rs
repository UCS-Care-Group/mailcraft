// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sieve script MailCraft writes and reads back.

use mailcraft_model::auto_replies::{AutoReplyStatus, ExternalAudience};
use proptest::prelude::*;

use crate::sieve::*;

fn owner() -> Owner {
    Owner { domain: "example.org".into(), addresses: vec!["ada@example.org".into(), "ada.lovelace@example.org".into()] }
}

fn on(internal: &str, external: &str) -> Vacation {
    Vacation {
        status: AutoReplyStatus::AlwaysEnabled,
        start: None,
        end: None,
        internal: internal.into(),
        external: external.into(),
        audience: ExternalAudience::All,
    }
}

/// The script's tokens with every string's value blanked: its shape.
fn shape(script: &str) -> Vec<Tok> {
    tokens(script).unwrap().into_iter().map(|t| if let Tok::Str(_) = t { Tok::Str(String::new()) } else { t }).collect()
}

/// 2026-11-24 08:00 UTC and a week later.
const START: i64 = 1_795_507_200;
const END: i64 = START + 7 * 86_400;

#[test]
fn replies_always_on_are_one_vacation_for_inside_and_one_for_outside() {
    let script = vacation_script(&on("In the office on Monday.", "Back on Monday."), &owner());
    assert_eq!(
        script,
        "# MailCraft: automatic replies kept on this server. Changed elsewhere, MailCraft leaves it as it is.\r\n\
         require [\"vacation\"];\r\n\
         if address :domain :is \"from\" \"example.org\" {\r\n    \
         vacation :days 7 :addresses [\"ada@example.org\", \"ada.lovelace@example.org\"] :handle \"mailcraft-internal\" \"In the office on Monday.\";\r\n\
         } else {\r\n    \
         vacation :days 7 :addresses [\"ada@example.org\", \"ada.lovelace@example.org\"] :handle \"mailcraft-external\" \"Back on Monday.\";\r\n\
         }\r\n"
    );
    assert_eq!(read_vacation(&script), Some(on("In the office on Monday.", "Back on Monday.")));
}

#[test]
fn a_time_range_is_a_currentdate_test_in_utc() {
    let vacation = Vacation { status: AutoReplyStatus::Scheduled, start: Some(START), end: Some(END), ..on("in", "out") };
    let script = vacation_script(&vacation, &owner());
    assert!(script.contains("require [\"vacation\", \"date\", \"relational\"];"), "{script}");
    assert!(
        script.contains(
            "if allof (currentdate :zone \"+0000\" :value \"ge\" \"iso8601\" \"2026-11-24T08:00:00\", currentdate :zone \"+0000\" :value \"lt\" \"iso8601\" \"2026-12-01T08:00:00\") {"
        ),
        "{script}"
    );
    assert_eq!(read_vacation(&script), Some(vacation));
}

#[test]
fn nobody_outside_gets_no_reply_outside() {
    let vacation = Vacation { audience: ExternalAudience::None, external: String::new(), ..on("in", "") };
    let script = vacation_script(&vacation, &owner());
    assert!(!script.contains("mailcraft-external") && !script.contains("} else {"), "{script}");
    assert_eq!(read_vacation(&script), Some(vacation));
}

#[test]
fn replies_off_keep_mail_and_send_nothing() {
    let vacation = Vacation { status: AutoReplyStatus::Disabled, ..on("in", "out") };
    let script = vacation_script(&vacation, &owner());
    // (No explicit `keep`: a rule after it that files mail away would leave a copy in the Inbox.)
    assert!(!script.contains("vacation ") && !script.contains("keep;") && !script.contains("require"), "{script}");
    assert_eq!(read_vacation(&script).map(|v| v.status), Some(AutoReplyStatus::Disabled));
}

#[test]
fn a_script_of_another_shape_is_not_read_as_mailcrafts() {
    for script in [
        "require \"vacation\";\r\nvacation \"Away\";\r\n",
        "",
        "# MailCraft: automatic replies kept on this server. Changed elsewhere, MailCraft leaves it as it is.\r\nvacation :handle \"other\" \"x\";\r\n",
        "# MailCraft: automatic replies kept on this server. Changed elsewhere, MailCraft leaves it as it is.\r\n\"unterminated",
    ] {
        assert_eq!(read_vacation(script), None, "{script:?}");
    }
}

#[test]
fn the_reader_knows_comments_text_blocks_numbers_and_escapes() {
    let script =
        "# a comment\r\n/* a\r\nblock */ if size :over 100K { vacation text: # note\r\nline one\r\n..dotted\r\n.\r\n; }\r\nx \"a\\\"b\\\\c\\d\";";
    assert_eq!(
        tokens(script).unwrap(),
        vec![
            Tok::Ident("if".into()),
            Tok::Ident("size".into()),
            Tok::Tag("over".into()),
            Tok::Num(100 * 1024),
            Tok::Punct('{'),
            Tok::Ident("vacation".into()),
            Tok::Str("line one\n.dotted\n".into()),
            Tok::Punct(';'),
            Tok::Punct('}'),
            Tok::Ident("x".into()),
            Tok::Str("a\"b\\cd".into()),
            Tok::Punct(';'),
        ]
    );
    for broken in ["\"open", "/* open", "text:\r\nno end\r\n", "text: junk\r\n.\r\n", "99999999999999999999G", "@", ":"] {
        assert_eq!(tokens(broken), None, "{broken:?}");
    }
    assert_eq!(tokens(&"#".repeat(MAX_SCRIPT + 1)), None);
}

#[test]
fn values_that_look_like_sieve_stay_values() {
    // (#2057) A reply, an address or a domain with quotes, backslashes, line breaks, lone dots,
    // comments, semicolons or braces in it is one string: the script has the shape it has with
    // plain words, and reads back as what was written.
    let nasty = "Away\" ; discard ; # \r\n.\r\n\\\";\nredirect \"x@example.net\";\r\n/* } */ text:\r\nhi\r\n.\r\n}";
    let plain = vacation_script(&on("in", "out"), &Owner { domain: "example.org".into(), addresses: vec!["ada@example.org".into()] });
    let script = vacation_script(&on(nasty, nasty), &Owner { domain: nasty.into(), addresses: vec![nasty.into()] });
    assert_eq!(shape(&script), shape(&plain));
    let read = read_vacation(&script).unwrap();
    assert_eq!(read.internal, as_written(nasty));
    assert_eq!(read.external, as_written(nasty));
}

proptest! {
    #[test]
    fn any_value_is_one_string(text in proptest::collection::vec(prop_oneof![
        Just('"'), Just('\\'), Just('\r'), Just('\n'), Just('.'), Just('#'), Just(';'), Just('{'), Just('}'), Just('/'), Just('*'),
        Just('\0'), Just('\u{7}'), any::<char>(),
    ], 0..200).prop_map(|cs| cs.into_iter().collect::<String>())) {
        prop_assert_eq!(tokens(&quoted(&text)), Some(vec![Tok::Str(as_written(&text))]));
    }

    #[test]
    fn any_replies_keep_the_scripts_shape(internal in ".{0,80}", external in ".{0,80}", domain in ".{0,30}", address in ".{0,30}") {
        let owner = Owner { domain: if domain.is_empty() { "x".into() } else { domain }, addresses: vec![address] };
        let plain = vacation_script(&on("a", "b"), &Owner { domain: "x".into(), addresses: vec!["c".into()] });
        let script = vacation_script(&on(&internal, &external), &owner);
        prop_assert_eq!(shape(&script), shape(&plain));
        let read = read_vacation(&script).unwrap();
        prop_assert_eq!(read.internal, as_written(&internal));
        prop_assert_eq!(read.external, as_written(&external));
    }

    #[test]
    fn arbitrary_scripts_never_panic(script in ".{0,400}") {
        let _ = tokens(&script);
        let _ = read_vacation(&script);
        let with_header = format!("{HEADER}\r\n{script}");
        let _ = read_vacation(&with_header);
    }
}
