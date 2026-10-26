// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A password field with an eye at its right end (#438): Add Account's password page, Account
//! Settings, and the Sign In dialog of Send/Receive.
//!
//! The password is dots until the owner clicks the eye, and dots again at the next click. Shown,
//! it is only drawn: the field stays egui's password field, so it can't be copied or cut, the
//! system's input method is told it is a password, and what screen readers and AccessKit are
//! given (the field's value and the text runs of its lines) is dots. The galley the field lays
//! out has its own text and characters masked too; only the shapes of its glyphs are the
//! password's.
//!
//! The eye is not a command. A click or a key the control channel injected doesn't move it, so
//! no agent, script or MCP client can show the password; and while the window is being captured
//! (`ui.screenshot`, the headless shots of `examples/ui_shot`), the field is drawn as dots
//! whatever the eye says. The one who shows it is the person at the screen.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::PASSWORD_REPLACEMENT_CHAR as MASK;
use egui::text::{ByteIndex, LayoutJob};
use egui::{FontSelection, Galley, Margin, Rect, Response, Role, Sense, TextBuffer, TextEdit, TextStyle, Ui, WidgetInfo, pos2, vec2};

use crate::icons;
use crate::theme::Tokens;

/// What the window is doing that the eye has to know about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Watch {
    /// The window is being captured: the password is drawn as dots, shown or not.
    pub captured: bool,
    /// This frame's input came from the control channel: the eye doesn't answer it.
    pub driven: bool,
}

/// The room the eye takes at the field's right end, in points.
const EYE: f32 = 24.0;
/// The eye's drawing, in points.
const EYE_ICON: f32 = 16.0;

/// The accessible name of the eye, for the password `shown` or not.
pub(crate) fn eye_name(shown: bool) -> &'static str {
    if shown { tl!("Hide password") } else { tl!("Show password") }
}

/// A one-line password field filling the row, with the eye at its right end. `shown` is the
/// eye's state, kept by the caller, which hides it again when its page or dialog goes. `style`
/// sets the rest of the field (its id, hint, limit); `margin` is the room round its text, to
/// which the eye's room is added on the right.
pub(crate) fn field(
    ui: &mut Ui,
    text: &mut String,
    shown: &mut bool,
    watch: Watch,
    margin: Margin,
    style: impl FnOnce(TextEdit<'_>) -> TextEdit<'_>,
) -> Response {
    let visible = *shown && !watch.captured;
    let mut layout = |ui: &Ui, buf: &dyn TextBuffer, _wrap: f32| drawn(ui, buf.as_str());
    let room = Margin { right: margin.right.saturating_add(EYE as i8), ..margin };
    // (The font is named, so that the dots egui draws and the text drawn here are alike.)
    let mut edit = style(TextEdit::singleline(text)).password(true).font(TextStyle::Body).margin(room).desired_width(f32::INFINITY);
    if visible {
        edit = edit.layouter(&mut layout);
    }
    let resp = ui.add(edit);
    eye(ui, &resp, shown, watch);
    resp
}

/// The eye over the right end of the field `resp`.
fn eye(ui: &mut Ui, resp: &Response, shown: &mut bool, watch: Watch) {
    let field = resp.rect;
    let r = Rect::from_min_max(pos2((field.right() - EYE).max(field.left()), field.top()), field.max).shrink2(vec2(1.0, 3.0));
    let button = ui.interact(r, resp.id.with("eye"), Sense::click());
    if button.clicked() && !watch.driven {
        *shown = !*shown;
        // Typing goes on where it was.
        if button.clicked_by(egui::PointerButton::Primary) {
            resp.request_focus();
        }
    }
    let name = eye_name(*shown);
    button.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
    let t = Tokens::get(ui.ctx());
    if button.hovered() {
        ui.painter().rect_filled(r, 3.0, if button.is_pointer_button_down_on() { t.pressed } else { t.hover });
    }
    let icon = if *shown { "hidePassword" } else { "showPassword" };
    icons::paint(ui.painter(), Rect::from_center_size(r.center(), vec2(EYE_ICON, EYE_ICON)), icon, t.icon, t.accent);
    button.on_hover_text(name);
}

/// The password laid out as itself for the screen, as egui lays out its dots, with everything
/// but the shapes of its glyphs masked.
fn drawn(ui: &Ui, text: &str) -> Arc<Galley> {
    let font = FontSelection::from(TextStyle::Body).resolve(ui.style());
    let color = ui.visuals().override_text_color.unwrap_or_else(|| ui.visuals().widgets.inactive.text_color());
    let line_height = ui.fonts_mut(|f| f.row_height(&font)) + ui.spacing().extra_text_line_spacing;
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.keep_trailing_whitespace = true;
    for section in &mut job.sections {
        section.format.line_height = Some(line_height);
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    Arc::new(masked(&galley))
}

/// `galley` with its text and its glyphs' characters as dots, one for each character.
pub(crate) fn masked(galley: &Galley) -> Galley {
    let mut out = galley.clone();
    let job = Arc::make_mut(&mut out.job);
    let text = std::mem::take(&mut job.text);
    // A byte of the text to the same place in the dots.
    let at = |byte: ByteIndex| ByteIndex(text.get(..byte.0).map_or(0, |s| s.chars().count()).saturating_mul(MASK.len_utf8()));
    for section in &mut job.sections {
        let Range { start, end } = section.byte_range.clone();
        section.byte_range = at(start)..at(end);
    }
    job.text = std::iter::repeat_n(MASK, text.chars().count()).collect();
    for placed in &mut out.rows {
        for glyph in &mut Arc::make_mut(&mut placed.row).glyphs {
            glyph.chr = MASK;
        }
    }
    out
}
