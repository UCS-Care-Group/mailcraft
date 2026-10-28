// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A right click where the owner types (#557): Cut, Copy, Paste and Select All in the body of
//! a message being written and in egui's text fields, each doing what its key does; Cut and Copy
//! greyed while nothing is selected, Paste alone in a password field, neither Cut nor Paste in a
//! text that is only read, and nothing cut, copied or pasted by a click the control channel
//! injected. Nothing on the clipboard to paste leaves the text as it was. The menu works in a
//! window of the system of its own, in a drop-down without closing it, and in the calendar's typed
//! entry without ending it. The clipboard is a stand-in: no test touches the system's.

use std::cell::RefCell;
use std::rc::Rc;

use egui::accesskit::Role;
use egui::{Event, Key, Modifiers, OutputCommand, PointerButton, Pos2, Rect, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::rich::Selection;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::widgets::key_text;
use crate::{MailApp, RichClipboard, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
/// What the tests type where a password goes. It opens nothing.
const TYPED: &str = "example-pass";

type Window = Harness<'static, MailApp>;

/// A clipboard that takes HTML, as the desktop's does: what was put on it, the HTML and the text
/// it holds, and how many times its text was read.
#[derive(Clone, Default)]
struct Board(Rc<RefCell<Held>>);

#[derive(Default)]
struct Held {
    put: Vec<(String, String)>,
    html: Option<String>,
    text: Option<String>,
    image: Option<Vec<u8>>,
    reads: usize,
}

impl Board {
    fn holding(text: &str) -> Board {
        let board = Board::default();
        board.0.borrow_mut().text = Some(text.to_string());
        board
    }
}

impl RichClipboard for Board {
    fn put(&self, html: &str, text: &str) -> Result<(), String> {
        self.0.borrow_mut().put.push((html.to_string(), text.to_string()));
        Ok(())
    }
    fn html(&self) -> Option<String> {
        self.0.borrow().html.clone()
    }
    fn text(&self) -> Option<String> {
        let mut held = self.0.borrow_mut();
        held.reads += 1;
        held.text.clone()
    }
    fn image(&self) -> Option<Vec<u8>> {
        self.0.borrow().image.clone()
    }
}

/// An item of the menu as it is read out: its name and its key.
fn item(label: &str, key: &str) -> String {
    format!("{label} {}", key_text(key))
}

fn cut() -> String {
    item("Cut", "Mod+X")
}
fn copy() -> String {
    item("Copy", "Mod+C")
}
fn paste() -> String {
    item("Paste", "Mod+V")
}
fn select_all() -> String {
    item("Select All", "Mod+A")
}

fn app(board: Option<&Board>) -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.services.clipboard = board.map(|b| Box::new(b.clone()) as Box<dyn RichClipboard>);
    a
}

fn window(app: MailApp) -> Window {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    settle(&mut h, 4);
    h
}

/// A new message with the subject "Plan for Friday" and `body`, the keyboard in the body, the
/// caret at its start.
fn writing(body: &str, board: Option<&Board>) -> Window {
    let mut a = app(board);
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"subject": "Plan for Friday", "body": body})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.body_focus_wanted = true;
    window(a)
}

/// A frame as the window runs it, with `events` and what the window injected for itself.
fn frame(h: &mut Window, events: Vec<Event>) {
    let mut raw = std::mem::take(h.input_mut());
    raw.events.extend(events);
    h.state_mut().raw_input_hook(&mut raw);
    *h.input_mut() = raw;
    h.step();
}

fn settle(h: &mut Window, n: usize) {
    for _ in 0..n {
        frame(h, Vec::new());
    }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

/// The owner's click at `pos`, a frame down and a frame up.
fn click(h: &mut Window, pos: Pos2, which: PointerButton) {
    frame(h, vec![Event::PointerMoved(pos)]);
    frame(h, vec![button(pos, which, true)]);
    frame(h, vec![button(pos, which, false)]);
}

/// The owner's right click at `pos`, and the frames its menu takes to be there.
fn menu_at(h: &mut Window, pos: Pos2) {
    click(h, pos, PointerButton::Secondary);
    settle(h, 2);
}

fn keys(h: &mut Window, modifiers: Modifiers, key: Key) {
    for pressed in [true, false] {
        frame(h, vec![Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }]);
    }
    settle(h, 1);
}

/// The item of the open menu called `label`: whether it can be chosen. `None`: there is none.
fn offered(h: &Window, label: &str) -> Option<bool> {
    h.query_all_by_label(label).next().map(|n| !n.accesskit_node().is_disabled())
}

/// Where the item of the open menu called `label` is.
fn place_of(h: &Window, label: &str) -> Rect {
    h.query_all_by_label(label).next().unwrap_or_else(|| panic!("nothing is called {label}")).rect()
}

/// Choose the item of the open menu called `label`, as the owner does.
fn choose(h: &mut Window, label: &str) {
    let at = place_of(h, label).center();
    click(h, at, PointerButton::Primary);
}

/// What the last frame handed to egui's clipboard, or else the next that does, a few at most.
fn copied_within(h: &mut Window, frames: usize) -> Option<String> {
    let copied = |h: &Window| {
        h.output().platform_output.commands.iter().rev().find_map(|c| match c {
            OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
    };
    for _ in 0..frames {
        if let Some(text) = copied(h) {
            return Some(text);
        }
        settle(h, 1);
    }
    copied(h)
}

fn plain(h: &Window) -> String {
    h.state().session.compose.as_ref().unwrap().body.plain()
}

fn sel(h: &Window) -> Selection {
    h.state().session.compose.as_ref().unwrap().body.selection()
}

fn subject(h: &Window) -> String {
    h.state().session.compose.as_ref().unwrap().subject.clone()
}

/// Where the subject line's text is.
fn subject_line(h: &Window) -> Rect {
    h.query_all_by_value("Plan for Friday").find(|n| n.accesskit_node().role() == Role::TextInput).expect("the subject line").rect()
}

/// The start of the first line of the body.
fn body_start(h: &Window) -> Pos2 {
    h.state().hit.rect("composeBody", 0).expect("the body").min + vec2(1.0, 8.0)
}

#[test]
fn the_bodys_menu_cuts_copies_and_pastes_as_its_keys_do() {
    let board = Board::default();
    let mut h = writing("Plan for Friday", Some(&board));
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
    settle(&mut h, 2);
    let start = body_start(&h);
    // On the selection: everything can be chosen, and the selection stays.
    menu_at(&mut h, start);
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [Some(true); 4]);
    assert_eq!(sel(&h), Selection { anchor: 0, head: 4 });
    // Copy: HTML and text, through the host's clipboard, as Ctrl+C does.
    choose(&mut h, &copy());
    settle(&mut h, 2);
    let (html, text) = board.0.borrow().put.last().cloned().expect("the selection went on the clipboard");
    assert_eq!(text, "Plan");
    assert!(html.contains("Plan"), "{html}");
    assert_eq!((plain(&h), sel(&h)), ("Plan for Friday".to_string(), Selection { anchor: 0, head: 4 }));
    assert_eq!(offered(&h, &copy()), None, "the menu shuts when something is chosen");
    // Cut: on the clipboard, and out of the text.
    menu_at(&mut h, start);
    choose(&mut h, &cut());
    settle(&mut h, 2);
    assert_eq!(board.0.borrow().put.len(), 2);
    assert_eq!(plain(&h), " for Friday");
    // Paste: the clipboard's HTML when it has some, reduced to what a message holds, as Ctrl+V.
    {
        let mut held = board.0.borrow_mut();
        held.html = Some("<b>Bold</b><script>x()</script>".into());
        held.text = Some("text that is not used".into());
    }
    menu_at(&mut h, start);
    choose(&mut h, &paste());
    settle(&mut h, 2);
    assert_eq!(plain(&h), "Bold for Friday");
    let body = h.state().session.compose.as_ref().unwrap().body.clone();
    assert!(body.text().paragraphs()[0].runs.iter().any(|r| r.style.bold && r.text == "Bold"));
    // Else its text. A right click away from the selection puts the caret where it was.
    {
        let mut held = board.0.borrow_mut();
        held.html = None;
        held.text = Some("Hi. ".into());
    }
    assert_eq!(sel(&h), Selection::caret(4));
    menu_at(&mut h, start);
    assert_eq!(sel(&h), Selection::caret(0));
    choose(&mut h, &paste());
    settle(&mut h, 2);
    assert_eq!(plain(&h), "Hi. Bold for Friday");
}

#[test]
fn with_nothing_selected_cut_and_copy_are_greyed() {
    let board = Board::default();
    let mut h = writing("Plan for Friday", Some(&board));
    let start = body_start(&h);
    menu_at(&mut h, start);
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [Some(false), Some(false), Some(true), Some(true)]);
    // Select All selects the whole body; then Cut and Copy can be chosen.
    choose(&mut h, &select_all());
    settle(&mut h, 2);
    assert_eq!(sel(&h), Selection { anchor: 0, head: 15 });
    menu_at(&mut h, start);
    assert_eq!([offered(&h, &cut()), offered(&h, &copy())], [Some(true); 2]);
    frame(&mut h, vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    settle(&mut h, 2);
    // The subject line: the caret in it, nothing selected.
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    settle(&mut h, 2);
    menu_at(&mut h, line.center());
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [Some(false), Some(false), Some(true), Some(true)]);
    assert!(board.0.borrow().put.is_empty());
}

