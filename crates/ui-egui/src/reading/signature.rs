// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's **Digital signature** where it is read (#30): one bar above it with what the
//! check found (verified · signer not verified · changed after signing · can't be checked), its
//! seal, and Details that open beneath it (who signed, the certificate's fingerprint, issuer and
//! dates, and that revocation wasn't checked); and the seal on the message's row in the list.
//!
//! What the check finds is the engine's ([`mailcraft_engine::Session::digital_signature`]):
//! this only shows it. The signer's name and addresses come from the certificate in the message:
//! data, shown cut short, never as ours.

use egui::{Color32, Sense, Ui, vec2};
use mailcraft_model::Message;
use mailcraft_model::digital_signature::{Coverage, DigitalSignature, SignatureFormat, Verdict, Why};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::json;

use super::Place;
use super::remote::notice;
use super::warnings::short;
use crate::theme::{Tokens, regular, semibold};
use crate::{MailApp, icons};

/// The seal a result wears.
fn seal(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Verified => "sealVerified",
        Verdict::NotVerified => "sealNotVerified",
        Verdict::Changed => "sealChanged",
        Verdict::Unchecked => "sealUnchecked",
    }
}

/// What a result is called, in a few words, for a tip and a screen reader.
fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Verified => tl!("Digitally signed and verified"),
        Verdict::NotVerified => tl!("Digitally signed, signer not verified"),
        Verdict::Changed => tl!("Changed after it was signed"),
        Verdict::Unchecked => tl!("Digital signature can't be checked"),
    }
}

/// Why a result is less than verified, in plain words.
fn why_said(why: Why) -> &'static str {
    match why {
        Why::UnknownIssuer => tl!("The certificate wasn't issued by a certification authority this computer trusts."),
        Why::SelfSigned => tl!("The certificate signed itself, and this computer doesn't trust it."),
        Why::Expired => tl!("The certificate is out of date."),
        Why::NotForEmail => tl!("The certificate isn't for signing mail."),
        Why::AddressMismatch => tl!("The certificate is for another address than the sender's."),
        Why::Constrained => tl!("An authority on the way to a trusted one doesn't allow the certificate to be used this way."),
        Why::NoAuthorities => tl!("No certification authorities for mail could be read on this computer, so no signer can be verified here."),
        Why::AuthoritiesNotRead => {
            tl!("This computer's certification authorities haven't been read yet: the signature is checked again when they are.")
        }
        Why::KeyNotVerified => tl!("The signer's key hasn't been marked verified."),
        Why::PartlySigned => {
            tl!("Only one part of this message was signed, and what's shown here holds more than that part: what's outside it could say anything.")
        }
        Why::Altered => tl!("What you see isn't what was signed: treat it with care."),
        Why::WeakAlgorithm => tl!("It uses an algorithm that's no longer safe."),
        Why::UnknownAlgorithm => tl!("It uses an algorithm MailCraft doesn't know."),
        Why::NoPublicKey => tl!("The signer's Public key (certificate) isn't in the message or kept on this computer."),
        Why::KeyRevoked => tl!("The signer's key was revoked by its owner."),
        Why::KeyExpired => tl!("The signer's key had expired when the message was signed."),
        Why::NotASigningKey => tl!("The key that made it isn't one that may sign."),
        Why::SignatureExpired => tl!("The Digital signature has expired."),
        Why::Unreadable => tl!("The Digital signature can't be read."),
        Why::NoOriginal => tl!("The message was downloaded before MailCraft kept the bytes it came as. Check Again downloads them."),
        Why::TooMuch => tl!("There's more in this Digital signature than MailCraft checks for one message."),
    }
}

/// The bar's lead and its hue for `d`.
fn lead(d: &DigitalSignature, t: &Tokens) -> (String, Color32) {
    let signer = d.signer.as_ref().map(|s| short(s.addresses.first().map_or(s.name.as_str(), String::as_str))).unwrap_or_default();
    let fill = |template: &str| crate::i18n::fmt(template, &[("signer", &signer)]);
    match d.verdict {
        Verdict::Verified => (fill(tl!("Digitally signed by {signer}, and verified.")), t.green),
        // (Never verified for the message: only its part is.)
        Verdict::NotVerified if d.why == Some(Why::PartlySigned) => {
            (fill(tl!("Part of this message is Digitally signed by {signer}, and verified; the rest isn't signed.")), t.orange)
        }
        Verdict::NotVerified if signer.is_empty() => (tl!("Digitally signed, but the signer isn't verified.").to_string(), t.orange),
        Verdict::NotVerified => (fill(tl!("Digitally signed by {signer}, but the signer isn't verified.")), t.orange),
        Verdict::Changed => (tl!("This message was changed after it was signed.").to_string(), t.red),
        Verdict::Unchecked => (tl!("This message's Digital signature can't be checked.").to_string(), t.text_dim),
    }
}

