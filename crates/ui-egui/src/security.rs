// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › Security › Digital signatures and encryption (#30 R12): the owner's own
//! certificates and keys (what the host finds in the system's places and what was imported
//! here), the Public keys kept for other people, and the two dialogs only the window has: the
//! import of a private key, and marking an OpenPGP key Verified after comparing its fingerprint.
//!
//! - **Private keys never pass through a command.** `ui.importPrivateKey` only opens the file
//!   picker; the file is read in the window and the passphrase typed into the import dialog's
//!   own field, and both go to the host ([`crate::Services::own_keys`]) in a job, never in a
//!   command's params, `ui.json`, a log or an error. The passphrase is held in a
//!   `Zeroizing<String>` while the dialog is open and moved into the job when Import is pressed.
//! - **Verified is the owner's word, in this window alone (R6, R23).** `ui.verifyKey` opens a
//!   dialog with the key's whole fingerprint; `ui.markKeyVerified` does anything only while that
//!   very dialog (the same address and fingerprint) is open, and its buttons are what run it.
//!   Neither is a command an agent may run ([`mailcraft_engine::agent`]), and neither the
//!   buttons that lead to it (Compare fingerprint…, Verify…), its own, nor Remove and its Confirm
//!   for an imported key, answer input the control channel injected (`MailApp::driven`, as the
//!   password field's eye): an agent allowed to click can't mark a key or delete one.
//! - The browser has no host for private keys: the page says signing and decrypting need the
//!   desktop program, and the Public keys are listed and imported as everywhere.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::{Color32, RichText, Ui, vec2};
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::public_keys::KeySource;
use mailcraft_model::{ContactKey, KeyKind};
use mailcraft_time::{DateTime, Timestamp};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::MailApp;
use crate::dialogs::{Dialog, Question, button_row, window};
use crate::files::base64_lite::encode;
use crate::own_keys::{Gnupg, Imported, ImportedKey, KeyFile, KeyPlace, Listing, OwnKey, OwnKeysAnswer, OwnKeysJob, Passphrase, SmimePlace};
use crate::theme::{Tokens, semibold};
use crate::widgets::primary_button;

/// What a file picker is opened for: a `.p12` to import, an OpenPGP secret key to import, a
/// file of Public keys to keep, a key for the contact card being edited.
pub(crate) const PURPOSE_SMIME: &str = "privateKeySmime";
pub(crate) const PURPOSE_OPENPGP: &str = "privateKeyOpenpgp";
pub(crate) const PURPOSE_PUBLIC: &str = "publicKey";
pub(crate) const PURPOSE_CARD: &str = "cardKey";

/// Whether a picker was opened for one of these.
pub(crate) fn is_purpose(purpose: &str) -> bool {
    matches!(purpose, PURPOSE_SMIME | PURPOSE_OPENPGP | PURPOSE_PUBLIC | PURPOSE_CARD)
}

/// How many Public keys the list shows at a time.
const PAGE: usize = 20;

/// What the window knows of the owner's keys, and the host's work under way.
#[derive(Default)]
pub struct State {
    /// The host's last listing.
    pub(crate) listing: Option<Listing>,
    listing_rx: Option<Receiver<OwnKeysAnswer>>,
    gnupg_rx: Option<Receiver<OwnKeysAnswer>>,
    import_rx: Option<Receiver<OwnKeysAnswer>>,
    /// GnuPG's valid keys were asked for at start.
    started: bool,
    /// When GnuPG's valid keys were last asked for, and last read (the mailbox's clock).
    gnupg_asked: Option<Timestamp>,
    pub(crate) gnupg_read: Option<Timestamp>,
    /// Whether the window had the focus at the last frame: GnuPG is asked again when it comes
    /// back (the owner may have changed a trust there meanwhile).
    focused: bool,
    /// The contact form's Add from file… pick under way: the one form a key picked goes to.
    pub(crate) card_pick: Option<u64>,
    /// The last such pick's number.
    picks: u64,
    /// What the last import, removal or refresh came to: whether it worked, and the words.
    pub(crate) said: Option<(bool, String)>,
}

/// How long after GnuPG was asked the window's coming back asks it again, in seconds.
const GNUPG_AGAIN: i64 = 60;

/// The dialogs of this page.
pub enum SecurityDialog {
    Import(ImportDialog),
    Verify(VerifyDialog),
}

impl SecurityDialog {
    pub fn name(&self) -> &'static str {
        match self {
            SecurityDialog::Import(_) => "importPrivateKey",
            SecurityDialog::Verify(_) => "verifyKey",
        }
    }
}

/// Import a private key: the file picked, and for a `.p12` its passphrase.
pub struct ImportDialog {
    pub format: SignatureFormat,
    /// The file's name, as the picker gave it.
    pub name: String,
    file: KeyFile,
    passphrase: Zeroizing<String>,
    shown: bool,
    pub error: String,
    /// Handed to the host; waiting for its answer.
    pub busy: bool,
}

/// Mark an OpenPGP key Verified, or take that back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyDialog {
    pub address: String,
    /// As it is kept.
    pub fingerprint: String,
    /// Whether the owner marked it Verified already.
    pub verified: bool,
}

/// `text`'s hexadecimal digits, in upper case: how two fingerprints are compared.
fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_hexdigit).map(|c| c.to_ascii_uppercase()).collect()
}