#[test]
fn without_the_hosts_clipboard_paste_is_greyed() {
    // The browser has none to read from: Ctrl+V pastes there, the menu can't.
    let mut h = writing("Plan for Friday", None);
    let start = body_start(&h);
    menu_at(&mut h, start);
    assert_eq!(offered(&h, &paste()), Some(false));
    let line = subject_line(&h);
    frame(&mut h, vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    settle(&mut h, 2);
    menu_at(&mut h, line.center());
    assert_eq!(offered(&h, &paste()), Some(false));
}

#[test]
fn the_subject_line_cuts_copies_and_pastes_through_its_menu() {
    let board = Board::holding("Agenda for Monday");
    let mut h = writing("", Some(&board));
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    // A right click on what is selected leaves it selected, for Cut and Copy.
    menu_at(&mut h, line.center());
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [Some(true); 4]);
    choose(&mut h, &copy());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("Plan for Friday"));
    assert_eq!(subject(&h), "Plan for Friday");
    menu_at(&mut h, line.center());
    choose(&mut h, &cut());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("Plan for Friday"));
    settle(&mut h, 2);
    assert_eq!(subject(&h), "");
    menu_at(&mut h, line.center());
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!(subject(&h), "Agenda for Monday");
    // Select All, then Paste over it.
    board.0.borrow_mut().text = Some("Minutes".into());
    menu_at(&mut h, line.center());
    choose(&mut h, &select_all());
    settle(&mut h, 2);
    menu_at(&mut h, line.center());
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!(subject(&h), "Minutes");
    // What went to the clipboard went through egui's, as Ctrl+C's does; the body's HTML clipboard
    // took nothing.
    assert!(board.0.borrow().put.is_empty());
}

#[test]
fn a_password_field_offers_paste_alone() {
    let board = Board::holding(TYPED);
    let mut a = app(Some(&board));
    a.auto_sync = false;
    let r = a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}));
    let account = r.unwrap()["id"].as_u64().unwrap();
    a.dialog = Some(Dialog::Password { account, password: String::new(), shown: false });
    let mut h = window(a);
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Text("typed first".into())]);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    let field = h.get_by_role(Role::PasswordInput).rect();
    // (Away from the eye at its right end.)
    let at = field.left_center() + vec2(12.0, 0.0);
    menu_at(&mut h, at);
    // Something is selected in it, and still it can't be cut or copied.
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [None, None, Some(true), None]);
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert!(matches!(&h.state().dialog, Some(Dialog::Password { password, .. }) if password == TYPED), "the paste took the selection's place");
    assert!(board.0.borrow().put.is_empty());
}

#[test]
fn a_click_the_control_channel_injects_cuts_copies_and_pastes_nothing() {
    let board = Board::holding("from the clipboard");
    let mut h = writing("", Some(&board));
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    // What `ui.click` puts in the window: the menu opens for it, as anything does that a click opens.
    let injected = |h: &mut Window, pos: Pos2, which: PointerButton| {
        let synthetic = &mut h.state_mut().synthetic;
        synthetic.push(Event::PointerMoved(pos));
        synthetic.push(button(pos, which, true));
        synthetic.push(button(pos, which, false));
        settle(h, 5);
    };
    for label in [paste(), copy(), cut()] {
        injected(&mut h, line.center(), PointerButton::Secondary);
        assert_eq!(offered(&h, &label), Some(true));
        let item = place_of(&h, &label).center();
        injected(&mut h, item, PointerButton::Primary);
        assert_eq!(copied_within(&mut h, 4), None, "{label}");
        assert_eq!((subject(&h).as_str(), board.0.borrow().reads), ("Plan for Friday", 0), "{label}");
    }
    // Select All is no clipboard's: it selects.
    keys(&mut h, Modifiers::NONE, Key::End);
    injected(&mut h, line.center(), PointerButton::Secondary);
    let item = place_of(&h, &select_all()).center();
    injected(&mut h, item, PointerButton::Primary);
    menu_at(&mut h, line.center());
    assert_eq!(offered(&h, &copy()), Some(true));
    // The owner's own click pastes.
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!((subject(&h).as_str(), board.0.borrow().reads), ("from the clipboard", 1));
}

/// A picture as PNG, as a clipboard holding one hands it over.
fn png() -> Vec<u8> {
    use image::ImageEncoder as _;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(&[200; 4 * 4 * 3], 4, 4, image::ExtendedColorType::Rgb8).unwrap();
    out
}

/// Where the one-line text field holding `value` is.
fn text_field(h: &Window, value: &str) -> Option<Rect> {
    h.query_all_by_value(value).find(|n| n.accesskit_node().role() == Role::TextInput).map(|n| n.rect())
}

#[test]
fn nothing_on_the_clipboard_to_paste_leaves_the_text_as_it_was() {
    // An empty clipboard; one that can't be read, or holds more than a paste takes (the desktop's
    // then gives no text); and one with a picture alone, which only the one-line fields leave be.
    let cases: [(&str, Option<&str>, Option<Vec<u8>>); 3] = [("empty", Some(""), None), ("unreadable", None, None), ("a picture", None, Some(png()))];
    for (what, text, picture) in cases {
        let board = Board::default();
        {
            let mut held = board.0.borrow_mut();
            held.text = text.map(str::to_string);
            held.image = picture;
        }
        let mut h = writing("Plan for Friday", Some(&board));
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
        settle(&mut h, 2);
        let start = body_start(&h);
        menu_at(&mut h, start);
        choose(&mut h, &paste());
        settle(&mut h, 3);
        // A picture alone goes into the message, in place of the selection (#547).
        let body = if what == "a picture" {
            ("\u{fffc} for Friday".to_string(), Selection { anchor: 1, head: 1 })
        } else {
            ("Plan for Friday".to_string(), Selection { anchor: 0, head: 4 })
        };
        assert_eq!((plain(&h), sel(&h)), body, "the body, {what}");
        // A line of one.
        let line = subject_line(&h);
        click(&mut h, line.center(), PointerButton::Primary);
        keys(&mut h, Modifiers::COMMAND, Key::A);
        menu_at(&mut h, line.center());
        choose(&mut h, &paste());
        settle(&mut h, 3);
        assert_eq!(subject(&h), "Plan for Friday", "the subject line, {what}");
        menu_at(&mut h, line.center());
        assert_eq!(offered(&h, &cut()), Some(true), "what was selected in the subject line still is, {what}");
    }
}

/// File › Signatures with "Standard" open (and a message being written behind it).
fn signature_page(board: &Board) -> Window {
    let mut a = app(Some(board));
    a.execute("mail.new", json!({})).unwrap();
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    let mut h = window(a);
    // The sample's reminders would stand over the page.
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    h
}

fn signature_text(h: &Window) -> String {
    h.state().session.signature_edit.as_ref().unwrap().body.plain()
}

#[test]
fn the_signatures_menu_cuts_copies_and_pastes_text_and_a_picture() {
    let board = Board::default();
    let mut h = signature_page(&board);
    assert_eq!(signature_text(&h), "Avery Quinn\nLumen Studio");
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
    settle(&mut h, 2);
    let start = h.state().hit.rect("signatureBody", 0).expect("the signature").min + vec2(1.0, 8.0);
    menu_at(&mut h, start);
    choose(&mut h, &copy());
    settle(&mut h, 2);
    assert_eq!(board.0.borrow().put.last().map(|(_, text)| text.clone()).as_deref(), Some("Avery"));
    menu_at(&mut h, start);
    choose(&mut h, &cut());
    settle(&mut h, 2);
    assert_eq!(signature_text(&h), " Quinn\nLumen Studio");
    board.0.borrow_mut().text = Some("Ms Avery".into());
    menu_at(&mut h, start);
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!(signature_text(&h), "Ms Avery Quinn\nLumen Studio");
    // A picture on the clipboard, and no text: it comes into the signature as a picture.
    {
        let mut held = board.0.borrow_mut();
        held.text = None;
        held.image = Some(png());
    }
    let pictures = |h: &Window| h.state().session.signature_edit.as_ref().unwrap().signature.pictures().len();
    let before = pictures(&h);
    menu_at(&mut h, start);
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!(pictures(&h), before + 1);
    assert!(signature_text(&h).ends_with("Ms Avery Quinn\nLumen Studio"), "{:?}", signature_text(&h));
}

#[test]
fn copy_and_select_all_work_in_a_text_that_is_only_read() {
    let mut a = app(None);
    let draft = crate::report_problem::Draft::new(&a, &json!({}));
    let text = draft.text.clone();
    a.dialog = Some(Dialog::ReportProblem(Box::new(draft)));
    let mut h = window(a);
    let field = h.query_all_by_value(&text).find(|n| n.accesskit_node().role() == Role::MultilineTextInput).expect("the diagnostics").rect();
    let at = field.left_top() + vec2(12.0, 8.0);
    menu_at(&mut h, at);
    assert_eq!([cut(), copy(), paste(), select_all()].map(|l| offered(&h, &l)), [None, Some(false), None, Some(true)]);
    choose(&mut h, &select_all());
    settle(&mut h, 2);
    menu_at(&mut h, at);
    assert_eq!(offered(&h, &copy()), Some(true));
    choose(&mut h, &copy());
    assert_eq!(copied_within(&mut h, 4).map(|t| t.replace("\r\n", "\n")), Some(text));
}

/// Windows of the system of their own, as the desktop has: egui lays each out as a viewport of
/// its own, with input of its own. What each is to be given next, and what it put out last, by
/// its viewport.
#[derive(Default)]
struct Native {
    to: std::collections::HashMap<egui::ViewportId, Vec<Event>>,
    outs: std::collections::HashMap<egui::ViewportId, egui::FullOutput>,
}