fn open_key(id: u64) -> egui::Id {
    egui::Id::unique(("digitalSignatureDetails", id))
}

/// The bar above `m` when it is Digitally signed: what the check found, and Details beneath.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Message, place: Place, t: &Tokens) {
    if m.id == 0 {
        return;
    }
    let Some(d) = app.session.digital_signature(m.id) else { return };
    let (lead, hue) = lead(&d, t);
    let open = ui.data(|data| data.get_temp::<bool>(open_key(m.id))).unwrap_or(false);
    let mut toggle = false;
    let mut check_again = false;
    let mut verify = false;
    let can_check_again = d.why == Some(Why::NoOriginal) && app.session.enabled("mail.checkAgain");
    // An OpenPGP key not marked Verified, kept for the sender: its fingerprint can be compared
    // and marked, in the window only (#30 R6).
    let to_verify = verifiable(app, &d, m);
    let shown = notice(hue, t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.horizontal(|ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            icons::paint(ui.painter(), r, seal(d.verdict), t.icon, t.accent);
            let name = verdict_name(d.verdict);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, name));
            resp.on_hover_text(name);
            ui.add(egui::Label::new(egui::RichText::new(&lead).font(semibold(11.5)).color(t.text)).wrap());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if open { tl!("Hide Details") } else { tl!("Details") };
                let resp = ui.add(egui::Button::new(egui::RichText::new(label).font(regular(11.0)).color(t.text)).small().frame(false));
                toggle = resp.on_hover_text(tl!("Who signed this message, and the certificate")).clicked();
                if can_check_again {
                    let resp = ui.add(egui::Button::new(egui::RichText::new(tl!("Check Again")).font(regular(11.0)).color(t.text)).small());
                    check_again = resp.on_hover_text(tl!("Download the bytes this message came as again, to check its Digital signature")).clicked();
                }
                if to_verify.is_some() {
                    let resp = ui.add(egui::Button::new(egui::RichText::new(tl!("Verify…")).font(regular(11.0)).color(t.text)).small());
                    if place == Place::Pane {
                        app.hit.add("verifySigner", m.id, resp.rect);
                    }
                    // (Not for a click the control channel injected: the dialog is the owner's.)
                    verify = resp.on_hover_text(tl!("Compare the signer's key's fingerprint with theirs, and mark it as verified")).clicked()
                        && !app.driven;
                }
            });
        });
        // (A part signed is said whatever else is: what's outside it is nobody's word.)
        let partly = (d.coverage == Coverage::Part && d.why != Some(Why::PartlySigned) && d.signer.is_some()).then_some(Why::PartlySigned);
        for why in d.why.into_iter().chain(partly) {
            indented(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(why_said(why)).font(regular(11.5)).color(t.text_dim)).wrap());
            });
        }
        if open {
            details(app, ui, &d, m, t);
        }
    });
    if place == Place::Pane {
        app.hit.add("digitalSignature", m.id, shown.response.rect);
    }
    if toggle {
        ui.data_mut(|data| data.insert_temp(open_key(m.id), !open));
    }
    if check_again {
        let _ = app.run("mail.checkAgain", json!({"id": m.id}));
    }
    if let (true, Some((address, fingerprint))) = (verify, to_verify) {
        let _ = app.run("ui.verifyKey", json!({"address": address, "fingerprint": fingerprint}));
    }
}

/// The From address and the signer's fingerprint, when `d` is an OpenPGP Digital signature whose
/// key isn't Verified and is kept for that address: what Verify… opens the dialog for.
fn verifiable(app: &MailApp, d: &DigitalSignature, m: &Message) -> Option<(String, String)> {
    if d.format != SignatureFormat::OpenPgp || d.why != Some(Why::KeyNotVerified) {
        return None;
    }
    let fingerprint = d.signer.as_ref().map(|s| s.fingerprint.clone()).filter(|f| !f.is_empty())?;
    let address = m.from.email.trim().to_lowercase();
    let digits = |f: &str| f.chars().filter(char::is_ascii_hexdigit).map(|c| c.to_ascii_uppercase()).collect::<String>();
    let kept = app.session.mailbox.public_keys.of(&address).iter().any(|k| digits(&k.fingerprint) == digits(&fingerprint));
    kept.then_some((address, fingerprint))
}

