// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The password's eye (#438), in the Sign In dialog of Send/Receive: hidden at first, shown and
//! hidden by clicks, never copied, cut or spoken, never moved by the control channel, and dots in
//! a screenshot. (Add Account's and Account Settings' are in `account_setup/tests/reveal.rs`.)

use egui::accesskit::Role;
use egui::epaint::text::PASSWORD_REPLACEMENT_CHAR as MASK;
use egui::{Event, Key, Modifiers, OutputCommand, Shape};
use egui_kittest::Harness;
use egui_kittest::kittest::{By, Queryable as _};
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::secret_field::{eye_name, masked};
use crate::{ControlRequest, MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// What the tests type where a password goes. It opens nothing.
const TYPED: &str = "example-pass";

/// The window with the Sign In dialog up for a new account, and the control channel's sender.
fn sign_in() -> (Harness<'static, MailApp>, std::sync::mpsc::Sender<ControlRequest>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default()).with_control(rx);
    (app.autosave, app.auto_sync) = (false, false);
    let r = app.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}));
    let account = r.unwrap()["id"].as_u64().unwrap();
    app.dialog = Some(Dialog::Password { account, password: String::new(), shown: false });
    let h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    let mut h = h;
    steps(&mut h, 6);
    (h, tx)
}

/// Frames as the window runs them, with what the control channel injected.
fn steps(h: &mut Harness<'static, MailApp>, n: usize) {
    for _ in 0..n {
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
    }
}

fn type_password(h: &mut Harness<'static, MailApp>) {
    h.event(Event::Text(TYPED.to_string()));
    steps(h, 3);
}

/// Whether the dialog's eye says `shown` (by its name) and the dialog keeps that.
fn eye_says(h: &Harness<'static, MailApp>, shown: bool) -> bool {
    let named = h.query_by_role_and_label(Role::Button, eye_name(shown)).is_some();
    named && matches!(h.state().dialog, Some(Dialog::Password { shown: s, .. }) if s == shown)
}

fn click_eye(h: &mut Harness<'static, MailApp>) {
    let name = h.query_by_role_and_label(Role::Button, eye_name(true)).map_or(eye_name(false), |_| eye_name(true));
    h.get_by_role_and_label(Role::Button, name).click();
    steps(h, 3);
}

/// The widths of the glyphs the password field drew, when it drew `TYPED` (as itself or as dots).
fn drawn(h: &Harness<'static, MailApp>) -> Option<Vec<f32>> {
    fn find(shape: &Shape, out: &mut Option<Vec<f32>>) {
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, out)),
            Shape::Text(t) if t.galley.text().chars().count() == TYPED.len() && t.galley.text().chars().all(|c| c == MASK) => {
                *out = Some(t.galley.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.advance_width)).collect());
            }
            _ => {}
        }
    }
    let mut out = None;
    h.output().shapes.iter().for_each(|s| find(&s.shape, &mut out));
    out
}

/// The field drew the password as itself: its glyphs aren't all the same dot.
fn drawn_as_itself(h: &Harness<'static, MailApp>) -> bool {
    drawn(h).is_some_and(|w| w.windows(2).any(|p| (p[0] - p[1]).abs() > 0.01))
}

/// The field drew dots: as many glyphs as were typed, all alike.
fn drawn_as_dots(h: &Harness<'static, MailApp>) -> bool {
    drawn(h).is_some_and(|w| w.len() == TYPED.len() && w.windows(2).all(|p| (p[0] - p[1]).abs() <= 0.01))
}

/// Whether anything screen readers and AccessKit are given has the password in it.
fn spoken(h: &Harness<'static, MailApp>) -> bool {
    let leaks = |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.value().is_some_and(|v| v.contains(TYPED))
            || n.label().is_some_and(|l| l.contains(TYPED))
            || n.description().is_some_and(|d| d.contains(TYPED))
    };
    h.query_all(By::new().include_labels().predicate(leaks)).next().is_some()
}

