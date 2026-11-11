// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Open & Export › Import a .pst File (#815): the owner picks a
//! file, gives its password when it has one and says where its mail goes (a new folder named after
//! the file, or under a folder on this computer); the file is read off the window's thread and its
//! mail comes in a batch at a time (`file.importPst` with `staged`), so that Cancel keeps what is in
//! and a second import brings the rest. Names come from the file: they are shown as text.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use egui::{RichText, Ui, vec2};
use mailcraft_engine::import_pst::{Event, Job, Progress, Staged};
use serde_json::{Value, json};

use crate::MailApp;
use crate::dialogs::{button_row, buttons};
use crate::theme::Tokens;

pub const WIDTH: f32 = 520.0;
/// Most folders offered to import under, and most left-out items listed.
const LISTED: usize = 500;
const LEFT_OUT_SHOWN: usize = 50;
/// How long a turn of the window's logic may spend putting batches in while the window is being
/// painted, so that a fast reader doesn't hold it (at least one batch goes in a turn).
const BUDGET_SHOWN: Duration = Duration::from_millis(8);
/// And while it isn't (hidden, minimised, on another desktop): nothing is painted then, so a turn
/// may take longer, and each is a quarter of a second or more apart.
const BUDGET_HIDDEN: Duration = Duration::from_millis(150);
/// Most events taken in a turn where there is no clock (the browser).
#[cfg(target_arch = "wasm32")]
const EVENTS_NO_CLOCK: usize = 4;

/// What the host is asked to do off the window's thread: read the file at `path`
/// ([`mailcraft_engine::import_pst::read_path`]) and send what it reads. The window shows
/// `progress` and cancels through it.
pub struct PstJob {
    pub path: String,
    pub job: Job,
    pub progress: Arc<Progress>,
}

enum Phase {
    /// Choosing the password and the place.
    Choose,
    /// Reading (off the window's thread); its batches come in as they arrive.
    Importing(Receiver<Event>),
    /// The report (or why it couldn't be done).
    Done(Result<Value, String>),
}

pub struct ImportPst {
    path: String,
    /// What a look at the file said: its kind and whether it needs a password, or why it can't be read.
    look: Result<(bool, String), String>,
    password: String,
    wrong_password: bool,
    /// `None`: a new folder named after the file; else under this folder on this computer.
    under: Option<u64>,
    /// `None`: a new calendar named after the file; else this calendar on this computer.
    calendar: Option<u64>,
    phase: Phase,
    progress: Arc<Progress>,
    /// The mailbox this was opened over: another one opened since stops the import.
    opening: u64,
    /// A batch that couldn't go in: the import is stopped and this is said.
    failed: Option<String>,
    /// Batches that went in.
    batches: usize,
    /// Whether the window was painted since the last turn of `pump`: when it wasn't, nothing is
    /// shown and the turn may be longer.
    painted: bool,
}

