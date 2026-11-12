// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Paragraphs aligned left, centred or right in the body of a message and of a signature (#580):
//! Ctrl+L, Ctrl+E and Ctrl+R, the three buttons of the Message tab, and the caret, a click and a
//! picture standing where the line is drawn. Driven in a window without a window.

use egui::{Event, Key, Modifiers, Pos2, Rect, Shape, vec2};
use mailcraft_model::rich::{Align, OBJECT, Selection};
use serde_json::json;

use super::editor::{self, Which};
use super::picture_tests::{PictureBoard, writing_with};
use super::tests::{Window, app, key, key_with, plain, sel, signature_page, type_in, window_at};

/// The screens the geometry is checked on: one pixel to the point, and 125%.
const SCALES: [f32; 2] = [1.0, 1.25];

/// A new message, with the keyboard in its body and nothing in it, drawn in MailCraft's own fonts
/// only, so that the geometry checked is the same on every system.
fn writing() -> Window {
    writing_at(1.0)
}

/// [`writing`] on a screen of `ppp` pixels to the point.
fn writing_at(ppp: f32) -> Window {
    let mut h = super::tests::writing_at(ppp);
    crate::theme::install_fonts_with(&h.ctx, &crate::theme::SystemFont::none());
    h.run_steps(2);
    h
}

/// How each paragraph of the message is aligned.
fn aligns(h: &Window) -> Vec<Align> {
    h.state().session.compose.as_ref().unwrap().body.text().paragraphs().iter().map(|p| p.align).collect()
}

/// The body on the screen.
fn body(h: &Window) -> Rect {
    h.state().hit.rect("composeBody", 0).unwrap()
}

/// How wide a line of the message's own text is.
fn width_of(h: &Window, text: &str) -> f32 {
    h.ctx.fonts_mut(|f| f.layout_no_wrap(text.to_string(), crate::theme::regular(editor::BASE_SIZE), egui::Color32::WHITE)).size().x
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

/// A picture `w` by `h` pixels, one colour.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(w, h, image::Rgba([0, 90, 200, 255])).write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0
}