/// What this frame put on the clipboard.
fn copied(h: &Harness<'static, MailApp>) -> bool {
    h.output().platform_output.commands.iter().any(|c| matches!(c, OutputCommand::CopyText(_)))
}

#[test]
fn the_eye_shows_the_password_until_it_is_clicked_again() {
    let (mut h, _tx) = sign_in();
    type_password(&mut h);
    // Hidden at first: dots, and an eye to show it.
    assert!(eye_says(&h, false) && drawn_as_dots(&h) && !spoken(&h));
    click_eye(&mut h);
    assert!(eye_says(&h, true) && drawn_as_itself(&h), "{:?}", drawn(&h));
    // The field keeps the keyboard: what is typed next goes on the end.
    assert!(matches!(&h.state().dialog, Some(Dialog::Password { password, .. }) if password == TYPED));
    click_eye(&mut h);
    assert!(eye_says(&h, false) && drawn_as_dots(&h));
}

#[test]
fn the_eye_is_named_for_screen_readers() {
    assert_eq!((eye_name(false), eye_name(true)), ("Show password", "Hide password"));
    let (mut h, _tx) = sign_in();
    assert!(h.query_by_role_and_label(Role::Button, "Show password").is_some());
    click_eye(&mut h);
    assert!(h.query_by_role_and_label(Role::Button, "Hide password").is_some());
    assert!(h.query_by_role_and_label(Role::Button, "Show password").is_none());
}

#[test]
fn shown_it_is_still_not_copied_cut_or_spoken() {
    let (mut h, _tx) = sign_in();
    type_password(&mut h);
    click_eye(&mut h);
    assert!(drawn_as_itself(&h));
    // The field is still a password's, and nothing in the tree has the password in it.
    assert!(h.query_by_role(Role::PasswordInput).is_some() && !spoken(&h));
    // All of it selected, then copied: nothing reaches the clipboard.
    h.get_by_role(Role::PasswordInput).focus();
    steps(&mut h, 2);
    h.event(Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND });
    h.step();
    h.event(Event::Copy);
    h.step();
    assert!(!copied(&h) && !spoken(&h));
    // Cut: nothing reaches the clipboard either.
    h.event(Event::Cut);
    h.step();
    assert!(!copied(&h));
    // (The selection is not told to the system, which on Linux would make it the primary one.)
    assert!(!h.output().platform_output.commands.iter().any(|c| matches!(c, OutputCommand::TextSelectionSettled(_))));
}

/// Each event in a frame of its own, as the window runs them: whether any of those frames put
/// something in the primary selection (on Linux, what a middle click pastes anywhere).
fn told_the_system(h: &mut Harness<'static, MailApp>, events: impl IntoIterator<Item = Event>) -> bool {
    let mut told = false;
    for event in events {
        h.event(event);
        steps(h, 1);
        told |= h.output().platform_output.commands.iter().any(|c| matches!(c, OutputCommand::TextSelectionSettled(_)));
    }
    told
}

#[test]
fn selected_with_the_keys_or_the_pointer_it_never_goes_to_the_primary_selection() {
    let (mut h, _tx) = sign_in();
    type_password(&mut h);
    for shown in [false, true] {
        if shown {
            click_eye(&mut h);
        }
        let field = h.get_by_role(Role::PasswordInput).rect();
        let (from, to) = (field.left_center() + egui::vec2(6.0, 0.0), field.center());
        let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let all = |pressed| Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND };
        // A drag across it, two clicks on it, and Ctrl+A in it.
        let mut events = vec![Event::PointerMoved(from), button(from, true), Event::PointerMoved(to), button(to, false)];
        events.extend([Event::PointerMoved(to), button(to, true), button(to, false), button(to, true), button(to, false)]);
        events.extend([all(true), all(false)]);
        events.push(Event::PointerMoved(to));
        assert!(!told_the_system(&mut h, events), "shown: {shown}");
        assert!(eye_says(&h, shown));
    }
}