impl Drop for ImportPst {
    /// A dialog that goes away stops the reading; what is in stays in.
    fn drop(&mut self) {
        self.progress.cancel();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn look(path: &str) -> Result<(bool, String), String> {
    mailcraft_engine::import_pst::look(path).map(|l| (l.needs_password, l.format.to_string()))
}

#[cfg(target_arch = "wasm32")]
fn look(_: &str) -> Result<(bool, String), String> {
    Err(tl!("Importing a .pst file isn't supported in the browser yet.").to_string())
}

impl ImportPst {
    pub fn open(app: &mut MailApp, path: &str) -> ImportPst {
        ImportPst {
            path: path.to_string(),
            look: look(path),
            password: String::new(),
            wrong_password: false,
            under: None,
            calendar: None,
            phase: Phase::Choose,
            progress: Arc::default(),
            opening: app.session.openings(),
            failed: None,
            batches: 0,
            painted: true,
        }
    }

    /// Whether it is reading: closing the window then stops the reading, and the window stays
    /// until what was read is in and the report is shown.
    pub fn stop(&mut self) -> bool {
        let importing = matches!(self.phase, Phase::Importing(_));
        if importing {
            self.progress.cancel();
        }
        importing
    }

    /// Whether it is reading and putting the mail in.
    pub fn is_importing(&self) -> bool {
        matches!(self.phase, Phase::Importing(_))
    }

    fn file(&self) -> String {
        self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path).to_string()
    }

    fn places(&self) -> Value {
        let mail = self.under.map_or(json!("new"), |id| json!(id));
        let calendar = self.calendar.map_or(json!("new"), |id| json!(id));
        json!({"mail": mail, "calendar": calendar})
    }

    /// Calendars on this computer the file's calendar items can go into.
    fn local_calendars(app: &MailApp) -> Vec<(u64, String)> {
        let mut out: Vec<(u64, String)> =
            app.session.mailbox.calendars.iter().filter(|c| c.origin.is_local()).map(|c| (c.id, c.name.clone())).take(LISTED).collect();
        out.sort_by_key(|c| c.1.to_lowercase());
        out
    }

    /// Folders on this computer the mail can go under (never an Outbox), by path.
    fn local_folders(app: &MailApp) -> Vec<(u64, String)> {
        let mb = &app.session.mailbox;
        let local: Vec<u64> = mb.accounts.iter().filter(|a| a.kind == mailcraft_model::AccountKind::Local).map(|a| a.id).collect();
        let mut out: Vec<(u64, String)> = mb
            .folders
            .iter()
            .filter(|f| local.contains(&f.account) && !mb.is_send_queue(f.id))
            .map(|f| (f.id, mb.folder_path(f.id)))
            .take(LISTED)
            .collect();
        out.sort_by_key(|f| f.1.to_lowercase());
        out
    }

    /// Start reading: on the host's thread when it has one; otherwise here, at once.
    fn start(&mut self, app: &mut MailApp) {
        let needs = matches!(self.look, Ok((true, _)));
        #[cfg(not(target_arch = "wasm32"))]
        if needs && !mailcraft_engine::import_pst::password_opens(&self.path, &self.password) {
            self.wrong_password = true;
            return;
        }
        self.wrong_password = false;
        self.progress = Arc::default();
        let places = json!({"places": self.places()});
        let job = match mailcraft_engine::import_pst::job(&app.session, &self.path, needs.then(|| self.password.clone()), false, &places) {
            Ok((job, _)) => job,
            Err(e) => {
                self.phase = Phase::Done(Err(e.in_lang(crate::i18n::current())));
                return;
            }
        };
        if let Some(spawn) = &app.services.import_pst {
            self.phase = Phase::Importing(spawn(PstJob { path: self.path.clone(), job, progress: self.progress.clone() }));
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        #[cfg(not(target_arch = "wasm32"))]
        mailcraft_engine::import_pst::read_path(&self.path, &job, &self.progress, &mut |e| tx.send(e).is_ok());
        #[cfg(target_arch = "wasm32")]
        let _ = (tx, job);
        self.phase = Phase::Importing(rx);
    }

    /// Hand what has arrived to the engine, a batch at a time, for as long as the turn's budget
    /// allows. Called from the window's logic, which the toolkit runs even when no frame is painted
    /// (the window hidden, minimised or on another desktop: #815), so the reading goes on there.
    pub fn pump(&mut self, app: &mut MailApp, ctx: &egui::Context) {
        if !self.is_importing() {
            return;
        }
        let budget = if self.painted { BUDGET_SHOWN } else { BUDGET_HIDDEN };
        self.painted = false;
        self.poll(app, budget);
        if self.is_importing() {
            // A turn with nothing painted asks for the next one itself.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn poll(&mut self, app: &mut MailApp, budget: Duration) {
        if self.opening != app.session.openings() {
            self.progress.cancel();
            self.phase = Phase::Done(Err(tl!("Another mailbox was opened: the import stopped.").to_string()));
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        #[cfg(target_arch = "wasm32")]
        let _ = budget;
        let mut taken = 0usize;
        loop {
            #[cfg(not(target_arch = "wasm32"))]
            let over = taken > 0 && started.elapsed() >= budget;
            #[cfg(target_arch = "wasm32")]
            let over = taken >= EVENTS_NO_CLOCK;
            if over {
                return;
            }
            taken += 1;
            let Phase::Importing(rx) = &self.phase else { return };
            let event = match rx.try_recv() {
                Ok(e) => e,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    mailcraft_engine::import_pst::end_staged(&mut app.session);
                    self.phase =
                        Phase::Done(Err(tl!("The import stopped unexpectedly. What came in stays; import the file again for the rest.").to_string()));
                    return;
                }
            };
            let params = json!({"staged": true, "name": mailcraft_engine::import_pst::folder_name(&self.path), "places": self.places()});
            match event {
                Event::Batch(batch) => {
                    if self.failed.is_some() {
                        continue;
                    }
                    app.session.staged_pst = Some(Staged::Batch(batch));
                    match app.run("file.importPst", params) {
                        Ok(_) => self.batches = self.batches.saturating_add(1),
                        Err(e) => {
                            self.progress.cancel();
                            self.failed = Some(e);
                        }
                    }
                    app.session.staged_pst = None;
                }
                Event::Done(Err(e)) => {
                    mailcraft_engine::import_pst::end_staged(&mut app.session);
                    let why = self.failed.take().unwrap_or_else(|| e.in_lang(crate::i18n::current()));
                    self.phase = Phase::Done(Err(if self.batches > 0 {
                        crate::i18n::fmt(tl!("{why} What came in before stays; import the file again for the rest."), &[("why", &why)])
                    } else {
                        why
                    }));
                }
                Event::Done(Ok(outcome)) => {
                    app.session.staged_pst = Some(Staged::Done(outcome));
                    let done = app.run("file.importPst", params);
                    app.session.staged_pst = None;
                    self.phase = Phase::Done(match self.failed.take() {
                        Some(e) => Err(e),
                        None => done,
                    });
                }
            }
        }
    }

    pub fn show(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| self.contents(app, ui, enter)).inner
    }

    fn contents(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool) -> bool {
        let t = Tokens::get(ui.ctx());
        self.painted = true;
        match &self.phase {
            Phase::Choose => self.choose(app, ui, enter, &t),
            Phase::Importing(_) => {
                self.importing(ui, &t);
                false
            }
            Phase::Done(done) => {
                match done {
                    Err(why) => {
                        ui.label(RichText::new(why.as_str()).color(t.red));
                    }
                    Ok(report) => done_page(ui, report, &t),
                }
                let mut close = enter;
                ui.add_space(4.0);
                button_row(ui, |ui| close |= ui.add(egui::Button::new(tl!("Close")).min_size(vec2(72.0, 26.0))).clicked());
                close
            }
        }
    }

    fn choose(&mut self, app: &mut MailApp, ui: &mut Ui, enter: bool, t: &Tokens) -> bool {
        ui.add(egui::Label::new(RichText::new(self.file()).strong()).truncate());
        let ready = match &self.look {
            Err(why) => {
                ui.label(RichText::new(crate::i18n::fmt(tl!("This file can't be imported: {why}"), &[("why", why)])).color(t.red));
                false
            }
            Ok((needs, format)) => {
                ui.label(RichText::new(crate::i18n::fmt(tl!(".pst file ({format})"), &[("format", format)])).color(t.text_dim));
                if *needs {
                    ui.add_space(4.0);
                    ui.label(tl!("This file has a password. Nothing is imported without it."));
                    ui.horizontal(|ui| {
                        ui.label(tl!("Password:"));
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true).desired_width(220.0));
                    });
                    if self.wrong_password {
                        ui.label(RichText::new(tl!("That isn't the password of this file.")).color(t.red));
                    }
                }
                true
            }
        };
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Put the mail")).strong());
        let name = mailcraft_engine::import_pst::folder_name(&self.path);
        ui.radio_value(&mut self.under, None, crate::i18n::fmt(tl!("In a new folder called {name} on this computer"), &[("name", &name)]));
        let folders = Self::local_folders(app);
        ui.horizontal(|ui| {
            let mut picked = self.under.is_some();
            if ui.radio(picked, tl!("Under a folder on this computer:")).clicked() && !picked {
                picked = true;
                self.under = folders.first().map(|f| f.0);
            }
            ui.add_enabled_ui(picked && !folders.is_empty(), |ui| {
                let shown = self.under.and_then(|id| folders.iter().find(|f| f.0 == id)).map_or(String::new(), |f| f.1.clone());
                egui::ComboBox::from_id_salt("pstUnder").width(240.0).selected_text(shown).show_ui(ui, |ui| {
                    for (id, path) in &folders {
                        ui.selectable_value(&mut self.under, Some(*id), path.as_str());
                    }
                });
            });
        });
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Put the contacts")).strong());
        ui.label(tl!("Into your contacts on this computer, each marked as imported from the file."));
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Put the calendar items")).strong());
        let calendars = Self::local_calendars(app);
        ui.radio_value(&mut self.calendar, None, tl!("In a new calendar named after the file on this computer"));
        ui.horizontal(|ui| {
            let mut picked = self.calendar.is_some();
            if ui.add_enabled(!calendars.is_empty(), egui::RadioButton::new(picked, tl!("In this calendar:"))).clicked() && !picked {
                picked = true;
                self.calendar = calendars.first().map(|c| c.0);
            }
            ui.add_enabled_ui(picked && !calendars.is_empty(), |ui| {
                let shown = self.calendar.and_then(|id| calendars.iter().find(|c| c.0 == id)).map_or(String::new(), |c| c.1.clone());
                egui::ComboBox::from_id_salt("pstCalendar").width(240.0).selected_text(shown).show_ui(ui, |ui| {
                    for (id, name) in &calendars {
                        ui.selectable_value(&mut self.calendar, Some(*id), name.as_str());
                    }
                });
            });
        });
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Put the tasks")).strong());
        ui.label(tl!("Into your to-do list on this computer. Nothing is sent to anybody."));
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("Keep the notes and journal entries")).strong());
        ui.label(tl!("On this computer. They have no views of their own yet."));
        ui.add_space(6.0);
        ui.label(RichText::new(tl!("A report is written beside your mailbox.")).color(t.text_dim));
        let can = ready && !(matches!(self.look, Ok((true, _))) && self.password.is_empty());
        let (ok, cancel) = buttons(ui, tl!("Import"), can);
        if cancel {
            return true;
        }
        if can && (ok || enter) {
            self.start(app);
        }
        false
    }

    fn importing(&self, ui: &mut Ui, t: &Tokens) {
        use std::sync::atomic::Ordering::Relaxed;
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        let p = &self.progress;
        let (done, total) = (p.done.load(Relaxed), p.total.load(Relaxed));
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new());
            let folder = p.folder();
            ui.add(
                egui::Label::new(crate::i18n::fmt(
                    tl!("Folder {folder} ({n} of {of})"),
                    &[("folder", &folder), ("n", &p.folders_done.load(Relaxed).to_string()), ("of", &p.folders_total.load(Relaxed).to_string())],
                ))
                .truncate(),
            );
        });
        ui.label(crate::i18n::fmt(tl!("{done} of {total}"), &[("done", &done.to_string()), ("total", &tln!(total as u64, "{n} item", "{n} items"))]));
        ui.add(egui::ProgressBar::new((done as f32 / total.max(1) as f32).clamp(0.0, 1.0)));
        let stopping = p.is_cancelled();
        ui.label(
            RichText::new(if stopping {
                tl!("Stopping… what has come in stays.")
            } else {
                tl!("Mail comes in as it is read. Cancel keeps what is in; importing the file again brings the rest.")
            })
            .color(t.text_dim),
        );
        ui.add_space(4.0);
        button_row(ui, |ui| {
            if ui.add_enabled(!stopping, egui::Button::new(tl!("Cancel")).min_size(vec2(72.0, 26.0))).clicked() {
                p.cancel();
            }
        });
    }
}

