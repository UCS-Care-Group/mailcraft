//! The security bar: whether a message is signed (by whom, whether the signature holds, whether
//! anybody the owner trusts vouches for the signer, whether the signer is the sender the message
//! names) and encrypted (opened, or why not).
//!
//! It says plainly when a signature is mathematically right but nobody vouches for the signer,
//! and when the signer isn't the sender the message names: a valid signature from a stranger
//! proves nothing about who wrote it.

use std::sync::Arc;

use egui::{Sense, Ui, vec2};
use mailcraft_engine::security::{Level, SecurityView, SigState};
use mailcraft_model::Message;
use serde_json::json;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The bar, between the header and the body, for a message that is signed or encrypted.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, view: &SecurityView) {
    let t = Tokens::get(ui.ctx());
    let level = view.level();
    let sig = view.signature.as_ref();
    // The keys a good signature brings are kept, when the owner wants that.
    if app.session.prefs.collect_keys && !view.collectable.is_empty() && sig.is_some_and(|s| s.validity == SigState::Valid && s.from_matches) {
        let _ = app.run("security.collect", json!({"id": m.id}));
    }
    let (icon, color, headline) = match level {
        Level::Good if sig.is_some() => ("seal", t.green, tl!("Signed and verified")),
        Level::Good => ("encrypt", t.green, tl!("Encrypted")),
        Level::Notice if sig.is_some_and(|s| s.validity == SigState::Valid && !s.from_matches) => {
            ("warning", t.orange, tl!("Signed by someone else"))
        }
        Level::Notice if sig.is_some_and(|s| s.validity == SigState::Valid) => ("warning", t.orange, tl!("Signed, signer not verified")),
        Level::Notice => ("warning", t.orange, tl!("Signature not checked")),
        Level::Bad => ("warning", t.red, tl!("Invalid signature")),
        Level::Locked => ("lock", t.text_dim, tl!("Encrypted, can't be opened")),
    };
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
            icons::paint(ui.painter(), r, icon, color, color);
            ui.label(egui::RichText::new(headline).font(semibold(13.0)).color(t.text));
            if view.encrypted.is_some() && view.locked.is_none() && sig.is_some() {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                icons::paint(ui.painter(), r, "encrypt", t.text_dim, t.text_dim);
                ui.label(egui::RichText::new(tl!("Encrypted")).font(regular(12.0)).color(t.text_dim));
            }
        });
        ui.add(egui::Label::new(egui::RichText::new(view.summary()).font(regular(12.0)).color(t.text_dim)).wrap());
        if let Some(sig) = sig
            && !sig.fingerprint.is_empty()
        {
            let grouped: String =
                sig.fingerprint.chars().collect::<Vec<_>>().chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ");
            let mut line = format!("{} {} · {}", sig.scheme.label(), tl!("fingerprint"), grouped);
            if let Some(at) = sig.signed_at {
                let local = app.session.local(at);
                line.push_str(&format!(
                    " · {} {} {}",
                    tl!("signed"),
                    crate::i18n::date_long(local.date),
                    local.time_short(app.session.prefs.twelve_hour)
                ));
            }
            ui.add(egui::Label::new(egui::RichText::new(line).font(regular(11.0)).color(t.text_dim)).wrap());
            // (Trusting the signer can't make a stranger the sender the message names.)
            if !sig.verified && sig.validity == SigState::Valid && sig.from_matches {
                ui.horizontal_wrapped(|ui| {
                    let tip =
                        tl!("Vouch for this certificate or key from now on: do this only once you have checked its fingerprint with its holder.");
                    if ui.button(tl!("Trust This Signer")).on_hover_text(tip).clicked() {
                        let _ = app.run("security.trust", json!({"fingerprint": sig.fingerprint, "value": true}));
                    }
                    if !view.collectable.is_empty() && !app.session.prefs.collect_keys && ui.button(tl!("Add Sender's Certificate")).clicked() {
                        let _ = app.run("security.collect", json!({"id": m.id}));
                    }
                });
            }
        }
    });
}