/// A fingerprint in groups of four, as people read it out to each other.
pub(crate) fn in_fours(fingerprint: &str) -> String {
    let d = digits(fingerprint);
    d.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

/// The end of a fingerprint, for a list: the last sixteen digits.
fn short(fingerprint: &str) -> String {
    let d = digits(fingerprint);
    let tail = d.get(d.len().saturating_sub(16)..).unwrap_or(&d);
    format!("…{}", in_fours(tail))
}

/// A date for people, in the zone the mailbox is shown in.
fn day(app: &MailApp, ts: Timestamp) -> String {
    crate::dates::medium(DateTime::from_timestamp(ts, app.session.offset_at(ts)).date)
}

fn format_name(format: SignatureFormat) -> &'static str {
    match format {
        SignatureFormat::Smime => "S/MIME",
        SignatureFormat::OpenPgp => "OpenPGP",
    }
}

/// What a job's channel holds this frame.
enum Taken {
    /// Nothing yet: the job is under way.
    Waiting,
    Answer(OwnKeysAnswer),
    /// The host let go of the job without answering (its thread ended): it is over.
    Gone,
}

fn take(rx: &mut Option<Receiver<OwnKeysAnswer>>) -> Taken {
    let Some(r) = rx.as_ref() else { return Taken::Waiting };
    let taken = match r.try_recv() {
        Ok(answer) => Taken::Answer(answer),
        Err(TryRecvError::Empty) => return Taken::Waiting,
        Err(TryRecvError::Disconnected) => Taken::Gone,
    };
    *rx = None;
    taken
}

/// What a job the host let go of is told.
fn host_gone() -> String {
    tl!("MailCraft's search for your keys stopped without an answer. Try again.").to_string()
}

/// Ask the host for the owner's keys.
fn request_list(app: &mut MailApp) {
    if app.security.listing_rx.is_some() {
        return;
    }
    if let Some(host) = &app.services.own_keys {
        app.security.listing_rx = Some(host(OwnKeysJob::List));
    }
}

/// Ask the host which keys the owner's GnuPG trusts fully.
fn request_gnupg_valid(app: &mut MailApp) {
    if app.security.gnupg_rx.is_some() {
        return;
    }
    if let Some(host) = &app.services.own_keys {
        app.security.gnupg_rx = Some(host(OwnKeysJob::GnupgValid));
        app.security.gnupg_asked = Some(app.session.now());
    }
}

/// Each frame: GnuPG's valid keys once the window is up (R6), and again when the window comes
/// back to the front (at most once a minute: the owner may have changed a trust in GnuPG
/// meanwhile), and what the host answered. `focused`: whether the window has the focus.
pub(crate) fn tick(app: &mut MailApp, focused: bool) {
    let regained = focused && !app.security.focused;
    app.security.focused = focused;
    if !app.data_removed && app.services.own_keys.is_some() {
        let now = app.session.now();
        let due = app.security.gnupg_asked.is_none_or(|at| now.saturating_sub(at) >= GNUPG_AGAIN);
        if !app.security.started || (regained && due) {
            app.security.started = true;
            request_gnupg_valid(app);
        }
    }
    match take(&mut app.security.listing_rx) {
        Taken::Answer(OwnKeysAnswer::Listed(listing)) => app.security.listing = Some(listing),
        // (A listing, so that the page shows the trouble and doesn't ask again every frame.)
        Taken::Gone => {
            let mut listing = app.security.listing.take().unwrap_or_default();
            listing.trouble.retain(|t| *t != host_gone());
            listing.trouble.push(host_gone());
            app.security.listing = Some(listing);
        }
        _ => {}
    }
    match take(&mut app.security.gnupg_rx) {
        Taken::Answer(OwnKeysAnswer::GnupgValid(Ok(pairs))) => {
            app.session.set_gnupg_valid(pairs);
            app.security.gnupg_read = Some(app.session.now());
        }
        Taken::Answer(OwnKeysAnswer::GnupgValid(Err(why))) => log::info!("GnuPG's valid keys couldn't be read: {why}"),
        Taken::Gone => log::info!("GnuPG's valid keys weren't answered"),
        _ => {}
    }
    match take(&mut app.security.import_rx) {
        Taken::Answer(OwnKeysAnswer::Imported(result)) => imported(app, result),
        Taken::Gone => imported(app, Err(tl!("The import stopped without an answer. Try again.").to_string())),
        _ => {}
    }
}

/// What an import came to.
fn imported(app: &mut MailApp, result: Result<Imported, String>) {
    let dialog_busy = matches!(&app.dialog, Some(Dialog::Security(d)) if matches!(&**d, SecurityDialog::Import(i) if i.busy));
    match result {
        Ok(Imported::System) => {
            if dialog_busy {
                app.dialog = None;
            }
            let said = tl!("Imported: the system keeps it, and it is listed with your certificates.").to_string();
            app.status(said.clone());
            app.security.said = Some((true, said));
            request_list(app);
        }
        Ok(Imported::Kept { key, public }) => {
            if dialog_busy {
                app.dialog = None;
            }
            let Some(key) = key.sane() else {
                app.security.said = Some((false, tl!("The import gave back something that isn't a key's name in the credential store.").to_string()));
                return;
            };
            let again = app.ui.imported_keys.iter().any(|k| k.store_key == key.store_key);
            if !again && app.ui.imported_keys.len() >= crate::own_keys::MAX_IMPORTED {
                // (Full since the import started: what the host kept goes again, so that nothing
                // stays in the credential store that the page doesn't list.)
                if let Some(store) = &app.services.secrets {
                    store.delete(&key.store_key);
                }
                app.security.said = Some((false, full()));
                return;
            }
            if let Some(public) = public {
                own_public_key(app, &key, &public);
            }
            app.ui.imported_keys.retain(|k| k.store_key != key.store_key);
            app.ui.imported_keys.push(key);
            let said = tl!("Imported: MailCraft keeps it in the system's credential store, still protected by its passphrase.").to_string();
            app.status(said.clone());
            app.security.said = Some((true, said));
        }
        Err(why) => {
            // (The rules' words are the engine's, in the catalogue; the host's own may not be.)
            let why = crate::i18n::t(&why).to_string();
            if let Some(Dialog::Security(d)) = &mut app.dialog
                && let SecurityDialog::Import(i) = &mut **d
                && i.busy
            {
                i.busy = false;
                i.error = why;
                return;
            }
            app.security.said = Some((false, why));
        }
    }
}

