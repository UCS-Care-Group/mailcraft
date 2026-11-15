// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures' formatting bar is the Message tab's Basic Text group (#580): every one of
//! its controls, showing what the selection of the open signature is and acting on it, with the
//! keyboard left in the signature after a click. Driven in a window without a window.

use egui::{Event, Key, Modifiers, Pos2, Rect, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::rich::{Align, ListKind};
use serde_json::json;

use crate::compose::editor;
use crate::ribbon::{basic_text, hit_id, look_now};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = Harness<'static, MailApp>;

/// File › Signatures with the sample's "Standard" open ("Avery Quinn", "Lumen Studio") and a
/// message being written behind it.
fn signature_page() -> Window {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.execute("mail.new", json!({})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1280.0, 1800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    h.run_steps(3);
    // The sample's reminders would stand over the page.
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(2);
    h
}

fn click(h: &mut Window, at: Pos2) {
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(1);
}

fn key_with(h: &mut Window, modifiers: Modifiers, key: Key) {
    for pressed in [true, false] {
        h.event(Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
        h.step();
    }
    h.run_steps(1);
}

fn type_in(h: &mut Window, text: &str) {
    h.event(Event::Text(text.into()));
    h.run_steps(2);
}

/// The control of the signature's bar called `key` (as the ribbon calls it), where it was drawn.
fn button(h: &Window, key: &str) -> Rect {
    h.state().hit.rect("signatureBar", hit_id(key)).unwrap_or_else(|| panic!("no {key} on the signature's bar"))
}

/// A click on the control of the bar called `key`.
fn press(h: &mut Window, key: &str) {
    let at = button(h, key).center();
    click(h, at);
    h.run_steps(2);
}

/// A click at the start of the signature's text: the caret there, and the keyboard.
fn into_text(h: &mut Window) {
    let r = h.state().hit.rect("signatureBody", 0).unwrap();
    click(h, r.min + vec2(1.0, 8.0));
    assert!(editor::has_focus(&h.ctx));
}

fn edited(h: &Window) -> &mailcraft_model::RichBody {
    &h.state().session.signature_edit.as_ref().unwrap().body
}

fn message(h: &Window) -> String {
    h.state().session.compose.as_ref().unwrap().body.plain()
}

/// Everything the last frame painted, one shape after another.
fn painted(h: &Window) -> Vec<Shape> {
    fn add(shape: &Shape, out: &mut Vec<Shape>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| add(s, out)),
            other => out.push(other.clone()),
        }
    }
    let mut out = Vec::new();
    h.output().shapes.iter().for_each(|s| add(&s.shape, &mut out));
    out
}

/// The controls of the bar drawn as on: those of the Basic Text group that run a command and have
/// the ribbon's "checked" fill painted over them.
fn shown_on(h: &Window) -> Vec<&'static str> {
    let fill = crate::theme::Tokens::get(&h.ctx).checked;
    let rects: Vec<Rect> = painted(h)
        .into_iter()
        .filter_map(|s| match s {
            Shape::Rect(r) if r.fill == fill => Some(r.rect),
            _ => None,
        })
        .collect();
    basic_text(&look_now(h.state()))
        .into_iter()
        .filter(|item| item.command.is_some())
        .filter(|item| h.state().hit.rect("signatureBar", hit_id(item.key)).is_some_and(|b| rects.contains(&b)))
        .map(|item| item.key)
        .collect()
}

#[test]
fn the_signature_bar_offers_every_basic_text_control_of_the_message_tab() {
    let mut h = signature_page();
    // The Message tab's group is the one list the bar is drawn from.
    h.state_mut().ui.tab = "Message".into();
    let tab = crate::ribbon::describe(h.state());
    let group = tab.groups.iter().find(|g| g.title == "Basic Text").unwrap();
    assert_eq!(group.items, basic_text(&look_now(h.state())));
    // Each of its buttons is on the bar, in the group's order, along one row under the first.
    let commands: Vec<&str> = group.items.iter().filter(|i| i.command.is_some()).map(|i| i.key).collect();
    assert_eq!(commands.len(), 11);
    let rects: Vec<Rect> = commands.iter().map(|key| button(&h, key)).collect();
    for pair in rects.windows(2) {
        assert!(pair[0].max.x <= pair[1].min.x && (pair[0].center().y - pair[1].center().y).abs() < 0.5, "{pair:?}");
    }
    // Its menus are the bar's Font, Size, Colour and Highlight, and the rest of the first row is as
    // it was.
    let menus: Vec<&str> = group.items.iter().filter(|i| i.menu.is_some()).map(|i| i.key).collect();
    assert_eq!(menus, ["font", "fontSize", "fontColor", "highlight"]);
    for label in ["Font", "Size", "Colour", "Highlight", "Link", "Insert Picture"] {
        let at = h.get_by_label(label).rect();
        assert!(at.max.y <= rects[0].min.y, "{label} is above the row of buttons");
    }
    // Each button's tooltip names it, with its own key: three that run one command say three.
    let tips = [
        ("format.bold", "Bold (Mod+B)"),
        ("format.alignLeft", "Align Left (Mod+L)"),
        ("format.alignCenter", "Center (Mod+E)"),
        ("format.alignRight", "Align Right (Mod+R)"),
        ("format.bullets", "Bullets"),
        ("format.clear", "Clear All Formatting"),
    ];
    for (key, tip) in tips {
        h.event(Event::PointerMoved(button(&h, key).center()));
        h.run_steps(4);
        let tip = crate::widgets::key_text(tip);
        assert!(h.query_by_label(&tip).is_some(), "{key}: no tooltip {tip:?}");
    }
}