/// Have `h` lay windows out as windows of the system, each with its own input.
fn natively(h: &mut Window) -> Rc<RefCell<Native>> {
    let native = Rc::new(RefCell::new(Native::default()));
    let inside = native.clone();
    h.ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let id = viewport.ids.this;
        let events = inside.borrow_mut().to.remove(&id).unwrap_or_default();
        let mut raw = egui::RawInput {
            viewport_id: id,
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
            time: Some(ctx.input(|i| i.time)),
            events,
            focused: true,
            ..Default::default()
        };
        raw.viewports.insert(id, egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() });
        let mut out = ctx.run_ui(raw, |ui| (viewport.viewport_ui_cb)(ui));
        // (Nothing is drawn: the textures it brought are let go.)
        out.textures_delta.clear();
        inside.borrow_mut().outs.insert(id, out);
    });
    native
}

/// The viewport of the window numbered `id` (as `windows` names it).
fn viewport_of(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("mailcraft-window", id))
}

/// A frame of the main window, in which the window of `viewport` gets `events`.
fn in_window(h: &mut Window, native: &Rc<RefCell<Native>>, viewport: egui::ViewportId, events: Vec<Event>) {
    native.borrow_mut().to.entry(viewport).or_default().extend(events);
    frame(h, Vec::new());
}

/// Where the window of `viewport` last drew the first thing `wanted` picks out, the highest.
fn rect_in(native: &Rc<RefCell<Native>>, viewport: egui::ViewportId, wanted: impl Fn(&egui::accesskit::Node) -> bool) -> Option<Rect> {
    let native = native.borrow();
    let tree = native.outs.get(&viewport)?.platform_output.accesskit_update.as_ref()?;
    let node = tree.nodes.iter().map(|(_, n)| n).filter(|n| wanted(n)).min_by(|a, b| {
        let y = |n: &egui::accesskit::Node| n.bounds().map_or(f64::MAX, |r| r.y0);
        y(a).total_cmp(&y(b))
    })?;
    node.bounds().map(|r| Rect::from_min_max(egui::pos2(r.x0 as f32, r.y0 as f32), egui::pos2(r.x1 as f32, r.y1 as f32)))
}

fn click_in(h: &mut Window, native: &Rc<RefCell<Native>>, viewport: egui::ViewportId, pos: Pos2, which: PointerButton) {
    in_window(h, native, viewport, vec![Event::PointerMoved(pos)]);
    in_window(h, native, viewport, vec![button(pos, which, true)]);
    in_window(h, native, viewport, vec![button(pos, which, false)]);
    settle(h, 2);
}

/// The first text field of the window of its own whose text is `value`.
fn native_field(value: &'static str) -> impl Fn(&egui::accesskit::Node) -> bool {
    move |n| n.role() == Role::TextInput && n.value().unwrap_or_default() == value
}

/// The menu item `label` in the window of its own.
fn native_item(label: String) -> impl Fn(&egui::accesskit::Node) -> bool {
    move |n| n.role() == Role::Button && n.label().is_some_and(|l| l == label)
}

#[test]
fn a_choice_in_a_window_of_its_own_lands_there() {
    let board = Board::holding("Lunch with Bo");
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let native = natively(&mut h);
    let w = h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap()["window"].as_u64().unwrap();
    let w = viewport_of(w);
    settle(&mut h, 4);
    // The main window's search box has the keyboard there, with something typed in it.
    h.state_mut().search_focus = true;
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Text("root".into())]);
    settle(&mut h, 2);
    assert_eq!(h.state().search_text, "root");
    // In the appointment's window: its title, typed and selected.
    let title = rect_in(&native, w, native_field("")).expect("the appointment's title");
    click_in(&mut h, &native, w, title.center(), PointerButton::Primary);
    in_window(&mut h, &native, w, vec![Event::Text("Lunch".into())]);
    settle(&mut h, 1);
    assert!(rect_in(&native, w, native_field("Lunch")).is_some(), "typed into the window's title");
    for pressed in [true, false] {
        in_window(&mut h, &native, w, vec![Event::Key { key: Key::A, physical_key: None, pressed, repeat: false, modifiers: Modifiers::COMMAND }]);
    }
    settle(&mut h, 1);
    click_in(&mut h, &native, w, title.center(), PointerButton::Secondary);
    let item = rect_in(&native, w, native_item(paste())).expect("the menu, in the window");
    click_in(&mut h, &native, w, item.center(), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(rect_in(&native, w, native_field("Lunch with Bo")).is_some(), "the paste went to the window's title");
    assert_eq!(h.state().search_text, "root", "and nowhere else");
}

#[test]
fn a_field_in_a_drop_down_keeps_it_open_through_its_menu() {
    let board = Board::holding("Tokyo");
    let mut a = app(Some(&board));
    a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let shown = crate::zones::short(a.session.zone(), a.session.now());
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    // The time zone's drop-down: its search field has the keyboard.
    let is_zone = move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == Role::ComboBox && (n.value().as_deref() == Some(shown.as_str()) || n.label().as_deref() == Some(shown.as_str()))
    };
    h.query_all(egui_kittest::kittest::By::new().predicate(is_zone.clone())).next().expect("the time zone's drop-down").scroll_to_me();
    settle(&mut h, 3);
    let zone = h.query_all(egui_kittest::kittest::By::new().predicate(is_zone)).next().expect("the time zone's drop-down").rect();
    click(&mut h, zone.center(), PointerButton::Primary);
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Text("Par".into())]);
    settle(&mut h, 2);
    let field = text_field;
    let at = field(&h, "Par").expect("the drop-down's search field").center();
    menu_at(&mut h, at);
    settle(&mut h, 3);
    assert!(field(&h, "Par").is_some(), "the drop-down stays open under the menu");
    choose(&mut h, &select_all());
    settle(&mut h, 3);
    menu_at(&mut h, at);
    choose(&mut h, &paste());
    settle(&mut h, 4);
    assert!(field(&h, "Tokyo").is_some(), "the paste landed, and the drop-down is open");
}

#[test]
fn the_folder_search_of_a_menu_keeps_it_open_through_its_menu() {
    let board = Board::holding("Sent");
    let mut a = app(Some(&board));
    let account = a.session.mailbox.default_account().expect("an account").id;
    let inbox = a.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).expect("an Inbox");
    let first = a.session.mailbox.messages.values().find(|m| m.folder == inbox && !m.draft).expect("a message in the Inbox").id;
    a.execute("mail.select", json!({"id": first})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let move_button = h.state().hit.rect("ribbon", crate::ribbon::hit_id("moveMessages")).expect("Move in the ribbon");
    click(&mut h, move_button.center(), PointerButton::Primary);
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Text("Dra".into())]);
    settle(&mut h, 2);
    let field = text_field;
    let at = field(&h, "Dra").expect("the menu's folder search").center();
    menu_at(&mut h, at);
    settle(&mut h, 3);
    assert!(field(&h, "Dra").is_some(), "the menu stays open under the text's menu");
    choose(&mut h, &select_all());
    settle(&mut h, 3);
    menu_at(&mut h, at);
    choose(&mut h, &paste());
    settle(&mut h, 4);
    assert!(field(&h, "Sent").is_some(), "the paste landed, and the menu is open");
}

#[test]
fn the_calendars_typed_entry_is_not_ended_by_its_menu() {
    let board = Board::holding("Lunch with Bo");
    let mut a = app(Some(&board));
    a.execute("view.calendar", json!({})).unwrap();
    a.execute("view.week", json!({})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let review =
        h.state().session.mailbox.events.values().find(|e| e.title.starts_with("Design review")).map(|e| e.id).expect("the sample's Design review");
    let block = h.state().hit.rect("event", review).expect("Design review is drawn");
    // Well after it, empty: picked out, and typed into.
    let at = egui::pos2(block.center().x, block.max.y + 100.0);
    click(&mut h, at, PointerButton::Primary);
    for c in "Lunch".chars() {
        frame(&mut h, vec![Event::Text(c.to_string())]);
    }
    settle(&mut h, 2);
    let events = h.state().session.mailbox.events.len();
    let entry = text_field;
    let field = entry(&h, "Lunch").expect("the typed entry").center();
    let unchanged = |h: &Window, what: &str| assert_eq!(h.state().session.mailbox.events.len(), events, "{what} made nothing");
    // Select All, Copy.
    menu_at(&mut h, field);
    choose(&mut h, &select_all());
    settle(&mut h, 2);
    unchanged(&h, "Select All");
    menu_at(&mut h, field);
    choose(&mut h, &copy());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("Lunch"));
    unchanged(&h, "Copy");
    // Cut: the entry stays, empty.
    menu_at(&mut h, field);
    choose(&mut h, &cut());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("Lunch"));
    settle(&mut h, 2);
    unchanged(&h, "Cut");
    assert!(h.query_all_by_role(Role::TextInput).any(|n| n.rect().contains(field)), "the entry is still open after Cut");
    // Paste: the entry has it, and Enter makes the appointment.
    menu_at(&mut h, field);
    choose(&mut h, &paste());
    settle(&mut h, 3);
    unchanged(&h, "Paste");
    assert!(entry(&h, "Lunch with Bo").is_some(), "the paste is in the entry");
    keys(&mut h, Modifiers::NONE, Key::Enter);
    settle(&mut h, 2);
    assert!(h.state().session.mailbox.events.values().any(|e| e.title == "Lunch with Bo"));
    assert_eq!(h.state().session.mailbox.events.len(), events + 1);
}

fn press(h: &mut Window, key: Key) {
    for pressed in [true, false] {
        frame(h, vec![Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE }]);
    }
}

