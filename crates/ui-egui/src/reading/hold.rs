//! The hold bar: a message in the Outbox that the server would not send as the account it goes
//! from (no Send As right, or no permission for it) is held until the owner sends it again,
//! rather than tried every few minutes with the same refusal. The bar says why, and offers to
//! send it again or to open it for changes.

use std::sync::Arc;

use egui::{Sense, Ui, vec2};
use mailcraft_model::Message;
use serde_json::json;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The bar, between the header and the body, for a message that is held ([`Message::hold`]).
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>) {
    let Some(why) = m.hold.as_deref().map(str::trim).filter(|why| !why.is_empty()) else { return };
    let t = Tokens::get(ui.ctx());
    let color = t.orange;
    let frame = egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.6))).inner_margin(egui::Margin {
        left: 20,
        right: 12,
        top: 7,
        bottom: 8,
    });
    frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint(ui.painter(), r, "warning", color, color);
            ui.label(egui::RichText::new(tl!("Not sent")).font(semibold(13.0)).color(t.text));
        });
        // (The server's words, one line at most and not too long: they are shown, not trusted.)
        let shown: String = why.chars().filter(|c| !c.is_control()).take(600).collect();
        ui.add(egui::Label::new(egui::RichText::new(shown).font(regular(12.0)).color(t.text_dim)).wrap());
        ui.horizontal_wrapped(|ui| {
            let again = ui.button(tl!("Send Again")).on_hover_text(tl!("Goes with the next Send/Receive, as it is."));
            app.hit.add("holdSendAgain", m.id, again.rect);
            if again.clicked() {
                if let Err(e) = app.run("mail.resend", json!({"id": m.id})) {
                    app.status(e.to_string());
                }
            }
            let open = ui.button(tl!("Open")).on_hover_text(tl!("Change the message before it goes."));
            app.hit.add("holdOpen", m.id, open.rect);
            if open.clicked() {
                if let Err(e) = app.run("mail.editDraft", json!({"id": m.id})) {
                    app.status(e.to_string());
                }
            }
        });
    });
}
