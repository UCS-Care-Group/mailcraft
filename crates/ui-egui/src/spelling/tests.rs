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
fn spelling_offsets_exclude_links_addresses_and_signatures() {
    let mut link = Run::plain("sopratutto");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("È 📧 sopratutto sopratutto@example.org "), link], ListKind::None);
    let report = check(&Italian, "it-IT", &paragraph);
    assert!(report.available);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 4..14);
    assert!(!check(&Italian, "fr-FR", &paragraph).available);
    assert!(check(&Italian, "it-IT", &Paragraph { signature: true, ..paragraph }).issues.is_empty());
}

#[test]
fn spelling_suggestion_uses_composer_undo_and_checks_the_original_word() {
    let mut app = MailApp::new(mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(0, 0), 0, 0), Default::default());
    app.execute("mail.new", json!({})).unwrap();
    app.execute("compose.set", json!({"body": "È sopratutto così."})).unwrap();
    let issue = Issue { span: 2..12, word: "sopratutto".into() };
    let suggestions = Suggestions::new(&Italian, "it-IT", issue, 0);
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
    assert!(check(&Italian, "it-IT", &oversized).partial);
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
    assert_eq!(writing_locale(&app, &ctx).as_deref(), Some("tr-TR"));
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