/// What an import is told when the page lists as many imported keys as it keeps.
fn full() -> String {
    crate::i18n::fmt(
        tl!("MailCraft keeps at most {n} keys imported here, and has that many: remove one you don't use, then import this one."),
        &[("n", &crate::own_keys::MAX_IMPORTED.to_string())],
    )
}

/// The public half of the owner's own OpenPGP key, kept for its addresses so that the owner's own
/// Digital signatures check, and Verified for those of its addresses that are the owner's
/// accounts' (the owner holds its private key; a User ID naming anyone else is only that key's
/// word, and stays not Verified).
fn own_public_key(app: &mut MailApp, key: &ImportedKey, public: &[u8]) {
    if let Err(e) = app.run("keys.import", json!({"name": "own key", "data": encode(public)})) {
        log::info!("the public half of an imported key wasn't kept: {e}");
        return;
    }
    let own: Vec<String> = app.session.mailbox.accounts.iter().map(|a| a.email.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    for address in key.addresses.iter().filter(|a| own.contains(&a.trim().to_lowercase())) {
        let _ = app.session.set_key_verified(address, &key.fingerprint, true);
    }
}

/// The window's own commands of this page; `None` for any other id.
pub(crate) fn command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let s = |k: &str| p.get(k).and_then(Value::as_str);
    Some(match id {
        "ui.importPrivateKey" => {
            let purpose = match s("format") {
                Some("smime") => PURPOSE_SMIME,
                Some("openpgp") => PURPOSE_OPENPGP,
                _ => return Some(Err(tl!("`format` is \"smime\" or \"openpgp\"").to_string())),
            };
            if app.services.own_keys.is_none() {
                return Some(Err(no_host().to_string()));
            }
            if app.security.import_rx.is_some() {
                return Some(Err(tl!("An import is under way").to_string()));
            }
            // (Before anything is kept, so that nothing is kept that the page couldn't list.)
            if app.ui.imported_keys.len() >= crate::own_keys::MAX_IMPORTED {
                let said = full();
                app.security.said = Some((false, said.clone()));
                return Some(Err(said));
            }
            crate::files::open(app, purpose);
            Ok(json!({"pending": purpose}))
        }
        "ui.importPublicKey" => {
            crate::files::open(app, PURPOSE_PUBLIC);
            Ok(json!({"pending": PURPOSE_PUBLIC}))
        }
        "ui.refreshOwnKeys" => {
            if app.services.own_keys.is_none() {
                return Some(Err(no_host().to_string()));
            }
            request_list(app);
            request_gnupg_valid(app);
            Ok(json!({"pending": "ownKeys"}))
        }
        // A script's request (the control channel's `execute`) never removes the owner's key, it
        // only asks; nor ever marks a key Verified, whatever dialog is open: those are the owner's
        // press alone ([`owners_press`], through `MailApp::run`).
        "ui.removePrivateKey" if p.get("confirmed").and_then(Value::as_bool) == Some(true) => {
            Err(tl!("Your own key is removed only by you, in the window").to_string())
        }
        "ui.removePrivateKey" => remove_private_key(app, p),
        "ui.verifyKey" => verify_key(app, s("address").unwrap_or_default(), s("fingerprint").unwrap_or_default()),
        "ui.markKeyVerified" => Err(tl!("A key is marked Verified only in the dialog that shows its fingerprint, by you").to_string()),
        _ => return None,
    })
}

/// The owner's own press (`MailApp::run`, never the control channel's `execute`): the Verify
/// dialog's buttons (`ui.markKeyVerified`) and the answer to Remove Your Key (the confirmed
/// `ui.removePrivateKey`). Input the control channel injected (`driven`) is refused there too.
pub(crate) fn owners_press(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    Some(match id {
        "ui.markKeyVerified" => {
            let Some(verified) = p.get("verified").and_then(Value::as_bool) else {
                return Some(Err(tl!("`verified` is true or false").to_string()));
            };
            mark_key_verified(app, &s("address"), &s("fingerprint"), verified)
        }
        "ui.removePrivateKey" => remove_private_key(app, p),
        _ => return None,
    })
}

fn no_host() -> &'static str {
    tl!(
        "Your own certificates and keys are used by MailCraft's desktop program: here Digital signatures are checked, and nothing is signed or decrypted."
    )
}

