// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures' Import HTML… (R4.5): a new signature made of the owner's own HTML document
//! and the picture files it names, each file chosen by the owner.
//!
//! The owner presses Import HTML… (`signature.import` with nothing given, [`begin`]): the host's
//! picker is asked for one document (purpose [`HTML`]). Once it has come, the page shows a name
//! for the new signature, which the owner may change, and one row for each picture the document
//! names, by its logical resource name as the importer reads it
//! (`mailcraft_mime_model::signature_import::required_assets`); each row's Choose image… asks the
//! picker for that one picture (purpose [`ASSET`]). Import, pressed by the owner once every row
//! has its file, hands the document and the files to `signature.import`, which makes the
//! signature and appends it, or says why not. Nothing is saved, opened, switched or chosen as a
//! default here, and the job is the window's alone: it is never written anywhere.
//!
//! Every pick has a request token of its own ([`State::last_ticket`]): an answer is taken only
//! for the one request still outstanding with that purpose and token, and only once. A request
//! remembers the window it was made in ([`Origin`]: the mailbox open and the signature in the
//! editor, by its name and the editor's generation, so that the same signature closed and opened
//! again is another editor); an answer that comes once either has changed is let go of, and so is
//! Import. At most one pick is outstanding, and Import waits while it is. Cancel lets go of the job
//! and of the answer still outstanding, which keeps its place until it has come and is then
//! ignored.
//!
//! The document is at most [`MAX_HTML`] bytes of UTF-8, and is kept exactly as it came; each
//! picture at most [`MAX_PICTURE_BYTES`], and the pictures together at most [`MAX_PICTURES_BYTES`].
//! A file over a cap changes nothing of the job. `signature.import` checks all of it again.
//!
//! In the browser the picker delivers the file's bytes, read by the host within the cap
//! ([`crate::files::signature_cap`]), or says it was cancelled ([`crate::Delivered::cancelled`]).
//! On the desktop the dialog gives a path, which the host's reader reads off the window's thread
//! within the same cap ([`crate::Services::read_signature_file`]); the window never reads it. One
//! reader is admitted at a time, and it keeps its place until it answers or goes away, whatever
//! became of the job.

use std::sync::mpsc::{Receiver, TryRecvError};

use egui::Ui;
use mailcraft_engine::mime::signature_import::required_assets;
use mailcraft_model::signature::{MAX_HTML, MAX_PICTURE_BYTES, MAX_PICTURES_BYTES};
use serde_json::{Value, json};

use crate::MailApp;
use crate::files::base64_lite::encode;
use crate::theme::{Tokens, semibold};

/// The picker's purpose for the HTML document.
pub(crate) const HTML: &str = "signatureHtml";
/// The picker's purpose for one picture the document names.
pub(crate) const ASSET: &str = "signatureAsset";

/// Whether `purpose` is one of Import HTML…'s picks.
pub(crate) fn is_purpose(purpose: &str) -> bool {
    purpose == HTML || purpose == ASSET
}

/// What one pick is for: the document, or the picture of one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Html,
    Asset(usize),
}

impl Kind {
    fn purpose(self) -> &'static str {
        match self {
            Kind::Html => HTML,
            Kind::Asset(_) => ASSET,
        }
    }
}

/// The window a request was made in: the mailbox open (counted by the session's openings) and
/// the signature open in the editor, if any: its name and the editor's generation
/// (`SignatureEdit::generation`, new each time the editor is given a signature, the same one
/// again included). Not the mailbox's revision: an unrelated change keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Origin {
    openings: u64,
    editor: Option<(String, u64)>,
}

fn origin(app: &MailApp) -> Origin {
    Origin { openings: app.session.openings(), editor: app.session.signature_edit.as_ref().map(|e| (e.signature.name.clone(), e.generation)) }
}

/// Whether the window is still the one `was` was taken in, with no other mailbox on its way in.
fn current(app: &MailApp, was: &Origin) -> bool {
    app.opening.is_none() && origin(app) == *was
}

