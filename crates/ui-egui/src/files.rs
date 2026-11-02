// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files coming in and going out: picking a mailbox to open, something to import or attach, and
//! where an export goes.
//!
//! Desktop: the host shows a native dialog without blocking the window ([`Services::file_dialog`])
//! and the answer is picked up on a later frame. Browser: a picked or dropped file arrives as bytes
//! through [`Services::inbox`], and whatever is saved is handed over as a download.
//!
//! [`Services::file_dialog`]: crate::Services::file_dialog
//! [`Services::inbox`]: crate::Services::inbox

use std::sync::mpsc::{Receiver, TryRecvError};

use base64_lite::encode;
use serde_json::{Value, json};

use crate::MailApp;

/// A native file dialog the host is asked to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialogRequest {
    /// Pick a file to open; `purpose` says what for (`mailbox`, `import`, `attach`). Files to
    /// attach may be picked several at once ([`FileDialogRequest::picks_many`]).
    Open { purpose: String },
    /// Pick a folder (to import the mbox files under it).
    Folder,
    /// Pick a path to save to, given a suggested file name.
    Save { name: String },
}

/// What to do with the path picked in a dialog.
pub(crate) enum AfterPick {
    /// Open the picked mailbox, bring the picked file in, or attach it (by purpose), to the
    /// message being written that asked for it (#286: the keyboard may be elsewhere by then).
    Open(String, Option<u64>),
    /// Run this command with the picked path as `path`.
    Export { command: String, params: Value },
    /// Show what is in the picked folder, ready to import ([`crate::import_tree`]).
    Tree,
}

impl FileDialogRequest {
    /// Whether the dialog lets the owner pick several files: only to attach them, each in turn.
    pub fn picks_many(&self) -> bool {
        matches!(self, FileDialogRequest::Open { purpose } if purpose == "attach")
    }
}

pub struct PendingDialog {
    rx: Receiver<Option<Vec<String>>>,
    after: AfterPick,
}

/// Commands that write a file, and the name they suggest.
fn export_name(app: &MailApp, id: &str, params: &Value) -> Option<String> {
    let s = &app.session;
    // The same names the commands give: made safe by `opening`, as every file named from mail is.
    let clean = mailcraft_engine::opening::export_name;
    Some(match id {
        "file.saveAs" => format!("mailbox.{}", mailcraft_engine::io::MAILBOX_EXT),
        "file.save" if s.path.is_none() && app.services.persist.is_none() => format!("mailbox.{}", mailcraft_engine::io::MAILBOX_EXT),
        // The message named (a Save As that waited for its download names it), else the selected,
        // by the name the command gives it.
        "file.exportMessage" => {
            let message = params.get("id").and_then(Value::as_u64).and_then(|id| s.mailbox.message(id)).or_else(|| s.focused_message());
            message.map_or_else(|| clean("message", "eml"), mailcraft_engine::cmd::file::eml_name)
        }
        "file.exportFolder" => clean(&s.mailbox.folder(s.view.folder).map(|f| f.name.clone()).unwrap_or_default(), "mbox"),
        "file.exportCalendar" => clean(
            &params.get("id").and_then(Value::as_u64).and_then(|id| s.mailbox.event(id)).map_or("calendar".to_string(), |e| e.title.clone()),
            "ics",
        ),
        "file.exportContacts" => "contacts.vcf".to_string(),
        // Report a Problem on GitHub with the diagnostics: a file to attach on the page.
        "ui.reportProblem" if crate::report_problem::saves_file(params) => crate::report_problem::file_name(app),
        "mail.saveAttachment" => {
            let index = params.get("index").and_then(Value::as_u64)? as usize;
            let id = params.get("id").and_then(Value::as_u64).or(s.sel.focus())?;
            // Only the file's own name, cleaned: a message can name its attachment anything.
            mailcraft_engine::opening::save_name(&s.mailbox.message(id)?.attachments.get(index)?.name)
        }
        _ => return None,
    })
}

