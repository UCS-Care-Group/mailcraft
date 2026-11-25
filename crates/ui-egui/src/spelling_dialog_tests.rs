// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::ops::Range;

use egui::{Event, Key, Modifiers, PointerButton, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_spell::{EN_US_TAG, HostChecker};
use serde_json::json;

use super::SpellingDialog;
use crate::dialogs::{Dialog, input_dialog};
use crate::{MailApp, Services};

struct Host;

impl HostChecker for Host {
    fn locales(&self) -> Vec<String> {
        vec![EN_US_TAG.to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != EN_US_TAG {
            return None;
        }
        let mut ranges = Vec::new();
        whole(text, "teh", &mut ranges);
        whole(text, "misstake", &mut ranges);
        Some(ranges)
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        if locale != EN_US_TAG {
            return Vec::new();
        }
        match word {
            "teh" => ["the", "tea", "ten", "tech", "eh", "he"].into_iter().map(str::to_string).collect(),
            "misstake" => vec!["mistake".to_string()],
            _ => Vec::new(),
        }
    }
}

fn whole(text: &str, word: &str, ranges: &mut Vec<Range<usize>>) {
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(at) = text.get(from..).and_then(|rest| rest.find(word)) {
        let abs = from.saturating_add(at);
        let end = abs.saturating_add(word.len());
        let before = abs == 0 || bytes.get(abs.saturating_sub(1)).copied().is_some_and(|b| !word_byte(b));
        let after = bytes.get(end).copied().is_none_or(|b| !word_byte(b));
        if before && after {
            ranges.push(abs..end);
        }
        from = end.max(from.saturating_add(1));
        if from >= text.len() {
            break;
        }
    }
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'\'')
}

fn hosted() -> MailApp {
    let mut app = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0),
        Services { spelling: Some(Box::new(Host)), ..Services::default() },
    );
    app.autosave = false;
    app.ui.spelling_language = EN_US_TAG.to_string();
    app
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1280.0, 860.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

fn message(app: &mut MailApp, subject: &str, body: impl AsRef<str>) {
    app.execute("mail.new", json!({})).unwrap();
    app.execute("compose.set", json!({"subject": subject, "body": body.as_ref()})).unwrap();
}

fn plain(app: &MailApp) -> String {
    app.session.compose.as_ref().unwrap().body.plain()
}

fn subject(app: &MailApp) -> String {
    app.session.compose.as_ref().unwrap().subject.clone()
}

fn spelling(app: &MailApp) -> &SpellingDialog {
    match &app.dialog {
        Some(Dialog::Spelling(dialog)) => dialog,
        _ => panic!("the spelling dialog isn't open"),
    }
}

fn word(app: &MailApp) -> String {
    spelling(app).current.as_ref().map(|mistake| mistake.word.clone()).unwrap_or_default()
}