/// The one pick asked for and not answered yet.
struct Asked {
    ticket: u64,
    kind: Kind,
    origin: Origin,
    /// Let go of by the owner's Cancel: its answer is still waited for, then ignored.
    discarded: bool,
}

/// A desktop file being read by the host's reader for the request it answers.
struct Reading {
    rx: Receiver<Result<Vec<u8>, crate::Unread>>,
    kind: Kind,
    origin: Origin,
    /// The picked file's own name (without its folders): offered as the new signature's name.
    file: String,
    /// Let go of by the owner's Cancel: its answer is still waited for, then ignored.
    discarded: bool,
}

/// One picture the document names, and the file chosen for it (its own name and bytes).
struct Slot {
    resource: String,
    picked: Option<(String, Vec<u8>)>,
}

/// A document picked and waiting for its pictures, its name and the owner's Import.
struct Job {
    origin: Origin,
    html: String,
    name: String,
    slots: Vec<Slot>,
}

impl Job {
    fn ready(&self) -> bool {
        !self.name.trim().is_empty() && self.slots.iter().all(|s| s.picked.is_some())
    }
}

/// Import HTML…'s state in this window. Never saved.
#[derive(Default)]
pub(crate) struct State {
    /// The last request token given out: each pick is given the next, and none is given twice.
    last_ticket: u64,
    asked: Option<Asked>,
    /// The desktop reader admitted, until it answers or goes away (not when the job is let go of).
    reading: Option<Reading>,
    job: Option<Job>,
    /// What the page last has to say about the import (a refusal), shown under its button.
    note: Option<String>,
}

impl State {
    /// Whether a pick or a read is outstanding: Import waits for it, whatever became of the job.
    fn busy(&self) -> bool {
        self.asked.is_some() || self.reading.is_some()
    }
}

/// `signature.import` with nothing given, from the owner's own press: the picker is asked for
/// the document.
pub(crate) fn begin(app: &mut MailApp) -> Result<Value, String> {
    ask_for(app, Kind::Html)
}

fn ask_for(app: &mut MailApp, kind: Kind) -> Result<Value, String> {
    if app.opening.is_some() {
        return refused(app, crate::opening::STILL_OPENING);
    }
    if app.signature_import.asked.is_some() || app.signature_import.reading.is_some() {
        return refused(app, tl!("A file is still being chosen for the import."));
    }
    if let Kind::Asset(i) = kind
        && !app.signature_import.job.as_ref().is_some_and(|j| i < j.slots.len() && current(app, &j.origin))
    {
        return refused(app, tl!("That picture is no longer part of the import."));
    }
    // The browser's picker is the host's to show; a desktop's dialog is one at a time.
    if app.services.open_async.is_none() {
        if app.services.file_dialog.is_none() || app.services.read_signature_file.is_none() {
            return refused(app, tl!("File dialogs aren't available here"));
        }
        if app.file_dialog.is_some() {
            return refused(app, tl!("A file dialog is already open."));
        }
    }
    let Some(ticket) = app.signature_import.last_ticket.checked_add(1) else {
        return refused(app, tl!("No more files can be chosen in this window."));
    };
    app.signature_import.last_ticket = ticket;
    // Asked for before the host is: a host that answers at once finds its request waiting.
    app.signature_import.asked = Some(Asked { ticket, kind, origin: origin(app), discarded: false });
    app.signature_import.note = None;
    crate::files::open_for(app, kind.purpose(), Some(ticket));
    Ok(json!({"pending": kind.purpose()}))
}

fn refused(app: &mut MailApp, why: &str) -> Result<Value, String> {
    app.signature_import.note = Some(why.to_string());
    app.status(why.to_string());
    Err(why.to_string())
}