/// `ui.removePrivateKey`: asks first, then takes an imported key out of the credential store.
fn remove_private_key(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let wanted = digits(p.get("fingerprint").and_then(Value::as_str).unwrap_or_default());
    let Some(key) = app.ui.imported_keys.iter().find(|k| !wanted.is_empty() && digits(&k.fingerprint) == wanted).cloned() else {
        return Err(tl!("No key imported here has that fingerprint").to_string());
    };
    if p.get("confirmed").and_then(Value::as_bool) != Some(true) {
        let who = key.addresses.first().cloned().unwrap_or_else(|| key.name.clone());
        let text = crate::i18n::fmt(
            tl!(
                "Remove your {format} key for {who} from this computer? It is deleted from the system's credential store, and MailCraft can't sign with it any more. Keep a copy of the file if you may need it again."
            ),
            &[("format", format_name(key.format)), ("who", &crate::own_keys::clip(&who))],
        );
        app.dialog = Some(Dialog::Confirm {
            command: "ui.removePrivateKey".to_string(),
            params: json!({"fingerprint": key.fingerprint}),
            ask: Question::new(tl!("Remove Your Key"), text, tl!("Remove"), json!({"confirmed": true})),
        });
        return Ok(json!({"pending": "confirm"}));
    }
    // The owner's own answer: never input the control channel injected, or one of its requests.
    if app.driven {
        return Err(tl!("Your own key is removed only by you, in the window").to_string());
    }
    let gone = match (&app.services.secrets, crate::own_keys::is_store_name(&key.store_key)) {
        (Some(store), true) => {
            store.delete(&key.store_key);
            store.get(&key.store_key).is_none()
        }
        // (Nothing of the store's but a key's own name is ever deleted.)
        (None, _) | (_, false) => true,
    };
    if !gone {
        let said = tl!("The key couldn't be removed from the system's credential store.").to_string();
        app.security.said = Some((false, said.clone()));
        return Err(said);
    }
    app.ui.imported_keys.retain(|k| k.store_key != key.store_key);
    let said = tl!("The key was removed from this computer.").to_string();
    app.status(said.clone());
    app.security.said = Some((true, said));
    Ok(json!({"removed": key.fingerprint}))
}

/// Take every imported key out of the credential store (Remove MailCraft's data from this
/// computer): the names of those still there afterwards.
pub(crate) fn forget_imported(app: &mut MailApp) -> Vec<String> {
    let keys = std::mem::take(&mut app.ui.imported_keys);
    let Some(store) = &app.services.secrets else { return Vec::new() };
    let mut left = Vec::new();
    for key in keys.into_iter().filter(|k| crate::own_keys::is_store_name(&k.store_key)) {
        store.delete(&key.store_key);
        if store.get(&key.store_key).is_some() {
            left.push(key.store_key.clone());
            app.ui.imported_keys.push(key);
        }
    }
    left
}

/// `ui.verifyKey`: the dialog for the OpenPGP key kept for `address` with `fingerprint`.
fn verify_key(app: &mut MailApp, address: &str, fingerprint: &str) -> Result<Value, String> {
    let address = address.trim().to_lowercase();
    let wanted = digits(fingerprint);
    let key = app.session.mailbox.public_keys.of(&address).iter().find(|k| !wanted.is_empty() && digits(&k.fingerprint) == wanted);
    let Some(key) = key else { return Err(tl!("No Public key with that fingerprint is kept for that address").to_string()) };
    if key.format != SignatureFormat::OpenPgp {
        return Err(tl!("An S/MIME certificate is trusted through its certification authority and isn't marked Verified").to_string());
    }
    let dialog = VerifyDialog { address: address.clone(), fingerprint: key.fingerprint.clone(), verified: key.verified };
    app.dialog = Some(Dialog::Security(Box::new(SecurityDialog::Verify(dialog))));
    Ok(json!({"dialog": "verifyKey", "address": address}))
}

/// `ui.markKeyVerified`: only while the Verify dialog of that very key is open.
fn mark_key_verified(app: &mut MailApp, address: &str, fingerprint: &str, verified: bool) -> Result<Value, String> {
    let address = address.trim().to_lowercase();
    let open = match &app.dialog {
        Some(Dialog::Security(d)) => match &**d {
            SecurityDialog::Verify(v) => v.address == address && !digits(fingerprint).is_empty() && digits(&v.fingerprint) == digits(fingerprint),
            SecurityDialog::Import(_) => false,
        },
        _ => false,
    };
    // (Input the control channel injected, or one of its requests, is never the owner's word.)
    if !open || app.driven {
        return Err(tl!("A key is marked Verified only in the dialog that shows its fingerprint, by you").to_string());
    }
    app.session.set_key_verified(&address, fingerprint, verified).map_err(|e| e.to_string())?;
    app.dialog = None;
    app.status(if verified { tl!("The key is marked as verified.") } else { tl!("The key is not marked as verified.") });
    Ok(json!({"address": address, "verified": verified}))
}

/// A file picked for this page: `bytes` read by the window (the host's picker, or the browser's).
pub(crate) fn picked(app: &mut MailApp, purpose: &str, name: &str, bytes: Result<Vec<u8>, String>) {
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(why) => {
            app.security.said = Some((false, why.clone()));
            app.status(why);
            return;
        }
    };
    let name = crate::own_keys::clip(name.rsplit(['/', '\\']).next().unwrap_or(name));
    match purpose {
        PURPOSE_SMIME | PURPOSE_OPENPGP => {
            let file = KeyFile(Zeroizing::new(bytes));
            if file.0.len() > crate::own_keys::MAX_FILE || file.0.is_empty() {
                app.security.said = Some((false, tl!("That file isn't a key file: it is empty, or larger than one is.").to_string()));
                return;
            }
            let format = if purpose == PURPOSE_SMIME { SignatureFormat::Smime } else { SignatureFormat::OpenPgp };
            let dialog =
                ImportDialog { format, name, file, passphrase: Zeroizing::new(String::new()), shown: false, error: String::new(), busy: false };
            app.dialog = Some(Dialog::Security(Box::new(SecurityDialog::Import(dialog))));
        }
        PURPOSE_PUBLIC => {
            let r = app.run("keys.import", json!({"name": name, "data": encode(&bytes)}));
            let said = match &r {
                Ok(v) => {
                    let n = v.get("imported").and_then(Value::as_array).map_or(0, Vec::len);
                    (true, tln!(n, "{n} Public key was imported.", "{n} Public keys were imported."))
                }
                Err(e) => (false, e.clone()),
            };
            app.status(said.1.clone());
            app.security.said = Some(said);
        }
        PURPOSE_CARD => {
            // Only to the very form that asked: not to another opened since, nor after it closed.
            let asked = app.security.card_pick.take();
            let key = mailcraft_engine::cmd::keys::card_key_from(&bytes).map_err(|e| e.to_string());
            let r = match (&mut app.dialog, key) {
                (Some(Dialog::Form(crate::forms::Form::Contact(form))), Ok(key)) if asked.is_some() && form.key_pick() == asked => form.add_key(key),
                (_, Err(e)) => Err(e),
                _ => Err(tl!("The contact's form was closed: open it again to add the key").to_string()),
            };
            if let Err(e) = r {
                app.status(e);
            }
        }
        _ => {}
    }
}

