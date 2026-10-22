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
    /// Pick a file to open; `purpose` says what for (`mailbox`, `import`, `attach`).
    Open { purpose: String },
    /// Pick a path to save to, given a suggested file name.
    Save { name: String },
}

/// What to do with the path picked in a dialog.
pub(crate) enum AfterPick {
    /// Open the picked mailbox, bring the picked file in, or attach it (by purpose).
    Open(String),
    /// Run this command with the picked path as `path`.
    Export { command: String, params: Value },
}

pub struct PendingDialog {
    rx: Receiver<Option<String>>,
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
        "file.exportMessage" => clean(&s.focused_message().map(|m| m.subject.clone()).unwrap_or_default(), "eml"),
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
    if params.get("path").is_none() && params.get("data").is_none() {
        let purpose = match id {
            "file.open" => Some("mailbox"),
            "file.import" => Some("import"),
            "compose.attach" if app.session.compose.is_some() => Some("attach"),
            "signature.picture.add" if app.session.signature_edit.is_some() => Some("picture"),
            _ => None,
        };
        if let Some(purpose) = purpose {
            open(app, purpose);
            return Some(Ok(json!({"pending": purpose})));
        }
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

/// Pick a file to open (`mailbox`), bring in (`import`) or attach (`attach`).
pub(crate) fn open(app: &mut MailApp, purpose: &str) {
    if let Some(f) = &app.services.open_async {
        f(purpose);
        app.pending_open = Some(purpose.to_string());
        return;
    }
    let _ = ask(app, FileDialogRequest::Open { purpose: purpose.to_string() }, AfterPick::Open(purpose.to_string()));
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
    let Some(path) = picked else { return Some(Ok(json!({"cancelled": true}))) };
    let r = match d.after {
        AfterPick::Open(purpose) => opened_path(app, &purpose, &path),
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

/// What a file is for when nobody said: a mailbox opens, a message/calendar/contact file is
/// brought in, anything else is attached while a message is being written.
fn purpose_of(app: &MailApp, name: &str) -> &'static str {
    use mailcraft_engine::io::Format;
    match Format::from_extension(name) {
        Some(Format::Mailbox) => "mailbox",
        Some(_) if app.session.compose.is_none() => "import",
        _ if app.session.compose.is_some() => "attach",
        _ => "import",
    }
}

fn opened_path(app: &mut MailApp, purpose: &str, path: &str) -> Result<Value, String> {
    let r = match purpose {
        "mailbox" => app.run("file.open", json!({"path": path})),
        "attach" => app.run("compose.attach", json!({"path": path})),
        "picture" => {
            crate::signatures::add_picture(app, json!({"path": path}));
            return Ok(json!({"picture": path}));
        }
        _ => app.run("file.import", json!({"path": path})),
    };
    if r.is_ok() {
        app.ui.backstage = false;
        if purpose == "mailbox" {
            app.ui.recent.retain(|p| p != path);
            app.ui.recent.insert(0, path.to_string());
            app.ui.recent.truncate(10);
        }
    }
    r
}

/// A file dropped on the window, or something the program was started with: a file, a file
/// named the way desktops name one (`file://`), or a `mailto:` link, which starts a new message.
pub(crate) fn dropped_path(app: &mut MailApp, path: &str) {
    if path.trim().get(..7).is_some_and(|scheme| scheme.eq_ignore_ascii_case("mailto:")) {
        if let Err(e) = app.run("mail.new", json!({"mailto": path.trim()})) {
            app.status(e);
        }
        return;
    }
    let path = local_path(path);
    let purpose = purpose_of(app, &path);
    let _ = opened_path(app, purpose, &path);
}

/// The path a `file://` URL names (percent-decoded; `file:///C:/x` is `C:/x`, `file:///home/x`
/// is `/home/x`); anything else is taken to be a path already.
fn local_path(text: &str) -> String {
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

/// Files that arrived asynchronously (the web picker, drops in the browser).
pub(crate) fn drain_inbox(app: &mut MailApp) {
    let Some(inbox) = app.services.inbox.clone() else { return };
    let files = std::mem::take(&mut *inbox.lock().unwrap_or_else(|e| e.into_inner()));
    for (name, bytes) in files {
        let purpose = app.pending_open.take().unwrap_or_else(|| purpose_of(app, &name).to_string());
        let data = encode(&bytes);
        let r = match purpose.as_str() {
            "mailbox" => app.run("file.open", json!({"name": name, "data": data})),
            "attach" => app.run("compose.attach", json!({"name": name, "data": data})),
            _ => app.run("file.import", json!({"name": name, "data": data})),
        };
        if r.is_ok() {
            app.ui.backstage = false;
        }
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
