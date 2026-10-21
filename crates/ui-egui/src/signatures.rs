// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures (R4.5.3): the signatures on the left (New, Rename, Delete), the one chosen
//! edited on the right with the composer's own editor and formatting, its pictures (Insert
//! Picture, a dropped file, a pasted picture; a picture's width and description; the layout) and
//! the size they add to every message; below, each account's signature for new messages and for
//! replies and forwards.
//!
//! Choosing a signature opens it (`signature.open`); every change is a command, and the page only
//! collects input. A signature with changes is saved when another is chosen and when the page is
//! left.

use egui::{Ui, vec2};
use mailcraft_model::signature::{HEAVY_BYTES, Layout, MAX_PICTURES_BYTES, MAX_WIDTH};
use serde_json::json;

use crate::MailApp;
use crate::compose::editor::{self, Which};
use crate::theme::{Tokens, regular, semibold};

pub(crate) fn page(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    ui.label(egui::RichText::new(tl!("Signatures")).font(semibold(22.0)).color(t.text));
    ui.label(
        egui::RichText::new(tl!(
            "Signatures are kept on this computer. They can't be kept in step with Outlook: on another device set them up there, or paste one from Outlook on the web (Settings › Signatures)."
        ))
        .font(regular(11.5))
        .color(t.text_dim),
    );
    ui.add_space(6.0);
    let names: Vec<String> = app.session.mailbox.signatures.iter().map(|s| s.name.clone()).collect();
    if app.session.signature_edit.is_none()
        && let Some(first) = names.first()
    {
        let _ = app.execute("signature.open", json!({"name": first}));
    }
    let open = app.session.signature_edit.as_ref().map(|e| e.signature.name.clone());
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(180.0);
            list(app, ui, t, &names, open.as_deref());
        });
        ui.add_space(12.0);
        ui.vertical(|ui| {
            ui.set_width((ui.available_width()).clamp(320.0, 560.0));
            if open.is_some() {
                editing(app, ui, t);
            } else {
                ui.label(egui::RichText::new(tl!("Make a signature with New.")).color(t.text_dim));
            }
        });
    });
    ui.add_space(10.0);
    ui.label(egui::RichText::new(tl!("Choose default signatures")).font(semibold(14.0)).color(t.accent_text));
    defaults(app, ui, &names);
}

/// Open `name`, keeping what was changed in the one open now.
fn choose(app: &mut MailApp, name: &str) {
    if app.session.signature_edit.as_ref().is_some_and(|e| e.dirty) && app.run("signature.save", json!({})).is_err() {
        return;
    }
    let _ = app.run("signature.open", json!({"name": name}));
}

fn list(app: &mut MailApp, ui: &mut Ui, t: &Tokens, names: &[String], open: Option<&str>) {
    egui::Frame::NONE.fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(4.0).show(ui, |ui| {
        ui.set_min_size(vec2(172.0, 160.0));
        for (i, name) in names.iter().enumerate() {
            let chosen = open.is_some_and(|o| o.eq_ignore_ascii_case(name));
            let r = ui.add_sized(vec2(172.0, 22.0), egui::Button::selectable(chosen, name.as_str()));
            app.hit.add("signature", i as u64, r.rect);
            if r.clicked() && !chosen {
                choose(app, name);
            }
        }
    });
    let renaming_id = egui::Id::new("mailcraft-signature-rename");
    let mut renaming: Option<String> = ui.data(|d| d.get_temp(renaming_id));
    if let (Some(text), Some(from)) = (renaming.as_mut(), open) {
        let r = ui.add(egui::TextEdit::singleline(text).desired_width(172.0).char_limit(100));
        r.request_focus();
        if r.lost_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                let _ = app.run("signature.rename", json!({"name": from, "to": text.trim()}));
            }
            renaming = None;
        }
    }
    ui.horizontal(|ui| {
        if ui.button(tl!("New")).clicked() {
            // "Signature", "Signature 2"… whichever is free.
            let name = (1..100)
                .map(|n| if n == 1 { "Signature".to_string() } else { format!("Signature {n}") })
                .find(|n| app.session.mailbox.signature(n).is_none())
                .unwrap_or_else(|| "Signature".to_string());
            let me = app.session.mailbox.default_account().map(|a| a.display_name.clone()).unwrap_or_default();
            if app.run("signature.set", json!({"name": name, "text": me, "default": names.is_empty()})).is_ok() {
                choose(app, &name);
            }
        }
        if ui.add_enabled(open.is_some(), egui::Button::new(tl!("Rename"))).clicked() {
            renaming = open.map(str::to_string);
        }
        if ui.add_enabled(open.is_some(), egui::Button::new(tl!("Delete"))).clicked()
            && let Some(name) = open
        {
            let _ = app.run("signature.remove", json!({"name": name}));
        }
    });
    ui.data_mut(|d| match renaming {
        Some(text) => {
            d.insert_temp(renaming_id, text);
        }
        None => d.remove::<String>(renaming_id),
    });
}

