// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A problem report: the relay's rules, kept here, and the diagnostics fitted to its room.

use proptest::prelude::*;
use serde_json::Value;

use crate::report::*;

fn report() -> ProblemReport {
    ProblemReport::new("Search stops", "I searched and it stopped", "0.9.0", "linux x86_64", "", Some("### MailCraft diagnostics\n"))
}

#[test]
fn the_body_holds_the_parts_and_the_empty_honeypot_and_nothing_else() {
    let mut r = report();
    r.username = Some("octo-cat".into());
    let body: Value = serde_json::from_str(&r.json().unwrap()).unwrap();
    let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys.len(), 7, "{keys:?}");
    for k in ["title", "text", "version", "platform", "username", "diagnostics", "website"] {
        assert!(keys.contains(&k), "{k}");
    }
    assert_eq!(body["website"], "");
    assert_eq!(body["username"], "octo-cat");
    // Without a username or diagnostics, they are left out rather than sent empty.
    let r = ProblemReport::new("t", "x", "1", "p", "  ", None);
    let body: Value = serde_json::from_str(&r.json().unwrap()).unwrap();
    assert!(body.get("username").is_none() && body.get("diagnostics").is_none());
}

#[test]
fn what_the_window_holds_is_tidied_as_the_relay_reads_it_and_never_cut() {
    let r = ProblemReport::new("  A\ntitle\u{7}  ", "  line one\n\tline two\u{0}\u{1b}  ", " 0.9.0\r\n", "linux", " @octocat ", Some("log\u{8}\n"));
    assert_eq!(r.title, "A title");
    assert_eq!(r.text, "line one\n\tline two");
    assert_eq!(r.version, "0.9.0");
    assert_eq!(r.username.as_deref(), Some("octocat"));
    assert_eq!(r.diagnostics.as_deref(), Some("log\n"));
    assert_eq!(r.check(), Ok(()));
    // A byte-order mark alone is nothing, as the relay trims it.
    assert_eq!(ProblemReport::new("\u{feff}", "x", "1", "p", "", None).check(), Err(Field::Title));
    assert_eq!(ProblemReport::new("t", " \u{feff} ", "1", "p", "", None).check(), Err(Field::Text));
    let long = "x".repeat(MAX_TEXT + 1);
    let r = ProblemReport::new("t", &long, "1", "p", "", None);
    assert_eq!(r.text.len(), MAX_TEXT + 1);
    assert_eq!(r.json(), Err(Field::Text));
}

#[test]
fn each_rule_of_the_relay_is_kept_here() {
    let ok = report();
    assert_eq!(ok.check(), Ok(()));
    let with = |f: fn(&mut ProblemReport)| {
        let mut r = ok.clone();
        f(&mut r);
        r.check()
    };
    assert_eq!(with(|r| r.title.clear()), Err(Field::Title));
    assert_eq!(with(|r| r.title = "a".repeat(MAX_TITLE + 1)), Err(Field::Title));
    // Counted as the relay counts: an emoji is two.
    assert_eq!(with(|r| r.title = "😀".repeat(MAX_TITLE / 2)), Ok(()));
    assert_eq!(with(|r| r.title = "😀".repeat(MAX_TITLE / 2 + 1)), Err(Field::Title));
    assert_eq!(with(|r| r.title = "a\nb".into()), Err(Field::Title));
    assert_eq!(with(|r| r.text = "   ".into()), Err(Field::Text));
    assert_eq!(with(|r| r.text = "a\u{1}".into()), Err(Field::Text));
    assert_eq!(with(|r| r.text = "é".repeat(MAX_TEXT)), Ok(()));
    assert_eq!(with(|r| r.version = String::new()), Err(Field::Version));
    assert_eq!(with(|r| r.platform = "p".repeat(MAX_LINE + 1)), Err(Field::Platform));
    for bad in ["-a", "a-", "a--b", "a_b", "a b", "", &"a".repeat(40)] {
        let bad = bad.to_string();
        let mut r = ok.clone();
        r.username = Some(bad.clone());
        assert_eq!(r.check(), Err(Field::Username), "{bad}");
    }
    for good in ["a", "octo-cat", "A1", "@octocat", &"a".repeat(39)] {
        assert!(is_github_username(good), "{good}");
    }
    assert_eq!(with(|r| r.diagnostics = Some("x".repeat(MAX_DIAGNOSTICS + 1))), Err(Field::Diagnostics));
}

