// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::compose::tests::{app, window};

const RANGE: &str = "<table><tr><td>Quantity</td><td>Price</td></tr><tr><td></td><td>12,30</td></tr></table>";

struct SpreadsheetClipboard;

impl crate::RichClipboard for SpreadsheetClipboard {
    fn put(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }

    fn html(&self) -> Option<String> {
        Some(RANGE.to_string())
    }
}

#[test]
fn a_pasted_table_fits_the_composer_and_has_one_position() {
    let fragment = mailcraft_mime_model::rich::from_html_for_compose(RANGE);
    let paragraph = fragment.paragraphs().first().unwrap();
    let mut layout = Layout::default();
    let ctx = egui::Context::default();
    let mut frame = ctx.run_ui(egui::RawInput::default(), |ui| {
        layout.paras.push(Layout::default().lay(ui.ctx(), paragraph, 0, false, &[], &Tokens::light(), 96.0));
    });
    frame.textures_delta.clear();
    let p = layout.paras.first().unwrap();
    let object = p.tables.first().unwrap();
    assert_eq!(p.chars, 1);
    assert!(object.rect.min.x >= 0.0 && object.rect.max.x <= 96.5 && object.rect.height() >= 48.0, "{:?}", object.rect);
    assert_eq!(layout.table_hit(object.rect.center().to_vec2()), Some(0));
    assert_eq!(layout.hit(object.rect.left_center().to_vec2()), 0);
    assert_eq!(layout.hit(object.rect.right_center().to_vec2()), 1);
}

#[test]
fn paste_save_reopen_copy_remove_and_undo_keep_the_grid() {
    let mut a = app();
    a.services.clipboard = Some(Box::new(SpreadsheetClipboard));
    a.execute("mail.new", json!({"to":"test@example.org", "subject":"Grid"})).unwrap();
    a.want_body_focus();
    let mut h = window(a);
    h.event(Event::Paste("Quantity\tPrice\n\t12,30".into()));
    h.run_steps(2);
    let a = h.state_mut();
    let saved = a.execute("compose.saveDraft", json!({})).unwrap()["draft"].as_u64().unwrap();
    a.execute("compose.close", json!({})).unwrap();
    a.execute("mail.editDraft", json!({"id": saved})).unwrap();
    a.execute("compose.select", json!({"from": 0, "to": 1})).unwrap();
    let copied = a.execute("edit.copy", json!({})).unwrap();
    assert_eq!(copied["text"], "Quantity\tPrice\n\t12,30");
    assert_eq!(copied["html"].as_str().unwrap().matches("<td ").count(), 4);
    a.execute("edit.cut", json!({})).unwrap();
    a.execute("compose.undo", json!({})).unwrap();
    let (body, _) = body(a, Which::Message).unwrap();
    assert!(body.text().paragraphs().iter().flat_map(|p| &p.runs).any(|r| r.table.is_some()));
}