#[test]
fn closing_the_dialog_hides_it() {
    let (mut h, _tx) = sign_in();
    type_password(&mut h);
    click_eye(&mut h);
    assert!(eye_says(&h, true));
    h.key_press(Key::Escape);
    steps(&mut h, 3);
    assert!(h.state().dialog.is_none());
    // Asked again, it starts hidden.
    let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "avery@mail.example").map(|a| a.id).unwrap();
    h.state_mut().execute("ui.dialog", json!({"name": "password", "account": account})).unwrap();
    steps(&mut h, 3);
    assert!(eye_says(&h, false));
}

#[test]
fn the_control_channel_cannot_move_the_eye() {
    let (mut h, tx) = sign_in();
    type_password(&mut h);
    let at = h.get_by_role_and_label(Role::Button, "Show password").rect().center();
    // A click where the eye is, as an agent or a script would send it.
    let (req, answer) = ControlRequest::new("ui.click", json!({"x": at.x, "y": at.y}));
    tx.send(req).unwrap();
    steps(&mut h, 8);
    assert_eq!(answer.try_recv().unwrap()["ok"], true);
    assert!(eye_says(&h, false) && drawn_as_dots(&h));
    // The owner's own click, at the same place, shows it.
    h.get_by_role_and_label(Role::Button, "Show password").click();
    steps(&mut h, 3);
    assert!(eye_says(&h, true));
}

#[test]
fn a_screenshot_draws_dots_while_it_is_shown() {
    let (mut h, tx) = sign_in();
    type_password(&mut h);
    click_eye(&mut h);
    assert!(drawn_as_itself(&h));
    // `ui.screenshot` waits for a frame: the frames it may take draw dots, with the eye as it is.
    let (req, _answer) = ControlRequest::new("ui.screenshot", json!({"path": "/nonexistent/shot.png"}));
    tx.send(req).unwrap();
    steps(&mut h, 1);
    for _ in 0..6 {
        steps(&mut h, 1);
        assert!(drawn_as_dots(&h) && eye_says(&h, true) && !spoken(&h));
    }
    // A window drawn for headless shots draws dots in every frame.
    let (mut h, _tx) = sign_in();
    h.state_mut().headless_shots = true;
    type_password(&mut h);
    click_eye(&mut h);
    assert!(eye_says(&h, true) && drawn_as_dots(&h));
}

/// The galley drawn for a shown password says dots wherever it is read: its text, its sections,
/// its glyphs' characters. Only the glyphs' shapes are the password's.
#[test]
fn the_drawn_galley_is_masked() {
    let ctx = egui::Context::default();
    let mut out = None;
    let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
        let job = egui::text::LayoutJob::simple_singleline("pässword".into(), egui::FontId::proportional(13.0), egui::Color32::WHITE);
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        out = Some((masked(&galley), galley));
    });
    // (No renderer takes the font's texture here.)
    frame.textures_delta.clear();
    let (dots, real) = out.unwrap();
    let n = "pässword".chars().count();
    assert_eq!(dots.text(), MASK.to_string().repeat(n));
    assert!(dots.job.sections.iter().all(|s| dots.text().get(s.byte_range.start.0..s.byte_range.end.0).is_some()));
    assert!(dots.rows.iter().flat_map(|r| r.glyphs.iter()).all(|g| g.chr == MASK));
    // The shapes stay the password's: the same glyphs, where they were.
    let widths = |g: &egui::Galley| g.rows.iter().flat_map(|r| r.glyphs.iter().map(|g| g.advance_width)).collect::<Vec<_>>();
    assert_eq!(widths(&dots), widths(&real));
    assert_eq!(
        dots.rows.iter().map(|r| r.visuals.mesh.vertices.len()).sum::<usize>(),
        real.rows.iter().map(|r| r.visuals.mesh.vertices.len()).sum::<usize>()
    );
}