/// The open signature: its formatting bar, the editor, the selected picture's width and
/// description, and the size its pictures add.
fn editing(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let look = crate::ribbon::look_now(app);
    ui.horizontal_wrapped(|ui| {
        ui.menu_button(tl!("Font"), |ui| crate::ribbon::font_menu(ui, app));
        ui.menu_button(tl!("Size"), |ui| crate::ribbon::size_menu(ui, app));
        ui.menu_button(tl!("Colour"), |ui| crate::ribbon::color_menu(ui, app));
        for (label, on, command) in [("B", look.bold, "format.bold"), ("I", look.italic, "format.italic"), ("U", look.underline, "format.underline")]
        {
            if ui.add(egui::Button::selectable(on, egui::RichText::new(label).strong())).on_hover_text(tl!(command_label(command))).clicked() {
                let _ = app.run(command, json!({}));
            }
        }
        if ui.add(egui::Button::selectable(look.link, tl!("Link"))).clicked() {
            let _ = app.run("compose.link", json!({}));
        }
        if ui.button(tl!("Insert Picture")).clicked() {
            let _ = app.run("signature.picture.add", json!({}));
        }
        // (A picture alone on the clipboard makes no paste in the editor: egui pastes text.)
        if app.services.clipboard.is_some() && ui.button(tl!("Paste Picture")).clicked() {
            match app.services.clipboard.as_ref().and_then(|b| b.image()) {
                Some(png) => {
                    let _ = app.run("signature.picture.add", json!({"data": crate::files::base64_lite::encode(&png), "file": "picture.png"}));
                }
                None => app.status(tl!("There is no picture on the clipboard.")),
            }
        }
        let layout = app.session.signature_edit.as_ref().map_or(Layout::Inline, |e| e.signature.layout);
        let label = |l: Layout| match l {
            Layout::Inline => tl!("In line with text"),
            Layout::PictureLeft => tl!("Picture beside text"),
            Layout::PictureAbove => tl!("Picture above text"),
        };
        egui::ComboBox::from_id_salt("signature_layout").selected_text(label(layout)).show_ui(ui, |ui| {
            for (l, code) in [(Layout::Inline, "inline"), (Layout::PictureLeft, "pictureLeft"), (Layout::PictureAbove, "pictureAbove")] {
                if ui.selectable_label(l == layout, label(l)).clicked() {
                    let _ = app.run("signature.layout", json!({"value": code}));
                }
            }
        });
    });
    egui::Frame::NONE.fill(t.surface).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(10.0).show(ui, |ui| {
        ui.set_min_height(150.0);
        editor::show_doc(app, ui, Which::Signature, 130.0, false);
    });
    picture(app, ui);
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    // What its pictures add to every message, counting those its text still holds.
    let held = e.body.text().pictures();
    let bytes: usize = e.signature.pictures().iter().filter(|p| held.contains(&p.id)).map(|p| p.data.len()).sum();
    let dirty = e.dirty;
    ui.horizontal_wrapped(|ui| {
        let kb = |n: usize| n.div_ceil(1024).to_string();
        let total = crate::i18n::fmt(tl!("Pictures: {n} KB of {max} KB"), &[("n", &kb(bytes)), ("max", &kb(MAX_PICTURES_BYTES))]);
        ui.label(egui::RichText::new(total).color(t.text_dim));
        if bytes > HEAVY_BYTES {
            ui.label(egui::RichText::new(tl!("Every message with this signature gets heavier by as much.")).color(t.text_dim));
        }
    });
    ui.horizontal(|ui| {
        if ui.add_enabled(dirty, egui::Button::new(tl!("Save"))).clicked() {
            let _ = app.run("signature.save", json!({}));
        }
        if dirty {
            ui.label(egui::RichText::new(tl!("Changed: saved when you choose another signature or leave this page.")).color(t.text_dim));
        }
    });
    details_form(app, ui, t);
    preview(app, ui, t);
}

/// The details as the form holds them while they are typed, for the open signature.
#[derive(Clone, Default)]
struct Form {
    signature: String,
    fields: [String; 8],
    /// The details kept when the form last took them: when they change by other means (a
    /// script, undo), the form takes them again.
    seen: Option<[String; 8]>,
}

fn fields_of(d: mailcraft_model::signature::Details) -> [String; 8] {
    [d.name, d.title, d.organisation, d.phone, d.mobile, d.email, d.website, d.address]
}

const FIELDS: [(&str, &str); 8] = [
    ("name", "Name"),
    ("title", "Job title"),
    ("organisation", "Organisation"),
    ("phone", "Phone"),
    ("mobile", "Mobile"),
    ("email", "Email"),
    ("website", "Website"),
    ("address", "Address"),
];