/// A user-run command that writes a file and has no `path` yet: ask where (desktop) or download
/// what it produces (browser). `None` for every other command.
pub(crate) fn intercept(app: &mut MailApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    // Open, Import and Attach without a file: pick one.
    if id == "file.importFolderTree" && params.get("path").is_none() && params.get("entries").is_none() && params.get("staged").is_none() {
        // A host that can't ask for a folder lets the command say what it needs.
        app.services.file_dialog.as_ref()?;
        return Some(ask(app, FileDialogRequest::Folder, AfterPick::Tree));
    }
    if params.get("path").is_none() && params.get("data").is_none() {
        let purpose = match id {
            "file.open" => Some("mailbox"),
            "file.import" => Some("import"),
            "compose.attach" if app.session.compose.is_some() => Some("attach"),
            "signature.picture.add" if app.session.signature_edit.is_some() => Some("picture"),
            "insert.picture" if app.session.compose.is_some() && app.session.signature_edit.is_none() => Some("bodyPicture"),
            _ => None,
        };
        if let Some(purpose) = purpose {
            open(app, purpose);
            return Some(Ok(json!({"pending": purpose})));
        }
    }
    // Save All Attachments: into a folder the owner picks (the browser downloads each file).
    // (Of the message named, which a window of its own names whatever is selected; one with no
    // files to save says so without a folder being asked for.)
    let has_files = || {
        let named = params.get("id").and_then(Value::as_u64).or(app.session.sel.focus());
        named.and_then(|m| app.session.mailbox.message(m)).is_some_and(|m| m.listed_attachments().next().is_some())
    };
    if id == "mail.saveAllAttachments" && params.get("path").is_none() && has_files() {
        if app.services.download.is_some() {
            return Some(download_all(app, params.clone()));
        }
        app.services.file_dialog.as_ref()?;
        return Some(ask(app, FileDialogRequest::Folder, AfterPick::Export { command: id.to_string(), params: params.clone() }));
    }
    if params.get("path").is_some() || !app.enabled(id) {
        return None;
    }
    let name = export_name(app, id, params)?;
    if app.services.download.is_some() {
        return Some(download(app, id, params.clone()));
    }
    // No way to ask: let the command answer with its bytes (scripts, tests).
    app.services.file_dialog.as_ref()?;
    Some(ask(app, FileDialogRequest::Save { name }, AfterPick::Export { command: id.to_string(), params: params.clone() }))
}

/// Run an export without a path and hand its bytes to the browser.
fn download(app: &mut MailApp, id: &str, params: Value) -> Result<Value, String> {
    let r = app.execute(id, params)?;
    let (Some(name), Some(data)) = (r.get("name").and_then(Value::as_str), r.get("data").and_then(Value::as_str)) else { return Ok(r) };
    let bytes = base64_lite::decode(data).ok_or_else(|| "the export wasn't readable".to_string())?;
    let Some(save) = &app.services.download else { return Ok(r) };
    save(name, &bytes)?;
    app.status(crate::i18n::fmt(tl!("Downloaded {name}"), &[("name", name)]));
    Ok(json!({"name": name, "bytes": bytes.len(), "downloaded": true}))
}

/// Save All Attachments in the browser: each file handed over as a download of its own.
fn download_all(app: &mut MailApp, params: Value) -> Result<Value, String> {
    let r = app.execute("mail.saveAllAttachments", params)?;
    let Some(save) = &app.services.download else { return Ok(r) };
    let mut n = 0usize;
    for f in r.get("files").and_then(Value::as_array).into_iter().flatten() {
        let (Some(name), Some(data)) = (f.get("name").and_then(Value::as_str), f.get("data").and_then(Value::as_str)) else { continue };
        let bytes = base64_lite::decode(data).ok_or_else(|| "the attachment wasn't readable".to_string())?;
        save(name, &bytes)?;
        n += 1;
    }
    app.status(tln!(n, "Downloaded {n} attachment", "Downloaded {n} attachments"));
    Ok(json!({"saved": n, "downloaded": true}))
}

/// Pick a file to open (`mailbox`), bring in (`import`) or attach (`attach`).
pub(crate) fn open(app: &mut MailApp, purpose: &str) {
    let asking = if matches!(purpose, "attach" | "bodyPicture") { app.session.compose_id() } else { None };
    if let Some(f) = &app.services.open_async {
        // (What comes of it says itself what it was picked for: [`crate::Delivered`].)
        f(purpose, asking);
        return;
    }
    let _ = ask(app, FileDialogRequest::Open { purpose: purpose.to_string() }, AfterPick::Open(purpose.to_string(), asking));
}