#[test]
fn a_picture_alone_in_html_pastes_nothing_and_a_text_beside_it_pastes_the_text() {
    let picture = "<p><img src=\"https://example.org/picture.png\"></p>";
    for menu in [true, false] {
        let how = if menu { "the menu" } else { "Ctrl+V" };
        let board = Board::default();
        board.0.borrow_mut().html = Some(picture.into());
        let mut h = writing("Plan for Friday", Some(&board));
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
        settle(&mut h, 2);
        let start = body_start(&h);
        let paste_now = |h: &mut Window, text: &str| {
            if menu {
                menu_at(h, start);
                choose(h, &paste());
            } else {
                frame(h, vec![Event::Paste(text.to_string())]);
            }
            settle(h, 3);
        };
        paste_now(&mut h, "");
        assert_eq!((plain(&h), sel(&h)), ("Plan for Friday".to_string(), Selection { anchor: 0, head: 4 }), "a picture alone, by {how}");
        board.0.borrow_mut().text = Some("Agenda".into());
        paste_now(&mut h, "Agenda");
        assert_eq!(plain(&h), "Agenda for Friday", "with a text beside it, by {how}");
    }
}

#[test]
fn what_the_menu_pastes_is_what_was_on_the_clipboard_when_it_was_chosen() {
    let board = Board::holding("first");
    let mut h = writing("Plan for Friday", Some(&board));
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
    settle(&mut h, 2);
    let start = body_start(&h);
    menu_at(&mut h, start);
    let at = place_of(&h, &paste()).center();
    frame(&mut h, vec![Event::PointerMoved(at)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, true)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, false)]);
    // Something else comes onto the clipboard right after.
    {
        let mut held = board.0.borrow_mut();
        held.text = Some("second".into());
        held.html = Some("<b>second</b>".into());
    }
    settle(&mut h, 3);
    assert_eq!(plain(&h), "first for Friday");
}

/// The calendar's week, with "Lunch" typed into an empty stretch of it: the window, where the
/// entry is, and how many appointments there are.
fn calendar_entry(board: &Board) -> (Window, Pos2, usize) {
    let mut a = app(Some(board));
    a.execute("view.calendar", json!({})).unwrap();
    a.execute("view.week", json!({})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let review =
        h.state().session.mailbox.events.values().find(|e| e.title.starts_with("Design review")).map(|e| e.id).expect("the sample's Design review");
    let block = h.state().hit.rect("event", review).expect("Design review is drawn");
    click(&mut h, egui::pos2(block.center().x, block.max.y + 100.0), PointerButton::Primary);
    for c in "Lunch".chars() {
        frame(&mut h, vec![Event::Text(c.to_string())]);
    }
    settle(&mut h, 2);
    let events = h.state().session.mailbox.events.len();
    let field = text_field(&h, "Lunch").expect("the typed entry").center();
    (h, field, events)
}

#[test]
fn escape_from_the_calendar_entrys_menu_keeps_the_entry() {
    let board = Board::holding("x");
    let (mut h, field, events) = calendar_entry(&board);
    menu_at(&mut h, field);
    assert_eq!(offered(&h, &paste()), Some(true));
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    assert_eq!(offered(&h, &paste()), None, "the menu closed");
    assert_eq!(h.state().session.mailbox.events.len(), events, "and nothing was made");
    assert!(text_field(&h, "Lunch").is_some(), "the entry is still open, with its title");
    // It still has the keyboard.
    frame(&mut h, vec![Event::Text("!".into())]);
    settle(&mut h, 2);
    assert!(text_field(&h, "Lunch!").is_some());
}

#[test]
fn what_is_chosen_comes_before_what_is_typed_next() {
    let board = Board::holding("Agenda");
    let mut h = writing("", Some(&board));
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    // Cut, and a key typed in the very next frame.
    menu_at(&mut h, line.center());
    let at = place_of(&h, &cut()).center();
    frame(&mut h, vec![Event::PointerMoved(at)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, true)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, false)]);
    frame(&mut h, vec![Event::Text("x".into())]);
    let copied = h.output().platform_output.commands.iter().any(|c| matches!(c, OutputCommand::CopyText(t) if t == "Plan for Friday"));
    settle(&mut h, 2);
    assert_eq!(subject(&h), "x", "the cut, then the key");
    assert!(copied || board.0.borrow().put.is_empty(), "nothing else went to the clipboard");
    // Paste, and a key typed in the very next frame: the paste first.
    menu_at(&mut h, line.center());
    let at = place_of(&h, &paste()).center();
    frame(&mut h, vec![Event::PointerMoved(at)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, true)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, false)]);
    frame(&mut h, vec![Event::Text("!".into())]);
    settle(&mut h, 2);
    assert_eq!(subject(&h), "xAgenda!");
    // Select All, and Tab in the very next frame: the line is selected before the keyboard leaves.
    menu_at(&mut h, line.center());
    let at = place_of(&h, &select_all()).center();
    frame(&mut h, vec![Event::PointerMoved(at)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, true)]);
    frame(&mut h, vec![button(at, PointerButton::Primary, false)]);
    press(&mut h, Key::Tab);
    settle(&mut h, 2);
    let id = h.query_all_by_value("xAgenda!").find(|n| n.accesskit_node().role() == Role::TextInput).map(|n| n.rect());
    assert!(id.is_some(), "the line kept its text");
}

#[test]
fn in_a_window_of_its_own_what_is_chosen_comes_before_what_is_typed_next() {
    let board = Board::holding("Lunch with Bo");
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let native = natively(&mut h);
    let w = h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap()["window"].as_u64().unwrap();
    let w = viewport_of(w);
    settle(&mut h, 4);
    let title = rect_in(&native, w, native_field("")).expect("the appointment's title");
    click_in(&mut h, &native, w, title.center(), PointerButton::Primary);
    click_in(&mut h, &native, w, title.center(), PointerButton::Secondary);
    let item = rect_in(&native, w, native_item(paste())).expect("the menu, in the window");
    in_window(&mut h, &native, w, vec![Event::PointerMoved(item.center())]);
    in_window(&mut h, &native, w, vec![button(item.center(), PointerButton::Primary, true)]);
    in_window(&mut h, &native, w, vec![button(item.center(), PointerButton::Primary, false)]);
    in_window(&mut h, &native, w, vec![Event::Text("!".into())]);
    settle(&mut h, 3);
    assert!(rect_in(&native, w, native_field("Lunch with Bo!")).is_some(), "the paste, then the key");
}

/// A new appointment in a window inside the main one: where its field showing `value` is.
fn appointment_field(h: &Window, value: &str) -> Option<Rect> {
    text_field(h, value)
}

#[test]
fn a_clock_field_is_not_tidied_under_its_menu() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap();
    settle(&mut h, 4);
    // The start time: typed as "9", which the field makes "9:00 AM" once it is left.
    let times: Vec<Rect> = h
        .query_all_by_role(Role::TextInput)
        .filter(|n| n.accesskit_node().value().is_some_and(|v| v.ends_with(" AM") || v.ends_with(" PM")))
        .map(|n| n.rect())
        .collect();
    let start = *times.first().expect("the start time");
    click(&mut h, start.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("9".into())]);
    settle(&mut h, 2);
    let field = appointment_field(&h, "9").expect("the start time, as typed").center();
    keys(&mut h, Modifiers::COMMAND, Key::A);
    menu_at(&mut h, field);
    choose(&mut h, &copy());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("9"), "what was typed, not what the field makes of it");
    assert!(appointment_field(&h, "9").is_some(), "the field is as it was typed");
    menu_at(&mut h, field);
    choose(&mut h, &cut());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("9"));
    settle(&mut h, 2);
    assert!(
        h.query_all_by_role(Role::TextInput).any(|n| n.rect().contains(field) && n.accesskit_node().value().unwrap_or_default().is_empty()),
        "cut whole"
    );
}

#[test]
fn an_attendee_line_is_not_resolved_under_its_menu() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.openWindow", json!({"new": "meeting"})).unwrap();
    settle(&mut h, 4);
    // Who is required: what is typed there becomes a contact's name and address once the line is
    // left ("maya" is Maya Okafor's).
    // (The form's lines, top to bottom: the title, then Required.)
    let mut lines: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.min.y > 100.0).collect();
    lines.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let line = *lines.get(1).expect("the Required line");
    click(&mut h, line.center(), PointerButton::Primary);
    frame(&mut h, vec![Event::Text("maya".into())]);
    settle(&mut h, 2);
    let at = text_field(&h, "maya").expect("the attendees, as typed").center();
    // The whole line, copied.
    keys(&mut h, Modifiers::COMMAND, Key::A);
    menu_at(&mut h, at);
    choose(&mut h, &copy());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("maya"));
    settle(&mut h, 2);
    assert!(text_field(&h, "maya").is_some(), "the line is as it was typed");
    // A part of it, cut: the first word of two, picked out with a double click.
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("maya dev".into())]);
    settle(&mut h, 2);
    let word = line.left_center() + vec2(8.0, 0.0);
    for _ in 0..2 {
        frame(&mut h, vec![button(word, PointerButton::Primary, true)]);
        frame(&mut h, vec![button(word, PointerButton::Primary, false)]);
    }
    settle(&mut h, 1);
    menu_at(&mut h, word);
    choose(&mut h, &cut());
    assert_eq!(copied_within(&mut h, 4).as_deref(), Some("maya"));
    settle(&mut h, 2);
    assert!(text_field(&h, " dev").is_some(), "the rest of what was typed, not a contact's name and address");
}

