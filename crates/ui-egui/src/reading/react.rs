// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): the button beside the reply buttons of the reading pane and the row of six emoji
//! it opens. Picking one runs `mail.react`, which sends it as a short mail (RFC 9078); nothing is
//! written in a message window. The emoji are drawn with the window's own fonts (#832).

use egui::{Button, Ui, vec2};
use mailcraft_engine::cmd::react::{REACTIONS, why_not};
use mailcraft_model::Message;
use serde_json::json;

use crate::MailApp;
use crate::theme::regular;

/// The React button, and its row of emoji while it is open. Greyed, with the reason of a message
/// that can't be reacted to as the engine judges it, and not there for a message not filed.
pub(super) fn button(app: &mut MailApp, ui: &mut Ui, m: &Message) {
    if m.id == 0 {
        return;
    }
    let blocked = why_not(&app.session, m).is_some();
    let tip = if blocked { tl!("A reaction can't be sent to this message") } else { tl!("React") };
    let resp = crate::widgets::icon_button(ui, "react", tip, 17.0, !blocked);
    app.hit.add("react", m.id, resp.rect);
    if blocked {
        return;
    }
    let id = m.id;
    // The words for the emoji of REACTIONS, in their order.
    let names = [tl!("Like"), tl!("Love"), tl!("Celebrate"), tl!("Laugh"), tl!("Surprised"), tl!("Sad")];
    egui::Popup::menu(&resp).show(|ui| {
        ui.horizontal(|ui| {
            for (index, ((emoji, _), name)) in REACTIONS.iter().zip(names).enumerate() {
                let r = ui.add(Button::new(egui::RichText::new(*emoji).font(regular(22.0))).frame(false).min_size(vec2(36.0, 34.0)));
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                app.hit.add("reaction", index as u64, r.rect);
                if r.on_hover_text(name).clicked() {
                    let _ = app.run("mail.react", json!({"id": id, "emoji": emoji}));
                }
            }
        });
    });
}