fn ask(app: &mut MailApp, req: FileDialogRequest, after: AfterPick) -> Result<Value, String> {
    if app.file_dialog.is_some() {
        app.status(tl!("A file dialog is already open."));
        return Ok(json!({"pending": "fileDialog"}));
    }
    let Some(show) = &app.services.file_dialog else {
        let msg = tl!("File dialogs aren't available here").to_string();
        app.status(msg.clone());
        return Err(msg);
    };
    app.file_dialog = Some(PendingDialog { rx: show(req), after });
    // A host that answered at once is handled right away.
    poll(app).unwrap_or(Ok(json!({"pending": "fileDialog"})))
}

/// Pick up the answer of an open file dialog, if it has arrived (every frame, from `logic`), and
/// finish what it was for. `None` while there is nothing to finish.
pub(crate) fn poll(app: &mut MailApp) -> Option<Result<Value, String>> {
    let picked = match app.file_dialog.as_ref().map(|d| d.rx.try_recv()) {
        None | Some(Err(TryRecvError::Empty)) => return None,
        Some(Ok(picked)) => picked,
        // The host dropped the dialog without an answer: as good as cancelled.
        Some(Err(TryRecvError::Disconnected)) => None,
    };
    let d = app.file_dialog.take()?;
    let mut picked = picked.unwrap_or_default();
    // Several files to attach: each in turn, and one that can't be (too large, unreadable) doesn't
    // keep the others out.
    if let AfterPick::Open(purpose, asking) = &d.after
        && purpose == "attach"
        && picked.len() > 1
    {
        return Some(attach_all(app, &picked, *asking));
    }
    // A dialog for one file answers with one; anything more is not taken.
    picked.truncate(1);
    let Some(path) = picked.pop() else { return Some(Ok(json!({"cancelled": true}))) };
    let r = match d.after {
        AfterPick::Open(purpose, asking) => opened_path(app, &purpose, &path, asking),
        AfterPick::Tree => app.run("ui.dialog", json!({"name": "importTree", "path": path})),
        AfterPick::Export { command, mut params } => {
            if let Some(o) = params.as_object_mut() {
                o.insert("path".into(), json!(path));
            }
            let r = app.execute(&command, params);
            // (Report a Problem says itself where its file went and what to do with it.)
            if r.is_ok() && command == "ui.reportProblem" {
                // The file is saved and GitHub's page open: now the window it came from closes.
                if matches!(app.dialog, Some(crate::dialogs::Dialog::ReportProblem(_))) {
                    app.dialog = None;
                }
            } else if let (Ok(v), "mail.saveAllAttachments") = (&r, command.as_str()) {
                let n = v.get("saved").and_then(Value::as_u64).unwrap_or(0) as usize;
                app.status(tln!(n, "Saved {n} attachment", "Saved {n} attachments"));
            } else if r.is_ok() {
                let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                app.status(crate::i18n::fmt(tl!("Saved {name}"), &[("name", &name)]));
            }
            r
        }
    };
    if let Err(e) = &r {
        app.status(e.clone());
    }
    Some(r)
}

/// The most files attached from one pick: a person picks a handful, a stuck key a great many.
pub const MOST_ATTACHED: usize = 100;

/// The most bytes one file to attach may have: a larger one is never read
/// ([`mailcraft_engine::cmd::compose::MAX_ATTACHMENT`], which refuses it after reading).
pub const MOST_ATTACHMENT_BYTES: u64 = mailcraft_engine::cmd::compose::MAX_ATTACHMENT as u64;

/// The most bytes read for one pick together: on the desktop that much memory, in the browser
/// more than a page can hold at all, and the message being written would go down with it.
pub const MOST_ATTACHED_BYTES: u64 = 256 << 20;

/// Which of a pick's files are read, from their sizes alone, before any is read: in order, at
/// most [`MOST_ATTACHED`], none larger than [`MOST_ATTACHMENT_BYTES`] (never read; a message
/// couldn't carry it), and none from the first that would take the pick past
/// [`MOST_ATTACHED_BYTES`] together. `None` is read; `Some` says why it isn't. The host that
/// picks in the browser does this before reading anything, and so does the desktop.
pub fn reads_of_pick(sizes: &[u64]) -> Vec<Option<crate::Unread>> {
    let mut total: u64 = 0;
    let mut past_total = false;
    sizes
        .iter()
        .enumerate()
        .map(|(i, size)| {
            if i >= MOST_ATTACHED {
                return Some(crate::Unread::PastMost);
            }
            if *size > MOST_ATTACHMENT_BYTES {
                return Some(crate::Unread::TooLarge);
            }
            past_total |= total.saturating_add(*size) > MOST_ATTACHED_BYTES;
            if past_total {
                return Some(crate::Unread::PastTotal);
            }
            total = total.saturating_add(*size);
            None
        })
        .collect()
}

