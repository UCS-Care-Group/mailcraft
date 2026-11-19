// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures (R4.5.3): the signatures on the left (New, Rename, Delete), the one chosen
//! edited on the right with the composer's own editor and formatting, its pictures (Insert
//! Picture, a dropped file, a pasted picture; a picture's width and description; the layout) and
//! the size they add to every message; below, each account's signature for new messages and for
//! replies and forwards. An HTML signature is edited as its HTML source instead.
//!
//! Choosing a signature opens it (`signature.open`); every change is a command, and the page only
//! collects input. A signature with changes is saved when another is chosen and when the page is
//! left.

use egui::{Ui, vec2};
use mailcraft_model::signature::{HEAVY_BYTES, Layout, MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES_BYTES, MAX_WIDTH};
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
    after_leave_question(app, ui.ctx());
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
            ui.add_space(6.0);
            crate::signature_import::panel(app, ui, t);
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

/// The page's last word on a picture: what refused it, or that it was made smaller.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Note {
    pub text: String,
    pub error: bool,
}

/// A picture of this many bytes at most is read to be made smaller; more is refused outright.
const MAX_SOURCE_BYTES: u64 = 32 * 1024 * 1024;

/// Add a picture to the open signature (`path` or `data`, as `signature.picture.add` takes them):
/// one over the cap is made smaller to fit; what can't be added is said on the page, which is
/// all the owner sees (the status bar is hidden behind File).
pub(crate) fn add_picture(app: &mut MailApp, params: serde_json::Value) {
    app.signature_note = None;
    let Err(refused) = app.run("signature.picture.add", params.clone()) else { return };
    let (stem, bytes) = match source(&params) {
        Some(s) => s,
        None => return refuse(app, refused),
    };
    let Some((small, w, ext)) = shrink(&bytes) else { return refuse(app, refused) };
    let data = crate::files::base64_lite::encode(&small);
    let mut again = json!({"data": data, "file": format!("{stem}.{ext}")});
    if let Some(alt) = params.get("alt") {
        again["alt"] = alt.clone();
    }
    match app.run("signature.picture.add", again) {
        Ok(_) => {
            let text = crate::i18n::fmt(tl!("The picture was too big, so it was made smaller ({w} px wide)."), &[("w", &w.to_string())]);
            app.signature_note = Some(Note { text, error: false });
        }
        Err(_) => refuse(app, refused),
    }
}

fn refuse(app: &mut MailApp, why: String) {
    app.signature_note = Some(Note { text: why, error: true });
}

/// The picture's name without its extension, and its bytes, from a file or from the data.
fn source(params: &serde_json::Value) -> Option<(String, Vec<u8>)> {
    let name = |n: &str| {
        let n = n.rsplit(['/', '\\']).next().unwrap_or(n);
        n.rsplit_once('.').map_or(n, |(stem, _)| stem).to_string()
    };
    if let Some(data) = params.get("data").and_then(|d| d.as_str()) {
        let file = params.get("file").and_then(|f| f.as_str()).unwrap_or("picture");
        return Some((name(file), crate::files::base64_lite::decode(data)?));
    }
    let path = params.get("path")?.as_str()?.trim();
    if std::fs::metadata(path).ok()?.len() > MAX_SOURCE_BYTES {
        return None;
    }
    Some((name(path), std::fs::read(path).ok()?))
}

