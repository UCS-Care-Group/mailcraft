// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regression tests for bookmarks (#951 review fixes: A, B, G).

use std::sync::Arc;

use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_model::{Address, Body, Id, Message};
use serde_json::json;

use super::Place;
use super::images::Pictures;
use super::layout::Laid;
use super::select::{Action, Surface, act, find_bookmark_target, selected_quote_and_offset};
use super::view::{Doc, kept_at};
use crate::theme::{Appearance, ThemeChoice};

fn html_message(id_hint: usize, subject: &str, html: &str) -> Message {
    Message {
        from: Address::new("Sender", "sender@example.org"),
        to: vec![Address::new("Reader", "reader@example.org")],
        subject: subject.to_string(),
        body: Body { text: String::new(), html: Some(html.to_string()) },
        received: 1_791_993_600 + id_hint as i64 * 60,
        ..Default::default()
    }
}

fn drag(harness: &mut egui_kittest::Harness<'static, crate::MailApp>, from: egui::Pos2, to: egui::Pos2) {
    let mut step = |event: egui::Event| {
        harness.event(event);
        harness.step();
    };
    let click = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    step(egui::Event::PointerMoved(from));
    step(click(from, true));
    for n in 1..=5 {
        step(egui::Event::PointerMoved(from + (to - from) * (n as f32 / 5.0)));
    }
    step(click(to, false));
}

fn gap_at(harness: &egui_kittest::Harness<'static, crate::MailApp>, place: Place, id: Id, substr: &str, char_in_seg: usize) -> egui::Pos2 {
    let ctx = harness.ctx.clone();
    kept_at(&ctx, place, id, |laid, origin| {
        let item = laid
            .items
            .iter()
            .find(|i| matches!(&i.kind, super::layout::ItemKind::Text(t) if t.job.text.contains(substr)))
            .unwrap_or_else(|| panic!("no segment containing {substr:?}"));
        let super::layout::ItemKind::Text(text) = &item.kind else { panic!() };
        let byte_start = text.job.text.find(substr).unwrap();
        let base_chars = text.job.text[..byte_start].chars().count();
        let galley = ctx.fonts_mut(|f| f.layout_job((*text.job).clone()));
        let at = galley.pos_from_cursor(egui::text::CCursor::new(base_chars + char_in_seg));
        origin + item.rect.min.to_vec2() + egui::vec2(text.anchor + at.center().x, at.center().y)
    })
    .unwrap_or_else(|| panic!("no layout for {place:?} msg {id}"))
}