fn click_label(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

fn click_last(h: &mut Harness<'static, MailApp>, label: &str) {
    h.query_all_by_label(label).last().unwrap().click();
    h.run_steps(3);
}

#[test]
fn review_spelling_asks_for_the_dialog() {
    assert_eq!(input_dialog("review.spelling", &json!({})), Some("spelling"));
    assert_eq!(input_dialog("review.spelling", &json!({"language": "en-US"})), Some("spelling"));
}

#[test]
fn f7_and_the_button_open_the_dialog_and_execute_does_not() {
    let mut app = hosted();
    message(&mut app, "Hello", "Please check teh spelling.");
    let answer = app.execute("review.spelling", json!({})).unwrap();
    assert!(app.dialog.is_none(), "execute runs the engine command");
    assert!(answer["issues"].as_array().is_some_and(|issues| !issues.is_empty()));

    let opened = app.run("review.spelling", json!({})).unwrap();
    assert_eq!(opened["pending"], "spelling");
    assert!(matches!(app.dialog, Some(Dialog::Spelling(_))));

    let mut h = window(app);
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    click_last(&mut h, "Close");
    assert!(h.state().dialog.is_none());

    h.key_press(Key::F7);
    h.run_steps(4);
    assert!(matches!(h.state().dialog, Some(Dialog::Spelling(_))), "F7 opens the dialog");
    assert_eq!(word(h.state()), "teh");
}

#[test]
fn check_spelling_and_custom_dictionary_are_on_the_menu() {
    let mut h = window(hosted());
    // The sample's reminder window lies over the ribbon.
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    message(h.state_mut(), "Hello", "Please check teh spelling.");
    h.state_mut().execute("ui.tab", json!({"tab": "Review"})).unwrap();
    h.run_steps(3);
    open_spelling_menu(&mut h);
    // The item's accessible name includes its shortcut ("Check Spelling… F7").
    assert!(h.query_all_by_label_contains("Check Spelling").next().is_some());
    h.get_by_label("Custom Dictionary…").click();
    h.run_steps(3);
    assert!(h.state().ui.backstage);
    assert_eq!(h.state().ui.backstage_page, "options");
    h.state_mut().ui.backstage = false;
    h.run_steps(2);
    open_spelling_menu(&mut h);
    h.query_all_by_label_contains("Check Spelling").next().expect("Check Spelling").click();
    h.run_steps(4);
    assert_eq!(word(h.state()), "teh");
}

fn open_spelling_menu(h: &mut Harness<'static, MailApp>) {
    let pos = h.state().hit.rect("ribbon", crate::ribbon::hit_id("spelling")).expect("Spelling").center();
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

#[test]
fn the_walk_starts_with_the_subject_then_the_body() {
    let mut app = hosted();
    message(&mut app, "See teh now.", "A misstake stays.");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    assert!(matches!(spelling(h.state()).current.as_ref().unwrap().place, super::Place::Subject));
    click_label(&mut h, "Change");
    assert_eq!(subject(h.state()), "See the now.");
    assert_eq!(plain(h.state()), "A misstake stays.");
    assert_eq!(word(h.state()), "misstake");
    let selected = {
        let body = &h.state().session.compose.as_ref().unwrap().body;
        let sel = body.selection();
        body.plain().chars().skip(sel.start()).take(sel.end().saturating_sub(sel.start())).collect::<String>()
    };
    assert_eq!(selected, "misstake");
    click_label(&mut h, "Change");
    assert_eq!(plain(h.state()), "A mistake stays.");
    assert!(spelling(h.state()).complete);
    assert!(h.query_by_label("The spelling check is complete.").is_some());
    // Each Change is one step of the message's undo, newest first.
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(plain(h.state()), "A misstake stays.");
    assert_eq!(subject(h.state()), "See the now.");
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(subject(h.state()), "See teh now.");
}

#[test]
fn change_edits_one_body_word() {
    let mut app = hosted();
    message(&mut app, "Hello", "teh and teh");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    click_label(&mut h, "Change");
    assert_eq!(plain(h.state()), "the and teh");
    assert_eq!(word(h.state()), "teh");
}

#[test]
fn change_all_leaves_tehran_and_one_undo_restores_the_body() {
    let mut app = hosted();
    message(&mut app, "teh night", "teh Tehran tehee teh");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    click_label(&mut h, "Change All");
    assert_eq!(subject(h.state()), "the night");
    assert_eq!(plain(h.state()), "the Tehran tehee the");
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(plain(h.state()), "teh Tehran tehee teh", "compose.undo restores every body replacement");
    // The Subject was one compose.set, a step of the session undo, beside the body's one step.
    assert_eq!(subject(h.state()), "the night");
}

#[test]
fn change_all_leaves_a_misspelling_in_the_quoted_original() {
    let mut app = hosted();
    let id = plain_note(&mut app, "Please fix teh typo");
    app.execute("mail.reply", json!({"id": id})).unwrap();
    app.execute("compose.type", json!({"text": "See teh note"})).unwrap();
    let before = plain(&app);
    assert!(before.contains("-----Original Message-----") && before.matches("teh").count() >= 2, "{before}");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(12);
    assert_eq!(word(h.state()), "teh");
    click_label(&mut h, "Change All");
    let after = plain(h.state());
    assert!(after.contains("See the note"), "{after}");
    assert!(after.contains("Please fix teh typo"), "the quoted original is not rewritten: {after}");
}

fn plain_note(app: &mut MailApp, text: &str) -> u64 {
    use mailcraft_model::{Address, Message};
    let inbox = app.session.mailbox.inbox().unwrap();
    let account = app.session.mailbox.accounts[0].id;
    app.session
        .mailbox
        .add_message(Message {
            account,
            folder: inbox,
            from: Address::new("Sam Rivera", "sam@rivera.example"),
            to: vec![Address::new("Avery Quinn", "avery@lumen.example")],
            subject: "Notes".into(),
            body: mailcraft_model::Body { text: text.into(), html: None },
            ..Default::default()
        })
        .unwrap()
}

#[test]
fn change_all_and_the_signature() {
    let mut app = hosted();
    message(&mut app, "Hello", "Hello teh friend");
    app.execute("signature.set", json!({"name": "Short", "text": "Sign teh"})).unwrap();
    app.execute("compose.signature", json!({"name": "Short"})).unwrap();
    let before = plain(&app);
    assert!(before.contains("Hello teh friend") && before.contains("Sign teh"), "{before}");
    app.execute("edit.replace", json!({"text": "teh", "with": "the", "matchCase": true, "wholeWord": true, "all": true})).unwrap();
    let after = plain(&app);
    // reach is the whole document, so the signature block is included.
    assert!(!after.contains("teh"), "reach includes the signature block: {after}");
    assert!(after.contains("Sign the"), "{after}");

    app.execute("compose.undo", json!({})).unwrap();
    app.execute("signature.set", json!({"name": "Short", "text": "say teh please"})).unwrap();
    let message = plain(&app);
    app.execute("signature.open", json!({"name": "Short"})).unwrap();
    app.execute("edit.replace", json!({"text": "teh", "with": "the", "matchCase": true, "wholeWord": true, "all": true})).unwrap();
    assert_eq!(plain(&app), message, "a signature open for editing is the document; the message is left alone");
    let edited = app.session.signature_edit.as_ref().unwrap().body.plain();
    assert!(edited.contains("say the please"), "{edited}");
}

#[test]
fn ignore_once_skips_one_occurrence() {
    let mut app = hosted();
    message(&mut app, "Hello", "teh then teh");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    let first = spelling(h.state()).current.as_ref().unwrap().from;
    click_label(&mut h, "Ignore Once");
    let second = spelling(h.state()).current.as_ref().unwrap().clone();
    assert_eq!(second.word, "teh");
    assert!(second.from > first);
    click_label(&mut h, "Change");
    assert_eq!(plain(h.state()), "teh then the");
    assert!(spelling(h.state()).complete);
}

#[test]
fn ignore_all_and_add_to_dictionary_stop_the_word() {
    let mut app = hosted();
    message(&mut app, "Hello", "teh and teh");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    click_label(&mut h, "Ignore All");
    assert!(spelling(h.state()).complete);
    assert!(h.state().spelling_ignored.iter().any(|word| word == "teh"));
    click_last(&mut h, "Close");
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert!(spelling(h.state()).complete, "Ignore All is the window's filter");

    message(h.state_mut(), "Hello", "misstake once");
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "misstake");
    click_label(&mut h, "Add to Dictionary");
    assert!(spelling(h.state()).complete);
    assert!(h.state().ui.custom_dictionary.iter().any(|word| word.eq_ignore_ascii_case("misstake")));
}

#[test]
fn a_long_paragraph_is_named_when_the_check_finishes() {
    let mut app = hosted();
    message(&mut app, "Hello", format!("{}\nteh", "a".repeat(9_000)));
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(4);
    assert_eq!(word(h.state()), "teh");
    click_label(&mut h, "Change");
    assert!(spelling(h.state()).complete);
    assert!(h.query_by_label("The spelling check is complete. Some of the message was not checked.").is_some());
}

#[test]
fn closing_the_message_closes_the_dialog() {
    let mut app = hosted();
    message(&mut app, "Hello", "teh");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::Spelling(_))));
    h.state_mut().execute("compose.discard", json!({})).unwrap();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
}

