//! File › Options › Trust Center: the certificates and OpenPGP keys for signed and encrypted mail.
//!
//! The list shows what the mailbox keeps (public keys only), with Trust, Export and Remove. Import
//! takes a certificate, a digital ID with its private key (`.p12`, `.pfx`) or OpenPGP keys
//! (`.asc`); a file under a passphrase asks for it here, and the passphrase goes to the session
//! for that one import (`Session::set_passphrase`), never into a command's params.
//!
//! Private keys go to the operating system's credential store ([`crate::Services::secrets`]) when
//! the owner has sign-ins remembered, and come back from it at the next start; otherwise they are
//! kept for this session only, and the list says so. A key is kept in pieces small enough for
//! every store (Windows Credential Manager takes about 2.5 KB a secret).

use egui::{Sense, Ui, vec2};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, SecretStore, icons};

/// The size of one piece of a private key in the credential store, in characters.
const PIECE: usize = 1000;
/// Most pieces read back for one key.
const MAX_PIECES: usize = 64;

/// A key file waiting for its passphrase.
pub(crate) struct PendingImport {
    /// What `security.import` is given: `{"path": …}` or `{"name": …, "data": …}`.
    source: Value,
    name: String,
    /// What the owner types; wiped when dropped.
    passphrase: Zeroizing<String>,
    wrong: bool,
}

/// Import a key file the owner picked (desktop: a path; browser: its bytes).
pub(crate) fn import(app: &mut MailApp, source: Value) {
    let name = source.get("name").or(source.get("path")).and_then(Value::as_str).unwrap_or("").rsplit(['/', '\\']).next().unwrap_or("").to_string();
    match app.run("security.import", source.clone()) {
        Ok(r) => imported(app, &r),
        Err(e) if e.contains("passphrase") => {
            app.key_import = Some(PendingImport { source, name, passphrase: Zeroizing::new(String::new()), wrong: false })
        }
        Err(e) => app.status(e),
    }
}

fn imported(app: &mut MailApp, r: &Value) {
    let n = r.get("imported").and_then(Value::as_array).map_or(0, Vec::len);
    let private = r.get("privateKeys").and_then(Value::as_u64).unwrap_or(0);
    let text = if private > 0 {
        crate::i18n::fmt(tl!("Imported {n} certificate(s) or key(s), with your private key"), &[("n", &n.to_string())])
    } else {
        crate::i18n::fmt(tl!("Imported {n} certificate(s) or key(s)"), &[("n", &n.to_string())])
    };
    app.status(text);
    keep_secrets(app);
}

/// Hand newly imported private keys to the credential store, and drop removed ones from it
/// (every frame; nothing to do most of the time).
pub(crate) fn keep_secrets(app: &mut MailApp) {
    let remember = app.session.take_secrets_to_remember();
    let forget = app.session.take_secrets_to_forget();
    let Some(store) = app.services.secrets.as_deref() else { return };
    if app.ui.remember_sign_in {
        for (name, text) in remember {
            if let Err(e) = write(store, &name, &text) {
                log::warn!("couldn't keep a private key in the credential store: {e}");
            }
            // Read back at the next start, not again in this one.
            app.keys_tried.insert(name);
        }
    }
    for name in forget {
        delete(store, &name);
    }
}

/// Take back the private keys the credential store keeps for the owner's keys (each tried once
/// a run).
pub(crate) fn restore_secrets(app: &mut MailApp) {
    if !app.ui.remember_sign_in {
        return;
    }
    let Some(store) = app.services.secrets.as_deref() else { return };
    for name in app.session.secret_names() {
        if !app.keys_tried.insert(name.clone()) {
            continue;
        }
        if let Some(text) = read(store, &name)
            && !app.session.restore_secret(&text)
        {
            log::warn!("a private key in the credential store couldn't be read");
        }
    }
}

/// Forget every private key the store keeps for the owner's keys (remembering was switched off).
pub(crate) fn forget_all(app: &mut MailApp) {
    let Some(store) = app.services.secrets.as_deref() else { return };
    for name in app.session.secret_names() {
        delete(store, &name);
    }
}

