// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Sender images: a picture in place of a sender's initials, in the message list and over the
//! message being read.
//!
//! Off unless the owner turns it on in File › Options. Asking for a picture tells whoever keeps
//! it whose picture is looked at, so it is asked for only
//!
//! - for a sender the owner knows: one of the contacts, or somebody mail in Sent Items went to;
//! - or for mail whose pictures the owner lets come anyway: all mail (Download pictures in mail
//!   automatically), its sender's or its sender's domain's, or that message's
//!   ([`crate::reading::remote::access`]). Anybody can write to anybody, and a stranger's
//!   address, or a domain made for one recipient alone, learns when it is looked up: that is for
//!   the owner to allow, as the pictures in a stranger's mail are;
//! - only when the server that took the mail in vouches for its sender (DMARC,
//!   [`crate::reading::remote::vouched`]): anybody can write a `From:`, and a forged contact's
//!   face or a forged bank's logo would lend the forgery their trust. Mail from a server that
//!   writes no `Authentication-Results` keeps its initials;
//! - never for mail in Junk E-mail, for the same reason;
//! - only while a row of theirs is on screen, a few at a time, and once while the program runs.
//!
//! This crate opens no connections. It hands the host an address ([`Services::sender_images`])
//! and takes back bytes or nothing: the host asks Gravatar, then favicon.im, then Google,
//! through the same checked download and the same cache on disk as the pictures in mail. What
//! comes is a stranger's bytes, decoded within the same caps; an address that has no picture
//! keeps its initials.
//!
//! [`Services::sender_images`]: crate::Services::sender_images

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, Rect, Ui, vec2};
use mailcraft_model::{Address, Message};
use serde_json::{Value, json};

use crate::MailApp;
use crate::reading::images;
use crate::theme::Tokens;

/// Most addresses whose picture, or lack of one, is kept in memory.
const MAX_KEPT: usize = 1_000;
/// Most addresses the host looks up at a time.
const AT_A_TIME: usize = 4;
/// Most addresses waiting for their turn.
const MAX_WANTED: usize = 64;
/// Largest picture taken, in bytes: a sender's picture is small.
const MAX_BYTES: usize = 1024 * 1024;
/// Longest side a picture is kept at, in pixels.
const SIDE: u32 = 128;
/// Longest side of a picture taken, in pixels: it is decoded whole on the window's thread.
const MAX_SIDE: u32 = 1024;
/// How long the host is waited on for one address, in milliseconds: it may try three places,
/// each for up to 30 s (`mailcraft_net::pictures::PATIENCE`).
const PATIENCE_MS: f64 = 100_000.0;

/// What is known of one address's picture.
enum Look {
    /// The host is looking (since when, in milliseconds).
    Asked(Receiver<Result<Vec<u8>, String>>, f64),
    /// Its bytes, not decoded yet: one picture is decoded in a frame.
    Came(Vec<u8>),
    Ready(egui::TextureHandle),
    /// There is none to be had: the initials stay.
    // shortcut: not asked about again until the program restarts, even when the network was down.
    Missing,
}

/// Senders' pictures, for as long as the program runs.
#[derive(Default)]
pub struct SenderImages {
    /// By address in lower case, with the frame it was last on screen in.
    looks: HashMap<String, (Look, u64)>,
    /// Addresses on screen that nothing is known of yet: asked for at the next frame.
    wanted: Vec<String>,
    /// The frame arrivals were last looked for in, and the frame a picture was last decoded in.
    ticked: Option<u64>,
    decoded: Option<u64>,
    /// How many look-ups the host had in hand at the last frame.
    flying: usize,
    /// Handed to the host with every look-up, weakly: gone with this state, it stops what is in
    /// flight before its next request.
    alive: Arc<()>,
}

/// Whether the owner knows `address` (in lower case): it is one of the contacts', or mail in
/// Sent Items went to it.
fn is_known(app: &mut MailApp, address: &str) -> bool {
    app.session.knows(address)
}