/// The outstanding request, taken, when `purpose` and `asking` are its own; else nothing.
fn take_matching(app: &mut MailApp, purpose: &str, asking: Option<u64>) -> Option<Asked> {
    let st = &mut app.signature_import;
    match &st.asked {
        Some(a) if a.kind.purpose() == purpose && Some(a.ticket) == asking => st.asked.take(),
        _ => None,
    }
}

/// A file the browser's picker delivered for one of Import HTML…'s picks.
pub(crate) fn delivered(app: &mut MailApp, purpose: &str, asking: Option<u64>, name: String, bytes: Result<Vec<u8>, crate::Unread>) {
    let Some(asked) = take_matching(app, purpose, asking) else { return };
    if asked.discarded {
        return;
    }
    arrived(app, asked.kind, asked.origin, name, bytes);
}

/// The desktop dialog's answer to one of Import HTML…'s requests: the picked path is handed to the
/// host's reader, if the window is still the one it was asked in. Nothing is read here.
pub(crate) fn picked_path(app: &mut MailApp, purpose: &str, path: &str, asking: Option<u64>) {
    let Some(asked) = take_matching(app, purpose, asking) else { return };
    if asked.discarded || !current(app, &asked.origin) {
        return;
    }
    let read = match asked.kind {
        Kind::Html => crate::files::SignatureRead::document(path),
        Kind::Asset(_) => crate::files::SignatureRead::picture(path),
    };
    let Some(reader) = &app.services.read_signature_file else {
        return said(app, tl!("File dialogs aren't available here").to_string());
    };
    let rx = reader(read);
    let file = path.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    app.signature_import.reading = Some(Reading { rx, kind: asked.kind, origin: asked.origin, file, discarded: false });
}

/// The desktop dialog of one of Import HTML…'s requests was cancelled (or went away): the
/// request is let go of, and the job stays as it was.
pub(crate) fn cancelled(app: &mut MailApp, purpose: &str, asking: Option<u64>) {
    let _ = take_matching(app, purpose, asking);
}

/// The desktop reader's answer, once it has come (every frame, from `logic`): its place is free
/// again, and the answer is taken as a delivered file's is. A reader that went away without an
/// answer is a file that couldn't be read.
pub(crate) fn poll(app: &mut MailApp) {
    let answer = match app.signature_import.reading.as_ref().map(|r| r.rx.try_recv()) {
        None | Some(Err(TryRecvError::Empty)) => return,
        Some(Ok(answer)) => answer,
        Some(Err(TryRecvError::Disconnected)) => Err(crate::Unread::Failed(tl!("the reader stopped unexpectedly").to_string())),
    };
    let Some(reading) = app.signature_import.reading.take() else { return };
    if reading.discarded {
        return;
    }
    arrived(app, reading.kind, reading.origin, reading.file, answer);
}

/// The answer to a request of this window's: taken into the job if the window is still the one
/// it was asked in, else let go of.
fn arrived(app: &mut MailApp, kind: Kind, was: Origin, name: String, bytes: Result<Vec<u8>, crate::Unread>) {
    if !current(app, &was) {
        return;
    }
    let bytes = match bytes {
        Ok(bytes) => bytes,
        // A cancelled pick is no failure: the job stays as it was, ready to be chosen again.
        Err(crate::Unread::Cancelled) => return,
        Err(why) => return said(app, unread(&why)),
    };
    match kind {
        Kind::Html => staged_html(app, was, &name, bytes),
        Kind::Asset(i) => staged_asset(app, i, name, bytes),
    }
}

fn said(app: &mut MailApp, why: String) {
    app.status(why.clone());
    app.signature_import.note = Some(why);
}

/// Why a file had no bytes, as the page says it.
fn unread(why: &crate::Unread) -> String {
    match why {
        crate::Unread::Failed(why) => crate::i18n::fmt(tl!("The file couldn't be read ({why})"), &[("why", why)]),
        crate::Unread::TooLarge => tl!("That file is larger than a signature may take.").to_string(),
        _ => tl!("The file couldn't be read").to_string(),
    }
}