#[test]
fn an_empty_message_finishes_without_panic() {
    let mut app = hosted();
    message(&mut app, "", "");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert!(spelling(h.state()).complete);
    assert!(spelling(h.state()).current.is_none());
    assert!(h.query_by_label("The spelling check is complete.").is_some());
    click_last(&mut h, "Close");
    assert!(h.state().dialog.is_none());
}

#[test]
fn spelling_off_or_missing_is_said_and_opens_nothing() {
    let mut app = hosted();
    message(&mut app, "Hello", "teh");
    app.services.spelling = None;
    app.run("review.spelling", json!({})).unwrap();
    assert!(app.dialog.is_none());
    assert!(app.status_msg.as_ref().unwrap().0.contains("unavailable"));

    let mut h = window(hosted());
    message(h.state_mut(), "Hello", "teh");
    h.run_steps(2);
    let off = crate::compose::editor::id(h.state()).with("spelling-off");
    h.ctx.data_mut(|data| data.insert_temp(off, true));
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(2);
    assert!(h.state().dialog.is_none());
    let said = h.state().status_msg.as_ref().unwrap().0.clone();
    assert!(said.contains("off"), "{said}");
}

#[test]
fn the_walk_restarts_when_the_body_changes_before_a_mistake_is_shown() {
    let fillers = 30;
    let mut lines = vec!["Hello".to_string()];
    lines.extend(std::iter::repeat_with(|| "Ok".to_string()).take(fillers));
    lines.push("teh".to_string());
    let original = lines.join("\n");
    lines[0] = "Hello teh@example.org".to_string();
    let edited = lines.join("\n");

    let mut app = hosted();
    message(&mut app, "Hello", &original);
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    let mut started = false;
    for _ in 0..6 {
        h.run_steps(1);
        let dialog = spelling(h.state());
        if dialog.current.as_ref().is_some_and(|mistake| mistake.word == "teh") || dialog.complete {
            panic!("the scan reached the mistake before the body changed");
        }
        if matches!(dialog.step, super::Step::Body(n) if n > 0) {
            started = true;
            break;
        }
    }
    assert!(started, "the scan did not leave the subject");
    h.state_mut().execute("compose.set", json!({"body": edited})).unwrap();
    for _ in 0..20 {
        h.run_steps(1);
        let dialog = spelling(h.state());
        if dialog.current.is_some() || dialog.complete {
            break;
        }
    }
    assert_eq!(word(h.state()), "teh");
    let text = plain(h.state());
    let want = text.rfind("\nteh").map(|at| at + 1).expect("the word is still its own paragraph");
    let mistake = spelling(h.state()).current.as_ref().expect("a mistake is shown");
    assert_eq!(mistake.from, want, "the offset follows the body as it is now");
    let selected: String = text.chars().skip(mistake.from).take(mistake.to.saturating_sub(mistake.from)).collect();
    assert_eq!(selected, "teh");
    click_label(&mut h, "Change");
    let after = plain(h.state());
    assert!(after.contains("teh@example.org"), "{after}");
    assert!(after.contains("\nthe"), "{after}");
    assert!(!after.contains("\nteh"), "{after}");
}