/// Take in what the host found, and hand it the addresses rows wanted a frame ago. Once a frame.
fn tick(app: &mut MailApp, ctx: &egui::Context) {
    let frame = images::frame(ctx);
    let offline = app.offline();
    let state = &mut app.sender_images;
    if state.ticked == Some(frame) {
        return;
    }
    state.ticked = Some(frame);
    let now = crate::now_ms();
    let mut flying = 0;
    for (look, _) in state.looks.values_mut() {
        let Look::Asked(answer, since) = look else { continue };
        *look = match answer.try_recv() {
            // A stranger's bytes: a picture within the caps, or none.
            Ok(Ok(bytes)) if bytes.len() <= MAX_BYTES && images::probe_icon(&bytes).is_some_and(|[w, h]| w.max(h) <= MAX_SIDE) => Look::Came(bytes),
            Ok(_) | Err(TryRecvError::Disconnected) => Look::Missing,
            Err(TryRecvError::Empty) if now - *since > PATIENCE_MS => Look::Missing,
            Err(TryRecvError::Empty) => {
                flying += 1;
                continue;
            }
        };
        ctx.request_repaint();
    }
    // (Working offline, nobody is asked: senders keep their initials.)
    if !offline && let Some(look_up) = &app.services.sender_images {
        // The rest is wanted again by its row, for as long as that is on screen.
        for address in std::mem::take(&mut state.wanted).into_iter().take(AT_A_TIME.saturating_sub(flying)) {
            if state.looks.contains_key(&address) {
                continue;
            }
            trim(&mut state.looks, frame);
            let answer = look_up(address.clone(), Arc::downgrade(&state.alive));
            state.looks.insert(address, (Look::Asked(answer, now), frame));
            flying += 1;
        }
    }
    state.flying = flying;
    if flying > 0 {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Forget what was on screen longest ago while there is too much; never what is on its way.
fn trim(looks: &mut HashMap<String, (Look, u64)>, frame: u64) {
    while looks.len() >= MAX_KEPT {
        let oldest = looks
            .iter()
            .filter(|(_, (look, used))| *used < frame && !matches!(look, Look::Asked(..)))
            .min_by_key(|(_, (_, used))| *used)
            .map(|(address, _)| address.clone());
        match oldest {
            Some(address) => drop(looks.remove(&address)),
            None => return,
        }
    }
}

/// The picture to show for `who`, on the mail `m`, in place of their initials. `None` while
/// there is none; one that is still to be looked up is noted, and asked for at the next frame.
pub fn picture(app: &mut MailApp, ctx: &egui::Context, m: &Message, who: &Address) -> Option<egui::TextureHandle> {
    if !app.ui.sender_images || app.services.sender_images.is_none() {
        return None;
    }
    if app.session.mailbox.in_junk(m.folder) {
        return None;
    }
    tick(app, ctx);
    let address = who.email.trim().to_lowercase();
    // The say-so for a message's pictures is its sender's: whoever it goes to must be known.
    let sender = address == m.from.email.trim().to_lowercase();
    // Who it went to is the owner's word (it is theirs); who sent it, only the server's.
    if sender && !crate::reading::remote::vouched(app.session.mailbox.account(m.account), m) {
        return None;
    }
    if !is_known(app, &address) && !(sender && crate::reading::remote::access(app, m) == crate::reading::Access::Allowed) {
        return None;
    }
    let frame = images::frame(ctx);
    let state = &mut app.sender_images;
    let Some((look, used)) = state.looks.get_mut(&address) else {
        if state.wanted.len() < MAX_WANTED && !state.wanted.contains(&address) {
            state.wanted.push(address);
            // With every place taken, the 250 ms repaint of `tick` comes back for it.
            if state.flying < AT_A_TIME {
                ctx.request_repaint();
            }
        }
        return None;
    };
    *used = frame;
    if let Look::Came(bytes) = look {
        if state.decoded == Some(frame) {
            ctx.request_repaint();
            return None;
        }
        state.decoded = Some(frame);
        *look = match images::decode_icon(bytes, SIDE) {
            Some(image) => Look::Ready(ctx.load_texture("mailcraft-sender-image", image, egui::TextureOptions::LINEAR)),
            None => Look::Missing,
        };
    }
    match look {
        Look::Ready(texture) => Some(texture.clone()),
        _ => None,
    }
}

/// `who`'s round avatar on the mail `m`: their picture when there is one, else their initials.
pub fn avatar(app: &mut MailApp, ui: &Ui, center: egui::Pos2, radius: f32, m: &Message, who: &Address) {
    let Some(texture) = picture(app, ui.ctx(), m, who) else {
        crate::widgets::avatar(ui, center, radius, who);
        return;
    };
    // On white, as a browser shows an icon: one with see-through parts stays readable in the dark.
    ui.painter().circle_filled(center, radius, Color32::WHITE);
    egui::Image::from_texture(egui::load::SizedTexture::from_handle(&texture))
        .corner_radius(radius)
        .paint_at(ui, Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0)));
}

/// The row of File › Options, saying who is asked and what each is told.
pub fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let mut on = app.ui.sender_images;
    if ui.checkbox(&mut on, tl!("Show sender images from Gravatar, favicon.im and Google")).changed() {
        let _ = app.run("ui.senderImages", json!({"value": on}));
    }
    let said = tl!(
        "Gravatar is asked for the sender's picture and told a hash of their address; else favicon.im, then Google, are asked for their website's icon and told the sender's domain. This tells those services whose pictures you look at. Only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for Junk E-mail, and a sender's picture only when your mail server confirms the mail is from them."
    );
    ui.label(egui::RichText::new(said).color(t.text_dim));
}

/// `ui.senderImages`; forgetting what is in memory when it is turned off.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    (id == "ui.senderImages").then(|| {
        let on = p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.sender_images);
        app.ui.sender_images = on;
        if !on {
            app.sender_images = SenderImages::default();
        }
        Ok(json!({"value": on}))
    })
}

#[cfg(test)]
#[path = "sender_images_tests.rs"]
mod tests;