#[test]
fn escape_closes_a_drop_downs_text_menu_first_and_the_drop_down_next() {
    let board = Board::holding("Tokyo");
    let mut a = app(Some(&board));
    a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let shown = crate::zones::short(a.session.zone(), a.session.now());
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let is_zone = move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == Role::ComboBox && (n.value().as_deref() == Some(shown.as_str()) || n.label().as_deref() == Some(shown.as_str()))
    };
    h.query_all(egui_kittest::kittest::By::new().predicate(is_zone.clone())).next().expect("the time zone's drop-down").scroll_to_me();
    settle(&mut h, 3);
    let zone = h.query_all(egui_kittest::kittest::By::new().predicate(is_zone)).next().expect("the time zone's drop-down").rect();
    click(&mut h, zone.center(), PointerButton::Primary);
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Text("Par".into())]);
    settle(&mut h, 2);
    let at = text_field(&h, "Par").expect("the drop-down's search field").center();
    menu_at(&mut h, at);
    assert_eq!(offered(&h, &paste()), Some(true));
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    assert_eq!(offered(&h, &paste()), None, "the first Escape closes the text's menu");
    assert!(text_field(&h, "Par").is_some(), "and leaves the drop-down open, with what was typed");
    frame(&mut h, vec![Event::Text("i".into())]);
    settle(&mut h, 2);
    assert!(text_field(&h, "Pari").is_some(), "the field has the keyboard back");
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    assert!(text_field(&h, "Pari").is_none(), "the second closes the drop-down");
}

#[test]
fn a_drop_down_opened_low_in_a_small_window_shows_its_fields_menu_where_it_can_be_used() {
    // Unfiltered, in a smaller window, the drop-down low down; a right click at one spot, the
    // pointer still there.
    let board = Board::holding("Tokyo");
    let mut a = app(Some(&board));
    a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let shown = crate::zones::short(a.session.zone(), a.session.now());
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(1000.0, 600.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        a,
    );
    settle(&mut h, 4);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let is_zone = move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == Role::ComboBox && (n.value().as_deref() == Some(shown.as_str()) || n.label().as_deref() == Some(shown.as_str()))
    };
    h.query_all(egui_kittest::kittest::By::new().predicate(is_zone.clone())).next().expect("the time zone's drop-down").scroll_to_me();
    settle(&mut h, 3);
    let zone = h.query_all(egui_kittest::kittest::By::new().predicate(is_zone)).next().expect("the time zone's drop-down").rect();
    let before: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).collect();
    click(&mut h, zone.center(), PointerButton::Primary);
    settle(&mut h, 3);
    let field = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).find(|f| !before.contains(f)).expect("the drop-down's search field");
    let drop_down = h.ctx.layer_id_at(field.center()).expect("the drop-down");
    frame(&mut h, vec![Event::PointerMoved(field.center())]);
    frame(&mut h, vec![button(field.center(), PointerButton::Secondary, true)]);
    frame(&mut h, vec![button(field.center(), PointerButton::Secondary, false)]);
    settle(&mut h, 2);
    let item = place_of(&h, &paste());
    // The menu is drawn in the window, and is what is under the pointer at its item.
    assert!(h.ctx.content_rect().contains_rect(item), "{item:?}");
    let on_top = h.ctx.layer_id_at(item.center()).expect("something is there");
    assert_ne!(on_top, drop_down, "the text's menu is over the drop-down at its item");
    click(&mut h, item.center(), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(text_field(&h, "Tokyo").is_some(), "the paste landed, and the drop-down is open");
}

#[test]
fn a_click_elsewhere_with_the_calendar_entrys_menu_open_still_makes_the_appointment() {
    let board = Board::holding("x");
    let (mut h, field, events) = calendar_entry(&board);
    menu_at(&mut h, field);
    assert_eq!(offered(&h, &paste()), Some(true));
    // Another empty stretch of the calendar, well below.
    click(&mut h, field + vec2(0.0, 150.0), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(
        h.state().session.mailbox.events.values().any(|e| e.title == "Lunch"),
        "what was typed became an appointment, as a click elsewhere makes it"
    );
    assert_eq!(h.state().session.mailbox.events.len(), events + 1);
}

#[test]
fn a_click_elsewhere_with_a_clock_fields_menu_open_tidies_it_as_leaving_it_does() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap();
    settle(&mut h, 4);
    let times: Vec<Rect> = h
        .query_all_by_role(Role::TextInput)
        .filter(|n| n.accesskit_node().value().is_some_and(|v| v.ends_with(" AM") || v.ends_with(" PM")))
        .map(|n| n.rect())
        .collect();
    let start = *times.first().expect("the start time");
    click(&mut h, start.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("9".into())]);
    settle(&mut h, 2);
    let field = appointment_field(&h, "9").expect("the start time, as typed").center();
    menu_at(&mut h, field);
    // A click on the form beside the fields, on nothing that takes the keyboard: it leaves the
    // clock field, which tidies it, as it does with no menu open.
    let mut lines: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.max.y < start.min.y && r.min.y > 100.0).collect();
    lines.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let title = *lines.first().expect("the title");
    click(&mut h, title.left_center() - vec2(40.0, 0.0), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(appointment_field(&h, "9:00 AM").is_some(), "tidied once left");
}

#[test]
fn the_junk_lists_entry_has_the_menu() {
    let board = Board::holding("spam@example.org");
    let mut a = app(Some(&board));
    a.execute("ui.dialog", json!({"name": "junkOptions", "list": "blocked"})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let entry = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).max_by(|a, b| a.min.y.total_cmp(&b.min.y)).expect("the entry");
    menu_at(&mut h, entry.center());
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert!(text_field(&h, "spam@example.org").is_some());
}

#[test]
fn a_number_typed_into_a_box_has_the_menu() {
    // A box of a number (egui's `DragValue`) is a text field while it is typed into: Paste puts
    // a number in it, whole.
    let board = Board::holding("3");
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let w = h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap()["window"].as_u64().unwrap();
    settle(&mut h, 4);
    let recurrence = h.state().hit.rect("windowBar", crate::forms::kit::bar::hit_id(w, "recurrence")).expect("Recurrence in the bar");
    click(&mut h, recurrence.center(), PointerButton::Primary);
    settle(&mut h, 3);
    let every = h.query_all_by_value("1").map(|n| n.rect()).next().expect("how often, a number");
    menu_at(&mut h, every.center());
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert!(h.query_all_by_value("3").next().is_some(), "the number pasted");
}

/// A clock field of a new appointment: its text ends in AM or PM.
fn clock(n: &egui::accesskit::Node) -> bool {
    n.role() == Role::TextInput && n.value().is_some_and(|v| v.ends_with(" AM") || v.ends_with(" PM"))
}

/// Two new appointments, each in a window of its own: their viewports.
fn two_windows(h: &mut Window) -> (egui::ViewportId, egui::ViewportId) {
    let a = h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap()["window"].as_u64().unwrap();
    let b = h.state_mut().execute("ui.openWindow", json!({"new": "meeting"})).unwrap()["window"].as_u64().unwrap();
    settle(h, 4);
    (viewport_of(a), viewport_of(b))
}