#[test]
fn bullets_from_the_signature_bar_make_the_open_signature_a_list_item() {
    let mut h = signature_page();
    let before = message(&h);
    into_text(&mut h);
    press(&mut h, "format.bullets");
    let lists: Vec<ListKind> = edited(&h).text().paragraphs().iter().map(|p| p.list).collect();
    assert_eq!(lists, [ListKind::Bullet, ListKind::None]);
    assert!(h.state().session.signature_edit.as_ref().unwrap().dirty);
    // Numbering, then the indent both ways, act on the same line.
    press(&mut h, "format.numbering");
    assert_eq!(edited(&h).text().paragraphs()[0].list, ListKind::Numbered);
    press(&mut h, "format.indent");
    assert_eq!(edited(&h).text().paragraphs()[0].level, 1);
    press(&mut h, "format.outdent");
    assert_eq!(edited(&h).text().paragraphs()[0].level, 0);
    assert_eq!(message(&h), before, "the message behind is not touched");
}

#[test]
fn the_signature_bar_shows_what_the_selection_is() {
    let mut h = signature_page();
    into_text(&mut h);
    assert_eq!(shown_on(&h), ["format.alignLeft"]);
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    key_with(&mut h, Modifiers::COMMAND, Key::B);
    h.run_steps(1);
    assert_eq!(shown_on(&h), ["format.bold", "format.alignCenter"]);
    // A list item is always left.
    h.state_mut().run("format.numbering", json!({})).unwrap();
    h.run_steps(2);
    assert_eq!(shown_on(&h), ["format.bold", "format.numbering", "format.alignLeft"]);
    // Over both lines, an item and a line that is none: only that both are left.
    key_with(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(shown_on(&h), ["format.alignLeft"]);
}

#[test]
fn clear_formatting_on_the_signature_bar_leaves_plain_left_text() {
    let mut h = signature_page();
    into_text(&mut h);
    key_with(&mut h, Modifiers::COMMAND, Key::A);
    for (command, params) in [("format.bold", json!({})), ("format.underline", json!({})), ("format.align", json!({"value": "right"}))] {
        h.state_mut().run(command, params).unwrap();
    }
    key_with(&mut h, Modifiers::COMMAND, Key::End);
    h.state_mut().run("format.bullets", json!({})).unwrap();
    key_with(&mut h, Modifiers::COMMAND, Key::A);
    press(&mut h, "format.clear");
    let text = edited(&h).text();
    assert_eq!(text.to_plain(), "Avery Quinn\nLumen Studio");
    assert!(!text.has_formatting(), "{:?}", text.paragraphs());
    assert!(text.paragraphs().iter().all(|p| p.align == Align::Left && p.list == ListKind::None));
}

#[test]
fn centre_from_the_signature_bar_is_shown_in_the_preview_as_sent() {
    let mut h = signature_page();
    // Where the preview as recipients see it starts each of the two lines: under the sample
    // reply's own line, which only the preview has.
    let starts = |h: &Window| {
        let texts: Vec<(Pos2, String)> = painted(h)
            .into_iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some((t.pos + t.galley.rect.min.to_vec2(), t.galley.text().to_string())),
                _ => None,
            })
            .collect();
        let reply = texts.iter().find(|(_, text)| text.starts_with("Thanks, Thursday")).expect("the preview").0.y;
        let at =
            |line: &str| texts.iter().find(|(at, text)| text == line && at.y > reply).unwrap_or_else(|| panic!("{line} is not in the preview")).0.x;
        (at("Avery Quinn"), at("Lumen Studio"))
    };
    let (name, studio) = starts(&h);
    assert!((name - studio).abs() < 0.5);
    into_text(&mut h);
    press(&mut h, "format.alignCenter");
    assert_eq!(edited(&h).text().paragraphs()[0].align, Align::Center);
    h.run_steps(2);
    let (centred, left) = starts(&h);
    assert!((left - studio).abs() < 0.5, "the second line stays left");
    assert!(centred > name + 100.0, "the first line is centred in the preview: {centred} from {name}");
    // As it is sent.
    let e = h.state().session.signature_edit.as_ref().unwrap();
    let mut sig = e.signature.clone();
    sig.set_body(e.body.text().clone()).unwrap();
    let html = mailcraft_engine::mime::rich::signature_html(&sig, &|p| format!("sig-{}@x.invalid", p.id));
    assert!(html.contains("<div style=\"text-align:center\">Avery Quinn</div>"), "{html}");
}