#[test]
fn a_request_over_the_whole_cap_is_refused_and_the_fullest_window_fits() {
    // The most the window lets through, written as long as JSON writes it, still fits.
    let diagnostics = fit_diagnostics(&"\"\\\n".repeat(60_000));
    let r = ProblemReport {
        title: "\"".repeat(MAX_TITLE),
        text: "我".repeat(MAX_TEXT),
        version: "\"".repeat(MAX_LINE),
        platform: "\"".repeat(MAX_LINE),
        username: Some("a".repeat(39)),
        diagnostics: Some(diagnostics),
    };
    let body = r.json().unwrap();
    assert!(body.len() <= MAX_REQUEST, "{}", body.len());
    // Diagnostics that weren't fitted, and are long in JSON, are refused as a whole.
    let r = ProblemReport { diagnostics: Some("\"".repeat(MAX_DIAGNOSTICS)), ..report() };
    assert_eq!(r.json(), Err(Field::Request));
}

#[test]
fn diagnostics_that_fit_are_kept_whole_and_long_ones_keep_their_head_and_end() {
    let short = "### MailCraft diagnostics\n\n- Version: 0.9.0\n";
    assert_eq!(fit_diagnostics(short), short);
    let mut long = String::from("### MailCraft diagnostics\n\n- Version: 0.9.0\n- Platform: linux x86_64\n\n```\n");
    for n in 0..2_000 {
        long.push_str(&format!("line {n}: {}\n", "x".repeat(60)));
    }
    long.push_str("```\nSend/Receive timings\n");
    let fitted = fit_diagnostics(&long);
    assert!(fitted.len() <= DIAGNOSTICS_ROOM, "{}", fitted.len());
    assert!(fitted.starts_with("### MailCraft diagnostics\n\n- Version: 0.9.0\n- Platform: linux x86_64\n"));
    assert!(fitted.ends_with("Send/Receive timings\n"));
    assert!(fitted.contains("earlier lines left out to fit the report"));
    // The oldest lines of the log go; the newest stay.
    assert!(!fitted.contains("line 100:") && fitted.contains("line 1999:"));
}

#[test]
fn the_relay_names_its_fields_as_they_are_here() {
    for f in [Field::Title, Field::Text, Field::Version, Field::Platform, Field::Username, Field::Diagnostics, Field::Request] {
        assert_eq!(Field::named(f.name()), Some(f));
    }
    assert_eq!(Field::named("website"), None);
}

proptest! {
    #[test]
    fn whatever_is_typed_the_report_never_panics_and_what_passes_fits(
        title in ".{0,300}", text in ".{0,800}", user in ".{0,50}", diag in proptest::option::of(".{0,2000}")
    ) {
        let r = ProblemReport::new(&title, &text, "0.9.0", "linux", &user, diag.as_deref());
        if let Ok(body) = r.json() {
            prop_assert!(body.len() <= MAX_REQUEST);
            let v: Value = serde_json::from_str(&body).unwrap();
            prop_assert_eq!(v["website"].as_str(), Some(""));
        }
        let fitted = fit_diagnostics(diag.as_deref().unwrap_or_default());
        prop_assert!(fitted.len() <= MAX_DIAGNOSTICS);
    }

    #[test]
    fn long_diagnostics_fit_the_room_in_json_and_keep_their_head(lines in proptest::collection::vec("[^\n]{0,400}", 0..400)) {
        let head = "### MailCraft diagnostics\n\n- Version: 0.9.0\n- Platform: linux x86_64\n";
        let text = format!("{head}{}", lines.join("\n"));
        let fitted = fit_diagnostics(&text);
        // (Written in JSON, less its two quotes.)
        let in_json = serde_json::to_string(&fitted).unwrap().len() - 2;
        prop_assert!(in_json <= DIAGNOSTICS_ROOM, "{}", in_json);
        prop_assert!(fitted.starts_with(head));
    }
}