#[test]
fn the_walk_restarts_when_the_subject_changes_under_it() {
    let mut app = hosted();
    message(&mut app, "teh", "Hello");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    h.state_mut().execute("compose.set", json!({"subject": "See teh"})).unwrap();
    h.run_steps(3);
    click_label(&mut h, "Change");
    assert_eq!(subject(h.state()), "See the");
}

#[test]
fn change_all_fixes_prose_and_leaves_an_address() {
    let mut app = hosted();
    message(&mut app, "teh night teh@example.org", "Say teh to teh@example.org today");
    let mut h = window(app);
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(3);
    assert_eq!(word(h.state()), "teh");
    click_label(&mut h, "Change All");
    assert_eq!(subject(h.state()), "the night teh@example.org");
    assert_eq!(plain(h.state()), "Say the to teh@example.org today");
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(plain(h.state()), "Say teh to teh@example.org today");
    assert_eq!(subject(h.state()), "the night teh@example.org");
}

// A full draft refuses the longer body and keeps the Subject, so a shorter retry starts from the original.
#[test]
fn change_all_leaves_the_subject_when_the_body_cannot_grow() {
    let mut app = hosted();
    let width = 4_000usize;
    let line = format!("teh {}", "x".repeat(width - 4));
    let count = mailcraft_model::rich::MAX_CHARS / (width + 1);
    let mut body = String::new();
    for i in 0..count {
        if i > 0 {
            body.push('\n');
        }
        body.push_str(&line);
    }
    let slack = mailcraft_model::rich::MAX_CHARS.saturating_sub(body.chars().count());
    body.push_str(&"y".repeat(slack));
    message(&mut app, "teh night", &body);
    let before = plain(&app);
    assert_eq!(before.chars().count(), mailcraft_model::rich::MAX_CHARS, "the draft is full");
    let mistake = super::Mistake {
        place: super::Place::Body,
        from: 0,
        to: 3,
        word: "teh".to_string(),
        nth: 0,
        words: Vec::new(),
        context: String::new(),
        emphasis: 0..0,
    };
    let mut dialog = SpellingDialog::default();
    dialog.change_all(&mut app, &mistake, "there", EN_US_TAG);
    assert_eq!(subject(&app), "teh night", "a body that cannot grow leaves the Subject");
    assert_eq!(plain(&app), before);
    assert!(!dialog.note.is_empty(), "the refusal is shown");
}