#[test]
fn window_place_bookmarks_displayed_message_not_window_counter_and_eml_disables() {
    let m1 = html_message(1, "Windowed Alpha", "<p>Unique sentence in windowed alpha message.</p>");
    let (app, ids) = super::tests::app_with(vec![m1]);
    let mid = ids[0];
    let mut harness = super::tests::window(app, (1280.0, 820.0));
    super::tests::select(&mut harness, mid);
    harness.run_steps(4);

    // Pop out into window 1 (window id = 1, while mailbox message id != 1).
    let win_id = harness.state_mut().execute("ui.openWindow", json!({"message": mid})).unwrap()["window"].as_u64().unwrap();
    assert_ne!(win_id, mid, "window counter must differ from message id for this test");
    harness.run_steps(6);

    let place = Place::Window(win_id);
    let from = gap_at(&harness, place, mid, "Unique sentence", 0);
    let to = gap_at(&harness, place, mid, "Unique sentence", "Unique sentence".len());
    drag(&mut harness, from, to);

    // Trigger AddBookmark from Place::Window(win_id) directly and verify message id + offset.
    {
        let app = harness.state_mut();
        act(app, place, Action::AddBookmark);
        let bms = app.session.mailbox.bookmarks_for_message(mid);
        assert_eq!(bms.len(), 1, "bookmark must land on displayed message {mid}, not window {win_id}");
        assert!(app.session.mailbox.bookmarks_for_message(win_id).is_empty());
        assert_eq!(bms[0].quote, "Unique sentence");
        assert_eq!(bms[0].offset, 0);
        assert!(app.last_error.is_none());
    }

    // Now open an unfiled .eml window (id == 0): Add Bookmark must be disabled, and invoking it is a no-op with no error.
    let eml = Arc::new(html_message(2, "Standalone EML", "<p>Opened from disk without mailbox id.</p>"));
    let eml_win =
        harness.state_mut().windows.add_file(eml, Arc::new(b"From: a@example.org\r\nSubject: Standalone EML\r\n\r\nOpened".to_vec())).unwrap();
    harness.run_steps(6);
    let eml_place = Place::Window(eml_win);
    let from_eml = gap_at(&harness, eml_place, 0, "Opened from disk", 0);
    let to_eml = gap_at(&harness, eml_place, 0, "Opened from disk", 16);
    drag(&mut harness, from_eml, to_eml);

    // Right-click in the .eml window to open its context menu and check "Add Bookmark" is disabled.
    let mid_pos = gap_at(&harness, eml_place, 0, "Opened from disk", 4);
    harness.event(egui::Event::PointerMoved(mid_pos));
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: mid_pos,
        button: egui::PointerButton::Secondary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    harness.step();
    harness.event(egui::Event::PointerButton {
        pos: mid_pos,
        button: egui::PointerButton::Secondary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    harness.run_steps(2);
    if let Some(btn) = harness.query_by_label("Add Bookmark") {
        // Button if visible must not be enabled for clicking.
        assert!(btn.accesskit_node().is_disabled(), "Add Bookmark must be disabled for unfiled .eml window");
    } else {
        panic!("expected disabled Add Bookmark item in .eml context menu");
    }

    let before_count = harness.state().session.mailbox.bookmarks().len();
    act(harness.state_mut(), eml_place, Action::AddBookmark);
    assert_eq!(harness.state().session.mailbox.bookmarks().len(), before_count);
    assert!(harness.state().last_error.is_none());
}

#[test]
fn jump_resolves_multi_segment_repeated_moved_and_header_quotes() {
    let m = html_message(
        1,
        "Header Only Subject",
        "<p>Clause repeating phrase</p>\
         <p>Paragraph two filler before the second occurrence.</p>\
         <p>Clause repeating phrase</p>\
         <p>First half</p><p>Second half</p>\
         <table><tr><td>Cell A</td><td>Cell B</td></tr></table>",
    );
    let (app, ids) = super::tests::app_with(vec![m.clone()]);
    let mid = ids[0];
    let mut harness = super::tests::window(app, (1280.0, 820.0));
    super::tests::select(&mut harness, mid);
    harness.run_steps(5);

    // 1. Select across two paragraphs (<p>First half</p><p>Second half</p>) -> captures multi-segment quote and offset.
    let from_p = gap_at(&harness, Place::Pane, mid, "First half", 0);
    let to_p = gap_at(&harness, Place::Pane, mid, "Second half", "Second half".len());
    drag(&mut harness, from_p, to_p);
    let (multi_quote, multi_off) = {
        let ctx = harness.ctx.clone();
        let st = harness.state_mut().text_selection.state(Place::Pane).unwrap();
        selected_quote_and_offset(&ctx, st).unwrap()
    };
    assert!(multi_quote.contains("First half") && multi_quote.contains("Second half"), "{multi_quote:?}");
    assert!(multi_off > 0);

    // Verify find_bookmark_target matches across those two segments!
    kept_at(&harness.ctx.clone(), Place::Pane, mid, |laid, _| {
        let jump = find_bookmark_target(laid, &multi_quote, multi_off);
        assert!(jump.resolved && jump.scroll.is_some() && jump.range.is_some(), "multi-segment quote must resolve");
        let (p1, p2) = jump.range.unwrap();
        assert!(p1.seg < p2.seg, "multi-segment highlight should span segments: {p1:?}..{p2:?}");
    })
    .unwrap();

    // 2. Repeated quote: select the SECOND "Clause repeating phrase", verify jump picks the 2nd occurrence (not 1st!).
    let second_rect = kept_at(&harness.ctx.clone(), Place::Pane, mid, |laid, _| {
        let rects: Vec<egui::Rect> = laid
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                super::layout::ItemKind::Text(t) if t.job.text.contains("Clause repeating phrase") => Some(i.rect),
                _ => None,
            })
            .collect();
        assert_eq!(rects.len(), 2);
        rects[1]
    })
    .unwrap();

    let (rep_quote, rep_off) = kept_at(&harness.ctx.clone(), Place::Pane, mid, |laid, _| {
        // Find where the 2nd occurrence actually sits.
        let mut seg2 = 0;
        let mut seen = 0;
        for s in 0..laid.count() {
            if let Some(super::layout::ItemKind::Text(t)) = laid.segment(s).map(|i| &i.kind)
                && t.job.text.contains("Clause repeating phrase")
            {
                seen += 1;
                if seen == 2 {
                    seg2 = s;
                    break;
                }
            }
        }
        let mut total = 0;
        for (idx, ch_seg) in (0..seg2).map(|s| (s, laid.segment(s))).collect::<Vec<_>>() {
            let _ = idx;
            if let Some(item) = ch_seg
                && let super::layout::ItemKind::Text(t) = &item.kind
            {
                let mut tmp = String::new();
                t.join.write(&mut tmp);
                total += tmp.chars().count() + t.lead.chars().count() + t.chars as usize;
            }
        }
        ("Clause repeating phrase".to_string(), total)
    })
    .unwrap();

    kept_at(&harness.ctx.clone(), Place::Pane, mid, |laid, _| {
        // Exact offset hits 2nd occurrence:
        let jump_exact = find_bookmark_target(laid, &rep_quote, rep_off);
        assert_eq!(jump_exact.scroll, Some(second_rect));
        // Slightly shifted offset (+4 chars for moved text tolerance) STILL hits 2nd occurrence:
        let jump_moved = find_bookmark_target(laid, &rep_quote, rep_off + 4);
        assert_eq!(jump_moved.scroll, Some(second_rect));

        // 3. Header-only quote (absent from body) with offset 0 -> resolved with NO scroll and NO range/error.
        let jump_header = find_bookmark_target(laid, "Header Only Subject", 0);
        assert!(jump_header.resolved);
        assert!(jump_header.scroll.is_none());
        assert!(jump_header.range.is_none());

        // 4. Quote absent from body WITH offset > 0 -> falls back to offset!
        let jump_fallback = find_bookmark_target(laid, "Nonexistent quote", 30);
        assert!(jump_fallback.resolved);
        assert!(jump_fallback.scroll.is_some());
    })
    .unwrap();

    // 5. Partial layout must wait (`resolved: false`) if offset lies beyond currently laid text.
    let ctx = harness.ctx.clone();
    crate::theme::apply(&ctx, Appearance::Light.preference(None), &ThemeChoice::default());
    let doc = Doc::of(&m);
    let pictures = Pictures::of(&ctx);
    let mut pictures = pictures.lock().unwrap();
    let mut partial = Laid::new(600.0).cut(doc.cuts.clone());
    partial.advance(&ctx, &doc.blocks, Some(&m), &mut pictures, 1);
    if !partial.done {
        let jump_wait = find_bookmark_target(&partial, "Clause repeating phrase", 5_000);
        assert!(!jump_wait.resolved, "partial layout must wait when closer match could appear later");
    }
}

