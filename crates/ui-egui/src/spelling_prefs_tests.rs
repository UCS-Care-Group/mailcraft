// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Spelling options kept in the interface state (#1797): defaults, a hostile settings file,
//! the commands, and as-you-type checking in the composer.

use std::ops::Range;

use egui::Context;
use mailcraft_spell::HostChecker;
use serde_json::{Value, json};

use crate::compose::editor;
use crate::{MailApp, UiState};

struct Marker;

impl HostChecker for Marker {
    fn locales(&self) -> Vec<String> {
        vec!["it-IT".to_string(), "fr-FR".to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != "it-IT" && locale != "fr-FR" {
            return None;
        }
        Some(text.match_indices("sopratutto").map(|(at, word)| at..at + word.len()).collect())
    }

    fn suggest(&self, _locale: &str, word: &str) -> Vec<String> {
        if word == "sopratutto" { vec!["soprattutto".into()] } else { Vec::new() }
    }
}

fn app() -> MailApp {
    MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services { spelling: Some(Box::new(Marker)), ..Default::default() },
    )
}

#[test]
fn defaults_and_an_older_settings_file_check_as_you_type() {
    let fresh = UiState::default();
    assert!(fresh.check_spelling_as_you_type);
    assert_eq!(fresh.spelling_language, "");
    assert!(fresh.custom_dictionary.is_empty());
    assert!(fresh.spelling_ignore_uppercase && fresh.spelling_ignore_numbers);

    let older: UiState = serde_json::from_str("{}").unwrap();
    assert!(older.check_spelling_as_you_type);
    assert_eq!(older.spelling_language, "");
    assert!(older.custom_dictionary.is_empty());
    assert!(older.spelling_ignore_uppercase && older.spelling_ignore_numbers);

    let off: UiState = serde_json::from_value(json!({
        "checkSpellingAsYouType": false,
        "spellingIgnoreUppercase": false,
        "spellingIgnoreNumbers": false,
        "spellingLanguage": "it-IT",
    }))
    .unwrap();
    assert!(!off.check_spelling_as_you_type);
    assert!(!off.spelling_ignore_uppercase && !off.spelling_ignore_numbers);
    assert_eq!(off.spelling_language, "it-IT");
}

#[test]
fn a_hostile_settings_file_is_cleaned_and_capped() {
    let mut words = vec![
        json!("ok"),
        json!("ok"),
        json!("bad word"),
        json!("x\u{1}y"),
        json!("a".repeat(70)),
        json!("Hello"),
        json!("hello"),
        json!("Colour"),
        json!("COLOUR"),
        json!(1),
    ];
    for n in 0..10_001 {
        words.push(json!(format!("w{n}")));
    }
    let saved = json!({
        "customDictionary": words,
        "checkSpellingAsYouType": "no",
        "spellingIgnoreUppercase": null,
        "spellingIgnoreNumbers": 1,
        "spellingLanguage": format!("en\u{0}{}", "x".repeat(80)),
    });
    let ui: UiState = serde_json::from_value(saved).unwrap();
    assert!(ui.check_spelling_as_you_type, "a non-boolean stays on");
    assert!(ui.spelling_ignore_uppercase && ui.spelling_ignore_numbers);
    assert!(!ui.spelling_language.chars().any(char::is_control));
    assert!(ui.spelling_language.chars().count() <= crate::spelling::MAX_LANGUAGE_CHARS);
    assert_ne!(ui.spelling_language, "off");
    assert_eq!(ui.custom_dictionary.len(), 10_000);
    assert_eq!(ui.custom_dictionary.first().map(String::as_str), Some("ok"));
    assert!(ui.custom_dictionary.iter().any(|word| word == "Hello"));
    assert!(ui.custom_dictionary.iter().any(|word| word == "Colour"));
    assert!(!ui.custom_dictionary.iter().any(|word| word == "hello" || word == "COLOUR" || word.contains(' ')));
    assert!(!ui.custom_dictionary.iter().any(|word| word.chars().count() > 64));
    assert!(!ui.custom_dictionary.iter().any(|word| word == "w10000"));

    // The same cleaning when a `UiState` is applied without going through the file reader.
    let built = UiState {
        custom_dictionary: (0..10_001).map(|n| format!("z{n}")).chain(["bad word".into(), "off".into()]).collect(),
        spelling_language: "off".into(),
        ..Default::default()
    };
    let mut window = app();
    window.apply_prefs(built);
    assert_eq!(window.ui.custom_dictionary.len(), 10_000);
    assert!(window.ui.spelling_language.is_empty());
    assert!(window.spelling_rev >= 1);
}