/// The contact form's Add from file…: the picker opened, and the number of this pick, which the
/// form keeps, so that the key picked goes to that form and no other.
pub(crate) fn ask_card_key(app: &mut MailApp) -> u64 {
    app.security.picks = app.security.picks.wrapping_add(1).max(1);
    app.security.card_pick = Some(app.security.picks);
    crate::files::open(app, PURPOSE_CARD);
    app.security.picks
}

/// Press Import in the import dialog: the file and the passphrase go to the host.
fn start_import(app: &mut MailApp, d: &mut ImportDialog) {
    let Some(host) = &app.services.own_keys else {
        d.error = no_host().to_string();
        return;
    };
    if app.ui.imported_keys.len() >= crate::own_keys::MAX_IMPORTED {
        d.error = full();
        return;
    }
    // (The file stays in the dialog, still protected by its passphrase, for another try.)
    let file = KeyFile(Zeroizing::new(d.file.0.to_vec()));
    let passphrase = (d.format == SignatureFormat::Smime).then(|| Passphrase::new(std::mem::take(&mut *d.passphrase)));
    app.security.import_rx = Some(host(OwnKeysJob::Import { format: d.format, file, passphrase }));
    d.busy = true;
    d.shown = false;
    d.error.clear();
}

/// Lay out a dialog of this page. `true` when it is to close; a `ui.markKeyVerified` to run
/// with the dialog in place comes back in `mark`.
pub(crate) fn show_dialog(app: &mut MailApp, ctx: &egui::Context, dialog: &mut SecurityDialog, enter: bool, mark: &mut Option<Value>) -> bool {
    let t = Tokens::get(ctx);
    match dialog {
        SecurityDialog::Import(d) => {
            let mut close = false;
            let mut go = false;
            let title = match d.format {
                SignatureFormat::Smime => tl!("Import a Certificate"),
                SignatureFormat::OpenPgp => tl!("Import an OpenPGP Key"),
            };
            let place = app.security.listing.as_ref().map(|l| l.smime).unwrap_or_default();
            let watch = app.secret_watch();
            let open = window(ctx, title, 460.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tl!("File:")).font(semibold(12.5)));
                    ui.add(egui::Label::new(d.name.as_str()).wrap());
                });
                let note = match (d.format, place) {
                    (SignatureFormat::Smime, SmimePlace::WindowsStore) => {
                        tl!("Windows keeps the certificate and its private key in your certificate store, and asks you before MailCraft uses it.")
                    }
                    (SignatureFormat::Smime, SmimePlace::Keychain) => {
                        tl!("Your keychain keeps the certificate and its private key, and macOS asks you before MailCraft uses it.")
                    }
                    (SignatureFormat::Smime, _) => tl!(
                        "MailCraft keeps the file, still protected by its passphrase, in the system's credential store. Only an ECDSA certificate (P-256 or P-384) is kept this way: an RSA one belongs on a smartcard or token (PKCS#11)."
                    ),
                    (SignatureFormat::OpenPgp, _) => tl!(
                        "MailCraft reads the key without its passphrase and keeps the file, still protected by it, in the system's credential store. The key must have a passphrase and sign with Ed25519 or ECDSA; MailCraft asks for the passphrase when you sign."
                    ),
                };
                ui.add(egui::Label::new(RichText::new(note).color(t.text_dim).size(11.5)).wrap());
                if d.format == SignatureFormat::Smime {
                    ui.label(tl!("The file's passphrase:"));
                    let room = egui::Margin::from(ui.spacing().button_padding);
                    let mut passphrase = std::mem::take(&mut *d.passphrase);
                    let resp = crate::secret_field::field(ui, &mut passphrase, &mut d.shown, watch, room, |edit| edit.interactive(!d.busy));
                    *d.passphrase = passphrase;
                    crate::text_menu::hold(&resp);
                    ui.add(
                        egui::Label::new(
                            RichText::new(tl!("It is held in memory for this import only, and never written down.")).color(t.text_dim).size(11.0),
                        )
                        .wrap(),
                    );
                }
                if !d.error.is_empty() {
                    ui.add(egui::Label::new(RichText::new(d.error.as_str()).color(t.red)).wrap());
                }
                if d.busy {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Importing…"));
                    });
                }
                ui.add_space(4.0);
                button_row(ui, |ui| {
                    close |= ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
                    go |= primary_button(ui, tl!("Import"), !d.busy).clicked();
                });
            });
            if (go || enter) && !d.busy {
                start_import(app, d);
            }
            // (The passphrase typed and not used goes with the dialog, overwritten.)
            close || !open
        }
        SecurityDialog::Verify(v) => {
            let mut close = false;
            let open = window(ctx, tl!("Verify a Public Key"), 480.0, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(tl!("Address:")).font(semibold(12.5)));
                    ui.add(egui::Label::new(crate::own_keys::clip(&v.address)).wrap());
                });
                ui.label(RichText::new(tl!("Fingerprint:")).font(semibold(12.5)));
                // In two lines of groups of four, large enough to read out.
                let groups: Vec<String> = in_fours(&v.fingerprint).split(' ').map(str::to_string).collect();
                let half = groups.len().div_ceil(2);
                egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(4.0).inner_margin(10.0).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    for line in groups.chunks(half.max(1)) {
                        ui.label(RichText::new(line.join("  ")).font(egui::FontId::monospace(15.0)).color(t.text));
                    }
                });
                ui.add(
                    egui::Label::new(tl!(
                        "Compare it with the fingerprint the person gives you another way: in person, by phone, or on their business card. Mark the key as verified only if every character matches."
                    ))
                    .wrap(),
                );
                if v.verified {
                    ui.label(RichText::new(tl!("You marked this key as verified.")).color(t.green));
                }
                ui.add_space(4.0);
                button_row(ui, |ui| {
                    close |= ui.add(egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked();
                    let not = ui.add(egui::Button::new(tl!("Not verified")).min_size(vec2(96.0, 26.0)));
                    app.hit.add("keyNotVerified", 0, not.rect);
                    let not = not.on_hover_text(if v.verified { tl!("Take back that you verified it") } else { tl!("Leave it not verified") });
                    // (A click the control channel injected doesn't answer: the owner's word only.)
                    if not.clicked() && !app.driven {
                        *mark = Some(json!({"address": v.address, "fingerprint": v.fingerprint, "verified": false}));
                    }
                    let yes = primary_button(ui, tl!("Mark as verified"), true);
                    app.hit.add("keyVerified", 0, yes.rect);
                    if yes.clicked() && !app.driven {
                        *mark = Some(json!({"address": v.address, "fingerprint": v.fingerprint, "verified": true}));
                    }
                });
            });
            close || !open
        }
    }
}