fn indented(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE.inner_margin(egui::Margin { left: 22, right: 0, top: 0, bottom: 0 }).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
            add(ui);
        });
    });
}

/// A date for people, in the zone the mailbox is shown in.
fn day(app: &MailApp, ts: Timestamp) -> String {
    crate::dates::medium(DateTime::from_timestamp(ts, app.session.offset_at(ts)).date)
}

/// The lines of Details: who signed, the certificate, that revocation wasn't checked, and the
/// signature part itself (it isn't among the attachments).
fn details(app: &mut MailApp, ui: &mut Ui, d: &DigitalSignature, m: &Message, t: &Tokens) {
    let line = |ui: &mut Ui, label: &str, value: &str| {
        indented(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(label).font(semibold(11.0)).color(t.text)));
            ui.add(egui::Label::new(egui::RichText::new(value).font(regular(11.0)).color(t.text_dim)).wrap());
        });
    };
    if let Some(s) = &d.signer {
        let addresses: Vec<String> = s.addresses.iter().take(4).map(|a| short(a)).collect();
        let who = match (s.name.is_empty(), addresses.is_empty()) {
            (false, false) => format!("{} <{}>", short(&s.name), addresses.join(", ")),
            (true, false) => addresses.join(", "),
            _ => short(&s.name),
        };
        line(ui, tl!("Signed by:"), &who);
        if !s.issuer.is_empty() {
            line(ui, tl!("Issued by:"), &short(&s.issuer));
        }
        let openpgp = d.format == SignatureFormat::OpenPgp;
        if openpgp && s.not_after == 0 {
            // (What is known of an OpenPGP key is when it was made, not a range of validity.)
            if s.not_before != 0 {
                line(ui, tl!("Key made:"), &day(app, s.not_before));
            }
        } else if s.not_before != 0 || s.not_after != 0 {
            let valid = crate::i18n::fmt(tl!("{from} to {until}"), &[("from", &day(app, s.not_before)), ("until", &day(app, s.not_after))]);
            line(ui, tl!("Valid:"), &valid);
        }
        // (A fingerprint is long and has no spaces: it wraps where the pane needs it to. An
        // OpenPGP key's isn't a SHA-256.)
        line(ui, if openpgp { tl!("Fingerprint:") } else { tl!("Fingerprint (SHA-256):") }, &s.fingerprint);
    }
    if let Some(at) = d.signed_at {
        let when = crate::dates::header_label(at, app.session.offset_at(at), app.session.prefs.twelve_hour);
        line(ui, tl!("Signed at:"), &format!("{when} {}", tl!("(as the signer says)")));
    }
    if !d.revocation_checked {
        line(ui, tl!("Revocation:"), tl!("Revocation not checked."));
    }
    let file =
        app.session.digital_signature_part(m.id).and_then(|i| m.attachments.get(i).map(|a| (i, mailcraft_engine::opening::file_name(&a.name))));
    if let Some((index, name)) = file
        && m.id != 0
    {
        indented(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new(tl!("Signature file:")).font(semibold(11.0)).color(t.text)));
            let resp = ui.add(egui::Button::new(egui::RichText::new(&name).font(regular(11.0)).color(t.text)).small());
            app.hit.add("signatureFile", m.id, resp.rect);
            let resp = resp.on_hover_text(tl!("Open or save the Digital signature"));
            egui::Popup::menu(&resp).show(|ui| super::attachment_menu(ui, app, m.id, index));
        });
    }
}

/// The seal on `m`'s row in the list, when its Digital signature was checked: worked out for a
/// row on screen whose message keeps the bytes it came as, one row a frame (a check may take a
/// while: the list never waits for many), the rest in the frames after.
pub(crate) fn row_seal(app: &mut MailApp, m: &Message) -> Option<&'static str> {
    if let Some(d) = app.session.digital_signature_known(m.id) {
        return Some(seal(d.verdict));
    }
    if !m.has_original() || app.session.digital_signature_checked(m.id) {
        return None;
    }
    if app.row_checks_left == 0 {
        app.row_checks_wanted = true;
        return None;
    }
    app.row_checks_left -= 1;
    app.session.digital_signature(m.id).map(|d| seal(d.verdict))
}