#[test]
fn replace_ranges_keeps_quotes_around_the_flagged_word() {
    let quoted_span = 1..4;
    let quoted = vec![quoted_span];
    let next = super::replace_ranges("'teh'", &quoted, "teh", "the").expect("range");
    assert_eq!(next, "'the'");
    let short_span = 0..2;
    let short = vec![short_span];
    assert!(super::replace_ranges("teh", &short, "teh", "the").is_none());
}

#[test]
fn the_dialog_sits_under_the_word_when_the_line_has_room_below() {
    let screen = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1280.0, 860.0));
    let word = egui::Rect::from_min_max(egui::pos2(400.0, 200.0), egui::pos2(460.0, 220.0));
    let pane = egui::Rect::from_min_max(egui::pos2(280.0, 120.0), egui::pos2(1240.0, 820.0));
    let size = egui::vec2(640.0, 300.0);
    let pos = super::place(screen, Some(word), Some(pane), size);
    assert!(pos.y >= word.max.y, "under the line: {pos:?}");
    assert!(pos.y + size.y <= screen.max.y, "inside the window: {pos:?}");
    assert!(pos.x >= screen.min.x && pos.x + size.x <= screen.max.x);
}

#[test]
fn the_dialog_sits_over_the_word_when_the_line_is_near_the_bottom() {
    let screen = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1280.0, 860.0));
    let word = egui::Rect::from_min_max(egui::pos2(400.0, 780.0), egui::pos2(460.0, 800.0));
    let size = egui::vec2(640.0, 300.0);
    let pos = super::place(screen, Some(word), None, size);
    assert!(pos.y + size.y <= word.min.y + 0.5, "over the line: {pos:?}");
    assert!(pos.y >= screen.min.y);
}