#[test]
fn a_menu_pointed_at_in_one_window_of_its_own_leaves_another_window_as_it_is() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let native = natively(&mut h);
    let (a, b) = two_windows(&mut h);
    // In the second window, a clock field typed as "9".
    let start = rect_in(&native, b, clock).expect("the meeting's start time");
    click_in(&mut h, &native, b, start.center(), PointerButton::Primary);
    in_window(&mut h, &native, b, vec![Event::Key { key: Key::A, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
    in_window(&mut h, &native, b, vec![Event::Text("9".into())]);
    settle(&mut h, 2);
    // In the first, a text's menu open, the pointer on it.
    let title = rect_in(&native, a, |n| n.role() == Role::TextInput).expect("the appointment's title");
    click_in(&mut h, &native, a, title.center(), PointerButton::Secondary);
    let item = rect_in(&native, a, native_item(paste())).expect("the menu, in the first window");
    in_window(&mut h, &native, a, vec![Event::PointerMoved(item.center())]);
    settle(&mut h, 2);
    // A click on nothing in the second window leaves its clock field, which tidies it.
    let beside = egui::pos2(12.0, start.center().y);
    click_in(&mut h, &native, b, beside, PointerButton::Primary);
    settle(&mut h, 2);
    assert!(rect_in(&native, b, |n| n.role() == Role::TextInput && n.value() == Some("9:00 AM")).is_some(), "tidied as it is with no menu open");
}

#[test]
fn closing_the_window_whose_menu_was_pointed_at_leaves_nothing_behind() {
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    let normal = h.ctx.options(|o| o.input_options.surrender_focus_on);
    let native = natively(&mut h);
    let (a, _) = two_windows(&mut h);
    let title = rect_in(&native, a, |n| n.role() == Role::TextInput).expect("the appointment's title");
    click_in(&mut h, &native, a, title.center(), PointerButton::Secondary);
    let item = rect_in(&native, a, native_item(paste())).expect("the menu");
    in_window(&mut h, &native, a, vec![Event::PointerMoved(item.center())]);
    settle(&mut h, 2);
    h.state_mut().execute("ui.closeWindow", json!({"all": true})).unwrap();
    settle(&mut h, 4);
    assert_eq!(h.ctx.options(|o| o.input_options.surrender_focus_on), normal, "a click takes the keyboard as it did before");
}

/// Choose `label` in the menu of the body at `at`, egui having no pass to spare in that frame, and
/// click `then` in the very next frame.
fn choose_then_click(h: &mut Window, at: Pos2, label: &str, then: Pos2) {
    menu_at(h, at);
    let item = place_of(h, label).center();
    frame(h, vec![Event::PointerMoved(item)]);
    frame(h, vec![button(item, PointerButton::Primary, true)]);
    frame(h, vec![button(item, PointerButton::Primary, false)]);
    frame(h, vec![Event::PointerMoved(then), button(then, PointerButton::Primary, true)]);
    frame(h, vec![button(then, PointerButton::Primary, false)]);
    settle(h, 3);
}

#[test]
fn a_choice_egui_has_no_pass_for_gives_way_to_a_click_that_comes_first() {
    let board = Board::holding("Agenda");
    let mut h = writing("Plan for Friday", Some(&board));
    h.ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN);
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
    settle(&mut h, 2);
    let start = body_start(&h);
    let line = subject_line(&h).center();
    choose_then_click(&mut h, start, &paste(), line);
    // The paste is dropped whole: not into the body, and not into the line clicked.
    assert_eq!((plain(&h), subject(&h)), ("Plan for Friday".to_string(), "Plan for Friday".to_string()));
}

#[test]
fn a_menu_paste_that_was_dropped_leaves_nothing_for_the_next_ctrl_v() {
    let board = Board::default();
    {
        let mut held = board.0.borrow_mut();
        held.html = Some("<b>old</b>".into());
        held.text = Some("old".into());
    }
    let mut h = writing("Plan for Friday", Some(&board));
    h.ctx.options_mut(|o| o.max_passes = std::num::NonZeroUsize::MIN);
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
    settle(&mut h, 2);
    let start = body_start(&h);
    let line = subject_line(&h).center();
    choose_then_click(&mut h, start, &paste(), line);
    // Something else on the clipboard, and Ctrl+V in the body: that is what is pasted.
    {
        let mut held = board.0.borrow_mut();
        held.html = Some("<i>new</i>".into());
        held.text = Some("new".into());
    }
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
    h.state_mut().body_focus_wanted = true;
    settle(&mut h, 2);
    frame(&mut h, vec![Event::Paste("new".into())]);
    settle(&mut h, 2);
    assert_eq!(plain(&h), "new for Friday");
}

#[test]
fn html_beside_a_text_too_long_to_take_is_pasted_by_the_menu_and_ctrl_v() {
    for menu in [true, false] {
        let board = Board::default();
        {
            let mut held = board.0.borrow_mut();
            held.html = Some("<b>Agenda</b>".into());
            held.text = Some("x".repeat(5 << 20));
        }
        let mut h = writing("Plan for Friday", Some(&board));
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
        settle(&mut h, 2);
        if menu {
            let start = body_start(&h);
            menu_at(&mut h, start);
            choose(&mut h, &paste());
        } else {
            frame(&mut h, vec![Event::Paste("x".repeat(5 << 20))]);
        }
        settle(&mut h, 3);
        assert_eq!(plain(&h), "Agenda for Friday", "by {}", if menu { "the menu" } else { "Ctrl+V" });
    }
}

#[test]
fn a_paste_with_no_digits_in_a_port_changes_nothing() {
    let board = Board::holding("abc");
    let mut a = app(Some(&board));
    let server = json!({"email": "noor@lumen.example", "displayName": "Noor", "username": "noor",
        "incoming": {"host": "imap.lumen.example", "port": 993}, "outgoing": {"host": "smtp.lumen.example", "port": 587, "security": "starttls"}});
    let id = a.execute("account.add", server).unwrap()["id"].as_u64().unwrap();
    a.execute("ui.dialog", json!({"name": "account", "id": id})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let port = text_field(&h, "993").expect("the incoming port").center();
    for menu in [true, false] {
        let how = if menu { "the menu" } else { "Ctrl+V" };
        click(&mut h, port, PointerButton::Primary);
        keys(&mut h, Modifiers::COMMAND, Key::A);
        if menu {
            menu_at(&mut h, port);
            choose(&mut h, &paste());
        } else {
            frame(&mut h, vec![Event::Paste("abc".into())]);
        }
        settle(&mut h, 3);
        assert!(text_field(&h, "993").is_some(), "the port is as it was, by {how}");
    }
    // Digits among other things: the digits.
    board.0.borrow_mut().text = Some("port 465".into());
    menu_at(&mut h, port);
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert!(text_field(&h, "465").is_some(), "the digits, over what was selected");
}

#[test]
fn a_paste_into_a_signature_is_the_same_by_the_menu_and_ctrl_v_whatever_the_texts_size() {
    // A text larger than a message takes, with a picture, with and without HTML beside them: the
    // menu does what Ctrl+V does with the same clipboard.
    let huge = "x".repeat(5 << 20);
    for html in [Some("<b>Agenda</b>"), None] {
        let mut outcomes = Vec::new();
        for menu in [true, false] {
            let board = Board::default();
            {
                let mut held = board.0.borrow_mut();
                held.text = Some(huge.clone());
                held.html = html.map(str::to_string);
                held.image = Some(png());
            }
            let mut h = signature_page(&board);
            // The signature has the keyboard, "Avery" selected.
            let start = h.state().hit.rect("signatureBody", 0).expect("the signature").min + vec2(1.0, 8.0);
            click(&mut h, start, PointerButton::Primary);
            h.state_mut().execute("compose.select", json!({"from": 0, "to": 5})).unwrap();
            settle(&mut h, 2);
            let pictures = h.state().session.signature_edit.as_ref().unwrap().signature.pictures().len();
            if menu {
                menu_at(&mut h, start);
                choose(&mut h, &paste());
            } else {
                frame(&mut h, vec![Event::Paste(huge.clone())]);
            }
            settle(&mut h, 3);
            let added = h.state().session.signature_edit.as_ref().unwrap().signature.pictures().len() - pictures;
            outcomes.push((signature_text(&h), added));
        }
        assert_eq!(outcomes[0], outcomes[1], "with HTML: {}; the menu, then Ctrl+V", html.is_some());
    }
}

/// What the system's backend hands egui for Ctrl+V of `text` (egui-winit): line ends as `\n`, and
/// nothing for no text.
fn as_the_backend_pastes(text: &str) -> Vec<Event> {
    let text = text.replace("\r\n", "\n");
    if text.is_empty() { Vec::new() } else { vec![Event::Paste(text)] }
}

#[test]
fn a_screen_reader_moving_the_keyboard_to_a_menu_item_keeps_it_there() {
    let board = Board::holding("Agenda");
    let mut h = writing("", Some(&board));
    let line = subject_line(&h);
    click(&mut h, line.center(), PointerButton::Primary);
    menu_at(&mut h, line.center());
    h.query_all_by_label(&paste()).next().expect("the menu's Paste").focus();
    settle(&mut h, 3);
    assert!(h.query_all_by_label(&paste()).next().is_some_and(|n| n.is_focused()), "Paste has the keyboard");
}

/// The picture [`png`] is, as egui's backend hands one over for Ctrl+V when the clipboard has no
/// text (egui-winit: `Event::PasteImage`).
fn as_the_backend_pastes_a_picture() -> Event {
    let image = image::load_from_memory(&png()).unwrap().to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Event::PasteImage(std::sync::Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw())))
}

#[test]
fn a_picture_alone_on_the_clipboard_is_pasted_the_same_by_the_menu_and_ctrl_v() {
    // Into a signature: a picture, either way.
    let mut added = Vec::new();
    for menu in [true, false] {
        let board = Board::default();
        board.0.borrow_mut().image = Some(png());
        let mut h = signature_page(&board);
        let start = h.state().hit.rect("signatureBody", 0).expect("the signature").min + vec2(1.0, 8.0);
        click(&mut h, start, PointerButton::Primary);
        settle(&mut h, 2);
        let pictures = |h: &Window| h.state().session.signature_edit.as_ref().unwrap().signature.pictures().len();
        let before = pictures(&h);
        if menu {
            menu_at(&mut h, start);
            choose(&mut h, &paste());
        } else {
            frame(&mut h, vec![as_the_backend_pastes_a_picture()]);
        }
        settle(&mut h, 3);
        added.push(pictures(&h) - before);
    }
    assert_eq!(added, [1, 1], "the menu, then Ctrl+V");
    // Into a message (#547): a picture in place of what was selected, either way.
    for menu in [true, false] {
        let board = Board::default();
        board.0.borrow_mut().image = Some(png());
        let mut h = writing("Plan for Friday", Some(&board));
        h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
        settle(&mut h, 2);
        if menu {
            let start = body_start(&h);
            menu_at(&mut h, start);
            choose(&mut h, &paste());
        } else {
            frame(&mut h, vec![as_the_backend_pastes_a_picture()]);
        }
        settle(&mut h, 3);
        assert_eq!(plain(&h), "\u{fffc} for Friday", "menu: {menu}");
    }
}

/// What egui's backend hands over for Ctrl+V of what `board` holds (egui-winit): the text, line
/// ends as `\n`, when there is some; nothing when the text is empty; the picture, as pixels, only
/// when there is no text at all. (The body reads the HTML itself.)
fn backend_paste(board: &Board) -> Vec<Event> {
    let held = board.0.borrow();
    match &held.text {
        Some(text) => as_the_backend_pastes(text),
        None => held
            .image
            .as_ref()
            .map(|png| {
                let image = image::load_from_memory(png).unwrap().to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                vec![Event::PasteImage(std::sync::Arc::new(egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw())))]
            })
            .unwrap_or_default(),
    }
}

/// Where a paste goes in the table of what is on the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Into {
    Body,
    Signature,
    Line,
    Password,
}