/// A heading within the page's section.
fn heading(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).font(semibold(13.0)).color(t.text));
}

fn dim(ui: &mut Ui, text: &str, t: &Tokens) {
    ui.add(egui::Label::new(RichText::new(text).color(t.text_dim).size(11.5)).wrap());
}

/// File › Options › Security.
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    ui.label(RichText::new(tl!("Digital signatures and encryption")).font(semibold(13.5)).color(t.text));
    own_keys(app, ui, t);
    public_keys(app, ui, t);
}

fn place_name(place: &KeyPlace) -> String {
    match place {
        KeyPlace::WindowsStore => tl!("Windows certificate store").to_string(),
        KeyPlace::Keychain => tl!("Keychain").to_string(),
        KeyPlace::Token(label) => crate::i18n::fmt(tl!("Smartcard {label}"), &[("label", &crate::own_keys::clip(label))]),
        KeyPlace::Gnupg => tl!("GnuPG").to_string(),
        KeyPlace::Imported => tl!("Imported here").to_string(),
    }
}

/// Your certificates and keys.
fn own_keys(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Your certificates and keys"), t);
    if app.services.own_keys.is_none() {
        dim(ui, no_host(), t);
        return;
    }
    if app.security.listing.is_none() {
        request_list(app);
    }
    ui.horizontal_wrapped(|ui| {
        if ui
            .button(tl!("Import a certificate (.p12)…"))
            .on_hover_text(tl!("An S/MIME certificate with its private key, in a .p12 or .pfx file"))
            .clicked()
        {
            let _ = app.run("ui.importPrivateKey", json!({"format": "smime"}));
        }
        if ui.button(tl!("Import an OpenPGP key (.asc)…")).on_hover_text(tl!("An OpenPGP secret key, protected by a passphrase")).clicked() {
            let _ = app.run("ui.importPrivateKey", json!({"format": "openpgp"}));
        }
        let looking = app.security.listing_rx.is_some();
        if ui.add_enabled(!looking, egui::Button::new(tl!("Refresh"))).on_hover_text(tl!("Look for your certificates and keys again")).clicked() {
            let _ = app.run("ui.refreshOwnKeys", json!({}));
        }
        if looking {
            ui.spinner();
        }
    });
    let now = app.session.now();
    let mut rows: Vec<OwnKey> = app.security.listing.as_ref().map(|l| l.keys.clone()).unwrap_or_default();
    rows.extend(app.ui.imported_keys.iter().map(|k| OwnKey {
        format: k.format,
        name: k.name.clone(),
        addresses: k.addresses.clone(),
        fingerprint: k.fingerprint.clone(),
        not_after: k.not_after,
        expired: k.not_after != 0 && k.not_after < now,
        place: KeyPlace::Imported,
        can_sign: k.can_sign,
    }));
    if rows.is_empty() && app.security.listing.is_some() {
        dim(ui, tl!("No certificates or keys of yours were found on this computer."), t);
    }
    for (i, key) in rows.iter().enumerate() {
        own_key_row(app, ui, i, key, t);
    }
    if let Some(listing) = &app.security.listing {
        match &listing.gnupg {
            Gnupg::Missing => dim(ui, tl!("GnuPG isn't installed: OpenPGP signing will work only with a key imported here."), t),
            Gnupg::NotWorking(why) => dim(
                ui,
                &crate::i18n::fmt(
                    tl!("GnuPG is installed but didn't answer ({why}): OpenPGP signing will work only with a key imported here."),
                    &[("why", why)],
                ),
                t,
            ),
            Gnupg::Found | Gnupg::Unknown => {}
        }
        if let Some(at) = app.security.gnupg_read.filter(|_| listing.gnupg == Gnupg::Found) {
            let when = crate::dates::list_label(at, now, app.session.offset_at(at), app.session.prefs.twelve_hour);
            dim(
                ui,
                &crate::i18n::fmt(
                    tl!(
                        "Which keys your GnuPG trusts fully, last read: {when}. It is read again when MailCraft comes back to the front, and by Refresh."
                    ),
                    &[("when", &when)],
                ),
                t,
            );
        }
        if listing.smime == SmimePlace::None {
            dim(ui, tl!("This system has no place MailCraft knows for S/MIME certificates with a private key."), t);
        }
        for line in listing.trouble.iter().take(8) {
            dim(ui, line, t);
        }
    }
    if let Some((ok, said)) = &app.security.said {
        ui.add(egui::Label::new(RichText::new(said.as_str()).color(if *ok { t.text_dim } else { t.red })).wrap());
    }
}