fn write(store: &dyn SecretStore, name: &str, text: &str) -> Result<(), String> {
    let pieces: Vec<String> = text.chars().collect::<Vec<_>>().chunks(PIECE).map(|c| c.iter().collect()).collect();
    if pieces.len() > MAX_PIECES {
        return Err("the key is too large to keep".into());
    }
    for (i, piece) in pieces.iter().enumerate() {
        store.set(&format!("{name}#{i}"), piece)?;
    }
    store.set(&format!("{name}#n"), &pieces.len().to_string())
}

fn read(store: &dyn SecretStore, name: &str) -> Option<Zeroizing<String>> {
    let n: usize = store.get(&format!("{name}#n"))?.trim().parse().ok().filter(|n| *n > 0 && *n <= MAX_PIECES)?;
    let mut text = Zeroizing::new(String::new());
    for i in 0..n {
        text.push_str(&store.get(&format!("{name}#{i}"))?);
    }
    Some(text)
}

fn delete(store: &dyn SecretStore, name: &str) {
    let n: usize = store.get(&format!("{name}#n")).and_then(|n| n.trim().parse().ok()).unwrap_or(0).min(MAX_PIECES);
    for i in 0..n {
        store.delete(&format!("{name}#{i}"));
    }
    store.delete(&format!("{name}#n"));
}

/// The Trust Center's part of File › Options.
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let prefs = app.session.prefs.clone();
    let set = |app: &mut MailApp, params: Value| {
        let _ = app.run("security.options", params);
    };
    let mut collect = prefs.collect_keys;
    if ui
        .checkbox(&mut collect, tl!("Keep the certificates and keys that signed messages bring"))
        .on_hover_text(tl!("So that mail to their senders can be encrypted. Only public keys are kept."))
        .changed()
    {
        set(app, json!({"collectKeys": collect}));
    }
    let mut sign = prefs.sign_by_default;
    if ui.checkbox(&mut sign, tl!("Sign new messages")).changed() {
        set(app, json!({"signByDefault": sign}));
    }
    let mut encrypt = prefs.encrypt_by_default;
    if ui.checkbox(&mut encrypt, tl!("Encrypt new messages")).changed() {
        set(app, json!({"encryptByDefault": encrypt}));
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(tl!("Sign and encrypt with")).font(regular(12.5)).color(t.text));
        let label = |code: &str| match code {
            "smime" => tl!("S/MIME"),
            "openpgp" => tl!("OpenPGP"),
            _ => tl!("Whichever fits (S/MIME first)"),
        };
        egui::ComboBox::from_id_salt("opt_security_format").selected_text(label(&prefs.security_format)).show_ui(ui, |ui| {
            for code in ["auto", "smime", "openpgp"] {
                let current = if prefs.security_format.is_empty() { "auto" } else { prefs.security_format.as_str() };
                if ui.selectable_label(code == current, label(code)).clicked() {
                    set(app, json!({"format": code}));
                }
            }
        });
    });
    ui.add_space(6.0);
    passphrase_prompt(app, ui, t);
    ui.horizontal(|ui| {
        if ui
            .button(tl!("Import…"))
            .on_hover_text(tl!("A certificate (.cer, .crt, .pem), your digital ID with its private key (.p12, .pfx) or OpenPGP keys (.asc)"))
            .clicked()
        {
            crate::files::open(app, "key");
        }
    });
    if app.services.secrets.is_none() || !app.ui.remember_sign_in {
        ui.weak(tl!("Private keys you import are kept for this session only: import them again next time, or have sign-ins remembered (above) where a credential store is available."));
    }
    ui.add_space(4.0);
    let keys = app.session.mailbox.keys.clone();
    if keys.is_empty() {
        ui.weak(tl!(
            "No certificates or keys yet. Import your own to sign and to read encrypted mail; your correspondents' come with their signed messages."
        ));
        return;
    }
    let now = app.session.now();
    for k in keys {
        egui::Frame::NONE.stroke(egui::Stroke::new(1.0, t.border)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_min_width(ui.available_width().min(720.0));
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                icons::paint(ui.painter(), r, if k.own { "seal" } else { "lock" }, t.icon, t.accent);
                let name = if k.name.is_empty() { k.emails.first().cloned().unwrap_or_default() } else { k.name.clone() };
                ui.label(egui::RichText::new(name).font(semibold(12.5)).color(t.text));
                ui.label(egui::RichText::new(k.scheme.label()).font(regular(11.0)).color(t.text_dim));
                let mut tags = Vec::new();
                if k.own {
                    tags.push(if app.session.security_has_secret(k.scheme, &k.fingerprint) { tl!("Yours") } else { tl!("Yours (private key not here)") });
                }
                if k.trusted {
                    tags.push(tl!("Trusted"));
                }
                if k.is_ca {
                    tags.push(tl!("Certificate authority"));
                }
                if k.collected {
                    tags.push(tl!("From a signed message"));
                }
                if !k.in_force(now) {
                    tags.push(tl!("Expired"));
                }
                if !tags.is_empty() {
                    ui.label(egui::RichText::new(tags.join(" · ")).font(regular(11.0)).color(t.accent_text));
                }
            });
            let mut detail = k.emails.join(", ");
            if let Some(until) = k.not_after {
                let d = app.session.local(until).date;
                detail.push_str(&format!("{}{} {}", if detail.is_empty() { "" } else { " · " }, tl!("until"), crate::i18n::date_long(d)));
            }
            if !k.issuer.is_empty() && k.issuer != k.name {
                detail.push_str(&format!(" · {} {}", tl!("issued by"), k.issuer));
            }
            ui.label(egui::RichText::new(detail).font(regular(11.5)).color(t.text_dim));
            ui.label(egui::RichText::new(k.grouped_fingerprint()).font(egui::FontId::monospace(10.5)).color(t.text_dim));
            ui.horizontal(|ui| {
                let trust_label = if k.trusted { tl!("Don't Trust") } else { tl!("Trust") };
                if ui.small_button(trust_label).on_hover_text(tl!("A trusted root verifies everybody it issued a certificate to; a trusted OpenPGP key verifies its holder's signatures. Trust only what you checked.")).clicked() {
                    let _ = app.run("security.trust", json!({"fingerprint": k.fingerprint}));
                }
                if ui.small_button(tl!("Export…")).on_hover_text(tl!("Save the public certificate or key, to give to others. The private key never leaves.")).clicked() {
                    let _ = app.run("security.export", json!({"fingerprint": k.fingerprint}));
                }
                if ui.small_button(tl!("Remove")).clicked() {
                    let _ = app.run("security.remove", json!({"fingerprint": k.fingerprint}));
                    keep_secrets(app);
                }
            });
        });
        ui.add_space(4.0);
    }
}

