// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use mailcraft_model::rich::{ListKind, Run};

struct Italian;

impl HostChecker for Italian {
    fn locales(&self) -> Vec<String> {
        vec!["it-IT".to_string()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != "it-IT" {
            return None;
        }
        let mut ranges = vec![1..2, 1000..1001];
        for (at, word) in text.match_indices("sopratutto") {
            ranges.push(at..at + word.len());
        }
        Some(ranges)
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        assert_eq!((locale, word), ("it-IT", "sopratutto"));
        vec!["soprattutto".into()]
    }
}

#[test]
fn spelling_suggestion_uses_composer_undo_and_checks_the_original_word() {
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0), Default::default());
    app.execute("mail.new", json!({})).unwrap();
    app.execute("compose.set", json!({"body": "È sopratutto così."})).unwrap();
    let issue = Issue { span: 2..12, word: "sopratutto".into() };
    let suggestions = Suggestions::new(&Italian, "it-IT", issue, 0, &[]);
    let selected = suggestions.words[0].clone();
    suggestions.clone().apply(&mut app, selected.clone());
    assert_eq!(app.session.compose.as_ref().unwrap().body.plain(), "È soprattutto così.");
    suggestions.apply(&mut app, "altro".to_string());
    assert_eq!(app.session.compose.as_ref().unwrap().body.plain(), "È soprattutto così.");
    app.execute("compose.undo", json!({})).unwrap();
    assert_eq!(app.session.compose.as_ref().unwrap().body.plain(), "È sopratutto così.");
}

#[test]
fn composer_offers_an_italian_replacement_on_right_click() {
    use egui::{Event, Modifiers, PointerButton, vec2};
    use egui_kittest::kittest::Queryable as _;

    for theme in ["light", "dark"] {
        let now = 1_791_993_600;
        let session = mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(now, 0), now, 0);
        let mut app = MailApp::new(session, crate::Services { spelling: Some(Box::new(Italian)), ..Default::default() });
        app.autosave = false;
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("mail.new", json!({})).unwrap();
        app.execute("compose.set", json!({"body": "sopratutto funziona."})).unwrap();
        let mut harness =
            egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1280.0, 800.0)).build_ui_state(
                |ui, app: &mut MailApp| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                },
                app,
            );
        let language = editor::id(harness.state()).with("spelling-language");
        harness.ctx.data_mut(|data| data.insert_temp(language, "it-IT".to_string()));
        harness.run_steps(3);
        let body = harness.state().hit.rect("composeBody", 0).unwrap();
        let pos = body.min + vec2(17.0, 9.0);
        harness.event(Event::PointerMoved(pos));
        harness.step();
        for pressed in [true, false] {
            harness.event(Event::PointerButton { pos, button: PointerButton::Secondary, pressed, modifiers: Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(2);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("composer-spelling-{theme}.png"))).unwrap();
        }
        harness.get_by_label("soprattutto").click();
        harness.run_steps(3);
        assert_eq!(harness.state().session.compose.as_ref().unwrap().body.plain(), "soprattutto funziona.");
    }
}

#[test]
fn spelling_review_starts_at_the_caret_and_marks_large_paragraphs_partial() {
    let mut app = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services { spelling: Some(Box::new(Italian)), ..Default::default() },
    );
    let ctx = Context::default();
    app.execute("mail.new", json!({})).unwrap();
    let prefix = format!("{}\n", "a".repeat(4000)).repeat(17);
    let start = prefix.chars().count();
    app.execute("compose.set", json!({"body": format!("{prefix}sopratutto")})).unwrap();
    app.execute("compose.select", json!({"from": start, "to": start})).unwrap();
    review_next(&mut app, &ctx, "it-IT");
    advance_review(&mut app, &ctx, Some("it-IT"));
    let selection = app.session.compose.as_ref().unwrap().body.selection();
    assert_eq!((selection.start(), selection.end()), (start, start + 10));
    let oversized = Paragraph::new(vec![Run::plain("a".repeat(9000))], ListKind::None);
    assert!(check(&Italian, "it-IT", &oversized, &mailcraft_engine::spelling::Filter::default(), 0).partial);
}

#[test]
fn spelling_off_preserves_the_selected_writing_language() {
    let app = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services { spelling: Some(Box::new(Italian)), ..Default::default() },
    );
    let ctx = Context::default();
    let editor = editor::id(&app);
    ctx.data_mut(|d| {
        d.insert_temp(editor.with("spelling-language"), "tr-TR".to_owned());
        d.insert_temp(editor.with("spelling-off"), true);
    });
    assert_eq!(locale(&app, &ctx), None);
    assert_eq!(preference(&app, &ctx), "tr-TR");
}

#[test]
fn spelling_correction_preserves_the_formatting_of_unchanged_text() {
    use mailcraft_model::rich::{Format, RichBody, RichText};
    let mut body = RichBody::new(RichText::from_plain("sopratutto"));
    body.select(7, 10).unwrap();
    body.format(Format::Bold(true)).unwrap();
    body.replace_exact((0, 10), "sopratutto", "soprattutto").unwrap();
    assert_eq!(body.plain(), "soprattutto");
    assert!(body.text().all(8, 11, |style| style.bold).unwrap());
    body.undo();
    assert_eq!(body.plain(), "sopratutto");
    assert!(body.text().all(7, 10, |style| style.bold).unwrap());
}

struct Listed {
    tags: Vec<String>,
}

impl HostChecker for Listed {
    fn locales(&self) -> Vec<String> {
        self.tags.clone()
    }