/// `bytes` as a picture within [`MAX_PICTURE_BYTES`] and [`MAX_WIDTH`] px: PNG, or JPEG when it has
/// no transparency and PNG is still too big, at half the width again until it fits (a few tries).
/// And the width and the extension to name it by. `None` when it is no PNG, JPEG, GIF or WebP,
/// can't be read (limits on what it may decode to) or won't fit.
fn shrink(bytes: &[u8]) -> Option<(Vec<u8>, u32, &'static str)> {
    use image::{ImageDecoder as _, ImageEncoder as _};
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    // Only the kinds a message shows, by what the bytes are: never another decoder the build has.
    if !crate::reading::images::KINDS.contains(&reader.format()?) {
        return None;
    }
    let mut decoder = reader.into_decoder().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    decoder.set_limits(limits).ok()?;
    let img = image::DynamicImage::from_decoder(decoder).ok()?;
    let opaque = !img.color().has_alpha();
    let mut width = img.width().min(u32::from(MAX_WIDTH));
    for _ in 0..5 {
        if width < 16 {
            return None;
        }
        let height = u32::try_from(u64::from(img.height()) * u64::from(width) / u64::from(img.width().max(1))).ok()?.max(1);
        let small = img.resize_exact(width, height, image::imageops::FilterType::Triangle);
        let mut png = Vec::new();
        let rgba = small.to_rgba8();
        image::codecs::png::PngEncoder::new(&mut png).write_image(rgba.as_raw(), width, height, image::ExtendedColorType::Rgba8).ok()?;
        if png.len() <= MAX_PICTURE_BYTES {
            return Some((png, width, "png"));
        }
        if opaque {
            let mut jpeg = Vec::new();
            let rgb = small.to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
                .write_image(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
                .ok()?;
            if jpeg.len() <= MAX_PICTURE_BYTES {
                return Some((jpeg, width, "jpg"));
            }
        }
        width /= 2;
    }
    None
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
    let field = egui::Id::unique("mailcraft-signature-rename-field");
    // Whether the field was open when this frame began: the Rename button below keeps what
    // was typed while it is (#1156), and opens the field while it isn't.
    let was_open = app.signature_rename.is_some();
    let mut entered = false;
    if let Some(rename) = app.signature_rename.as_mut() {
        // Escape is the field's (it lets go of the name), never File's, which would close.
        let keys = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, tab: false, escape: true };
        let edit = egui::TextEdit::singleline(&mut rename.text).id(field).desired_width(172.0).char_limit(100).event_filter(keys);
        let r = crate::text_menu::field(ui.add(edit));
        // Enter, or leaving the field any other way (a click elsewhere, Tab, choosing another
        // signature, Save; Back is `MailApp::leave_signature`), keeps the name; Escape lets go of
        // it (#1651). The keyboard is asked for once, by Rename, or nothing could take it away.
        if crate::keys::take_escape_in(ui.ctx(), ui.layer_id(), field) {
            ui.memory_mut(|m| m.surrender_focus(field));
            finish_rename(app, false);
        } else if (r.has_focus() || r.lost_focus()) && crate::keys::enter(ui.ctx(), ui.layer_id()) {
            ui.memory_mut(|m| m.surrender_focus(field));
            finish_rename(app, true);
            entered = true;
        } else if crate::text_menu::left(&r) {
            finish_rename(app, true);
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
        if ui.add_enabled(open.is_some(), egui::Button::new(tl!("Rename"))).clicked()
            && let Some(from) = open
        {
            if was_open && !entered {
                // Pressed while the field holds the new name: keep what was typed (#1156),
                // rather than resetting the field to the old name and losing it.
                ui.memory_mut(|m| m.surrender_focus(field));
                finish_rename(app, true);
            } else if !was_open {
                app.signature_rename = Some(Rename { from: from.to_string(), text: from.to_string() });
                ui.memory_mut(|m| m.request_focus(field));
            }
        }
        // The open one as it is named now: leaving the field may have just renamed it.
        let current = app.session.signature_edit.as_ref().map(|e| e.signature.name.clone());
        if ui.add_enabled(open.is_some(), egui::Button::new(tl!("Delete"))).clicked()
            && let Some(name) = current
        {
            let _ = app.run("signature.remove", json!({"name": name}));
        }
    });
}

/// A name being typed in File › Signatures › Rename: for the signature it was opened on (even
/// when another is chosen meanwhile), and the text so far.
#[derive(Clone, Debug)]
pub(crate) struct Rename {
    from: String,
    text: String,
}

/// Close the Rename field: rename the signature it was opened on to what was typed when `keep`
/// (and that is a new name, not empty); let go of it otherwise.
pub(crate) fn finish_rename(app: &mut MailApp, keep: bool) {
    let Some(Rename { from, text }) = app.signature_rename.take() else { return };
    let to = text.trim();
    if keep && !to.is_empty() && to != from {
        let _ = app.run("signature.rename", json!({"name": from, "to": to}));
    }
}