fn badge(ui: &mut Ui, text: &str, t: &Tokens) {
    egui::Frame::NONE.fill(t.accent_soft).corner_radius(3.0).inner_margin(egui::Margin::symmetric(5, 1)).show(ui, |ui| {
        ui.label(RichText::new(text).font(semibold(10.5)).color(t.accent_soft_text));
    });
}

fn own_key_row(app: &mut MailApp, ui: &mut Ui, i: usize, key: &OwnKey, t: &Tokens) {
    egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).corner_radius(5.0).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.horizontal_wrapped(|ui| {
            badge(ui, format_name(key.format), t);
            let who = if key.name.is_empty() { key.addresses.first().cloned().unwrap_or_default() } else { key.name.clone() };
            ui.add(egui::Label::new(RichText::new(crate::own_keys::clip(&who)).font(semibold(12.5)).color(t.text)).truncate());
            if key.place == KeyPlace::Imported {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.small_button(tl!("Remove")).on_hover_text(tl!("Delete this key from this computer"));
                    app.hit.add("removeOwnKey", i as u64, r.rect);
                    // (Not for a click the control channel injected.)
                    if r.clicked() && !app.driven {
                        let _ = app.run("ui.removePrivateKey", json!({"fingerprint": key.fingerprint}));
                    }
                });
            }
        });
        if !key.addresses.is_empty() && key.addresses.first() != Some(&key.name) {
            let shown: Vec<String> = key.addresses.iter().take(4).map(|a| crate::own_keys::clip(a)).collect();
            ui.add(egui::Label::new(RichText::new(shown.join(", ")).color(t.text).size(12.0)).wrap());
        }
        let mut facts = vec![place_name(&key.place)];
        if key.expired {
            facts.push(tl!("Expired").to_string());
        } else if key.not_after != 0 {
            facts.push(crate::i18n::fmt(tl!("Valid until {date}"), &[("date", &day(app, key.not_after))]));
        }
        if !key.can_sign {
            facts.push(tl!("Can't sign").to_string());
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(facts.join(" · ")).color(if key.expired { t.red } else { t.text_dim }).size(11.5));
            ui.label(RichText::new(short(&key.fingerprint)).font(egui::FontId::monospace(11.0)).color(t.text_dim))
                .on_hover_text(key.fingerprint.as_str());
        });
    });
}

/// Public keys of other people.
fn public_keys(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    heading(ui, tl!("Public keys of other people"), t);
    dim(ui, tl!("Digital signatures are checked with these. They are collected from Digitally signed mail you receive, or imported here."), t);
    if ui.button(tl!("Import a Public key…")).on_hover_text(tl!("An X.509 certificate (.cer, .pem) or an OpenPGP Public key (.asc)")).clicked() {
        let _ = app.run("ui.importPublicKey", json!({}));
    }
    let rows: Vec<(String, mailcraft_model::public_keys::PublicKey)> =
        app.session.mailbox.public_keys.iter().flat_map(|(a, keys)| keys.iter().map(move |k| (a.to_string(), k.clone()))).take(4_000).collect();
    if rows.is_empty() {
        dim(ui, tl!("No Public keys are kept yet."), t);
        return;
    }
    let total = rows.len();
    let heading = crate::i18n::fmt(tl!("Public keys kept ({n}):"), &[("n", &total.to_string())]);
    egui::CollapsingHeader::new(heading).id_salt("publicKeys").default_open(total <= PAGE).show(ui, |ui| {
        let pages = total.div_ceil(PAGE).max(1);
        let id = egui::Id::unique("publicKeysPage");
        let mut page = ui.data(|d| d.get_temp::<usize>(id)).unwrap_or(0).min(pages - 1);
        for (i, (address, key)) in rows.iter().enumerate().skip(page.saturating_mul(PAGE)).take(PAGE) {
            public_key_row(app, ui, i, address, key, t);
        }
        if pages > 1 {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(page > 0, egui::Button::new(tl!("Previous")).small()).clicked() {
                    page = page.saturating_sub(1);
                }
                let at = crate::i18n::fmt(tl!("Page {page} of {pages}"), &[("page", &(page + 1).to_string()), ("pages", &pages.to_string())]);
                ui.label(at);
                if ui.add_enabled(page + 1 < pages, egui::Button::new(tl!("Next")).small()).clicked() {
                    page += 1;
                }
            });
        }
        ui.data_mut(|d| d.insert_temp(id, page));
    });
}

/// Whether a kept key is Verified, in words and a hue.
fn verified_said(format: SignatureFormat, verified: bool, t: &Tokens) -> (&'static str, Color32) {
    match (format, verified) {
        (SignatureFormat::OpenPgp, true) => (tl!("Verified"), t.green),
        (SignatureFormat::OpenPgp, false) => (tl!("Not verified"), t.orange),
        (SignatureFormat::Smime, _) => (tl!("Checked with its certification authority"), t.text_dim),
    }
}