    fn check(&self, _locale: &str, _text: &str) -> Option<Vec<Range<usize>>> {
        Some(Vec::new())
    }

    fn suggest(&self, _locale: &str, _word: &str) -> Vec<String> {
        Vec::new()
    }
}

fn app_with(tags: &[&str], system: Option<&str>) -> MailApp {
    MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services {
            spelling: Some(Box::new(Listed { tags: tags.iter().map(|tag| (*tag).to_string()).collect() })),
            system_locale: system.map(str::to_string),
            ..Default::default()
        },
    )
}

#[test]
fn the_built_in_dictionary_is_named_as_built_in() {
    assert_eq!(locale_name(mailcraft_spell::EN_US_TAG), "English (United States, built-in)");
    assert_eq!(locale_name("en-US"), "English (en-US)");
}

#[test]
fn automatic_prefers_an_os_en_us_and_falls_back_to_the_built_in_tag() {
    let ctx = Context::default();
    // The system tag is compared whole, after normalising. The built-in tag is not `en-US`,
    // so it does not win the system match even when it is listed first.
    let both = app_with(&[mailcraft_spell::EN_US_TAG, "en-US"], Some("en_US.UTF-8"));
    assert_eq!(locale(&both, &ctx).as_deref(), Some("en-US"));
    // With no system region, Automatic takes the first tag of the interface language.
    // The desktop host sorts, and `en-US` sorts before `en-US-x-builtin`.
    let sorted = app_with(&["en-US", mailcraft_spell::EN_US_TAG], None);
    assert_eq!(locale(&sorted, &ctx).as_deref(), Some("en-US"));
    // No operating-system English: the system tag does not equal the built-in tag, and the
    // language match then takes that tag.
    let only = app_with(&[mailcraft_spell::EN_US_TAG, "fr-FR"], Some("en-US"));
    assert_eq!(locale(&only, &ctx).as_deref(), Some(mailcraft_spell::EN_US_TAG));
}

struct Quiet;

impl HostChecker for Quiet {
    fn locales(&self) -> Vec<String> {
        vec!["en-GB".to_string()]
    }

    fn check(&self, _locale: &str, _text: &str) -> Option<Vec<Range<usize>>> {
        Some(Vec::new())
    }

    fn suggest(&self, _locale: &str, _word: &str) -> Vec<String> {
        Vec::new()
    }
}

#[test]
fn a_custom_word_one_edit_away_is_offered_before_the_hosts() {
    let issue = Issue { span: 0..4, word: "helo".into() };
    let suggestions = Suggestions::new(&Quiet, "en-GB", issue, 0, &["hello".into(), "zzzz".into()]);
    assert_eq!(suggestions.words, vec!["hello".to_string()]);
}

#[test]
fn the_built_in_list_is_asked_with_the_custom_dictionary() {
    if mailcraft_spell::en_us().is_none() {
        return;
    }
    let issue = Issue { span: 0..8, word: "mailcraf".into() };
    let suggestions = Suggestions::new(&Quiet, mailcraft_spell::EN_US_TAG, issue, 0, &["mailcraft".into()]);
    assert!(suggestions.words.iter().any(|word| word.eq_ignore_ascii_case("mailcraft")), "{:?}", suggestions.words);
}

#[test]
fn beginning_ime_composition_cancels_a_pending_spelling_review() {
    use egui::{Event, ImeEvent};
    use mailcraft_model::rich::Selection;

    let mut app = MailApp::new(
        mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0),
        crate::Services { spelling: Some(Box::new(Italian)), ..Default::default() },
    );
    app.autosave = false;
    app.execute("mail.new", json!({})).unwrap();
    app.execute("compose.set", json!({"body": "Prima sopratutto"})).unwrap();
    app.execute("compose.select", json!({"from": 0})).unwrap();
    app.want_body_focus();
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                app.logic(ui.ctx());
                app.ui(ui);
            },
            app,
        );
    let ctx = harness.ctx.clone();
    let language = editor::id(harness.state()).with("spelling-language");
    ctx.data_mut(|d| d.insert_temp(language, "it-IT".to_string()));
    harness.run_steps(3);
    assert!(editor::has_focus(&ctx));

    review_next(harness.state_mut(), &ctx, "it-IT");
    harness.event(Event::Ime(ImeEvent::Preedit { text: "ni".into(), active_range_chars: None }));
    harness.run_steps(2);
    assert_eq!(harness.state().session.compose.as_ref().unwrap().body.selection(), Selection::caret(0));
    assert!(ctx.data(|d| d.get_temp::<ReviewScan>(editor::id(harness.state()).with("spelling-scan")).is_none()));
    harness.event(Event::Ime(ImeEvent::Commit("你".into())));
    harness.run_steps(2);
    assert_eq!(harness.state().session.compose.as_ref().unwrap().body.plain(), "你Prima sopratutto");

    // A preedit left behind when the body loses focus must not cancel a new review.
    harness.state_mut().execute("compose.select", json!({"from": 7})).unwrap();
    harness.event(Event::Ime(ImeEvent::Preedit { text: "hao".into(), active_range_chars: None }));
    harness.run_steps(2);
    assert!(editor::has_focus(&ctx));
    assert_eq!(editor::preedit(harness.state(), &ctx), "hao");
    let body_id = editor::id(harness.state());
    ctx.memory_mut(|memory| memory.surrender_focus(body_id));
    review_next(harness.state_mut(), &ctx, "it-IT");
    harness.run_steps(2);
    let selection = harness.state().session.compose.as_ref().unwrap().body.selection();
    assert_eq!((selection.start(), selection.end()), (7, 17));
}