fn staged_html(app: &mut MailApp, was: Origin, file: &str, bytes: Vec<u8>) {
    if bytes.len() > MAX_HTML {
        return said(app, tl!("That HTML file is larger than a signature's HTML may be (64 KB).").to_string());
    }
    let Ok(html) = String::from_utf8(bytes) else {
        return said(app, tl!("That HTML file isn't UTF-8 text.").to_string());
    };
    let resources = match required_assets(&html) {
        Ok(r) => r,
        Err(e) => {
            return said(app, crate::i18n::fmt(tl!("That HTML can't be made a signature: {why}"), &[("why", &e.to_string())]));
        }
    };
    let slots = resources.into_iter().map(|resource| Slot { resource, picked: None }).collect();
    app.signature_import.note = None;
    app.signature_import.job = Some(Job { origin: was, html, name: suggested_name(file), slots });
}

/// The file chosen for row `i`, unless it is over a picture's cap or would take the pictures
/// together over theirs (the row's own old file not counted): then nothing of the job changes.
fn staged_asset(app: &mut MailApp, i: usize, name: String, bytes: Vec<u8>) {
    let Some(job) = app.signature_import.job.as_mut() else { return };
    if bytes.len() > MAX_PICTURE_BYTES {
        return said(app, tl!("That picture is larger than a signature's picture may be (256 KB).").to_string());
    }
    let others: usize = job.slots.iter().enumerate().filter(|(j, _)| *j != i).filter_map(|(_, s)| s.picked.as_ref()).map(|(_, b)| b.len()).sum();
    if others.saturating_add(bytes.len()) > MAX_PICTURES_BYTES {
        return said(app, tl!("The pictures would be larger than a signature's pictures may be together (512 KB).").to_string());
    }
    let Some(slot) = job.slots.get_mut(i) else { return };
    slot.picked = Some((name, bytes));
    app.signature_import.note = None;
}

/// The name offered for the new signature: the picked file's own name without its extension.
fn suggested_name(file: &str) -> String {
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    stem.trim().chars().filter(|c| !c.is_control()).take(100).collect()
}

/// The owner's Cancel: the job is let go of, and so is the answer of a pick or read still
/// outstanding, which keeps its place until it has come (or gone away) and is then ignored.
fn cancel_job(app: &mut MailApp) {
    let st = &mut app.signature_import;
    st.job = None;
    st.note = None;
    if let Some(asked) = st.asked.as_mut() {
        asked.discarded = true;
    }
    if let Some(reading) = st.reading.as_mut() {
        reading.discarded = true;
    }
}

/// The owner's Import: the document and the chosen files, handed to `signature.import`; nothing
/// while a pick or read is outstanding, so a file being replaced is never stored in its place.
fn commit(app: &mut MailApp) {
    if app.signature_import.busy() {
        return;
    }
    let Some(job) = app.signature_import.job.as_ref().filter(|j| j.ready()) else { return };
    if !current(app, &job.origin) {
        app.signature_import.job = None;
        return said(app, tl!("The mailbox or the signature being edited changed, so the HTML import was given up.").to_string());
    }
    let assets: Vec<Value> =
        job.slots.iter().filter_map(|s| s.picked.as_ref().map(|(_, bytes)| json!({"name": s.resource, "data": encode(bytes)}))).collect();
    let name = job.name.trim().to_string();
    let params = json!({"name": name, "html": job.html, "assets": assets});
    match app.run("signature.import", params) {
        Ok(_) => {
            app.signature_import.job = None;
            app.signature_import.note = None;
            app.status(crate::i18n::fmt(tl!("Imported the signature {name}"), &[("name", &name)]));
        }
        Err(why) => app.signature_import.note = Some(why),
    }
}