/// Attach each of `paths`, in the order the dialog lists them, to the message being written
/// `asking` (#286), reading only what [`reads_of_pick`] allows by their sizes. Said once: how
/// many went in, and why one didn't ([`attached_said`]).
fn attach_all(app: &mut MailApp, paths: &[String], asking: Option<u64>) -> Result<Value, String> {
    // A file whose size can't be read is read as a file of no size: attaching it says what is wrong.
    let sizes: Vec<u64> = paths.iter().map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)).collect();
    let mut batch = Batch::default();
    for (path, unread) in paths.iter().zip(reads_of_pick(&sizes)) {
        match unread {
            None => batch.took(app.run("compose.attach", on_message(json!({"path": path}), asking))),
            Some(why) => batch.unread(path.rsplit(['/', '\\']).next().unwrap_or(path), &why),
        }
    }
    attached_said(app, batch)
}

/// What attaching several files came to: how many went in, the first reason one didn't, and
/// how many weren't read at all, past the most files or the most bytes of one pick.
#[derive(Default)]
struct Batch {
    attached: usize,
    failed: Option<String>,
    past_most: usize,
    past_total: usize,
}

impl Batch {
    fn took(&mut self, r: Result<Value, String>) {
        match r {
            Ok(_) => self.attached += 1,
            Err(e) => self.failed = self.failed.take().or(Some(e)),
        }
    }

    /// `name` wasn't read, for `why`.
    fn unread(&mut self, name: &str, why: &crate::Unread) {
        match why {
            crate::Unread::Failed(e) => self.took(Err(e.clone())),
            crate::Unread::TooLarge => {
                let mb = (MOST_ATTACHMENT_BYTES >> 20).to_string();
                self.took(Err(crate::i18n::fmt(tl!("{name} is larger than {mb} MB"), &[("name", name), ("mb", &mb)])));
            }
            crate::Unread::PastMost => self.past_most += 1,
            crate::Unread::PastTotal => self.past_total += 1,
        }
    }
}

/// Say once what attaching a pick of files came to: how many went in, the first reason one
/// didn't, and that the rest were left out when more than [`MOST_ATTACHED`] were picked or
/// they would come to more than [`MOST_ATTACHED_BYTES`] together. An error only when none
/// went in.
fn attached_said(app: &mut MailApp, batch: Batch) -> Result<Value, String> {
    app.ui.backstage = false;
    let Batch { attached, failed, past_most, past_total } = batch;
    let mut said = tln!(attached, "Attached {n} file", "Attached {n} files");
    if past_most > 0 {
        let most = MOST_ATTACHED.to_string();
        let rest = tln!(past_most, "{n} more was left out: at most {most} files at a time", "{n} more were left out: at most {most} files at a time");
        said = format!("{said} · {}", crate::i18n::fmt(&rest, &[("most", &most)]));
    }
    if past_total > 0 {
        let mb = (MOST_ATTACHED_BYTES >> 20).to_string();
        let rest = tln!(past_total, "{n} more was left out: at most {mb} MB at a time", "{n} more were left out: at most {mb} MB at a time");
        said = format!("{said} · {}", crate::i18n::fmt(&rest, &[("mb", &mb)]));
    }
    let mut r = json!({"attached": attached});
    if past_most + past_total > 0 {
        r["leftOut"] = json!(past_most + past_total);
    }
    match failed {
        Some(e) if attached == 0 => {
            app.status(e.clone());
            Err(e)
        }
        Some(e) => {
            app.status(format!("{said} · {e}"));
            r["failed"] = json!(e);
            Ok(r)
        }
        None => {
            app.status(said);
            Ok(r)
        }
    }
}

