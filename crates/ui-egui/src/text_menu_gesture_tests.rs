// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The text menu changes nothing of what the left button does in a field (#557): for each kind
//! of field the window has, the same clicks, drags and Shift+clicks select the same in the field
//! with the menu as in the same field drawn by egui alone, with the menu's work at the start and
//! end of each pass done as the window does it (`text_menu::deliver`, `text_menu::finish`).

use egui::accesskit::Role;
use egui::{Event, Id, Modifiers, PointerButton, Pos2, TextEdit, pos2, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const TEXT: &str = "Plan for Friday";
const NUMBER: u32 = 12_345_678;

/// The kinds of field: one line; the search box's (its own layouter); a password; a line of
/// recipients (egui's completion popup); a box of a number while it is typed into; a field in a
/// drop-down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Line,
    Search,
    Password,
    Recipients,
    Number,
    DropDown,
}

const KINDS: [Kind; 6] = [Kind::Line, Kind::Search, Kind::Password, Kind::Recipients, Kind::Number, Kind::DropDown];

struct Rig {
    kind: Kind,
    menu: bool,
    text: String,
    number: u32,
    session: Session,
}

fn id() -> Id {
    Id::unique("gesture-field")
}

fn draw(ui: &mut egui::Ui, rig: &mut Rig) {
    let scope = crate::text_menu::deliver(ui.ctx());
    ui.add_space(40.0);
    let menu = rig.menu;
    let wrap = |resp: egui::Response| if menu { crate::text_menu::field(resp) } else { resp };
    match rig.kind {
        Kind::Line => {
            wrap(ui.add(TextEdit::singleline(&mut rig.text).id(id()).desired_width(300.0)));
        }
        Kind::Search => {
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                let font = egui::FontSelection::Default.resolve(ui.style());
                ui.fonts_mut(|f| f.layout_job(egui::text::LayoutJob::simple_singleline(buf.as_str().to_owned(), font, ui.visuals().text_color())))
            };
            wrap(ui.add(TextEdit::singleline(&mut rig.text).id(id()).desired_width(300.0).layouter(&mut layouter)));
        }
        Kind::Password => {
            let resp = ui.add(TextEdit::singleline(&mut rig.text).id(id()).desired_width(300.0).password(true));
            if menu {
                crate::text_menu::secret(resp);
            }
        }
        Kind::Recipients => {
            let edit = TextEdit::singleline(&mut rig.text).id(id()).desired_width(300.0);
            if menu {
                crate::compose::recipient_line(ui, &rig.session, id(), edit, false);
            } else {
                egui::CompletionPopup::new(id()).show(ui, edit, |_| Vec::new());
            }
        }
        Kind::Number => {
            wrap(ui.add(egui::DragValue::new(&mut rig.number)));
        }
        Kind::DropDown => {
            egui::ComboBox::from_id_salt("gesture-drop-down")
                .width(300.0)
                .selected_text("Time zone")
                // (As the time-zone picker's: a click in it leaves it open.)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    wrap(ui.add(TextEdit::singleline(&mut rig.text).id(id()).desired_width(300.0)));
                });
        }
    }
    crate::text_menu::finish(ui.ctx(), scope);
}

type Window = Harness<'static, Rig>;

fn window(kind: Kind, menu: bool) -> Window {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let rig = Rig { kind, menu, text: TEXT.to_string(), number: NUMBER, session };
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(600.0, 500.0)).build_ui_state(draw, rig);
    h.run_steps(3);
    h
}

fn button(pos: Pos2, pressed: bool, modifiers: Modifiers) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers }
}

fn step(h: &mut Window, events: Vec<Event>) {
    h.input_mut().events.extend(events);
    h.step();
}

/// Long enough for the next click not to count with the last.
fn pause(h: &mut Window) {
    h.run_steps(45);
}

/// The field's text and where its character `n` is on the screen, `part` of the way across it.
fn place(h: &Window, n: usize, part: f32) -> Pos2 {
    let field = h
        .query_all_by_role(Role::TextInput)
        .chain(h.query_all_by_role(Role::PasswordInput))
        .chain(h.query_all_by_role(Role::SpinButton))
        .find(|f| f.rect().min.y > 30.0)
        .unwrap_or_else(|| {
            panic!(
                "the field, as a text field: {:?}",
                h.query_all(egui_kittest::kittest::By::new().predicate(|_| true))
                    .map(|n| (n.accesskit_node().role(), n.accesskit_node().value()))
                    .collect::<Vec<_>>()
            )
        });
    let run = field.children().find(|c| c.accesskit_node().role() == Role::TextRun).expect("the field's text");
    let rect = run.rect();
    let data = run.accesskit_node().data().clone();
    let (at, widths) = (data.character_positions().unwrap(), data.character_widths().unwrap());
    pos2(rect.min.x + at[n] + widths[n] * part, rect.center().y)
}