/// The open signature: rich text or HTML, then its formatting bar, the editor, the selected
/// picture's width and description, and the size its pictures add; or its HTML source.
fn editing(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let html = app.session.signature_edit.as_ref().and_then(|e| e.signature.html().map(str::to_string));
    ui.horizontal(|ui| {
        for (on, label) in [(html.is_none(), tl!("Rich text")), (html.is_some(), tl!("HTML"))] {
            if ui.add(egui::Button::selectable(on, label)).clicked() && !on {
                app.signature_note = None;
                // HTML starts from what the signature is now.
                let source = match (&html, app.session.signature_edit.as_ref()) {
                    (None, Some(e)) => json!(mailcraft_engine::mime::rich::to_html(e.body.text())),
                    _ => serde_json::Value::Null,
                };
                if let Err(why) = app.run("signature.html", json!({"html": source})) {
                    refuse(app, why);
                }
            }
        }
    });
    match html {
        Some(source) => html_source(app, ui, t, source),
        None => rich(app, ui, t),
    }
    let dirty = app.session.signature_edit.as_ref().is_some_and(|e| e.dirty);
    // An HTML signature with an email compatibility error, or whose HTML could not be checked,
    // can't be kept, nor one naming a picture it can't be sent with: Save says why, and the
    // engine refuses it too.
    let saved = app.session.signature_edit.as_ref().map(as_saved);
    let refusal = saved
        .as_ref()
        .and_then(|sig| sig.html())
        .and_then(|h| crate::compat::save_refusal(&crate::compat::checked(ui.ctx(), h)))
        .or_else(|| if dirty { picture_refusal(app) } else { None });
    ui.horizontal(|ui| {
        let save = ui.add_enabled(dirty && refusal.is_none(), egui::Button::new(tl!("Save")));
        let save = match &refusal {
            Some(why) => save.on_disabled_hover_text(why.as_str()),
            None => save,
        };
        if save.clicked() {
            let _ = app.run("signature.save", json!({}));
        }
        if dirty {
            ui.label(egui::RichText::new(tl!("Changed: saved when you choose another signature or leave this page.")).color(t.text_dim));
        }
    });
    if app.session.signature_edit.as_ref().is_some_and(|e| e.signature.html().is_none()) {
        details_form(app, ui, t);
    }
    preview(app, ui, t);
}

/// An HTML signature's source, as the owner writes it.
fn html_source(app: &mut MailApp, ui: &mut Ui, t: &Tokens, mut source: String) {
    // Its own id, so that its caret and undo are found again; its layouter underlines what the
    // email-compatibility check found in the text being laid out (`compat`).
    let id = crate::compat::editor_id();
    let mut layouter = |ui: &Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
        let text = text.as_str();
        let checked = crate::compat::checked(ui.ctx(), text);
        let look = crate::compat::Look::of(ui, id, t);
        ui.fonts_mut(|f| f.layout_job(crate::compat::layout_job(text, checked.marks(), &look, wrap_width)))
    };
    let out = egui::TextEdit::multiline(&mut source)
        .id(id)
        .code_editor()
        .desired_rows(10)
        .desired_width(f32::INFINITY)
        .char_limit(MAX_HTML)
        .hint_text(tl!("Paste or write the signature's HTML"))
        .layouter(&mut layouter)
        .show(ui);
    app.hit.add("signatureHtmlSource", 0, out.response.rect);
    if crate::text_menu::field(out.response.response.clone()).changed()
        && let Err(why) = app.execute("signature.html", json!({"html": source}))
    {
        refuse(app, why);
    }
    // The popup of a finding pointed at, and its fix, sent as typing is sent.
    if let Some(fixed) = crate::compat::popup(app, ui, t, &out, &source)
        && let Err(why) = app.execute("signature.html", json!({"html": fixed}))
    {
        refuse(app, why);
    }
    // How many findings there are, below the editor.
    let checked = crate::compat::checked(ui.ctx(), &source);
    if let Some(line) = crate::compat::summary(&checked) {
        ui.label(egui::RichText::new(line).color(if checked.blocks_save() { t.red } else { t.orange }));
    }
    if let Some(note) = &app.signature_note {
        ui.label(egui::RichText::new(note.text.as_str()).color(if note.error { t.red } else { t.text_dim }));
    }
    ui.label(
        egui::RichText::new(tl!(
            "Sent as written, without anything that runs or tracks. Pictures come from their web address, and recipients' programs decide whether to load them. A message you write shows a simpler copy of this signature; if you edit it there, that copy is sent instead."
        ))
        .font(regular(11.5))
        .color(t.text_dim),
    );
}