/// What a file is for when nobody said: a mailbox opens, a message/calendar/contact file is
/// brought in, anything else is attached while a message is being written.
/// (The message is the one the window with the keyboard writes, [`crate::windows::compose_window::target`]:
/// from the main window, the reading pane's alone, never one in a window behind it.)
fn purpose_of(app: &MailApp, name: &str) -> &'static str {
    use mailcraft_engine::io::Format;
    let writing = crate::windows::compose_window::target(app).is_some();
    match Format::from_extension(name) {
        Some(Format::Mailbox) => "mailbox",
        Some(_) if !writing => "import",
        _ if writing => "attach",
        _ => "import",
    }
}

/// `params` for a command on the message being written `asking` (none: the current one).
fn on_message(mut params: Value, asking: Option<u64>) -> Value {
    if let (Some(compose), Some(map)) = (asking, params.as_object_mut()) {
        map.insert("compose".into(), json!(compose));
    }
    params
}

fn opened_path(app: &mut MailApp, purpose: &str, path: &str, asking: Option<u64>) -> Result<Value, String> {
    let r = match purpose {
        "mailbox" => app.run("file.open", json!({"path": path})),
        "attach" => app.run("compose.attach", on_message(json!({"path": path}), asking)),
        "bodyPicture" => app.run("insert.picture", on_message(json!({"path": path}), asking)),
        "picture" => {
            crate::signatures::add_picture(app, json!({"path": path}));
            return Ok(json!({"picture": path}));
        }
        _ => app.run("file.import", json!({"path": path})),
    };
    // (A mailbox goes into Recent, and File closes, once it is open: `MailApp::run`.)
    if r.is_ok() && purpose != "mailbox" {
        app.ui.backstage = false;
    }
    r
}

/// The mailbox file at `path` was opened: first in File's Recent, and File closes.
pub(crate) fn remember_recent(app: &mut MailApp, path: &str) {
    app.ui.backstage = false;
    app.ui.recent.retain(|p| p != path);
    app.ui.recent.insert(0, path.to_string());
    app.ui.recent.truncate(10);
}

/// A file dropped on the window, or something the program was started with: a file, a file
/// named the way desktops name one (`file://`), or a `mailto:` link, which starts a new message.
pub(crate) fn dropped_path(app: &mut MailApp, path: &str) {
    if is_mailto_link(path) {
        // A link from outside doesn't destroy a message being written: it is saved to Drafts
        // first, and if it can't be, the link waits for the owner.
        let saved = app.session.compose.as_ref().is_some_and(|c| c.dirty);
        if saved && let Err(e) = app.run("compose.saveDraft", json!({})) {
            app.status(crate::i18n::fmt(
                tl!("A link asked for a new message, but the one you are writing couldn't be saved first: {why}"),
                &[("why", &e)],
            ));
            return;
        }
        if let Err(e) = app.run("mail.new", json!({"mailto": path.trim()})) {
            app.status(e);
        } else if saved {
            app.status(tl!("The message you were writing was saved to Drafts; a link opened a new one"));
        }
        return;
    }
    let path = local_path(path);
    let purpose = purpose_of(app, &path);
    let _ = opened_path(app, purpose, &path, crate::windows::compose_window::target(app));
}

/// The most calendar and contact files waiting to be asked about; more are left alone.
const MOST_WAITING: usize = 32;

/// [`MailApp::open_from_system`].
pub(crate) fn from_system(app: &mut MailApp, what: &str) {
    use mailcraft_engine::io::Format;
    if is_mailto_link(what) {
        return dropped_path(app, what);
    }
    let path = local_path(what);
    match Format::from_extension(&path) {
        Some(Format::Eml) => {
            // Read in a window of its own and filed nowhere (#515): the owner keeps it with Move
            // to Folder or Save As, and closing the window leaves nothing in the mailbox.
            app.ui.backstage = false;
            let opened = crate::opened_message::read(&path).and_then(|o| app.windows.add_file(o.message, o.bytes));
            if let Err(e) = opened {
                app.status(e);
            }
        }
        Some(Format::Ics | Format::Vcf) => {
            let asking = matches!(&app.dialog, Some(crate::dialogs::Dialog::Confirm { command, params, .. })
                if command == "file.import" && params.get("path").and_then(Value::as_str) == Some(path.as_str()));
            if !asking && app.opened_waiting.len() < MOST_WAITING && !app.opened_waiting.contains(&path) {
                app.opened_waiting.push_back(path);
            }
            ask_next(app);
        }
        _ => dropped_path(app, &path),
    }
}