/// What is selected, as characters `[from, to)`; `None` when nothing has the keyboard.
fn selected(h: &Window) -> Option<(usize, usize)> {
    let focused = h.ctx.memory(|m| m.focused())?;
    let range = TextEdit::load_state(&h.ctx, focused)?.cursor.char_range()?;
    let [a, b] = range.sorted_cursors();
    Some((a.index.0, b.index.0))
}

/// `count` clicks at `from`, the last held and dragged to `to` (one pointer event a frame).
fn clicks(h: &mut Window, from: Pos2, count: usize, to: Pos2, modifiers: Modifiers) {
    step(h, vec![Event::ModifiersChanged(modifiers), Event::PointerMoved(from)]);
    for _ in 1..count {
        step(h, vec![button(from, true, modifiers)]);
        step(h, vec![button(from, false, modifiers)]);
    }
    step(h, vec![button(from, true, modifiers)]);
    for n in 1..=5 {
        step(h, vec![Event::PointerMoved(from + (to - from) * (n as f32 / 5.0))]);
    }
    step(h, vec![button(to, false, modifiers), Event::ModifiersChanged(Modifiers::NONE)]);
    h.run_steps(2);
}

/// Every gesture in turn on one field, and what each left selected. `a`, `b` and `back` are
/// characters of the field's text: in a word, in a later word, and in the first.
fn gestures(kind: Kind, menu: bool) -> Vec<(&'static str, Option<(usize, usize)>)> {
    let mut h = window(kind, menu);
    // A box of a number is a text field once it has the keyboard; a drop-down shows its field
    // once it is open.
    match kind {
        Kind::Number => {
            let spin = h.query_all_by_role(Role::SpinButton).next().expect("the number").rect();
            step(&mut h, vec![Event::PointerMoved(spin.center())]);
            step(&mut h, vec![button(spin.center(), true, Modifiers::NONE)]);
            step(&mut h, vec![button(spin.center(), false, Modifiers::NONE)]);
            h.run_steps(3);
        }
        Kind::DropDown => {
            let combo = h.query_all_by_role(Role::ComboBox).next().expect("the drop-down").rect();
            step(&mut h, vec![Event::PointerMoved(combo.center())]);
            step(&mut h, vec![button(combo.center(), true, Modifiers::NONE)]);
            step(&mut h, vec![button(combo.center(), false, Modifiers::NONE)]);
            h.run_steps(3);
        }
        _ => {}
    }
    let (a, b, back) = if kind == Kind::Number { (2, 6, 0) } else { (6, 11, 1) };
    let mut out = Vec::new();
    let mut record = |h: &mut Window, name: &'static str| {
        out.push((name, selected(h)));
        pause(h);
    };
    pause(&mut h);
    let (at_a, at_b, at_back) = (place(&h, a, 0.5), place(&h, b, 0.5), place(&h, back, 0.5));
    clicks(&mut h, at_a, 1, at_a, Modifiers::NONE);
    record(&mut h, "click");
    clicks(&mut h, at_a, 2, at_a, Modifiers::NONE);
    record(&mut h, "double click");
    clicks(&mut h, at_a, 3, at_a, Modifiers::NONE);
    record(&mut h, "triple click");
    clicks(&mut h, at_a, 1, at_b, Modifiers::NONE);
    record(&mut h, "drag");
    clicks(&mut h, at_a, 2, at_b, Modifiers::NONE);
    record(&mut h, "double click and drag forward");
    clicks(&mut h, at_b, 2, at_back, Modifiers::NONE);
    record(&mut h, "double click and drag back");
    clicks(&mut h, at_a, 3, at_b, Modifiers::NONE);
    record(&mut h, "triple click and drag");
    clicks(&mut h, at_back, 1, at_back, Modifiers::NONE);
    pause(&mut h);
    clicks(&mut h, at_b, 1, at_b, Modifiers::SHIFT);
    record(&mut h, "Shift+click");
    out
}

#[test]
fn the_left_button_selects_the_same_with_the_menu_as_without_it() {
    for kind in KINDS {
        let (plain, with_menu) = (gestures(kind, false), gestures(kind, true));
        assert_eq!(with_menu, plain, "{kind:?}: with the menu, then egui alone");
        // (egui alone does select: the gestures reached the field.)
        assert!(plain.iter().any(|(_, s)| s.is_some_and(|(a, b)| b > a)), "{kind:?}: {plain:?}");
    }
}