#[test]
fn a_large_custom_dictionary_is_deduped_quickly() {
    let mut words =
        vec![json!("Hello"), json!("hello"), json!("HELLO"), json!("Colour"), json!("COLOUR"), json!("ABC"), json!("Abc"), json!("Hello")];
    for n in 0..9_996 {
        words.push(json!(format!("w{n}")));
    }
    for n in 0..9_996 {
        words.push(json!(format!("W{n}")));
    }
    let started = std::time::Instant::now();
    let ui: UiState = serde_json::from_value(json!({"customDictionary": words})).unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed.as_secs() < 2, "cleaning 10000 words took {elapsed:?}");
    assert_eq!(ui.custom_dictionary.len(), 10_000);
    assert_eq!(ui.custom_dictionary.first().map(String::as_str), Some("Hello"));
    assert!(ui.custom_dictionary.iter().any(|word| word == "Colour"));
    assert!(ui.custom_dictionary.iter().any(|word| word == "ABC" || word == "Abc"));
    assert!(ui.custom_dictionary.iter().any(|word| word == "ABC") && ui.custom_dictionary.iter().any(|word| word == "Abc"));
    assert!(!ui.custom_dictionary.iter().any(|word| word == "hello" || word == "HELLO" || word == "COLOUR"));
    assert_eq!(ui.custom_dictionary.iter().filter(|word| word.starts_with('w')).count(), 9_996);
    assert!(!ui.custom_dictionary.iter().any(|word| word.starts_with('W')));
}

#[test]
fn custom_words_live_in_the_settings_remove_mailcraft_data_deletes() {
    // File › Info › Remove MailCraft's data deletes the settings folder, and with it `ui.json`
    // (the desktop host; the wipe tests assert the file is gone). The browser keeps the same
    // `UiState` under its own key. The custom dictionary is that state, so it goes with it.
    // Ignore All is this run only and is not in the file.
    let mut window = app();
    window.execute("ui.dictionary.add", json!({"word": "colour"})).unwrap();
    window.execute("ui.dictionary.ignore", json!({"word": "mispelt"})).unwrap();
    let saved = serde_json::to_value(window.prefs()).unwrap();
    assert_eq!(saved["customDictionary"], json!(["colour"]));
    assert!(saved.get("spellingIgnored").is_none());
    let again: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(again.custom_dictionary, vec!["colour".to_string()]);
}