/// Save As in the window of a message file ([`crate::opened_message`]): the file's own bytes,
/// where the owner says (a download in the browser).
pub(crate) fn save_opened(app: &mut MailApp, window: u64, m: &mailcraft_model::Message, bytes: &[u8]) {
    let name = mailcraft_engine::cmd::file::eml_name(m);
    if let Some(save) = &app.services.download {
        match save(&name, bytes) {
            Ok(()) => app.status(crate::i18n::fmt(tl!("Downloaded {name}"), &[("name", &name)])),
            Err(e) => app.status(e),
        }
        return;
    }
    let _ = ask(
        app,
        FileDialogRequest::Save { name },
        AfterPick::Export { command: "ui.saveWindowMessage".to_string(), params: json!({"window": window}) },
    );
}

/// Whether `text` is a `mailto:` link, by its scheme.
fn is_mailto_link(text: &str) -> bool {
    text.trim().get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:"))
}

/// The next calendar or contact file the system asked MailCraft to open, asked about once
/// nothing else is: Import runs `file.import` with it, Cancel leaves it. The question is the
/// window's, so only the owner answers it (an agent can't name a file to import).
pub(crate) fn ask_next(app: &mut MailApp) {
    if app.dialog.is_some() {
        return;
    }
    let Some(path) = app.opened_waiting.pop_front() else { return };
    app.dialog =
        Some(crate::dialogs::Dialog::Confirm { command: "file.import".to_string(), params: json!({"path": path}), ask: import_question(&path) });
}

/// What the owner is asked before a calendar or contact file from outside is imported.
pub(crate) fn import_question(path: &str) -> crate::dialogs::Question {
    use mailcraft_engine::io::Format;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    let (title, text) = if Format::from_extension(path) == Some(Format::Vcf) {
        (tl!("Import Contacts"), crate::i18n::fmt(tl!("Import the contacts in {name} into Contacts?"), &[("name", &name)]))
    } else {
        (tl!("Import Calendar File"), crate::i18n::fmt(tl!("Import the appointments and tasks in {name} into your calendar?"), &[("name", &name)]))
    };
    crate::dialogs::Question::new(title, text, tl!("Import"), Value::Null)
}

/// The path a `file://` URL names (percent-decoded; `file:///C:/x` is `C:/x`, `file:///home/x`
/// is `/home/x`); anything else is taken to be a path already.
pub fn local_path(text: &str) -> String {
    let Some(rest) = text.get(..7).filter(|scheme| scheme.eq_ignore_ascii_case("file://")).and_then(|_| text.get(7..)) else {
        return text.to_string();
    };
    // Only this computer's files: `file://host/share` is left for the system to refuse.
    let Some(path) = rest.strip_prefix("localhost/").map(|p| format!("/{p}")).or_else(|| rest.starts_with('/').then(|| rest.to_string())) else {
        return text.to_string();
    };
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let hex = |at: usize| bytes.get(at).and_then(|h| char::from(*h).to_digit(16)).and_then(|d| u8::try_from(d).ok());
        match (b, hex(i + 1), hex(i + 2)) {
            (b'%', Some(high), Some(low)) => {
                out.push((high << 4) | low);
                i += 3;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    let path = String::from_utf8_lossy(&out).into_owned();
    // A drive letter after the leading slash is a Windows path.
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('/'), Some(drive), Some(':')) if drive.is_ascii_alphabetic() => path.get(1..).unwrap_or("").to_string(),
        _ => path,
    }
}