fn passphrase_prompt(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let Some(pending) = app.key_import.as_mut() else { return };
    let mut go = false;
    let mut cancel = false;
    egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
        let ask = crate::i18n::fmt(tl!("\"{name}\" is protected by a passphrase."), &[("name", &pending.name)]);
        ui.label(egui::RichText::new(ask).font(regular(12.5)).color(t.text));
        if pending.wrong {
            ui.label(egui::RichText::new(tl!("That passphrase isn't the file's.")).font(regular(12.0)).color(t.red));
        }
        ui.horizontal(|ui| {
            let field = ui.add(egui::TextEdit::singleline(&mut *pending.passphrase).password(true).hint_text(tl!("Passphrase")).desired_width(240.0));
            go = ui.button(tl!("Import")).clicked() || (field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button(tl!("Cancel")).clicked();
        });
    });
    if cancel {
        app.key_import = None;
        return;
    }
    if !go {
        return;
    }
    let Some(pending) = app.key_import.take() else { return };
    app.session.set_passphrase(&pending.passphrase);
    match app.run("security.import", pending.source.clone()) {
        Ok(r) => imported(app, &r),
        Err(e) if e.contains("passphrase") => {
            app.key_import = Some(PendingImport { wrong: true, passphrase: Zeroizing::new(String::new()), ..pending })
        }
        Err(e) => app.status(e),
    }
}