#[test]
fn spelling_commands_answer_and_refuse_what_they_should() {
    let mut window = app();
    let rev = window.spelling_rev;
    let state = window.execute("ui.spelling", json!({})).unwrap();
    assert_eq!(state["asYouType"], true);
    assert_eq!(state["language"], "");
    assert_eq!(state["ignoreUppercase"], true);
    assert_eq!(state["ignoreNumbers"], true);
    assert_eq!(state["customWords"], 0);
    assert_eq!(window.spelling_rev, rev, "reading the state changes nothing");

    let set =
        window.execute("ui.spelling", json!({"asYouType": false, "language": "it-IT", "ignoreUppercase": false, "ignoreNumbers": false})).unwrap();
    assert_eq!(set["asYouType"], false);
    assert_eq!(set["language"], "it-IT");
    assert_eq!(set["ignoreUppercase"], false);
    assert_eq!(set["ignoreNumbers"], false);
    assert!(window.spelling_rev > rev);

    let rev = window.spelling_rev;
    assert!(window.execute("ui.spelling", json!({"language": "no-such"})).is_err());
    assert!(window.execute("ui.spelling", json!({"language": "off"})).is_err());
    assert!(window.execute("ui.spelling", json!({"asYouType": "yes"})).is_err());
    assert_eq!(window.ui.spelling_language, "it-IT");
    assert!(!window.ui.check_spelling_as_you_type);
    assert_eq!(window.spelling_rev, rev, "a refused command leaves the revision");

    window.execute("ui.spelling", json!({"language": ""})).unwrap();
    assert!(window.ui.spelling_language.is_empty());

    assert_eq!(window.execute("ui.dictionary.add", json!({"word": " Colour "})).unwrap()["added"], true);
    let rev = window.spelling_rev;
    // `Colour` does not cover `colour` (the checker would still flag it), so the lowercase spelling replaces it.
    assert_eq!(window.execute("ui.dictionary.add", json!({"word": "colour"})).unwrap()["added"], true);
    assert!(window.spelling_rev > rev);
    let rev = window.spelling_rev;
    assert_eq!(window.execute("ui.dictionary.add", json!({"word": "COLOUR"})).unwrap()["added"], false);
    assert_eq!(window.spelling_rev, rev);
    assert!(window.execute("ui.dictionary.add", json!({"word": "bad word"})).is_err());
    assert!(window.execute("ui.dictionary.add", json!({"word": "a".repeat(70)})).is_err());
    assert!(window.execute("ui.dictionary.add", json!({})).is_err());
    assert_eq!(window.execute("ui.dictionary.list", json!({})).unwrap()["words"], json!(["colour"]));

    assert_eq!(window.execute("ui.dictionary.remove", json!({"word": "colour"})).unwrap()["removed"], 1);
    assert!(window.ui.custom_dictionary.is_empty());
    assert!(window.execute("ui.dictionary.remove", json!({"word": "colour"})).is_err());

    window.ui.custom_dictionary = (0..mailcraft_engine::spelling::MAX_CUSTOM_WORDS.saturating_sub(1)).map(|n| format!("n{n}")).collect();
    let kept = crate::spelling::words_not_added(&mut window, vec!["extra".into(), "more".into()]);
    assert_eq!(kept, vec!["more".to_string()]);
    assert!(window.ui.custom_dictionary.iter().any(|word| word == "extra"));
    assert!(!window.ui.custom_dictionary.iter().any(|word| word == "more"));
    window.ui.custom_dictionary = (0..mailcraft_engine::spelling::MAX_CUSTOM_WORDS).map(|n| format!("w{n}")).collect();
    assert!(window.execute("ui.dictionary.add", json!({"word": "extra"})).unwrap_err().contains("full"));
    let rev = window.spelling_rev;
    assert_eq!(window.execute("ui.dictionary.add", json!({"word": "w0"})).unwrap()["added"], false);
    assert_eq!(window.spelling_rev, rev);

    assert_eq!(window.execute("ui.dictionary.ignore", json!({"word": "mispelt"})).unwrap()["ignored"], true);
    let rev = window.spelling_rev;
    assert_eq!(window.execute("ui.dictionary.ignore", json!({"word": "mispelt"})).unwrap()["ignored"], true);
    assert_eq!(window.spelling_rev, rev);
    window.spelling_ignored.clear();
    assert_eq!(window.execute("ui.dictionary.ignore", json!({"word": "Zorblin"})).unwrap()["ignored"], true);
    let rev = window.spelling_rev;
    assert_eq!(window.execute("ui.dictionary.ignore", json!({"word": "zorblin"})).unwrap()["ignored"], true);
    assert!(window.spelling_rev > rev, "Ignore All on the lowercase word is not covered by the capitalised one");
    let rev = window.spelling_rev;
    assert_eq!(window.execute("ui.dictionary.ignore", json!({"word": "zorblin"})).unwrap()["ignored"], true);
    assert_eq!(window.spelling_rev, rev);
    assert!(window.execute("ui.dictionary.ignore", json!({"word": "has space"})).is_err());

    window.spelling_ignored = (0..mailcraft_engine::spelling::MAX_CUSTOM_WORDS).map(|n| format!("i{n}")).collect();
    assert!(window.execute("ui.dictionary.ignore", json!({"word": "another"})).unwrap_err().contains("full"));

    // Junk parameters are an error, never a panic.
    for id in ["ui.spelling", "ui.dictionary.add", "ui.dictionary.remove", "ui.dictionary.ignore", "ui.dictionary.list"] {
        let _ = window.execute(id, json!("nope"));
        let _ = window.execute(id, Value::Null);
        let _ = window.execute(id, json!({"word": 1, "asYouType": [], "language": {"x": true}}));
    }
}