#[test]
fn bookmark_target_isolated_to_destination_and_cleared_on_pane_off_and_replace() {
    let m1 = html_message(1, "Alpha Msg", "<p>Paragraph in alpha message for jump testing.</p>");
    let m2 = html_message(2, "Beta Msg", "<p>Second message body.</p>");
    let (app, ids) = super::tests::app_with(vec![m1, m2]);
    let (mid1, mid2) = (ids[0], ids[1]);
    let mut harness = super::tests::window(app, (1280.0, 820.0));
    super::tests::select(&mut harness, mid1);
    harness.run_steps(4);

    let added = harness.state_mut().run("bookmark.add", json!({"id": mid1, "quote": "Paragraph in alpha", "offset": 0})).unwrap();
    let bid = added["id"].as_u64().unwrap();

    // Pop out mid1 into its own window BEFORE triggering pane jump.
    let win_id = harness.state_mut().execute("ui.openWindow", json!({"message": mid1})).unwrap()["window"].as_u64().unwrap();

    // Turn reading pane off while mid1 is open in window, then trigger bookmark.jump:
    // target must NOT stay queued for a later reading-pane show.
    harness.state_mut().run("view.readingPane", json!({"value": "off"})).unwrap();
    harness.state_mut().run("bookmark.jump", json!({"id": bid})).unwrap();
    assert!(harness.state().bookmark_target.is_none());

    // Turn reading pane back on (right), select mid2, then jump to mid1's bookmark.
    harness.state_mut().run("view.readingPane", json!({"value": "right"})).unwrap();
    harness.state_mut().run("mail.select", json!({"id": mid2})).unwrap();
    assert!(harness.state().bookmark_target.is_none());

    harness.state_mut().run("bookmark.jump", json!({"id": bid})).unwrap();
    let target = harness.state().bookmark_target.clone().unwrap();
    assert_eq!(target.place, Place::Pane);
    assert_eq!(target.message, mid1);

    // Switching reading pane off drops pending bookmark_target.
    harness.state_mut().run("view.readingPane", json!({"value": "off"})).unwrap();
    assert!(harness.state().bookmark_target.is_none());

    // Jump again with reading pane open; pop-out window win_id of the same message must NOT steal Pane's jump.
    harness.state_mut().run("view.readingPane", json!({"value": "right"})).unwrap();
    harness.state_mut().run("bookmark.jump", json!({"id": bid})).unwrap();
    assert!(harness.state().bookmark_target.is_some());
    harness.run_steps(4);
    assert!(harness.state().bookmark_target.is_none(), "pane jump consumed and cleared once resolved");
    assert!(harness.state().text_selection.at(Place::Pane).is_some(), "pane should have bookmark selection");
    assert!(harness.state().text_selection.at(Place::Window(win_id)).is_none(), "pop-out window must not take pane's jump");

    // Replacing mailbox via file.new clears pending bookmark_target (via drop_secrets_if_replaced + run).
    harness.state_mut().run("bookmark.jump", json!({"id": bid})).unwrap();
    assert!(harness.state().bookmark_target.is_some());
    harness.state_mut().run("file.new", json!({})).unwrap();
    assert!(harness.state().bookmark_target.is_none());
}