#[test]
fn typing_goes_on_in_the_signature_after_a_bar_button() {
    let mut h = signature_page();
    let before = message(&h);
    into_text(&mut h);
    for key in ["format.bold", "format.alignRight", "format.bullets", "format.clear"] {
        press(&mut h, key);
        assert!(editor::has_focus(&h.ctx), "the keyboard is back in the signature after {key}");
    }
    press(&mut h, "format.italic");
    type_in(&mut h, "Dr ");
    let text = edited(&h).text();
    assert_eq!(text.to_plain(), "Dr Avery Quinn\nLumen Studio");
    assert!(text.style_at(1).italic);
    assert_eq!(message(&h), before, "nothing went into the message behind");
}

/// Whether the keyboard is in the open signature's text (a body, and not the message's).
fn in_signature(h: &Window) -> bool {
    h.ctx.memory(|m| m.focused()).is_some_and(|id| editor::is_body(&h.ctx, id) && id != editor::id(h.state()))
}

/// The message's own format (HTML, Plain Text) gives the keyboard back to the message even while
/// a signature is open (a message written in a window of its own, #286), and the request waits
/// for the message's editor to be able to take it; what edits the signature gives it back to the
/// signature.
#[test]
fn the_messages_format_gives_the_keyboard_to_the_message_not_the_open_signature() {
    let mut h = signature_page();
    into_text(&mut h);
    let compose = h.state().session.compose_id();
    assert!(compose.is_some());
    h.state_mut().run("format.bold", json!({})).unwrap();
    assert!(h.state().signature_focus_wanted && h.state().body_focus_wanted.is_none());
    h.run_steps(2);
    assert!(in_signature(&h));
    h.state_mut().run("compose.formatPlain", json!({})).unwrap();
    assert!(!h.state().signature_focus_wanted, "the signature isn't asked to take the keyboard");
    assert_eq!(h.state().body_focus_wanted, compose);
    h.run_steps(2);
    assert_eq!(h.state().body_focus_wanted, compose, "kept while the message can't take it");
    h.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    h.run_steps(3);
    assert_eq!(h.ctx.memory(|m| m.focused()), Some(editor::id(h.state())), "the message has the keyboard");
}

/// A signature closed before its editor took the keyboard back (Escape, `signature.close`)
/// leaves no request for the next one opened.
#[test]
fn a_signature_closed_before_it_took_the_keyboard_back_leaves_nothing_for_the_next() {
    let mut h = signature_page();
    into_text(&mut h);
    h.state_mut().run("format.bold", json!({})).unwrap();
    assert!(h.state().signature_focus_wanted);
    // Closed (as a script or the control channel closes it, between frames): the request goes
    // with it at once, before the page opens the first signature again.
    h.state_mut().execute("signature.close", json!({"discard": true})).unwrap();
    assert!(!h.state().signature_focus_wanted, "closing it takes the request with it");
    h.ctx.memory_mut(|m| m.stop_text_input());
    // Opened again over a request of its own: the request goes too.
    h.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    h.state_mut().run("format.bold", json!({})).unwrap();
    assert!(h.state().signature_focus_wanted);
    h.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    assert!(!h.state().signature_focus_wanted, "nor is one left for the signature opened next");
    h.run_steps(3);
    assert!(!in_signature(&h), "the signature opened again doesn't take the keyboard by itself");
    h.state_mut().execute("signature.close", json!({"discard": true})).unwrap();
    h.state_mut().execute("ui.backstage", json!({"value": false})).unwrap();
    h.ctx.memory_mut(|m| m.stop_text_input());
    h.run_steps(2);
    assert!(!h.state().signature_focus_wanted);
    h.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    h.run_steps(3);
    assert!(h.state().session.signature_edit.is_some(), "a signature is open again");
    assert!(!in_signature(&h), "it doesn't take the keyboard by itself");
}
