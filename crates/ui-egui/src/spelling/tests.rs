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

#[test]
fn an_address_split_between_plain_and_linked_text_is_not_marked() {
    let mut suffix = Run::plain("@example.org");
    suffix.style.link = Some("mailto:sopratutto@example.org".into());
    let mut prefix = Run::plain("delivery@");
    prefix.style.link = Some("mailto:delivery@sopratutto.example".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto"), suffix, Run::plain(" "), prefix, Run::plain("sopratutto.example")], ListKind::None);
    assert!(check(&Italian, "it-IT", &paragraph).issues.is_empty());
}

#[test]
fn typo_beside_a_link_is_still_marked() {
    let mut link = Run::plain("https://example.org");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto—"), link], ListKind::None);
    let report = check(&Italian, "it-IT", &paragraph);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
}

#[test]
fn a_url_path_with_a_dash_is_protected_across_formatting_runs() {
    for address in ["https://example.org/a—", "WWW.example.org/a—"] {
        let mut prefix = Run::plain(address);
        prefix.style.link = Some("https://example.org/a".into());
        let paragraph = Paragraph::new(vec![prefix, Run::plain("sopratutto")], ListKind::None);
        assert!(check(&Italian, "it-IT", &paragraph).issues.is_empty(), "{address}");
    }
}

#[test]
fn a_typo_before_a_parenthesized_address_is_still_marked() {
    let mut link = Run::plain("info@example.org");
    link.style.link = Some("mailto:info@example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto("), link, Run::plain(")")], ListKind::None);
    let report = check(&Italian, "it-IT", &paragraph);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
}

#[test]
fn balanced_parentheses_in_a_url_path_are_protected() {
    for address in ["https://example.org/a(", "www.example.org/a("] {
        let mut prefix = Run::plain(address);
        prefix.style.link = Some("https://example.org/a".into());
        let paragraph = Paragraph::new(vec![prefix, Run::plain("sopratutto)")], ListKind::None);
        assert!(check(&Italian, "it-IT", &paragraph).issues.is_empty(), "{address}");
    }
    let mut link = Run::plain("https://example.org");
    link.style.link = Some("https://example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto("), link, Run::plain(")sopratutto")], ListKind::None);
    let spans: Vec<_> = check(&Italian, "it-IT", &paragraph).issues.into_iter().map(|issue| issue.span).collect();
    assert_eq!(spans, [0..10, 31..41]);
}

#[test]
fn a_typo_before_a_comma_separated_address_is_still_marked() {
    let mut link = Run::plain("info@example.org");
    link.style.link = Some("mailto:info@example.org".into());
    let paragraph = Paragraph::new(vec![Run::plain("sopratutto,"), link], ListKind::None);
    let report = check(&Italian, "it-IT", &paragraph);
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].span, 0..10);
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