#[test]
fn without_a_word_the_dialog_sits_at_the_bottom_of_the_pane() {
    let screen = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1280.0, 860.0));
    let pane = egui::Rect::from_min_max(egui::pos2(300.0, 160.0), egui::pos2(1100.0, 800.0));
    let size = egui::vec2(640.0, 280.0);
    let pos = super::place(screen, None, Some(pane), size);
    assert!(((pos.y + size.y) - (pane.max.y - 8.0)).abs() < 1.0, "along the bottom of the pane: {pos:?}");
    assert!(pos.y >= screen.min.y && pos.x >= pane.min.x);
}

#[test]
fn a_window_smaller_than_the_dialog_or_a_broken_rect_never_panics() {
    let tiny = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
    let size = egui::vec2(640.0, 300.0);
    let word = egui::Rect::from_min_max(egui::pos2(20.0, 40.0), egui::pos2(60.0, 60.0));
    let pos = super::place(tiny, Some(word), Some(tiny), size);
    assert!(pos.x.is_finite() && pos.y.is_finite(), "{pos:?}");
    let pos = super::place(tiny, None, Some(tiny), size);
    assert!(pos.x.is_finite() && pos.y.is_finite(), "{pos:?}");
    let nan = egui::Rect::from_min_max(egui::pos2(f32::NAN, f32::NAN), egui::pos2(f32::NAN, f32::NAN));
    let pos = super::place(nan, Some(nan), Some(nan), egui::vec2(f32::NAN, f32::NAN));
    assert!(pos.x.is_finite() && pos.y.is_finite(), "{pos:?}");
    let pos = super::place(tiny, Some(nan), Some(nan), size);
    assert!(pos.x.is_finite() && pos.y.is_finite(), "{pos:?}");
}

#[test]
fn a_word_scrolled_out_of_the_body_has_no_selection_rect() {
    let mut body = String::from("teh\n");
    for _ in 0..120 {
        body.push_str("a line of the message stays here\n");
    }
    let mut h = window(hosted());
    message(h.state_mut(), "Hello", &body);
    h.run_steps(6);
    let on = crate::compose::editor::spelling_selection_rect(h.state(), &h.ctx, 0, 3);
    let on = on.expect("the word at the top is on screen");
    h.event(egui::Event::PointerMoved(on.center()));
    h.step();
    for _ in 0..8 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, -400.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
            source: egui::MouseWheelSource::Unknown,
        });
        h.step();
    }
    h.run_steps(8);
    let off = crate::compose::editor::spelling_selection_rect(h.state(), &h.ctx, 0, 3);
    assert!(off.is_none(), "a word scrolled out of view is not placed at the screen edge: {off:?}");
}

#[test]
fn suggestion_rows_are_whole_and_the_word_stays_visible() {
    let mut h = window(hosted());
    message(h.state_mut(), "Notes", "Please check teh spelling here.");
    h.state_mut().run("review.spelling", json!({})).unwrap();
    h.run_steps(8);
    let mut rows = Vec::new();
    for index in 0..6 {
        let rect = h.state().hit.rect("spellingSuggestion", index).unwrap_or_else(|| panic!("suggestion {index}"));
        rows.push(rect);
    }
    let height = rows[0].height();
    assert!(height >= 20.0, "{height}");
    for (index, rect) in rows.iter().enumerate().skip(1) {
        assert!((rect.height() - height).abs() < 0.5, "row {index} height {}", rect.height());
        let gap = rect.min.y - rows[index - 1].max.y;
        assert!((-0.5..1.0).contains(&gap), "row {index} is cut or gapped: {gap}");
    }
    let mistake = spelling(h.state()).current.as_ref().expect("a body word");
    let word = crate::compose::editor::spelling_selection_rect(h.state(), &h.ctx, mistake.from, mistake.to).expect("the word on screen");
    let dialog = h.ctx.memory(|m| m.area_rect(egui::Id::unique("dialog"))).expect("the dialog");
    let clear = dialog.min.y + 1.0 >= word.max.y || dialog.max.y <= word.min.y + 1.0;
    assert!(clear, "word {word:?} dialog {dialog:?}");
}