/// Files that arrived asynchronously: from the web picker, each for what it was opened for and
/// for the message that asked ([`crate::Delivered::asked`]); dropped in the browser, each for
/// what its kind says, to the message with the keyboard. A picker that was cancelled delivers
/// nothing, so what it was opened for is never what a dropped file is taken for. Files picked
/// to attach arrive together, and the files of one pick for one message are a batch, as on the
/// desktop: each in turn, and said once ([`attached_said`]); two picks for the same message
/// ([`crate::Delivered::pick`]) are said each by itself.
pub(crate) fn drain_inbox(app: &mut MailApp) {
    let take = |inbox: &Option<crate::Inbox>| {
        inbox.as_ref().map(|inbox| std::mem::take(&mut *inbox.lock().unwrap_or_else(|e| e.into_inner()))).unwrap_or_default()
    };
    let mut files = take(&app.services.picked);
    files.extend(take(&app.services.inbox));
    let mut files = files.into_iter().peekable();
    while let Some(first) = files.next() {
        let Some(("attach", asking)) = first.asked.as_ref().map(|(purpose, asking)| (purpose.as_str(), *asking)) else {
            delivered_one(app, first);
            continue;
        };
        // The rest of its pick: what follows it, picked to attach for the same message in the
        // same pick (a pick is delivered whole, under one lock).
        let number = first.pick;
        let mut pick = vec![first];
        while let Some(next) = files.next_if(|d| d.pick == number && d.asked.as_ref().is_some_and(|(p, a)| p == "attach" && *a == asking)) {
            pick.push(next);
        }
        if pick.len() == 1
            && let Some(one) = pick.pop()
        {
            delivered_one(app, one);
            continue;
        }
        let mut batch = Batch::default();
        for d in pick {
            match d.bytes {
                Ok(bytes) => batch.took(app.run("compose.attach", on_message(json!({"name": d.name, "data": encode(&bytes)}), asking))),
                Err(why) => batch.unread(&d.name, &why),
            }
        }
        let _ = attached_said(app, batch);
    }
}

/// One file that arrived: opened, brought in, attached or put in the body, as it was picked for.
fn delivered_one(app: &mut MailApp, d: crate::Delivered) {
    let (purpose, asking) = d.asked.unwrap_or_else(|| (purpose_of(app, &d.name).to_string(), crate::windows::compose_window::target(app)));
    let bytes = match d.bytes {
        Ok(bytes) => bytes,
        Err(why) => {
            let mut batch = Batch::default();
            batch.unread(&d.name, &why);
            let _ = attached_said(app, batch);
            return;
        }
    };
    let (name, data) = (d.name, encode(&bytes));
    let r = match purpose.as_str() {
        "mailbox" => app.run("file.open", json!({"name": name, "data": data})),
        "attach" => app.run("compose.attach", on_message(json!({"name": name, "data": data}), asking)),
        "bodyPicture" => app.run("insert.picture", on_message(json!({"file": name, "data": data}), asking)),
        _ => app.run("file.import", json!({"name": name, "data": data})),
    };
    if r.is_ok() {
        app.ui.backstage = false;
    }
}

/// Base64 for handing bytes to commands (the engine has the full codec; the front end only needs
/// these two directions and no dependency for them).
pub(crate) mod base64_lite {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
                if i <= chunk.len() {
                    out.push(TABLE.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A') as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        let (mut acc, mut bits) = (0u32, 0u32);
        for c in text.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
            let v = TABLE.iter().position(|t| *t == c)? as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
                acc &= (1 << bits) - 1;
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod attach_many_tests;

#[cfg(test)]
mod export_name_tests;

#[cfg(test)]
mod system_tests;

#[cfg(test)]
mod tests {
    use super::base64_lite::{decode, encode};
    use super::local_path;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(local_path("file:///home/avery/Mail/a%20note.eml"), "/home/avery/Mail/a note.eml");
        assert_eq!(local_path("FILE:///C:/Users/avery/caf%C3%A9.ics"), "C:/Users/avery/café.ics");
        assert_eq!(local_path("file://localhost/tmp/x.vcf"), "/tmp/x.vcf");
        // Not this computer's, not a URL, or broken: left as it came.
        assert_eq!(local_path("file://server/share/x.eml"), "file://server/share/x.eml");
        assert_eq!(local_path("C:\\mail\\100%.eml"), "C:\\mail\\100%.eml");
        assert_eq!(local_path("file:///tmp/100%zz%"), "/tmp/100%zz%");
        assert_eq!(local_path("file"), "file");
        assert_eq!(local_path(""), "");
    }

    #[test]
    fn base64_round_trips() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"abcd", &[0u8, 255, 7, 200, 13]] {
            assert_eq!(decode(&encode(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(encode(b"hello"), "aGVsbG8=");
        assert_eq!(decode("aGVs\nbG8="), Some(b"hello".to_vec()));
        assert_eq!(decode("***"), None);
    }
}