#[test]
fn the_default_language_applies_until_the_message_chooses() {
    let mut window = app();
    let ctx = Context::default();
    window.execute("ui.spelling", json!({"language": "it-IT"})).unwrap();
    assert_eq!(crate::spelling::locale(&window, &ctx).as_deref(), Some("it-IT"));

    let id = editor::id(&window).with("spelling-language");
    ctx.data_mut(|data| data.insert_temp(id, "fr-FR".to_string()));
    assert_eq!(crate::spelling::locale(&window, &ctx).as_deref(), Some("fr-FR"));

    ctx.data_mut(|data| data.insert_temp(id, String::new()));
    window.ui.spelling_language = "de-DE".into();
    // Not offered by this host: Automatic, which here matches neither tag.
    assert_ne!(crate::spelling::locale(&window, &ctx).as_deref(), Some("de-DE"));
}

#[test]
fn as_you_type_the_dictionary_and_ignore_all_drive_the_squiggles() {
    use egui::vec2;

    let mut window = app();
    window.autosave = false;
    window.execute("ui.spelling", json!({"language": "it-IT", "ignoreUppercase": false, "ignoreNumbers": false})).unwrap();
    window.execute("mail.new", json!({})).unwrap();
    window.execute("compose.set", json!({"body": "sopratutto funziona."})).unwrap();
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(900.0, 600.0)).build_ui_state(
            |ui, window: &mut MailApp| {
                window.logic(ui.ctx());
                window.ui(ui);
            },
            window,
        );
    harness.run_steps(4);
    let ctx = harness.ctx.clone();
    // The counter is the last frame only. Once the paragraph is cached, later frames paint the
    // squiggle without asking the host again.
    assert!(editor::spelling_marks(&ctx, harness.state()) > 0);

    harness.state_mut().execute("ui.spelling", json!({"asYouType": false})).unwrap();
    harness.run_steps(2);
    assert_eq!(editor::spelling_marks(&ctx, harness.state()), 0);
    assert_eq!(editor::spelling_checks(&ctx, harness.state()), 0, "as-you-type off does not check in the background");

    harness.state_mut().execute("ui.spelling", json!({"asYouType": true})).unwrap();
    harness.step();
    assert!(editor::spelling_checks(&ctx, harness.state()) > 0, "a revision change checks the paragraph again");
    assert!(editor::spelling_marks(&ctx, harness.state()) > 0);

    harness.state_mut().execute("ui.dictionary.add", json!({"word": "sopratutto"})).unwrap();
    harness.step();
    assert!(editor::spelling_checks(&ctx, harness.state()) > 0);
    assert_eq!(editor::spelling_marks(&ctx, harness.state()), 0, "a custom word drops the squiggle after the revision changes");

    harness.state_mut().execute("ui.dictionary.remove", json!({"word": "sopratutto"})).unwrap();
    harness.step();
    assert!(editor::spelling_marks(&ctx, harness.state()) > 0);

    harness.state_mut().execute("ui.dictionary.ignore", json!({"word": "sopratutto"})).unwrap();
    harness.step();
    assert_eq!(editor::spelling_marks(&ctx, harness.state()), 0);
}