fn public_key_row(app: &mut MailApp, ui: &mut Ui, i: usize, address: &str, key: &mailcraft_model::public_keys::PublicKey, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        ui.add(egui::Label::new(RichText::new(crate::own_keys::clip(address)).font(semibold(12.0)).color(t.text)).truncate());
        badge(ui, format_name(key.format), t);
        ui.label(RichText::new(short(&key.fingerprint)).font(egui::FontId::monospace(11.0)).color(t.text_dim))
            .on_hover_text(key.fingerprint.as_str());
        let (said, hue) = verified_said(key.format, key.verified, t);
        ui.label(RichText::new(said).color(hue).size(11.5));
        let source = match key.source {
            KeySource::Imported => tl!("imported"),
            _ => tl!("collected from mail"),
        };
        let mut facts = vec![source.to_string()];
        if key.not_after != 0 {
            facts.push(crate::i18n::fmt(tl!("valid until {date}"), &[("date", &day(app, key.not_after))]));
        }
        ui.label(RichText::new(facts.join(" · ")).color(t.text_dim).size(11.5));
        if key.format == SignatureFormat::OpenPgp {
            let r = ui
                .small_button(tl!("Compare fingerprint…"))
                .on_hover_text(tl!("Compare the key's fingerprint with the person's, and mark it as verified"));
            app.hit.add("compareFingerprint", i as u64, r.rect);
            // (Not for a click the control channel injected: the dialog is the owner's.)
            if r.clicked() && !app.driven {
                let _ = app.run("ui.verifyKey", json!({"address": address, "fingerprint": key.fingerprint}));
            }
        }
        let r = ui.small_button(tl!("Remove")).on_hover_text(tl!("Stop keeping this Public key"));
        app.hit.add("removePublicKey", i as u64, r.rect);
        if r.clicked() {
            let _ = app.run("keys.remove", json!({"address": address, "fingerprint": key.fingerprint}));
        }
    });
}

/// One of a contact card's keys, in a line: its kind, who it is for and its fingerprint; a key
/// by address is shown as the address and never fetched.
pub(crate) fn card_key_line(app: &MailApp, ui: &mut Ui, key: &ContactKey, t: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
        let facts = mailcraft_engine::cmd::keys::card_key_facts(key);
        match &key.kind {
            KeyKind::Uri => {
                badge(ui, tl!("Link"), t);
                ui.add(egui::Label::new(RichText::new(crate::own_keys::clip(&key.uri)).color(t.text).size(12.0)).truncate())
                    .on_hover_text(tl!("Where the card says the key is: MailCraft doesn't fetch it."));
                return;
            }
            KeyKind::X509 => badge(ui, "S/MIME", t),
            KeyKind::OpenPgp => badge(ui, "OpenPGP", t),
            KeyKind::Unknown(_) => badge(ui, tl!("Other"), t),
        }
        if facts.format.is_none() {
            ui.label(RichText::new(tl!("can't be read")).color(t.text_dim).size(11.5));
            return;
        }
        if !facts.name.is_empty() {
            ui.add(egui::Label::new(RichText::new(crate::own_keys::clip(&facts.name)).color(t.text).size(12.0)).truncate());
        }
        if !facts.issuer.is_empty() {
            ui.label(
                RichText::new(crate::i18n::fmt(tl!("issued by {issuer}"), &[("issuer", &crate::own_keys::clip(&facts.issuer))]))
                    .color(t.text_dim)
                    .size(11.5),
            );
        }
        ui.label(RichText::new(short(&facts.fingerprint)).font(egui::FontId::monospace(11.0)).color(t.text_dim))
            .on_hover_text(facts.fingerprint.as_str());
        if facts.not_after != 0 {
            ui.label(
                RichText::new(crate::i18n::fmt(tl!("valid until {date}"), &[("date", &day(app, facts.not_after))])).color(t.text_dim).size(11.5),
            );
        }
    });
}

/// The Public keys kept for a contact's addresses, each in a line, marked as kept (and Verified,
/// for OpenPGP).
pub(crate) fn kept_key_lines(app: &MailApp, ui: &mut Ui, addresses: &[String], t: &Tokens) -> usize {
    let mut n = 0usize;
    for address in addresses.iter().take(16) {
        let address = address.trim().to_lowercase();
        for key in app.session.mailbox.public_keys.of(&address) {
            n += 1;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
                badge(ui, format_name(key.format), t);
                ui.label(RichText::new(tl!("Kept Public key")).color(t.text).size(12.0));
                ui.add(egui::Label::new(RichText::new(crate::own_keys::clip(&address)).color(t.text_dim).size(11.5)).truncate());
                ui.label(RichText::new(short(&key.fingerprint)).font(egui::FontId::monospace(11.0)).color(t.text_dim))
                    .on_hover_text(key.fingerprint.as_str());
                let (said, hue) = verified_said(key.format, key.verified, t);
                ui.label(RichText::new(said).color(hue).size(11.5));
            });
        }
    }
    n
}

/// The Certificates section of a contact's card in People.
pub(crate) fn card_section(app: &MailApp, ui: &mut Ui, c: &mailcraft_model::Contact) {
    let addresses: Vec<String> = c.emails.iter().map(|e| e.value.clone()).collect();
    let kept = addresses.iter().any(|a| !app.session.mailbox.public_keys.of(&a.trim().to_lowercase()).is_empty());
    if c.keys.is_empty() && !kept {
        return;
    }
    let t = Tokens::get(ui.ctx());
    crate::people_view::section(ui, tl!("Certificates"));
    for key in c.keys.iter().take(mailcraft_model::Contact::MAX_KEYS) {
        card_key_line(app, ui, key, &t);
    }
    kept_key_lines(app, ui, &addresses, &t);
}