/// Import HTML…, and below it the document waiting for its pictures, its name and Import: in the
/// column of the signatures' list, under New, Rename and Delete.
pub(crate) fn panel(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let r = ui.button(tl!("Import HTML…"));
    app.hit.add("signatureHtmlImport", 0, r.rect);
    if r.clicked() {
        let _ = app.run("signature.import", json!({}));
    }
    if let Some(note) = &app.signature_import.note {
        ui.label(egui::RichText::new(note.as_str()).color(t.red));
    }
    let Some(job) = app.signature_import.job.as_ref() else { return waiting(app, ui, t) };
    let mut name = job.name.clone();
    let rows: Vec<(String, Option<String>)> = job.slots.iter().map(|s| (s.resource.clone(), s.picked.as_ref().map(|(n, _)| n.clone()))).collect();
    let ready = job.ready() && !app.signature_import.busy();
    let mut chosen = None;
    let (mut import, mut cancel) = (false, false);
    egui::Frame::NONE.fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(6.0).show(ui, |ui| {
        ui.label(egui::RichText::new(tl!("New signature from an HTML file")).font(semibold(13.0)).color(t.text));
        ui.label(tl!("Name"));
        let field = ui.add(
            egui::TextEdit::singleline(&mut name)
                .id(egui::Id::unique("mailcraft-signature-import-name"))
                .desired_width(ui.available_width())
                .char_limit(100),
        );
        app.hit.add("signatureHtmlImportName", 0, field.rect);
        if rows.is_empty() {
            ui.label(egui::RichText::new(tl!("This HTML names no picture files.")).color(t.text_dim));
        } else {
            ui.label(egui::RichText::new(tl!("Choose the file for each picture the HTML names:")).color(t.text_dim));
        }
        for (i, (resource, picked)) in rows.iter().enumerate() {
            ui.add(egui::Label::new(egui::RichText::new(resource.as_str()).monospace().color(t.text)).wrap());
            let r = ui.button(tl!("Choose image…"));
            app.hit.add("signatureHtmlImportAsset", i as u64, r.rect);
            if r.clicked() {
                chosen = Some(i);
            }
            let said = match picked {
                Some(file) => crate::i18n::fmt(tl!("Chosen: {file}"), &[("file", file)]),
                None => tl!("No file chosen yet").to_string(),
            };
            ui.label(egui::RichText::new(said).color(t.text_dim));
        }
        ui.horizontal(|ui| {
            let r = ui.add_enabled(ready, egui::Button::new(tl!("Import")));
            app.hit.add("signatureHtmlImportCommit", 0, r.rect);
            import = r.clicked();
            let r = ui.button(tl!("Cancel"));
            app.hit.add("signatureHtmlImportCancel", 0, r.rect);
            cancel = r.clicked();
        });
    });
    if let Some(job) = app.signature_import.job.as_mut() {
        job.name = name;
    }
    if let Some(i) = chosen {
        let _ = ask_for(app, Kind::Asset(i));
    } else if import {
        commit(app);
    } else if cancel {
        cancel_job(app);
    }
}

/// A pick or read outstanding with no job on the page (the first document, or one let go of by
/// Cancel): what it waits for, and Cancel.
fn waiting(app: &mut MailApp, ui: &mut Ui, t: &Tokens) {
    let st = &app.signature_import;
    let discarded = match (&st.asked, &st.reading) {
        (Some(asked), _) => asked.discarded,
        (None, Some(reading)) => reading.discarded,
        (None, None) => return,
    };
    let said = if discarded { tl!("Waiting for the previous file to finish being read.") } else { tl!("Waiting for the HTML file…") };
    let mut cancel = false;
    egui::Frame::NONE.fill(t.input).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(6.0).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(said).color(t.text_dim)).wrap());
        let r = ui.button(tl!("Cancel"));
        app.hit.add("signatureHtmlImportCancel", 0, r.rect);
        cancel = r.clicked();
    });
    if cancel {
        cancel_job(app);
    }
}

#[cfg(test)]
#[path = "signature_import_tests.rs"]
mod tests;