/// Create from details (S14): the form; changing a field lays the signature out again while it
/// isn't edited by hand.
fn details_form(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    let name = e.signature.name.clone();
    let kept = e.signature.details.clone();
    let hand_edited = kept.as_ref().is_some_and(|d| d.made != 0 && d.made != mailcraft_model::signature::text_hash(e.body.text()));
    let key = egui::Id::new("mailcraft-signature-details");
    let seen = kept.clone().map(fields_of);
    let mut form: Form =
        ui.data(|d| d.get_temp(key)).filter(|f: &Form| f.signature == name && (seen.is_none() || f.seen == seen)).unwrap_or_else(|| {
            // The details kept, or what the default account knows of the owner to start from.
            let d = kept.clone().unwrap_or_else(|| {
                let a = app.session.mailbox.default_account();
                mailcraft_model::signature::Details {
                    name: a.map(|a| a.display_name.clone()).unwrap_or_default(),
                    email: a.map(|a| a.email.clone()).unwrap_or_default(),
                    ..Default::default()
                }
            });
            Form { signature: name.clone(), fields: fields_of(d), seen: seen.clone() }
        });
    let mut changed = false;
    egui::CollapsingHeader::new(tl!("Create from details")).default_open(true).show(ui, |ui| {
        egui::Grid::new("signature_details").num_columns(2).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
            for (i, (_, label)) in FIELDS.iter().enumerate() {
                ui.label(tl!(label));
                if let Some(text) = form.fields.get_mut(i) {
                    let field = if i == 7 { egui::TextEdit::multiline(text).desired_rows(2) } else { egui::TextEdit::singleline(text) };
                    changed |= ui.add(field.desired_width(320.0).char_limit(mailcraft_model::signature::MAX_DETAIL)).changed();
                }
                ui.end_row();
            }
        });
        ui.label(
            egui::RichText::new(tl!("The logo is the signature's first picture: add one with Insert Picture.")).font(regular(11.5)).color(t.text_dim),
        );
        let details = || serde_json::Value::Object(FIELDS.iter().zip(&form.fields).map(|((k, _), v)| (k.to_string(), json!(v))).collect());
        if kept.is_none() {
            if ui.button(tl!("Make the signature from these details")).on_hover_text(tl!("Its text is replaced")).clicked() {
                let _ = app.run("signature.details", json!({"details": details(), "restart": true}));
            }
        } else if hand_edited {
            ui.label(egui::RichText::new(tl!("You edited the signature by hand, so the details no longer change it.")).color(t.text_dim));
            if ui.button(tl!("Start again from the details")).clicked() {
                let _ = app.run("signature.details", json!({"details": details(), "restart": true}));
            }
        } else if changed {
            let _ = app.execute("signature.details", json!({"details": details()}));
        }
    });
    // What is kept now, so that the next frame doesn't take it as a change from elsewhere.
    form.seen = app.session.signature_edit.as_ref().and_then(|e| e.signature.details.clone()).map(fields_of);
    ui.data_mut(|d| d.insert_temp(key, form));
}

/// A sample reply in which the signature is shown as recipients see it, and its plain-text form
/// (S15). It follows the window's light or dark.
fn preview(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    let mut sig = e.signature.clone();
    if sig.set_body(e.body.text().clone()).is_err() {
        return;
    }
    let ctx = ui.ctx().clone();
    let key = egui::Id::new("mailcraft-signature-preview");
    let stamp = (
        mailcraft_model::signature::text_hash(sig.body()),
        format!("{:?}", sig.layout),
        sig.pictures().iter().map(|p| (p.id, p.width, p.alt.clone())).collect::<Vec<_>>(),
    );
    let kept: Option<(u64, std::sync::Arc<mailcraft_model::Message>)> = ctx.data(|d| d.get_temp(key));
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        stamp.hash(&mut h);
        h.finish()
    };
    let message = match kept.filter(|(k, _)| *k == hash) {
        Some((_, m)) => m,
        None => {
            let cid = |p: &mailcraft_model::signature::Picture| format!("sig-{}@preview.invalid", p.id);
            let html = format!(
                "<p>{}</p>\r\n{}",
                tl!("Thanks, Thursday at ten works for me. I'll bring the drawings."),
                mailcraft_engine::mime::rich::signature_html(&sig, &cid)
            );
            let m = mailcraft_model::Message {
                body: mailcraft_model::Body { text: String::new(), html: Some(html) },
                attachments: sig
                    .pictures()
                    .iter()
                    .map(|p| mailcraft_model::Attachment {
                        content_id: cid(p),
                        inline: true,
                        data: p.data.clone(),
                        ..mailcraft_model::Attachment::new(p.name.clone(), p.mime.clone(), Vec::new())
                    })
                    .collect(),
                ..mailcraft_model::Message::default()
            };
            let m = std::sync::Arc::new(m);
            ctx.data_mut(|d| d.insert_temp(key, (hash, m.clone())));
            m
        }
    };
    egui::CollapsingHeader::new(tl!("Preview")).default_open(true).show(ui, |ui| {
        ui.label(egui::RichText::new(tl!("As recipients see it")).font(regular(11.5)).color(t.text_dim));
        egui::Frame::NONE.fill(t.surface).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(10.0).show(ui, |ui| {
            // Its own scrolling area, so that what it senses (selecting its text) stays inside it.
            egui::ScrollArea::vertical().id_salt("signature_preview").max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                crate::reading::show_plain_view(app, ui, &message, crate::reading::Place::Preview);
            });
        });
        ui.label(egui::RichText::new(tl!("In a plain-text message")).font(regular(11.5)).color(t.text_dim));
        egui::Frame::NONE.fill(t.surface).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(10.0).show(ui, |ui| {
            ui.label(egui::RichText::new(sig.plain()).font(crate::theme::mono(12.0)).color(t.text));
        });
    });
}