/// What the import came to, from its report.
fn done_page(ui: &mut Ui, r: &Value, t: &Tokens) {
    let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
    let folder = r.get("name").and_then(Value::as_str).unwrap_or_default();
    let only_mail = ["contacts", "lists", "events", "tasks", "stickyNotes", "journalEntries"].iter().all(|k| n(k) == 0);
    if r.get("cancelled").and_then(Value::as_bool) == Some(true) {
        ui.label(RichText::new(tl!("Stopped before the end. Import the file again to bring the rest.")).strong());
    }
    ui.label(
        RichText::new(crate::i18n::fmt(
            tl!("{added} brought in, of {found} found."),
            &[
                ("added", &if only_mail { tln!(n("added"), "{n} message", "{n} messages") } else { tln!(n("added"), "{n} item", "{n} items") }),
                ("found", &tln!(n("found"), "{n} item", "{n} items")),
            ],
        ))
        .strong(),
    );
    // (With contacts among them, what the items are is said line by line.)
    for (count, line) in [
        (if only_mail { 0 } else { n("messages") }, tln!(n("messages"), "{n} message", "{n} messages")),
        (n("contacts"), tln!(n("contacts"), "{n} contact", "{n} contacts")),
        (
            n("lists"),
            tln!(
                n("lists"),
                "{n} distribution list, kept as a contact listing its members",
                "{n} distribution lists, kept as contacts listing their members"
            ),
        ),
        (n("events"), tln!(n("events"), "{n} calendar item", "{n} calendar items")),
        (n("tasks"), tln!(n("tasks"), "{n} task", "{n} tasks")),
        (n("stickyNotes"), tln!(n("stickyNotes"), "{n} note", "{n} notes")),
        (n("journalEntries"), tln!(n("journalEntries"), "{n} journal entry", "{n} journal entries")),
    ] {
        if count > 0 {
            ui.label(line);
        }
    }
    if !folder.is_empty() {
        ui.label(crate::i18n::fmt(tl!("In {folder}"), &[("folder", folder)]));
    }
    for (count, line) in [
        (n("already"), tln!(n("already"), "{n} was here already", "{n} were here already")),
        (n("leftOut"), tln!(n("leftOut"), "{n} item couldn't be read and is left out", "{n} items couldn't be read and are left out")),
    ] {
        if count > 0 {
            ui.label(RichText::new(line).color(t.text_dim));
        }
    }
    let left: Vec<&Value> = r.get("leftOutItems").and_then(Value::as_array).into_iter().flatten().take(LEFT_OUT_SHOWN).collect();
    if !left.is_empty() {
        egui::ScrollArea::vertical().max_height(140.0).auto_shrink([false, true]).show(ui, |ui| {
            for e in left {
                let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or_default();
                ui.add(egui::Label::new(RichText::new(format!("{} — {}: {}", s("folder"), s("what"), s("why"))).color(t.text_dim)).truncate());
            }
        });
    }
    if let Some(path) = r.get("report").and_then(Value::as_str) {
        ui.add_space(4.0);
        ui.label(tl!("The full report is in:"));
        ui.add(egui::Label::new(RichText::new(path).monospace()).truncate());
    }
}

#[cfg(test)]
#[path = "import_pst_tests.rs"]
mod import_pst_tests;
