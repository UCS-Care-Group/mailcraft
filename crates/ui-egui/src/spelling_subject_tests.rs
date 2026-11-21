// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Subject is checked like a plain line: the span, the cache, as-you-type, and one undo
//! step when a suggestion replaces the word.

use std::ops::Range;

use egui::{Context, Id};
use mailcraft_spell::HostChecker;
use serde_json::json;

use crate::MailApp;
use crate::spelling_subject::{self, cached_issues};

struct Marker;

impl HostChecker for Marker {
    fn locales(&self) -> Vec<String> {
        vec!["en-US".to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != "en-US" {
            return None;
        }
        Some(text.match_indices("tomorow").map(|(at, word)| at..at + word.len()).collect())
    }

    fn suggest(&self, _locale: &str, word: &str) -> Vec<String> {
        if word == "tomorow" { vec!["tomorrow".into()] } else { Vec::new() }
    }
}

fn app() -> MailApp {
    let mut window = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services { spelling: Some(Box::new(Marker)), ..Default::default() },
    );
    window.autosave = false;
    window.execute("ui.spelling", json!({"language": "en-US", "ignoreUppercase": false, "ignoreNumbers": false})).unwrap();
    window
}

#[test]
fn a_misspelt_subject_is_an_issue_at_character_offsets_and_the_cache_follows_the_revision() {
    let mut window = app();
    let ctx = Context::default();
    let slot = Id::unique("subject-test");
    let text = "Café résumé tomorow";
    let issues = cached_issues(&ctx, &window, slot, "en-US", text);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].span, 12..19);
    let checks = spelling_subject::check_count(&ctx);
    assert_eq!(checks, 1);
    let again = cached_issues(&ctx, &window, slot, "en-US", text);
    assert_eq!(again[0].span, 12..19);
    assert_eq!(spelling_subject::check_count(&ctx), checks, "an unchanged line is not checked again");

    window.spelling_rev = window.spelling_rev.wrapping_add(1);
    let _ = cached_issues(&ctx, &window, slot, "en-US", text);
    assert!(spelling_subject::check_count(&ctx) > checks, "a new revision checks the line again");
}

#[test]
fn replacing_a_suggestion_edits_only_that_word_and_is_one_undo_step() {
    let mut window = app();
    window.execute("mail.new", json!({})).unwrap();
    window.execute("compose.set", json!({"subject": "Café résumé tomorow"})).unwrap();
    window.execute("compose.type", json!({"text": "Hi"})).unwrap();
    let body = window.session.compose.as_ref().unwrap().body.plain();
    window.execute("compose.replaceSubject", json!({"from": 12, "to": 19, "expected": "tomorow", "text": "tomorrow"})).unwrap();
    assert_eq!(window.session.compose.as_ref().unwrap().subject, "Café résumé tomorrow");
    assert_eq!(window.session.compose.as_ref().unwrap().body.plain(), body);

    window.execute("compose.undo", json!({})).unwrap();
    assert_eq!(window.session.compose.as_ref().unwrap().subject, "Café résumé tomorow");
    assert_eq!(window.session.compose.as_ref().unwrap().body.plain(), body);

    let refused = window.execute("compose.replaceSubject", json!({"from": 12, "to": 19, "expected": "tomorrow", "text": "tomorrow"}));
    assert!(refused.is_err());
    assert_eq!(window.session.compose.as_ref().unwrap().subject, "Café résumé tomorow");
}

#[test]
fn as_you_type_off_gives_no_subject_squiggles_and_a_frame_does_not_recheck() {
    use egui::vec2;

    let mut window = app();
    window.execute("mail.new", json!({})).unwrap();
    window.execute("compose.set", json!({"subject": "Lunch tomorow"})).unwrap();
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1100.0, 700.0)).build_ui_state(
            |ui, window: &mut MailApp| {
                window.logic(ui.ctx());
                window.ui(ui);
            },
            window,
        );
    harness.run_steps(4);
    let ctx = harness.ctx.clone();
    assert!(spelling_subject::mark_count(&ctx) > 0, "the misspelt subject is underlined");
    let checks = spelling_subject::check_count(&ctx);
    assert!(checks > 0);
    harness.step();
    assert_eq!(spelling_subject::check_count(&ctx), checks, "the next frame uses the cache");
    assert!(spelling_subject::mark_count(&ctx) > 0);

    harness.state_mut().execute("ui.spelling", json!({"asYouType": false})).unwrap();
    harness.run_steps(2);
    assert_eq!(spelling_subject::mark_count(&ctx), 0);
    assert_eq!(spelling_subject::check_count(&ctx), checks, "as-you-type off does not check the subject");
}