/// Paste what `board` holds into `into`, by the menu or as the backend hands Ctrl+V over: what the
/// text is after, its selection, and how many pictures were added.
fn pasted(into: Into, board: &Board, menu: bool) -> (String, Option<Selection>, usize) {
    // A signature's pictures, or those standing in a message's text (#547).
    let pictures = |h: &Window| match h.state().session.signature_edit.as_ref() {
        Some(e) => e.signature.pictures().len(),
        None => plain(h).matches('\u{fffc}').count(),
    };
    match into {
        Into::Body | Into::Signature => {
            let mut h = if matches!(into, Into::Body) { writing("Plan for Friday", Some(board)) } else { signature_page(board) };
            let kind = if matches!(into, Into::Body) { "composeBody" } else { "signatureBody" };
            let start = h.state().hit.rect(kind, 0).expect("the text").min + vec2(1.0, 8.0);
            click(&mut h, start, PointerButton::Primary);
            h.state_mut().execute("compose.select", json!({"from": 0, "to": 4})).unwrap();
            settle(&mut h, 2);
            let before = pictures(&h);
            if menu {
                menu_at(&mut h, start);
                choose(&mut h, &paste());
            } else {
                frame(&mut h, backend_paste(board));
            }
            settle(&mut h, 3);
            let (text, selection) = if matches!(into, Into::Body) {
                (plain(&h), sel(&h))
            } else {
                (signature_text(&h), h.state().session.signature_edit.as_ref().unwrap().body.selection())
            };
            (text, Some(selection), pictures(&h) - before)
        }
        Into::Line => {
            let mut h = writing("", Some(board));
            let line = subject_line(&h);
            click(&mut h, line.center(), PointerButton::Primary);
            keys(&mut h, Modifiers::COMMAND, Key::A);
            if menu {
                menu_at(&mut h, line.center());
                choose(&mut h, &paste());
            } else {
                frame(&mut h, backend_paste(board));
            }
            settle(&mut h, 3);
            (subject(&h), None, 0)
        }
        Into::Password => {
            let mut a = app(Some(board));
            a.auto_sync = false;
            let account = a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"]
                .as_u64()
                .unwrap();
            a.dialog = Some(Dialog::Password { account, password: String::new(), shown: false });
            let mut h = window(a);
            settle(&mut h, 2);
            if menu {
                let field = h.get_by_role(Role::PasswordInput).rect();
                menu_at(&mut h, field.left_center() + vec2(12.0, 0.0));
                choose(&mut h, &paste());
            } else {
                frame(&mut h, backend_paste(board));
            }
            settle(&mut h, 3);
            let password = match &h.state().dialog {
                Some(Dialog::Password { password, .. }) => password.clone(),
                _ => String::from("(no dialog)"),
            };
            (password, None, 0)
        }
    }
}

#[test]
fn the_menu_pastes_what_ctrl_v_pastes_whatever_is_on_the_clipboard() {
    for into in [Into::Body, Into::Signature, Into::Line, Into::Password] {
        // What each is before the paste, and what follows what is selected in it.
        let (before, rest) = match into {
            Into::Body => ("Plan for Friday", " for Friday"),
            Into::Signature => ("Avery Quinn\nLumen Studio", "y Quinn\nLumen Studio"),
            Into::Line => ("Plan for Friday", ""),
            Into::Password => ("", ""),
        };
        for text in [None, Some(""), Some("Agenda\r\nnext")] {
            for html in [None, Some("<b>Bold</b>")] {
                for picture in [false, true] {
                    let board = || {
                        let board = Board::default();
                        {
                            let mut held = board.0.borrow_mut();
                            held.text = text.map(str::to_string);
                            held.html = html.map(str::to_string);
                            held.image = picture.then(png);
                        }
                        board
                    };
                    let case = format!("{into:?}, text {text:?}, HTML {html:?}, picture {picture}");
                    let (by_menu, by_keys) = (pasted(into, &board(), true), pasted(into, &board(), false));
                    assert_eq!(by_menu, by_keys, "{case}: the menu, then Ctrl+V");
                    // Text when there is some, its line ends as `\n` (a line of one makes it a
                    // space), the HTML beside it in a body; nothing for an empty text; a picture
                    // only when there is no text, into a signature or a message (#547).
                    let rich = matches!(into, Into::Body | Into::Signature);
                    let added = usize::from(text.is_none() && picture && rich);
                    let want = match text {
                        // (The picture stands where the selection was.)
                        None if added == 1 => format!("\u{fffc}{rest}"),
                        Some(t) if !t.is_empty() && rich && html.is_some() => format!("Bold{rest}"),
                        Some(t) if !t.is_empty() && rich => format!("Agenda\nnext{rest}"),
                        Some(t) if !t.is_empty() => "Agenda next".to_string(),
                        _ => before.to_string(),
                    };
                    assert_eq!((by_menu.0.as_str(), by_menu.2), (want.as_str(), added), "{case}");
                    if rich && want == before {
                        assert_eq!(by_menu.1, Some(Selection { anchor: 0, head: 4 }), "{case}: the selection kept");
                    }
                }
            }
        }
    }
}

#[test]
fn a_picture_too_large_is_pasted_by_neither() {
    use image::ImageEncoder as _;
    let side = crate::MAX_PICTURE_SIDE as u32 + 1;
    let mut big = Vec::new();
    image::codecs::png::PngEncoder::new(&mut big).write_image(&vec![90; side as usize * 3], side, 1, image::ExtendedColorType::Rgb8).unwrap();
    for menu in [true, false] {
        let board = Board::default();
        board.0.borrow_mut().image = Some(big.clone());
        let (text, selection, added) = pasted(Into::Signature, &board, menu);
        assert_eq!((text.as_str(), selection, added), ("Avery Quinn\nLumen Studio", Some(Selection { anchor: 0, head: 4 }), 0), "menu: {menu}");
    }
}

#[test]
fn a_picture_too_big_for_a_signature_as_it_is_is_made_smaller_by_the_menu_and_ctrl_v() {
    // Within the side a paste takes, but more than a signature keeps as it is (noise doesn't
    // compress): made smaller and added, as the signature's Paste Picture does.
    let big = crate::tests::big_png(400);
    assert!(big.len() > mailcraft_model::signature::MAX_PICTURE_BYTES);
    for menu in [true, false] {
        let board = Board::default();
        board.0.borrow_mut().image = Some(big.clone());
        let (_, _, added) = pasted(Into::Signature, &board, menu);
        assert_eq!(added, 1, "menu: {menu}");
    }
    // And a paste that brings no text, where the editor reads the clipboard's picture itself.
    let board = Board::default();
    board.0.borrow_mut().image = Some(big);
    let mut h = signature_page(&board);
    let start = h.state().hit.rect("signatureBody", 0).expect("the signature").min + vec2(1.0, 8.0);
    click(&mut h, start, PointerButton::Primary);
    settle(&mut h, 2);
    let pictures = |h: &Window| h.state().session.signature_edit.as_ref().unwrap().signature.pictures().len();
    let before = pictures(&h);
    frame(&mut h, vec![Event::Paste(String::new())]);
    settle(&mut h, 3);
    assert_eq!(pictures(&h) - before, 1, "a paste with no text");
}

#[test]
fn a_dialog_leaves_the_keyboard_to_its_fields_menu() {
    // A dialog that keeps the keyboard on its one field takes it back from anything else, but
    // not from an item of that field's own menu.
    let board = Board::holding("2026-10-30");
    for name in ["Search Commands", "Go to Date", "Password"] {
        let mut a = app(Some(&board));
        a.auto_sync = false;
        let (dialog, role) = match name {
            "Search Commands" => (Dialog::Commands { query: String::new(), selected: 0 }, Role::TextInput),
            "Go to Date" => (Dialog::GoToDate { text: String::new(), error: String::new() }, Role::TextInput),
            _ => {
                let r = a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}}));
                (Dialog::Password { account: r.unwrap()["id"].as_u64().unwrap(), password: String::new(), shown: false }, Role::PasswordInput)
            }
        };
        a.dialog = Some(dialog);
        let mut h = window(a);
        settle(&mut h, 2);
        let field = h.query_all_by_role(role).find(|n| n.is_focused()).unwrap_or_else(|| panic!("{name}: its field has the keyboard")).rect();
        menu_at(&mut h, field.left_center() + vec2(12.0, 0.0));
        focus_item(&mut h, &paste());
        assert!(h.query_all_by_label(&paste()).next().is_some_and(|n| n.is_focused()), "{name}: Paste has the keyboard");
        // Once the menu is shut, the field has the keyboard back.
        press(&mut h, Key::Escape);
        settle(&mut h, 3);
        assert!(h.state().dialog.is_some(), "{name}: Escape shut the menu alone");
        assert!(h.query_all_by_role(role).any(|n| n.is_focused()), "{name}: the field has the keyboard back");
    }
}

/// Focus a menu item of the open menu, as a screen reader does (AccessKit), without choosing it.
fn focus_item(h: &mut Window, label: &str) {
    h.query_all_by_label(label).next().unwrap_or_else(|| panic!("no {label}")).focus();
    settle(h, 3);
}

