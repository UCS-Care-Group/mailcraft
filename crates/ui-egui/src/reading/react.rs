// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! React (#1544): the button beside the reply buttons of the reading pane and the row of six emoji
//! it opens. Picking one runs `mail.react`, which sends it as a short mail (RFC 9078); nothing is
//! written in a message window. The emoji are drawn with the window's own fonts (#832).

use egui::{Button, Ui, vec2};
use mailcraft_engine::cmd::react::{REACTIONS, recipient, refusal, text};
use mailcraft_model::Message;
use serde_json::json;

use crate::MailApp;
use crate::theme::regular;

/// `emoji` as the button draws it: without the variation selector that makes a heart a colour
/// emoji (U+FE0F), which none of the window's fonts has a glyph for (it would be drawn as a box
/// beside the heart). What is sent keeps it.
pub(crate) fn drawn(emoji: &str) -> &str {
    emoji.trim_end_matches('\u{FE0F}')
}

/// The most characters of the sender's name shown beside the emoji: a name comes from the mail, and
/// a long one must not push the address (what matters) out of the row or fill the screen.
const NAME_LIMIT: usize = 40;

/// `Name <address>` as the row shows it: the name cut at [`NAME_LIMIT`] characters, the address whole.
pub(crate) fn shown(a: &mailcraft_model::Address) -> String {
    let name = a.name.trim();
    if name.is_empty() {
        return a.email.clone();
    }
    let cut: String = name.chars().take(NAME_LIMIT).collect();
    let dots = if name.chars().count() > NAME_LIMIT { "\u{2026}" } else { "" };
    format!("{cut}{dots} <{}>", a.email)
}

pub(super) fn label() -> &'static str {
    tl!("React")
}

/// The React button, and its row of emoji while it is open. Greyed, with the reason of a message
/// that can't be reacted to as the engine judges it, and not there for a message not filed.
pub(super) fn button(app: &mut MailApp, ui: &mut Ui, m: &Message) {
    if m.id == 0 {
        return;
    }
    // The engine's own reason (a message with a sender warning too), as the tooltip of the greyed button.
    let why = refusal(&mut app.session, m);
    let blocked = why.is_some();
    let tip = match why {
        Some(why) => crate::i18n::t(text(why)),
        None => label(),
    };
    let to = recipient(&app.session, m).map(|a| shown(&a));
    let resp = crate::widgets::icon_text_button(ui, "react", label(), !blocked).on_hover_text(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !blocked, tip));
    app.hit.add("react", m.id, resp.rect);
    if blocked {
        return;
    }
    let id = m.id;
    // The words for the emoji of REACTIONS, in their order.
    let names = [tl!("Like"), tl!("Love"), tl!("Celebrate"), tl!("Laugh"), tl!("Surprised"), tl!("Sad")];
    egui::Popup::menu(&resp).show(|ui| {
        // Whom it goes to: a reaction is sent at one click, so the address is shown first.
        if let Some(to) = &to {
            let label = ui.label(crate::i18n::fmt(tl!("to {recipient}"), &[("recipient", to)]));
            app.hit.add("reaction-to", id, label.rect);
        }
        ui.horizontal(|ui| {
            for (index, ((emoji, _), name)) in REACTIONS.iter().zip(names).enumerate() {
                let r = ui.add(Button::new(egui::RichText::new(drawn(emoji)).font(regular(22.0))).frame(false).min_size(vec2(36.0, 34.0)));
                r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name));
                app.hit.add("reaction", index as u64, r.rect);
                if r.on_hover_text(name).clicked() {
                    let _ = app.run("mail.react", json!({"id": id, "emoji": emoji}));
                }
            }
        });
    });
}