/// A rich signature: its formatting bar, the editor, its pictures and the size they add.
fn rich(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let look = crate::ribbon::look_now(app);
    ui.horizontal_wrapped(|ui| {
        ui.menu_button(tl!("Font"), |ui| crate::ribbon::font_menu(ui, app));
        ui.menu_button(tl!("Size"), |ui| crate::ribbon::size_menu(ui, app));
        // The colour menu holds a field for a colour of the owner's own: a click in it is for the
        // field, not a way out of the menu, as on the ribbon (#1493, #1464).
        let colour = ui.button(tl!("Colour"));
        egui::Popup::menu(&colour).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| crate::ribbon::color_menu(ui, app));
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
            app.signature_note = None;
            let _ = app.run("signature.picture.add", json!({}));
        }
        // (Ctrl+V, or Paste in the body's menu, takes a picture alone on the clipboard too.)
        if app.services.clipboard.is_some() && ui.button(tl!("Paste Picture")).clicked() {
            match app.services.clipboard.as_ref().and_then(|b| b.image()) {
                Some(png) => {
                    add_picture(app, json!({"data": crate::files::base64_lite::encode(&png), "file": "picture.png"}));
                }
                None => app.signature_note = Some(Note { text: tl!("There is no picture on the clipboard.").to_string(), error: true }),
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
    if let Some(note) = &app.signature_note {
        ui.label(egui::RichText::new(note.text.as_str()).color(if note.error { t.red } else { t.text_dim }));
    }
    picture(app, ui);
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    // What its pictures add to every message, counting those its text still holds.
    let held = e.body.text().pictures();
    let bytes: usize = e.signature.pictures().iter().filter(|p| held.contains(&p.id)).map(|p| p.data.len()).sum();
    ui.horizontal_wrapped(|ui| {
        let kb = |n: usize| n.div_ceil(1024).to_string();
        let total = crate::i18n::fmt(tl!("Pictures: {n} KB of {max} KB"), &[("n", &kb(bytes)), ("max", &kb(MAX_PICTURES_BYTES))]);
        ui.label(egui::RichText::new(total).color(t.text_dim));
        if bytes > HEAVY_BYTES {
            ui.label(egui::RichText::new(tl!("Every message with this signature gets heavier by as much.")).color(t.text_dim));
        }
    });
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
    let key = egui::Id::unique("mailcraft-signature-details");
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
                    let field = crate::text_menu::field(ui.add(field.desired_width(320.0).char_limit(mailcraft_model::signature::MAX_DETAIL)));
                    changed |= field.changed();
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
/// A message that is only `html`, with `pictures` as its own pictures, each at the `cid:` that
/// `cid` names: what the reading pane draws for a signature that is not in a message yet.
pub(crate) fn message_of(
    html: String,
    pictures: &[mailcraft_model::signature::Picture],
    cid: impl Fn(&mailcraft_model::signature::Picture) -> String,
) -> mailcraft_model::Message {
    mailcraft_model::Message {
        body: mailcraft_model::Body { text: String::new(), html: Some(html) },
        attachments: pictures
            .iter()
            .map(|p| mailcraft_model::Attachment {
                content_id: cid(p),
                inline: true,
                data: p.data.clone(),
                ..mailcraft_model::Attachment::new(p.name.clone(), p.mime.clone(), Vec::new())
            })
            .collect(),
        ..mailcraft_model::Message::default()
    }
}

/// Whether `message` holds exactly `pictures` as its own parts, in that order: each one inline,
/// under the `cid:` address this preview binds it at in that message, with that picture's own file
/// name, type and bytes. A message kept for a signature's HTML is that signature's only while this
/// holds of the pictures it was drawn from: the HTML can name a picture whose bytes were replaced,
/// and nothing the address alone reads tells the two apart.
///
/// The bytes are the same allocation in the ordinary case — the kept message holds the signature's
/// own `Arc<Vec<u8>>` — so a frame costs a pointer each; only a part holding the same bytes of
/// another allocation is compared as bytes, and the bytes are never hashed.
pub(crate) fn message_holds_pictures(
    message: &mailcraft_model::Message,
    pictures: &[mailcraft_model::signature::Picture],
    cid: impl Fn(&mailcraft_model::signature::Picture) -> String,
) -> bool {
    message.attachments.len() == pictures.len()
        && message.attachments.iter().zip(pictures).all(|(part, p)| {
            let bytes = std::sync::Arc::ptr_eq(&part.data, &p.data) || part.data == p.data;
            part.inline && part.content_id == cid(p) && part.name == p.name && part.mime == p.mime && bytes
        })
}

fn preview(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let Some(e) = app.session.signature_edit.as_ref() else { return };
    let mut sig = e.signature.clone();
    if sig.set_body(e.body.text().clone()).is_err() {
        return;
    }
    let ctx = ui.ctx().clone();
    let key = egui::Id::unique("mailcraft-signature-preview");
    // The address each of its pictures is bound at in this preview, and the part the kept message
    // must carry for it: the stamp reads the pictures' id, width and description, which the same
    // signature with a picture's bytes replaced does not change.
    let cid = |p: &mailcraft_model::signature::Picture| format!("sig-{}@preview.invalid", p.id);
    let stamp = (
        mailcraft_model::signature::text_hash(sig.body()),
        format!("{:?}", sig.layout),
        sig.pictures().iter().map(|p| (p.id, p.width, p.alt.clone())).collect::<Vec<_>>(),
        sig.html().map(str::to_string),
    );
    let kept: Option<(u64, std::sync::Arc<mailcraft_model::Message>)> = ctx.data(|d| d.get_temp(key));
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        stamp.hash(&mut h);
        h.finish()
    };
    let message = match kept.filter(|(k, m)| *k == hash && message_holds_pictures(m, sig.pictures(), cid)) {
        Some((_, m)) => m,
        None => {
            // Its HTML as a message sends it: cleaned, and each picture the signature knows bound
            // to the part this preview gives it. Both kinds are laid out by the same code, so a
            // known picture address is at a part this message really holds. A `cid:` the signature
            // does not hold keeps the address the HTML gives it — the existing cleaning leaves such
            // a reference alone — and no part is invented here for it.
            let greeting = format!("<p>{}</p>\r\n", tl!("Thanks, Thursday at ten works for me. I'll bring the drawings."));
            // An HTML signature as a message sends it: its style sheets scoped to it in the
            // head (`authored_signature_html`); one whose sheets can't be, as a draft keeps it.
            let authored = sig.html().and_then(|own| {
                let scope = mailcraft_html::AuthoredScope::new("mc-preview").ok()?;
                mailcraft_engine::mime::rich::authored_signature_html(&sig, own, &scope, &cid).ok()
            });
            let html = match authored {
                Some(f) => mailcraft_engine::mime::rich::document_with_head(&f.head, &format!("{greeting}{}", f.body), None),
                None => format!("{greeting}{}", mailcraft_engine::mime::rich::signature_html(&sig, &cid)),
            };
            let m = std::sync::Arc::new(message_of(html, sig.pictures(), cid));
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
        if crate::text_menu::field(ui.add(egui::DragValue::new(&mut width).range(8..=MAX_WIDTH).suffix(" px"))).changed() {
            let _ = app.run("signature.picture.set", json!({"id": id, "width": width}));
        }
        ui.label(tl!("Description"));
        if crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut alt).desired_width(200.0).char_limit(200))).changed() {
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

#[cfg(test)]
#[path = "settings_preview_tests.rs"]
mod settings_preview_tests;

#[cfg(test)]
#[path = "signature_html_import_tests.rs"]
mod html_import_tests;

#[cfg(test)]
#[path = "signature_picture_refusal_tests.rs"]
mod picture_refusal_tests;

/// The open signature as `signature.save` keeps it: with the text it has now, which lets its HTML
/// go when that text is no longer the HTML's (`Signature::set_body`). Save is judged on this.
fn as_saved(e: &mailcraft_engine::SignatureEdit) -> mailcraft_model::Signature {
    let mut sig = e.signature.clone();
    match sig.set_body(e.body.text().clone()) {
        Ok(()) => sig,
        Err(_) => e.signature.clone(),
    }
}

/// Where the window notes that it asked [`ask_leave`]'s question, with where the owner was going
/// (File open or not, and its page).
fn leave_asked() -> egui::Id {
    egui::Id::unique("mailcraft-signature-leave-asked")
}

/// Why the open signature can't be saved for a picture its HTML names, in the owner's language
/// and as `signature.save` refuses it: the engine's own check
/// ([`mailcraft_engine::cmd::signature::sendable_pictures`]). `None` when it isn't HTML, or every
/// picture it names can be sent.
fn picture_refusal(app: &MailApp) -> Option<String> {
    let sig = as_saved(app.session.signature_edit.as_ref()?);
    let why = mailcraft_engine::cmd::signature::sendable_pictures(&sig, sig.html()?).err()?;
    Some(why.in_lang(crate::i18n::current()))
}

/// The page was left with a signature whose save was refused: when that is for its email
/// compatibility errors, or for a picture it can't be sent with, the owner is asked whether to
/// keep editing it (what Escape answers: the window stays on the page with every edit) or to
/// discard the changes, which closes the signature without keeping them and goes on to `to`, where
/// the owner was going (File open or not, and its page). Nothing is asked over another dialog.
pub(crate) fn ask_leave(app: &mut MailApp, to: (bool, String)) {
    let Some(ctx) = app.ctx.clone() else { return };
    let Some(html) = app.session.signature_edit.as_ref().and_then(|e| e.signature.html()) else { return };
    // The findings the editor keeps for this very text: the same answer its Save gave.
    let why = if crate::compat::checked(&ctx, html).blocks_save() {
        tl!("This signature can't be saved until its email compatibility errors are fixed.").to_string()
    } else if let Some(why) = picture_refusal(app) {
        crate::i18n::fmt(tl!("This signature can't be saved: {why}"), &[("why", &why)])
    } else {
        return;
    };
    if app.dialog.is_some() {
        return;
    }
    let mut ask = crate::dialogs::Question::new(tl!("Unsaved Signature"), why, tl!("Discard changes"), json!({}));
    ask.cancel = Some(tl!("Keep editing").to_string());
    app.dialog = Some(crate::dialogs::Dialog::Confirm { command: "signature.close".to_string(), params: json!({"discard": true}), ask });
    ctx.data_mut(|d| d.insert_temp(leave_asked(), to));
}

/// Once [`ask_leave`]'s question is answered: after Discard changes (the signature is closed),
/// the page is left for where the owner was going (another page of File, or out of File); after
/// Keep editing, nothing more.
fn after_leave_question(app: &mut MailApp, ctx: &egui::Context) {
    if app.dialog.is_some() {
        return;
    }
    let Some((backstage, page)) = ctx.data(|d| d.get_temp::<(bool, String)>(leave_asked())) else { return };
    ctx.data_mut(|d| d.remove::<(bool, String)>(leave_asked()));
    if app.session.signature_edit.is_none() {
        let to = if backstage { json!({"value": true, "page": page}) } else { json!({"value": false}) };
        if app.run("ui.backstage", to).is_err() {
            let _ = app.run("ui.backstage", json!({"value": false}));
        }
    }
}