#[test]
fn the_keyboard_on_the_calendar_entrys_menu_does_not_make_the_appointment() {
    // An item given the keyboard, then chosen.
    let board = Board::holding("Lunch with Bo");
    let (mut h, field, events) = calendar_entry(&board);
    menu_at(&mut h, field);
    focus_item(&mut h, &paste());
    assert_eq!(h.state().session.mailbox.events.len(), events, "the keyboard on an item made nothing");
    h.query_all_by_label(&paste()).next().expect("Paste").click_accesskit();
    settle(&mut h, 3);
    assert_eq!(h.state().session.mailbox.events.len(), events, "choosing it made nothing");
    assert!(text_field(&h, "LunchLunch with Bo").is_some() || text_field(&h, "Lunch with Bo").is_some(), "the paste is in the entry");
    // An item given the keyboard, then the menu shut with Escape: the entry stays, and has the
    // keyboard back.
    let (mut h, field, events) = calendar_entry(&board);
    menu_at(&mut h, field);
    focus_item(&mut h, &copy());
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    assert_eq!(h.state().session.mailbox.events.len(), events, "Escape from the menu made nothing");
    assert!(text_field(&h, "Lunch").is_some(), "the entry is open");
    // A click elsewhere then makes it, as it does with no menu used.
    click(&mut h, field + vec2(0.0, 150.0), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(h.state().session.mailbox.events.values().any(|e| e.title == "Lunch"));
}

#[test]
fn the_keyboard_on_a_clock_fields_menu_does_not_tidy_it() {
    let start_clock = |h: &mut Window| {
        h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
        h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap();
        settle(h, 4);
        let times: Vec<Rect> = h
            .query_all_by_role(Role::TextInput)
            .filter(|n| n.accesskit_node().value().is_some_and(|v| v.ends_with(" AM") || v.ends_with(" PM")))
            .map(|n| n.rect())
            .collect();
        let start = *times.first().expect("the start time");
        click(h, start.center(), PointerButton::Primary);
        keys(h, Modifiers::COMMAND, Key::A);
        frame(h, vec![Event::Text("9".into())]);
        settle(h, 2);
        (start, appointment_field(h, "9").expect("the start time, as typed").center())
    };
    let board = Board::default();
    let mut h = window(app(Some(&board)));
    let (start, field) = start_clock(&mut h);
    // An item given the keyboard, then chosen (Select All).
    menu_at(&mut h, field);
    focus_item(&mut h, &select_all());
    assert!(appointment_field(&h, "9").is_some(), "not tidied with the keyboard on an item");
    h.query_all_by_label(&select_all()).next().expect("Select All").click_accesskit();
    settle(&mut h, 3);
    assert!(appointment_field(&h, "9").is_some(), "not tidied by choosing it");
    // An item given the keyboard, then the menu shut with Escape.
    menu_at(&mut h, field);
    focus_item(&mut h, &copy());
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    assert!(appointment_field(&h, "9").is_some(), "not tidied by Escape from the menu");
    // A click on nothing beside it then tidies it, as leaving it does.
    let mut lines: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.max.y < start.min.y && r.min.y > 100.0).collect();
    lines.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let title = *lines.first().expect("the title");
    click(&mut h, title.left_center() - vec2(40.0, 0.0), PointerButton::Primary);
    settle(&mut h, 3);
    assert!(appointment_field(&h, "9:00 AM").is_some(), "tidied once left");
}

/// What happens to a text field the keyboard was moved off onto an item of its menu.
#[derive(Clone, Copy, Debug)]
enum Then {
    /// The menu shut with Escape.
    Escape,
    /// The item chosen.
    Chosen,
    /// Nothing: the next thing is a click elsewhere.
    Clicked,
}

/// A field that does something once it is left, `what` was typed into it: the window, where it
/// is, a place beside it that takes no keyboard, and whether what it does once left is done.
type Left = (Window, Pos2, Pos2, fn(&Window) -> bool);

fn left_clock() -> Left {
    let mut h = window(app(Some(&Board::default())));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap();
    settle(&mut h, 4);
    let start = h
        .query_all_by_role(Role::TextInput)
        .find(|n| n.accesskit_node().value().is_some_and(|v| v.ends_with(" AM") || v.ends_with(" PM")))
        .expect("the start time")
        .rect();
    click(&mut h, start.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("9".into())]);
    settle(&mut h, 2);
    let field = appointment_field(&h, "9").expect("the start time, as typed").center();
    let mut lines: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.max.y < start.min.y && r.min.y > 100.0).collect();
    lines.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let beside = lines.first().expect("the title").left_center() - vec2(40.0, 0.0);
    (h, field, beside, |h| appointment_field(h, "9:00 AM").is_some())
}

fn left_attendee() -> Left {
    let mut h = window(app(Some(&Board::default())));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.openWindow", json!({"new": "meeting"})).unwrap();
    settle(&mut h, 4);
    let mut lines: Vec<Rect> = h.query_all_by_role(Role::TextInput).map(|n| n.rect()).filter(|r| r.min.y > 100.0).collect();
    lines.sort_by(|a, b| a.min.y.total_cmp(&b.min.y));
    let line = *lines.get(1).expect("the Required line");
    click(&mut h, line.center(), PointerButton::Primary);
    frame(&mut h, vec![Event::Text("maya".into())]);
    settle(&mut h, 2);
    let field = text_field(&h, "maya").expect("the attendees, as typed").center();
    let beside = line.left_center() - vec2(40.0, 0.0);
    (h, field, beside, |h| h.query_all_by_role(Role::TextInput).any(|n| n.accesskit_node().value().is_some_and(|v| v.contains("maya@lumen.example"))))
}

fn left_calendar_entry() -> Left {
    let (h, field, _) = calendar_entry(&Board::default());
    (h, field, field + vec2(0.0, 150.0), |h| h.state().session.mailbox.events.values().any(|e| e.title == "Lunch"))
}

/// A signature's new name: it is given only with Enter, so what leaving it does is close it, the
/// signature keeping its name.
fn left_signature_name() -> Left {
    let mut h = signature_page(&Board::default());
    let rename = h.query_all_by_label("Rename").next().expect("Rename").rect();
    click(&mut h, rename.center(), PointerButton::Primary);
    settle(&mut h, 2);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("Formal".into())]);
    settle(&mut h, 2);
    let field = text_field(&h, "Formal").expect("the new name, as typed");
    let kept = |h: &Window| text_field(h, "Formal").is_none() && h.state().session.mailbox.signature("Standard").is_some();
    (h, field.center(), field.right_center() + vec2(10.0, 0.0), kept)
}

fn left_category_name() -> Left {
    let mut a = app(Some(&Board::default()));
    a.execute("ui.dialog", json!({"name": "categories"})).unwrap();
    let mut h = window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    settle(&mut h, 2);
    let line = text_field(&h, "Blue category").expect("Blue category");
    click(&mut h, line.center(), PointerButton::Primary);
    keys(&mut h, Modifiers::COMMAND, Key::A);
    frame(&mut h, vec![Event::Text("Teal".into())]);
    settle(&mut h, 2);
    let field = text_field(&h, "Teal").expect("the new name, as typed").center();
    // The gap under its row.
    (h, field, line.center_bottom() + vec2(0.0, 4.0), |h| h.state().session.mailbox.categories.iter().any(|c| c.name == "Teal"))
}

#[test]
fn a_signatures_new_name_is_given_with_enter_after_its_menu_too() {
    // (The field asked for the keyboard every pass, so it was never left, nor renamed.)
    let renamed = |h: &Window| h.state().session.mailbox.signature("Formal").is_some() && text_field(h, "Formal").is_none();
    let (mut h, ..) = left_signature_name();
    press(&mut h, Key::Enter);
    settle(&mut h, 3);
    assert!(renamed(&h), "renamed with Enter");
    let (mut h, field, ..) = left_signature_name();
    menu_at(&mut h, field);
    focus_item(&mut h, &copy());
    press(&mut h, Key::Escape);
    settle(&mut h, 3);
    press(&mut h, Key::Enter);
    settle(&mut h, 3);
    assert!(renamed(&h), "renamed with Enter after Escape from the menu");
}

#[test]
fn a_field_left_after_the_keyboard_was_on_its_menu_is_left() {
    // Left by a click elsewhere, with no menu: once, though egui says so for two passes.
    let fired = Rc::new(std::cell::Cell::new(0));
    let count = fired.clone();
    let mut plain = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(400.0, 200.0)).build_ui(move |ui| {
        let mut text = String::from("9");
        let resp = crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut text).id(egui::Id::unique("left once"))));
        if crate::text_menu::left(&resp) {
            count.set(count.get() + 1);
        }
    });
    plain.run_steps(2);
    let field = plain.get_by_role(Role::TextInput).rect().center();
    for at in [field, field + vec2(0.0, 120.0)] {
        plain.event(Event::PointerMoved(at));
        plain.event(button(at, PointerButton::Primary, true));
        plain.event(button(at, PointerButton::Primary, false));
        plain.run_steps(4);
    }
    assert_eq!(fired.get(), 1, "left once");
    let mut wrong = Vec::new();
    for (name, field) in [
        ("a clock", left_clock as fn() -> Left),
        ("an attendee line", left_attendee),
        ("the calendar's entry", left_calendar_entry),
        ("a signature's name", left_signature_name),
        ("a category's name", left_category_name),
    ] {
        for then in [Then::Escape, Then::Chosen, Then::Clicked] {
            let (mut h, at, beside, done) = field();
            menu_at(&mut h, at);
            focus_item(&mut h, &select_all());
            match then {
                Then::Escape => press(&mut h, Key::Escape),
                Then::Chosen => h.query_all_by_label(&select_all()).next().expect("Select All").click_accesskit(),
                // The keyboard stays on the item a while.
                Then::Clicked => settle(&mut h, 4),
            }
            settle(&mut h, 3);
            if done(&h) {
                wrong.push(format!("{name}, {then:?}: done before it was left"));
            }
            click(&mut h, beside, PointerButton::Primary);
            settle(&mut h, 3);
            if !done(&h) {
                wrong.push(format!("{name}, {then:?}: not done once left"));
            }
        }
    }
    assert_eq!(wrong, Vec::<String>::new());
}

#[test]
fn advanced_searchs_fields_have_the_menu() {
    let board = Board::holding("Maya");
    let mut h = window(app(Some(&board)));
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.state_mut().execute("ui.advancedSearch", json!({"open": true})).unwrap();
    settle(&mut h, 3);
    // Its first field, From: the highest text field in the panel.
    let panel = h.state().hit.rect("advancedSearch", 0).expect("the panel");
    let from = h
        .query_all_by_role(Role::TextInput)
        .map(|n| n.rect())
        .filter(|r| panel.contains_rect(*r))
        .min_by(|a, b| a.min.y.total_cmp(&b.min.y))
        .expect("From");
    click(&mut h, from.center(), PointerButton::Primary);
    menu_at(&mut h, from.center());
    choose(&mut h, &paste());
    settle(&mut h, 3);
    assert_eq!(h.state().advanced_search.as_ref().map(|p| p.fields.from.as_str()), Some("Maya"));
}