/// Whether `x` points is a whole number of the screen's pixels.
fn on_pixels(x: f32, ppp: f32) -> bool {
    (x * ppp - (x * ppp).round()).abs() < 0.01
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

/// The rectangles painted in the selection's colour over `area`, those that show (wider than
/// nothing).
fn selection_painted(h: &Window, area: Rect) -> Vec<Rect> {
    let fill = h.ctx.global_style().visuals.selection.bg_fill;
    let rect = |s: Shape| match s {
        Shape::Rect(r) if r.fill == fill && area.expand(8.0).contains_rect(r.rect) && r.rect.width() > 0.1 => Some(r.rect),
        _ => None,
    };
    painted(h).into_iter().filter_map(rect).collect()
}

/// Where a picture (a texture that isn't the font's) was painted over `area`.
fn picture_painted(h: &Window, area: Rect) -> Option<Rect> {
    let mesh = |s: Shape| match s {
        Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(m.calc_bounds()),
        _ => None,
    };
    painted(h).into_iter().filter_map(mesh).find(|r| area.contains_rect(*r))
}

/// The frame painted round the selected picture over `area`, and the corner that sizes it.
fn frame_and_corner(h: &Window, area: Rect) -> (Option<Rect>, Option<Rect>) {
    let accent = crate::theme::Tokens::get(&h.ctx).accent;
    let (mut frame, mut corner) = (None, None);
    for shape in painted(h) {
        match shape {
            Shape::Rect(r) if !area.expand(8.0).contains_rect(r.rect) => {}
            Shape::Rect(r) if r.stroke.color == accent && r.fill == egui::Color32::TRANSPARENT => frame = Some(r.rect),
            Shape::Rect(r) if r.fill == accent && near(r.rect.width(), 8.0) && near(r.rect.height(), 8.0) => corner = Some(r.rect),
            _ => {}
        }
    }
    (frame, corner)
}

#[test]
fn ctrl_e_centres_the_carets_paragraph_and_ctrl_l_and_ctrl_r_align_it_left_and_right() {
    let mut h = writing();
    type_in(&mut h, "Dear Maya");
    key_with(&mut h, Modifiers::NONE, Key::Enter);
    type_in(&mut h, "A line of mine");
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    assert_eq!(aligns(&h), [Align::Left, Align::Center]);
    key_with(&mut h, Modifiers::COMMAND, Key::R);
    assert_eq!(aligns(&h), [Align::Left, Align::Right]);
    key_with(&mut h, Modifiers::COMMAND, Key::L);
    assert_eq!(aligns(&h), [Align::Left, Align::Left]);
    // The keyboard stays in the text, and typing goes on where it was.
    assert!(editor::has_focus(&h.ctx));
    type_in(&mut h, "!");
    assert_eq!(plain(&h), "Dear Maya\nA line of mine!");
}

#[test]
fn in_the_browser_ctrl_l_e_and_r_are_left_to_the_browser_and_align_nothing() {
    // The browser acts on them itself (its address bar, its search, loading the page again), and
    // the paragraph is not changed behind it; the buttons align it there.
    for k in [Key::L, Key::E, Key::R] {
        assert_eq!(editor::align_key(k, true), None, "{k:?}");
    }
    assert_eq!([Key::L, Key::E, Key::R].map(|k| editor::align_key(k, false)), [Some("left"), Some("center"), Some("right")]);
}

#[test]
fn in_the_browser_the_alignment_buttons_show_no_key_and_the_other_formatting_keys_stay() {
    // Ctrl+L, Ctrl+E and Ctrl+R are the browser's there, so a tooltip or a menu offering them
    // would promise what the keys do not do. Asked as the ribbon's tooltips and menus ask it.
    let a = app();
    let text = |key: &str, id: &str, browser: bool| crate::widgets::control_shortcut_in(&a, key, id, browser);
    let alignments = ["format.alignLeft", "format.alignCenter", "format.alignRight"];
    assert_eq!(alignments.map(|k| text(k, "format.align", true)), ["", "", ""]);
    assert_eq!(alignments.map(|k| text(k, "format.align", false)), ["Mod+L", "Mod+E", "Mod+R"].map(crate::widgets::key_text));
    let others = ["format.bold", "format.italic", "format.underline", "compose.link", "compose.undo", "compose.redo"];
    let keys = ["Mod+B", "Mod+I", "Mod+U", "Mod+K", "Mod+Z", "Mod+Y"].map(crate::widgets::key_text);
    for browser in [true, false] {
        assert_eq!(others.map(|c| text(c, c, browser)), keys, "in the browser: {browser}");
    }
}

#[test]
fn the_caret_of_a_centred_line_stands_at_the_bodys_middle_and_a_click_there_lands_in_it() {
    for ppp in SCALES {
        let mut h = writing_at(ppp);
        let line = "centre me";
        type_in(&mut h, line);
        key_with(&mut h, Modifiers::COMMAND, Key::E);
        h.run_steps(1);
        let (room, w) = (body(&h).width(), width_of(&h, line));
        let x = |h: &Window, pos: usize| editor::caret_rect(&h.ctx, h.state(), Which::Message, pos).min.x;
        assert!(near(x(&h, 0), (room - w) / 2.0), "at {ppp}: the line starts at {} in {room}, {w} wide", x(&h, 0));
        assert!(near(x(&h, 9), (room + w) / 2.0), "at {ppp}: the line ends at {}", x(&h, 9));
        assert!(on_pixels(x(&h, 0), ppp), "at {ppp}: the line was moved by {}, not by whole pixels", x(&h, 0));
        // A click at the middle of the body puts the caret between the letters nearest it.
        let caret = editor::caret_rect(&h.ctx, h.state(), Which::Message, 0);
        let middle = body(&h).min + vec2(room / 2.0, caret.center().y);
        let nearest = (0..=9).min_by(|a, b| (x(&h, *a) - room / 2.0).abs().total_cmp(&(x(&h, *b) - room / 2.0).abs())).unwrap();
        assert_eq!(editor::hit_at(&h.ctx, h.state(), Which::Message, middle - body(&h).min), nearest);
        click(&mut h, middle);
        assert_eq!(sel(&h), Selection::caret(nearest));
        assert!((3..=6).contains(&nearest), "{nearest}");
    }
}

#[test]
fn an_empty_centred_or_right_aligned_paragraph_puts_the_caret_in_the_middle_or_at_the_right_edge() {
    for ppp in SCALES {
        for (key, part, align) in [(Key::E, 0.5, Align::Center), (Key::R, 1.0, Align::Right)] {
            let mut h = writing_at(ppp);
            key_with(&mut h, Modifiers::COMMAND, key);
            h.run_steps(1);
            assert_eq!(aligns(&h), [align]);
            let (caret, room) = (editor::caret_rect(&h.ctx, h.state(), Which::Message, 0), body(&h).width());
            assert!(near(caret.min.x, room * part), "at {ppp}: {caret:?} in a body {room} wide");
            assert!(on_pixels(caret.min.x, ppp), "at {ppp}: {caret:?}");
        }
    }
}

#[test]
fn a_picture_alone_in_a_centred_paragraph_is_drawn_centred_and_selected_where_it_is_drawn() {
    let board = PictureBoard::default();
    let mut h = writing_with(&board);
    *board.0.borrow_mut() = (Some(png(80, 40)), None);
    h.event(Event::Paste(String::new()));
    h.run_steps(2);
    assert_eq!(plain(&h), OBJECT.to_string());
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    // Decoded, and painted.
    h.run_steps(4);
    let area = body(&h);
    let drawn = picture_painted(&h, area).expect("the picture is painted in the body");
    assert!(near(drawn.width(), 80.0), "{drawn:?}");
    assert!(near(drawn.center().x, area.center().x), "the picture is painted at {drawn:?} in a body at {area:?}");
    // Where it was before is only the empty start of its line; where it is, a click selects it.
    click(&mut h, area.min + vec2(4.0, drawn.center().y - area.min.y));
    assert_eq!(sel(&h), Selection::caret(0));
    click(&mut h, drawn.center());
    assert_eq!(sel(&h), Selection { anchor: 0, head: 1 });
}

/// A new message on a screen of `ppp` pixels to the point, with the keyboard in its body and
/// `board` for its clipboard, drawn in MailCraft's own fonts only: they have no glyph for the
/// character a picture stands as, so it is drawn as the box that stands for a missing one, as wide
/// as on a system whose font has one (Noto Sans, Arial).
fn writing_with_at(board: &PictureBoard, ppp: f32) -> Window {
    let mut a = app();
    a.services.clipboard = Some(Box::new(board.clone()));
    a.execute("mail.new", json!({"to": "maya@lumen.example"})).unwrap();
    a.execute("compose.set", json!({"body": ""})).unwrap();
    a.execute("compose.select", json!({"from": 0})).unwrap();
    a.want_body_focus();
    let mut h = window_at(a, ppp);
    crate::theme::install_fonts_with(&h.ctx, &crate::theme::SystemFont::none());
    h.run_steps(2);
    let object = h.ctx.fonts_mut(|f| f.layout_no_wrap(OBJECT.to_string(), crate::theme::regular(1.0), egui::Color32::WHITE)).size().x;
    assert!(object > 0.5, "the character a picture stands as is {object} wide");
    h
}

#[test]
fn a_picture_wider_than_the_body_fills_its_line_however_the_line_is_aligned() {
    for ppp in SCALES {
        for align in [Key::L, Key::E, Key::R] {
            let board = PictureBoard::default();
            let mut h = writing_with_at(&board, ppp);
            *board.0.borrow_mut() = (Some(png(1600, 100)), None);
            h.event(Event::Paste(String::new()));
            h.run_steps(2);
            assert_eq!(plain(&h), OBJECT.to_string());
            key_with(&mut h, Modifiers::COMMAND, align);
            h.run_steps(4);
            // Made as wide as the body, it stands where the body does.
            let area = body(&h);
            let drawn = picture_painted(&h, area.expand(1.0)).unwrap_or_else(|| panic!("at {ppp}, {align:?}: the picture is painted in {area:?}"));
            assert!(near(drawn.min.x, area.min.x) && near(drawn.max.x, area.max.x), "at {ppp}, {align:?}: painted at {drawn:?} in {area:?}");
            // The caret stands before it at the left of the body and after it at the right.
            let caret = |pos: usize| editor::caret_rect(&h.ctx, h.state(), Which::Message, pos);
            assert!(near(caret(0).min.x, 0.0), "at {ppp}, {align:?}: before it, {:?}", caret(0));
            assert!(near(caret(1).min.x, area.width()), "at {ppp}, {align:?}: after it, {:?} in {}", caret(1), area.width());
            // A click on it selects it.
            click(&mut h, drawn.center());
            assert_eq!(sel(&h), Selection { anchor: 0, head: 1 }, "at {ppp}, {align:?}");
        }
    }
}

#[test]
fn a_centred_paragraph_that_wraps_centres_each_row() {
    for ppp in SCALES {
        let mut h = writing_at(ppp);
        let text = vec!["lorem ipsum dolor"; 40].join(" ");
        type_in(&mut h, &text);
        key_with(&mut h, Modifiers::COMMAND, Key::E);
        h.run_steps(1);
        let room = body(&h).width();
        let rows = editor::row_ends(&h.ctx, h.state(), Which::Message);
        assert!(rows.len() >= 3, "{rows:?}");
        for (left, right) in rows {
            assert!(near(left, room - right), "at {ppp}: a row from {left} to {right} in {room}");
            assert!(on_pixels(left, ppp), "at {ppp}: a row moved by {left}, not by whole pixels");
        }
    }
}

/// A message of one paragraph, `short` and twenty spaces, then a word too long to follow them on
/// their row, aligned by Mod and `key`; underlined, a link's underline as much as any, when
/// `underlined`.
fn a_row_wrapped_after_spaces(ppp: f32, key: Key, underlined: bool) -> Window {
    let mut h = writing_at(ppp);
    if underlined {
        key_with(&mut h, Modifiers::COMMAND, Key::U);
    }
    type_in(&mut h, &format!("short{}", " ".repeat(20)));
    while editor::row_ends(&h.ctx, h.state(), Which::Message).len() < 2 {
        type_in(&mut h, "lorem");
    }
    key_with(&mut h, Modifiers::COMMAND, key);
    h.run_steps(1);
    h
}

#[test]
fn the_spaces_a_centred_or_right_row_wraps_after_keep_the_caret_and_the_selection_inside_the_body() {
    // The row's last word reaches the edge, and the spaces after it, kept on the row by epaint,
    // stand at the edge too: the caret goes through them there, as in a word processor, and none
    // of them is beyond the body, where it would be cut off and out of reach of a click.
    for ppp in SCALES {
        for (key, part) in [(Key::E, 0.5), (Key::R, 1.0)] {
            let mut h = a_row_wrapped_after_spaces(ppp, key, false);
            let area = body(&h);
            let room = area.width();
            let (left, right) = editor::row_ends(&h.ctx, h.state(), Which::Message)[0];
            assert!(near(left, room - right) || part == 1.0, "at {ppp}, {key:?}: the first row from {left} to {right} in {room}");
            assert!(near(right, room) || part == 0.5, "at {ppp}, {key:?}: the first row ends at {right} in {room}");
            for pos in 0..=plain(&h).chars().count() {
                let caret = editor::caret_rect(&h.ctx, h.state(), Which::Message, pos);
                assert!(caret.max.x <= room + 0.01, "at {ppp}, {key:?}: the caret at {pos} is at {caret:?}, beyond a body {room} wide");
            }
            key_with(&mut h, Modifiers::COMMAND, Key::A);
            h.run_steps(1);
            // Looked for to the right of the body as well, where a band beyond it would be.
            let shown = selection_painted(&h, Rect::from_min_max(area.min, area.max + vec2(room, 0.0)));
            assert!(!shown.is_empty(), "at {ppp}, {key:?}: no selection painted");
            for shown in shown {
                assert!(shown.max.x <= area.max.x + 0.01, "at {ppp}, {key:?}: the selection is painted at {shown:?} in {area:?}");
            }
        }
    }
}

/// What the rows of text painted over `area` drew besides their letters (their underlines), each
/// row's as one rectangle.
fn lines_painted(h: &Window, area: Rect) -> Vec<Rect> {
    let text = |s: Shape| match s {
        Shape::Text(t) => Some(t),
        _ => None,
    };
    let mut out = Vec::new();
    for t in painted(h).into_iter().filter_map(text) {
        for row in &t.galley.rows {
            let v = &row.visuals;
            let mut r = Rect::NOTHING;
            for (_, x) in v.mesh.vertices.iter().enumerate().filter(|(i, _)| !v.glyph_vertex_range.contains(i)) {
                r.extend_with(x.pos);
            }
            let r = r.translate(t.pos.to_vec2() + row.pos.to_vec2());
            if r.is_positive() && area.contains(r.left_top()) {
                out.push(r);
            }
        }
    }
    out
}

#[test]
fn the_underline_of_the_spaces_a_centred_or_right_row_wraps_after_stops_at_the_bodys_edge() {
    // Drawn under every space of the row, it goes as far as they stand: to the edge, and not into
    // the margin beyond it, but for the pixel epaint softens the end of a line with.
    for ppp in SCALES {
        for key in [Key::E, Key::R] {
            let h = a_row_wrapped_after_spaces(ppp, key, true);
            let area = body(&h);
            let lines = lines_painted(&h, area);
            assert!(!lines.is_empty(), "at {ppp}, {key:?}: no underline painted");
            for line in lines {
                assert!(line.max.x <= area.max.x + 1.0 / ppp, "at {ppp}, {key:?}: an underline is painted at {line:?} in {area:?}");
            }
        }
    }
}

#[test]
fn two_of_the_spaces_a_right_row_wraps_after_show_as_selected_at_the_bodys_edge() {
    // They stand at the edge, none of them wide; selected, a sliver inside the edge says so.
    for ppp in SCALES {
        let mut h = a_row_wrapped_after_spaces(ppp, Key::R, false);
        let area = body(&h);
        let caret = |h: &Window, pos: usize| editor::caret_rect(&h.ctx, h.state(), Which::Message, pos).min.x;
        assert_eq!(caret(&h, 23), caret(&h, 25), "at {ppp}: the last two spaces of the row take room");
        h.state_mut().execute("compose.select", json!({"from": 23, "to": 25})).unwrap();
        h.run_steps(1);
        let shown = selection_painted(&h, Rect::from_min_max(area.min, area.max + vec2(area.width(), 0.0)));
        assert!(!shown.is_empty(), "at {ppp}: no selection painted");
        for shown in shown {
            assert!(area.contains_rect(shown), "at {ppp}: the selection is painted at {shown:?} in {area:?}");
        }
    }
}

/// A centred paragraph that is one link, wrapped so that its last row is one word, and the middle
/// of that row (from the body's top left corner).
fn a_wrapped_centred_link(url: &str) -> (Window, Pos2) {
    let mut h = writing();
    type_in(&mut h, "lorem");
    while editor::row_ends(&h.ctx, h.state(), Which::Message).len() < 2 {
        type_in(&mut h, " lorem");
    }
    key_with(&mut h, Modifiers::COMMAND, Key::A);
    h.state_mut().run("compose.link", json!({"url": url})).unwrap();
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    h.run_steps(1);
    let (left, right) = editor::row_ends(&h.ctx, h.state(), Which::Message)[1];
    let y = editor::caret_rect(&h.ctx, h.state(), Which::Message, plain(&h).chars().count()).center().y;
    (h, Pos2::new((left + right) / 2.0, y))
}

#[test]
fn a_wrapped_centred_link_is_under_the_pointer_on_its_letters_and_not_in_the_margins_of_its_rows() {
    let url = "https://lumen.example/";
    let (h, last) = a_wrapped_centred_link(url);
    let link = |x: f32| editor::link_under(&h.ctx, h.state(), Which::Message, vec2(x, last.y));
    assert_eq!(link(last.x).as_deref(), Some(url), "on the word of the last row");
    let (left, right) = editor::row_ends(&h.ctx, h.state(), Which::Message)[1];
    assert!(left > 40.0, "the last row is short: {left} to {right}");
    assert_eq!(link(left - 20.0), None, "in the margin before the last row");
    assert_eq!(link(4.0), None, "at the start of the body, beside the last row");
    assert_eq!(link(right + 20.0), None, "in the margin after the last row");
}

#[test]
fn a_selected_line_break_shows_inside_the_body_after_a_right_aligned_line_and_an_empty_one() {
    for ppp in SCALES {
        let mut h = writing_at(ppp);
        type_in(&mut h, "one");
        key_with(&mut h, Modifiers::NONE, Key::Enter);
        key_with(&mut h, Modifiers::NONE, Key::Enter);
        type_in(&mut h, "three");
        key_with(&mut h, Modifiers::COMMAND, Key::A);
        key_with(&mut h, Modifiers::COMMAND, Key::R);
        assert_eq!(aligns(&h), [Align::Right; 3]);
        let area = body(&h);
        // The break after "one", from the end of its line; and the empty line's own.
        key_with(&mut h, Modifiers::COMMAND, Key::Home);
        key(&mut h, Key::End);
        key_with(&mut h, Modifiers::SHIFT, Key::ArrowRight);
        assert_eq!(sel(&h), Selection { anchor: 3, head: 4 });
        for line in ["after one", "of the empty line"] {
            h.run_steps(1);
            let shown = selection_painted(&h, area);
            assert_eq!(shown.len(), 1, "at {ppp}, {line}: {shown:?}");
            let sliver = shown[0];
            assert!(near(sliver.width(), 5.0), "at {ppp}, {line}: {sliver:?}");
            assert!(sliver.max.x <= area.max.x + 0.01 && sliver.min.x >= area.min.x, "at {ppp}, {line}: {sliver:?} in {area:?}");
            h.state_mut().execute("compose.select", json!({"from": 4, "to": 5})).unwrap();
        }
    }
}

#[test]
fn a_selected_line_and_its_break_are_painted_once_each_however_the_line_is_aligned() {
    for ppp in SCALES {
        for (align, how) in [(Key::L, Align::Left), (Key::E, Align::Center), (Key::R, Align::Right)] {
            let mut h = writing_at(ppp);
            type_in(&mut h, "one");
            key_with(&mut h, Modifiers::NONE, Key::Enter);
            type_in(&mut h, "two");
            key_with(&mut h, Modifiers::COMMAND, Key::A);
            key_with(&mut h, Modifiers::COMMAND, align);
            assert_eq!(aligns(&h), [how; 2]);
            h.run_steps(1);
            let area = body(&h);
            let shown = selection_painted(&h, area);
            assert!(!shown.is_empty(), "at {ppp}, {how:?}: no selection painted");
            // The selection's colour shows through, so a part painted twice shows darker.
            for (i, a) in shown.iter().enumerate() {
                for b in &shown[i + 1..] {
                    let both = a.intersect(*b);
                    assert!(both.width() <= 0.01 || both.height() <= 0.01, "at {ppp}, {how:?}: {a:?} and {b:?} overlap");
                }
            }
        }
    }
}

/// File › Signatures with a picture 80 points wide on the first line of "Standard", that line
/// aligned by Ctrl and `align`, and the picture selected.
fn a_signature_picture_aligned(align: Key) -> Window {
    let mut h = signature_page();
    let r = h.state().hit.rect("signatureBody", 0).unwrap();
    click(&mut h, r.min + vec2(1.0, 8.0));
    let data = crate::files::base64_lite::encode(&png(80, 40));
    crate::signatures::add_picture(h.state_mut(), json!({"data": data, "file": "logo.png"}));
    h.run_steps(2);
    let e = h.state().session.signature_edit.as_ref().unwrap();
    assert_eq!(e.body.plain(), format!("{OBJECT}Avery Quinn\nLumen Studio"));
    // On a line of its own.
    h.state_mut().execute("compose.select", json!({"from": 1})).unwrap();
    key(&mut h, Key::Enter);
    h.state_mut().execute("compose.select", json!({"from": 0})).unwrap();
    key_with(&mut h, Modifiers::COMMAND, align);
    h.state_mut().execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    h.run_steps(4);
    h
}

#[test]
fn a_signatures_picture_is_sized_by_a_corner_that_stays_under_the_pointer_wherever_its_line_stands() {
    // The key, how far the corner is dragged to the right, the corner, and how wide the picture
    // comes out.
    let cases = [(Key::L, 20.0, "right", 100), (Key::E, 20.0, "right", 120), (Key::R, -20.0, "left", 100)];
    for (align, dx, side, wide) in cases {
        let mut h = a_signature_picture_aligned(align);
        let area = h.state().hit.rect("signatureBody", 0).unwrap();
        let (frame, corner) = frame_and_corner(&h, area);
        let (frame, corner) = (frame.expect("the picture's frame"), corner.expect("its corner"));
        let edge = if side == "left" { frame.left_bottom() } else { frame.right_bottom() };
        assert!(
            near(corner.center().x, edge.x) && near(corner.center().y, edge.y),
            "{align:?}: the corner is at the {side}: {corner:?} on {frame:?}"
        );
        let from = corner.center();
        let button = |pos, pressed| Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        h.event(Event::PointerMoved(from));
        h.step();
        h.event(button(from, true));
        h.step();
        let mut at = from;
        for n in 1..=4 {
            at = from + vec2(dx * n as f32 / 4.0, 0.0);
            h.event(Event::PointerMoved(at));
            h.step();
            let (shown, corner) = frame_and_corner(&h, area);
            let corner = corner.expect("the corner while it is dragged");
            assert!(near(corner.center().x, at.x), "{align:?}: the corner is at {corner:?}, the pointer at {at:?}, the frame {shown:?}");
        }
        h.event(button(at, false));
        h.run_steps(3);
        let e = h.state().session.signature_edit.as_ref().unwrap();
        assert_eq!(e.signature.pictures()[0].width, Some(wide), "{align:?}");
        let (frame, corner) = frame_and_corner(&h, area);
        let corner = corner.expect("the corner once it was let go");
        assert!(near(corner.center().x, at.x), "{align:?}: the picture came out with its corner at {corner:?} on {frame:?}, the pointer at {at:?}");
    }
}

#[test]
fn a_centred_line_is_drawn_left_once_the_message_is_plain_text_and_centred_again_as_html() {
    let mut h = writing();
    let line = "centred";
    type_in(&mut h, line);
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    h.run_steps(1);
    let start = |h: &Window| editor::caret_rect(&h.ctx, h.state(), Which::Message, 0).min.x;
    let centred = (body(&h).width() - width_of(&h, line)) / 2.0;
    assert!(near(start(&h), centred), "{}", start(&h));
    h.state_mut().execute("compose.formatPlain", json!({})).unwrap();
    h.run_steps(2);
    // Sent left, and drawn as it is sent; the buttons show no alignment.
    assert!(near(start(&h), 0.0), "{}", start(&h));
    assert_eq!(crate::ribbon::look_now(h.state()).align, None);
    // Ctrl+E does nothing to a plain message.
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    assert_eq!(aligns(&h), [Align::Center]);
    h.state_mut().execute("compose.formatHtml", json!({})).unwrap();
    h.run_steps(2);
    assert!(near(start(&h), centred), "{}", start(&h));
    assert_eq!(crate::ribbon::look_now(h.state()).align, Some(Align::Center));
}

#[test]
fn the_three_align_buttons_show_which_way_the_selection_goes_and_each_says_its_own_key() {
    let mut h = writing();
    type_in(&mut h, "one");
    key_with(&mut h, Modifiers::NONE, Key::Enter);
    type_in(&mut h, "two");
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    h.state_mut().execute("ui.tab", json!({"tab": "Message"})).unwrap();
    h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(3);
    let buttons = |h: &Window| {
        let tab = crate::ribbon::describe(h.state());
        ["format.alignLeft", "format.alignCenter", "format.alignRight"].map(|key| tab.items().find(|i| i.key == key).cloned().unwrap())
    };
    let checked = |h: &Window| buttons(h).map(|b| b.checked);
    assert_eq!(checked(&h), [false, true, false]);
    // Each runs Align Text; a button that is on puts the paragraph back to left, as Outlook's do.
    let sent = |h: &Window| buttons(h).map(|b| b.command.unwrap().1["value"].as_str().unwrap().to_string());
    assert_eq!(sent(&h), ["left", "left", "right"]);
    // Each says its own key, which is the body's.
    let keys = buttons(&h).map(|b| crate::widgets::control_shortcut_text(h.state(), b.key, b.command.unwrap().0));
    assert_eq!(keys, ["Mod+L", "Mod+E", "Mod+R"].map(crate::widgets::key_text));
    // The selection over a left and a centred paragraph: neither way.
    key_with(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(checked(&h), [false, false, false]);
    assert_eq!(sent(&h), ["left", "center", "right"]);
    // The paragraph the caret is in: left.
    key_with(&mut h, Modifiers::CTRL, Key::Home);
    assert_eq!(checked(&h), [true, false, false]);
    // A click on Center centres it, and a second puts it back.
    let center = h.state().hit.rect("ribbon", crate::ribbon::hit_id("format.alignCenter")).unwrap();
    click(&mut h, center.center());
    h.run_steps(2);
    assert_eq!(aligns(&h), [Align::Center, Align::Center]);
    assert!(editor::has_focus(&h.ctx), "the keyboard went back to the text");
    click(&mut h, center.center());
    h.run_steps(2);
    assert_eq!(aligns(&h), [Align::Left, Align::Center]);
}

#[test]
fn ctrl_e_centres_a_line_of_the_open_signature() {
    let mut h = signature_page();
    let message = plain(&h);
    let r = h.state().hit.rect("signatureBody", 0).unwrap();
    click(&mut h, r.min + vec2(1.0, 8.0));
    assert!(editor::has_focus(&h.ctx));
    key_with(&mut h, Modifiers::COMMAND, Key::E);
    let e = h.state().session.signature_edit.as_ref().unwrap();
    let aligned: Vec<Align> = e.body.text().paragraphs().iter().map(|p| p.align).collect();
    assert_eq!(aligned, [Align::Center, Align::Left]);
    assert!(e.dirty);
    assert_eq!(plain(&h), message, "the message behind is not touched");
    assert!(h.state().session.compose.as_ref().unwrap().body.text().paragraphs().iter().all(|p| p.align == Align::Left));
    // The line is drawn centred where it is edited.
    h.run_steps(1);
    let caret = editor::caret_rect(&h.ctx, h.state(), Which::Signature, 0);
    let room = h.state().hit.rect("signatureBody", 0).unwrap().width();
    assert!(caret.min.x > room / 4.0, "{caret:?} in {room}");
    // Ctrl+R and Ctrl+L there too.
    key_with(&mut h, Modifiers::COMMAND, Key::R);
    assert_eq!(h.state().session.signature_edit.as_ref().unwrap().body.text().paragraphs()[0].align, Align::Right);
    key_with(&mut h, Modifiers::COMMAND, Key::L);
    assert_eq!(h.state().session.signature_edit.as_ref().unwrap().body.text().paragraphs()[0].align, Align::Left);
}

#[test]
fn the_format_text_tabs_paragraph_group_aligns_too() {
    let mut h = writing();
    type_in(&mut h, "one");
    key_with(&mut h, Modifiers::COMMAND, Key::R);
    h.state_mut().execute("ui.tab", json!({"tab": "Format Text"})).unwrap();
    h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(3);
    // As Outlook's Format Text tab has them: in Paragraph, after Numbering, each with its key.
    let tab = crate::ribbon::describe(h.state());
    let buttons = ["format.alignLeft", "format.alignCenter", "format.alignRight"].map(|key| tab.items().find(|i| i.key == key).cloned().unwrap());
    assert_eq!(buttons.clone().map(|b| b.checked), [false, false, true]);
    let keys = buttons.map(|b| crate::widgets::control_shortcut_text(h.state(), b.key, b.command.unwrap().0));
    assert_eq!(keys, ["Mod+L", "Mod+E", "Mod+R"].map(crate::widgets::key_text));
    let order: Vec<&str> = tab.items().map(|i| i.key).collect();
    let at = |key: &str| order.iter().position(|k| *k == key).unwrap();
    assert!(at("format.numbering") < at("format.alignLeft") && at("format.alignRight") < at("format.outdent"), "{order:?}");
    // A click on Center centres the line.
    let center = h.state().hit.rect("ribbon", crate::ribbon::hit_id("format.alignCenter")).unwrap();
    click(&mut h, center.center());
    h.run_steps(2);
    assert_eq!(aligns(&h), [Align::Center]);
}