fn command_label(command: &str) -> &'static str {
    match command {
        "format.bold" => "Bold",
        "format.italic" => "Italic",
        _ => "Underline",
    }
}

/// The picture that is the selection, alone: its width and description, and taking it out.
fn picture(app: &mut MailApp, ui: &mut Ui) {
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    let sel = e.body.selection();
    let Some(id) = e
        .body
        .text()
        .slice(sel.start(), sel.end())
        .ok()
        .map(|part| part.pictures())
        .filter(|p| p.len() == 1 && sel.end() == sel.start() + 1)
        .and_then(|p| p.first().copied())
    else {
        return;
    };
    let Some(p) = e.signature.picture(id) else { return };
    let frame = crate::reading::images::frame(ui.ctx());
    let own = crate::reading::images::Pictures::of(ui.ctx()).lock().ok().and_then(|mut c| c.size(&p.data, frame)).map(|[w, _]| w);
    let mut width = p.width.or_else(|| own.and_then(|w| u16::try_from(w).ok())).unwrap_or(MAX_WIDTH).min(MAX_WIDTH);
    let mut alt = p.alt.clone();
    ui.horizontal(|ui| {
        ui.label(tl!("Width"));
        if ui.add(egui::DragValue::new(&mut width).range(8..=MAX_WIDTH).suffix(" px")).changed() {
            let _ = app.run("signature.picture.set", json!({"id": id, "width": width}));
        }
        ui.label(tl!("Description"));
        if ui.add(egui::TextEdit::singleline(&mut alt).desired_width(200.0).char_limit(200)).changed() {
            let _ = app.execute("signature.picture.set", json!({"id": id, "alt": alt}));
        }
        if ui.button(tl!("Remove Picture")).clicked() {
            let _ = app.run("signature.picture.remove", json!({"id": id}));
        }
    });
}

/// Each account's signature for new messages, and for replies and forwards: a shared mailbox's
/// under its owner's account, as the folder pane lists them (#70), each with its own (none until
/// one is chosen for it); not of an account a newer MailCraft added, which keeps its own as that
/// version wrote them.
fn defaults(app: &mut MailApp, ui: &mut Ui, names: &[String]) {
    let accounts: Vec<(u64, String, String, String)> = app
        .session
        .mailbox
        .accounts_in_order()
        .into_iter()
        .filter(|a| a.kind != mailcraft_model::AccountKind::Unknown)
        .map(|a| {
            let shown = if a.is_shared() {
                format!("    {}", crate::i18n::fmt(tl!("{address} (shared mailbox)"), &[("address", &a.email)]))
            } else {
                a.email.clone()
            };
            (a.id, shown, a.signature.clone(), a.reply_signature().to_string())
        })
        .collect();
    if accounts.is_empty() {
        ui.label(tl!("Add an account first"));
    }
    egui::Grid::new("signature_defaults").num_columns(3).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
        for (id, email, new, reply) in accounts {
            ui.label(email);
            for (key, now, label) in [("new", new, tl!("New messages")), ("reply", reply, tl!("Replies and forwards"))] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    let shown = if now.is_empty() { tl!("(none)").to_string() } else { now.clone() };
                    egui::ComboBox::from_id_salt(("signature_default", id, key)).selected_text(shown).width(150.0).show_ui(ui, |ui| {
                        for name in std::iter::once("").chain(names.iter().map(String::as_str)) {
                            let text = if name.is_empty() { tl!("(none)") } else { name };
                            if ui.selectable_label(name.eq_ignore_ascii_case(&now), text).clicked() {
                                let _ = app.run("signature.defaults", json!({"account": id, key: name}));
                            }
                        }
                    });
                });
            }
            ui.end_row();
        }
    });
}
