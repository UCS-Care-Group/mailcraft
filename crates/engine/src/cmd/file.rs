// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File: the mailbox file, importing and exporting, options.

use mailcraft_model::{Id, Mailbox};
use mailcraft_time::Weekday;
use serde_json::{Value, json};

use super::{bad, deliver, fail, source};
use crate::io::{Format, MAILBOX_EXT};
use crate::opening::{export_name, export_name_or};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("file.new", "New Mailbox", "File", new).pure().params(r#"{"sample"?: bool (start with the sample mailbox)}"#),
        CommandSpec::new("file.open", "Open Mailbox", "File › Open & Export", open).pure().key("Mod+O").params(r#"{"path": string} | {"name": string, "data": base64}"#),
        CommandSpec::new("file.save", "Save Mailbox", "File", save).pure().key("Mod+Shift+S").params(
            r#"{"path"?: string} — the mailbox file, with its messages' content in the folder `<file>.blobs` beside it; without a path or an open file, answers one self-contained file's bytes as base64 `data`"#,
        ),
        CommandSpec::new("file.saveAs", "Save Mailbox As", "File › Open & Export", save_as)
            .pure()
            .params(r#"{"path": string} — everything in one self-contained file; a copy, unless the mailbox wasn't kept anywhere yet"#),
        CommandSpec::new("file.import", "Import", "File › Open & Export", import)
            .params(r#"{"path": string} | {"name": string, "data": base64}, "folder"?: number — .eml, .mbox, .ics, .vcf; messages meant for a Microsoft account go to the local folder Imported (`local`), or into its Drafts to this computer's Drafts, as drafts (`drafts`); `"opened": true` (a message file the system asked to open) puts one .eml into the local folder Imported as a received message, or finds the one already there (`opened`, `already`), whatever `folder` says"#),
        CommandSpec::new("file.importFolderTree", "Import a Folder of Mailboxes", "File › Open & Export", import_tree).params(
            r#"{"path": string (a folder) | "entries": [{"path": "Mail/Inbox.sbd/Sent", "data": base64}], "name"?: string (the new top folder; default the folder's name), "dryRun"?: bool (only say what would come in)} — every mbox file under the folder, found by its first bytes, into subfolders of one folder On this computer; a second import adds nothing already there (Message-ID)"#,
        ),
        CommandSpec::new("file.exportMessage", "Save Message As", "File › Save As", export_message).pure().params(r#"{"id"?: number, "path"?: string} — one message as .eml"#),
        CommandSpec::new("file.exportFolder", "Export Folder", "File › Open & Export", export_folder).pure().params(r#"{"id"?: number, "path"?: string} — a folder's messages as .mbox"#),
        CommandSpec::new("file.exportCalendar", "Save Calendar", "File › Save Calendar", export_calendar).pure().params(r#"{"calendar"?: number, "id"?: number (one event), "path"?: string} — .ics"#),
        CommandSpec::new("file.exportContacts", "Export Contacts", "File › Open & Export", export_contacts).pure().params(r#"{"ids"?: [number], "path"?: string} — .vcf"#),
        CommandSpec::new("file.info", "Info", "File › Info", info).pure(),
        CommandSpec::new("file.options", "Options", "File", options).pure().params(
            r#"{"twelveHour"?: bool, "weekStart"?: "sun"|"mon"|…, "workDays"?: ["mon",…], "workStart"?: 0-23, "workEnd"?: 1-24, "markReadOnOpen"?: bool, "defaultReminder"?: minutes | null, "quoteOnReply"?: bool, "previewLines"?: 0-3, "syncLimit"?: number, "syncMinutes"?: 0-1440 (0: only when asked)}"#,
        ),
    ]
}

/// Before another mailbox takes this one's place. Where a host keeps the mailbox file up to date
/// by itself (the desktop app's background saver), what it hasn't written yet is written now:
/// replacing the mailbox must not lose the changes that fall between two saves. That host's
/// window doesn't wait out a long save the saver is in the middle of (the first after a large
/// download): the mailbox stays open and says so. Anywhere else, leaving a mailbox unsaved is the
/// caller's business, as it always was.
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
fn settle(s: &Session) -> Result<(), crate::CmdError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(path) = &s.path
            && crate::store::followed(path)
        {
            crate::store::save_within(path, &s.mailbox, SETTLE_WAIT)
                .map_err(|e| failed!("the mailbox couldn't be saved first, so it stays open ({e})", e))?;
        }
    }
    Ok(())
}

/// How long [`settle`] waits for a save in the background to be done with the file.
#[cfg(not(target_arch = "wasm32"))]
const SETTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

fn new(s: &mut Session, v: &Value) -> CmdResult {
    settle(s)?;
    let mailbox =
        if p::bool(v, "sample") == Some(true) { crate::sample::mailbox(s.now(), s.offset_at(s.now())) } else { Mailbox::with_local_account("", "") };
    s.replace_mailbox(mailbox);
    s.path = None;
    Ok(json!({"sample": p::bool(v, "sample") == Some(true)}))
}

/// What opening a mailbox file left out, for `file.open` to say.
#[derive(Default)]
struct Left {
    /// Bodies and attachments that couldn't be read back.
    unavailable: usize,
    /// Saves left out because a record of theirs is damaged, and the bytes of the file not read.
    lost_saves: u32,
    unfinished: u64,
    /// Items of the mailbox file's first save that are damaged and were left out.
    lost_records: u32,
    /// All of it in a sentence for the owner (`store::Opened::notice`).
    notice: Option<String>,
}

/// The mailbox `file.open` is pointed at, and what of it couldn't be read back.
fn read(v: &Value) -> Result<(Mailbox, Left), crate::CmdError> {
    // A file on disk is opened where it is: it may keep its content in a folder beside it.
    #[cfg(not(target_arch = "wasm32"))]
    {
        if v.get("data").is_none()
            && let Some(path) = p::str(v, "path")
        {
            if Format::from_extension(path).is_some_and(|f| f != Format::Mailbox) {
                return Err(failed!("{path} isn't a mailbox; bring it in with File › Import", path));
            }
            let opened = crate::store::open(std::path::Path::new(path)).map_err(fail)?;
            let left = Left {
                unavailable: opened.unavailable,
                lost_saves: opened.lost_saves,
                unfinished: opened.unfinished,
                lost_records: opened.lost_records,
                notice: opened.notice(),
            };
            return Ok((opened.mailbox, left));
        }
    }
    let (name, bytes) = source(v)?;
    match Format::detect(&name, &bytes) {
        Some(Format::Mailbox) => {}
        Some(_) => return Err(failed!("{name} isn't a mailbox; bring it in with File › Import", name)),
        None => return Err(failed!("{name} isn't a MailCraft mailbox", name)),
    }
    Ok((crate::io::read_mailbox(&bytes).map_err(fail)?, Left::default()))
}

/// What a mailbox file would connect to, for the question put to the owner when an agent asks to
/// open it: each account's address and servers as the file has them, or that it can't be read.
/// Nothing is kept of what is read.
pub(crate) fn servers_in(v: &Value) -> String {
    let Ok((mailbox, _)) = read(v) else { return "(the file can't be read)".to_string() };
    let accounts: Vec<String> = mailbox
        .accounts
        .iter()
        .filter(|a| a.kind.has_server())
        .take(10)
        .map(|a| format!("{} ({})", a.email.chars().take(80).collect::<String>(), super::account::servers_text(a)))
        .collect();
    if accounts.is_empty() { "no account connects to a server".to_string() } else { format!("accounts connect to {}", accounts.join("; ")) }
}

fn open(s: &mut Session, v: &Value) -> CmdResult {
    // First, so that opening the file the mailbox is kept in reads what was just written.
    settle(s)?;
    let (mailbox, left) = read(v)?;
    s.replace_mailbox(mailbox);
    // Only a real file is a place to save back to.
    s.path = if v.get("data").is_some() { None } else { p::str(v, "path").map(Into::into) };
    // A mailbox whose owner chose another way than Microsoft Graph of reading Outlook's
    // Favorites, which is no more: said once, and the file is saved without it.
    let favorites_now_graph = s.mailbox.take_retired_favorites_choices();
    if favorites_now_graph > 0 {
        s.mailbox_changed();
    }
    // Mail of Microsoft accounts kept with characters that couldn't be read is fetched again,
    // once (#185).
    let fetched_again = super::graph_down::refetch_unreadable(&mut s.mailbox);
    if fetched_again > 0 {
        s.mailbox_changed();
    }
    // So is that of IMAP accounts, by UID, at the next Send/Receive (#650).
    let fetched_again = fetched_again.saturating_add(super::imap_again::mark_unreadable(&mut s.mailbox));
    if fetched_again > 0 {
        s.mailbox_changed();
    }
    Ok(json!({
        "favoritesNowGraph": favorites_now_graph,
        "fetchedAgain": fetched_again,
        "accounts": s.mailbox.accounts.len(),
        "messages": s.mailbox.messages.len(),
        "contacts": s.mailbox.contacts.len(),
        "events": s.mailbox.events.len(),
        "tasks": s.mailbox.tasks.len(),
        "unavailable": left.unavailable,
        // What of the file was damaged and left out, and what to tell the owner about it.
        "lostSaves": left.lost_saves,
        "unfinished": left.unfinished,
        "lostRecords": left.lost_records,
        "notice": left.notice,
    }))
}

/// The mailbox as one self-contained file: what a caller without a place for files gets. Every
/// byte of it: what the mailbox keeps outside memory is fetched first.
fn single_file(s: &Session) -> Result<Vec<u8>, crate::CmdError> {
    crate::io::write_mailbox(&s.loaded_mailbox()?).map_err(fail)
}

/// Save where the mailbox is kept (or to `path`, which is where it is kept from then on): the
/// mailbox file and, beside it, the content that isn't there yet (`crate::store`). What a save
/// costs is what changed since the last one.
fn save(s: &mut Session, v: &Value) -> CmdResult {
    let path = p::str(v, "path").map(str::to_string).or_else(|| s.path.as_ref().map(|p| p.to_string_lossy().to_string()));
    // Nowhere to keep it: the bytes of one file, handed back (the browser downloads them). They
    // may or may not be kept, so the mailbox stays edited.
    let Some(path) = path else { return deliver(v, &format!("mailbox.{MAILBOX_EXT}"), single_file(s)?) };
    #[cfg(not(target_arch = "wasm32"))]
    let r = {
        // Saved somewhere else than where it is kept, the mailbox takes everything along: the
        // content folder there has none of what this one keeps.
        let elsewhere = s.path.as_ref().is_none_or(|kept| kept.as_path() != std::path::Path::new(&path));
        let saved = if elsewhere {
            crate::store::save(std::path::Path::new(&path), &s.loaded_mailbox()?).map_err(fail)?
        } else {
            let saved = crate::store::save(std::path::Path::new(&path), &s.mailbox).map_err(fail)?;
            // What mail outside its window let go of goes from disk now (#143).
            crate::store::forget_due(std::path::Path::new(&path), s.rev());
            saved
        };
        json!({"path": path, "bytes": saved.bytes, "blobs": saved.blobs})
    };
    #[cfg(target_arch = "wasm32")]
    let r = deliver(&json!({"path": path}), "", single_file(s)?)?;
    if r.get("path").is_some() {
        s.path = Some(path.into());
        s.dirty = false;
        s.note_saved();
    }
    Ok(r)
}

/// Everything in one self-contained file, to carry somewhere or keep as a copy. A mailbox that
/// wasn't kept anywhere lives in that file from now on (and takes the form with a content folder
/// the next time it is saved there); one that already has its place stays there, so that saving
/// as you go doesn't wander off to the copy.
fn save_as(s: &mut Session, v: &Value) -> CmdResult {
    let path = p::req_str(v, "path")?.to_string();
    #[cfg(not(target_arch = "wasm32"))]
    let r = {
        let bytes = crate::store::save_single(std::path::Path::new(&path), &s.loaded_mailbox()?).map_err(fail)?;
        json!({"path": path, "bytes": bytes})
    };
    #[cfg(target_arch = "wasm32")]
    let r = deliver(v, "", single_file(s)?)?;
    let home = s.path.as_ref().is_none_or(|kept| kept.as_path() == std::path::Path::new(&path));
    if r.get("path").is_some() && home {
        s.path = Some(path.into());
        s.dirty = false;
        s.note_saved();
    }
    Ok(with(r, "copy", json!(!home)))
}

fn import(s: &mut Session, v: &Value) -> CmdResult {
    let (name, bytes) = source(v)?;
    if p::bool(v, "opened") == Some(true) {
        return crate::io::import_opened(s, &name, &bytes);
    }
    crate::io::import(s, &name, &bytes, p::id(v, "folder"))
}

fn import_tree(s: &mut Session, v: &Value) -> CmdResult {
    crate::import_tree::run(s, v)
}

fn export_message(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    // Whole: what the mailbox keeps outside memory goes into the file.
    let m = s.loaded_message(id)?;
    // A draft keeps its Bcc line; a sent or received message never had one to show.
    let opts = mailcraft_mime::WriteOptions { include_bcc: m.draft, offset_minutes: s.offset_at(m.date) };
    let name = eml_name(&m);
    deliver(v, &name, mailcraft_mime::write(&m, &opts))
}

/// The name of a message as a file of its own: its subject, fit for a file name, as `.eml`;
/// `message.eml` when nothing of the subject is left (none, or only what a name can't keep).
pub fn eml_name(m: &mailcraft_model::Message) -> String {
    export_name_or(&m.subject, "eml", "message")
}

fn export_folder(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").unwrap_or(s.view.folder);
    let folder = s.mailbox.folder(id).ok_or_else(|| failed!("no folder with id {id}", id))?;
    // Each message whole: what the mailbox keeps outside memory goes into the file.
    let mut whole: Vec<std::sync::Arc<mailcraft_model::Message>> = Vec::new();
    for m in s.mailbox.messages_in(id) {
        whole.push(s.loaded_message(m.id)?);
    }
    whole.sort_by_key(|m| (m.date, m.id));
    let messages: Vec<&mailcraft_model::Message> = whole.iter().map(|m| m.as_ref()).collect();
    let opts = mailcraft_mime::WriteOptions { include_bcc: false, offset_minutes: s.offset_at(s.now()) };
    let bytes = mailcraft_mime::write_mbox(&messages, &opts);
    let r = deliver(v, &export_name(&folder.name, "mbox"), bytes)?;
    Ok(with(r, "messages", json!(messages.len())))
}

fn with(mut r: Value, key: &str, value: Value) -> Value {
    if let Some(o) = r.as_object_mut() {
        o.insert(key.into(), value);
    }
    r
}

fn export_calendar(s: &mut Session, v: &Value) -> CmdResult {
    let opts = mailcraft_ical::WriteOptions { method: String::new(), now: s.now() };
    // Each time in its zone, with the zone's definition: what another program reads right
    // wherever it is. (Floating, as before, when the system can't say which zone this is.)
    let zone = Some(s.zone());
    if let Some(id) = p::id(v, "id") {
        let e = s.mailbox.event(id).ok_or_else(|| failed!("no event with id {id}", id))?;
        let text = mailcraft_ical::write_in(&[e], &[], &opts, zone);
        return deliver(v, &export_name(if e.title.is_empty() { "event" } else { &e.title }, "ics"), text.into_bytes());
    }
    let calendar = p::id(v, "calendar");
    if let Some(c) = calendar
        && s.mailbox.calendar(c).is_none()
    {
        return Err(failed!("no calendar with id {id}", id = c));
    }
    let events: Vec<&mailcraft_model::Event> =
        s.mailbox.events.values().map(|e| e.as_ref()).filter(|e| calendar.is_none_or(|c| e.calendar == c)).collect();
    let tasks: Vec<&mailcraft_model::Task> = if calendar.is_none() { s.mailbox.tasks.values().map(|t| t.as_ref()).collect() } else { Vec::new() };
    let name = calendar.and_then(|c| s.mailbox.calendar(c)).map_or("calendar", |c| c.name.as_str());
    let r = deliver(v, &export_name(name, "ics"), mailcraft_ical::write_in(&events, &tasks, &opts, zone).into_bytes())?;
    Ok(with(with(r, "events", json!(events.len())), "tasks", json!(tasks.len())))
}

fn export_contacts(s: &mut Session, v: &Value) -> CmdResult {
    let ids: Vec<Id> = match p::ids(v) {
        Some(ids) => ids,
        None => crate::cmd::people::sorted(s, ""),
    };
    let contacts: Vec<&mailcraft_model::Contact> = ids.iter().filter_map(|id| s.mailbox.contact(*id)).collect();
    if contacts.is_empty() {
        return Err(fail("there are no contacts to export"));
    }
    let name = match contacts.as_slice() {
        [one] => export_name(&one.name(), "vcf"),
        _ => "contacts.vcf".to_string(),
    };
    let r = deliver(v, &name, mailcraft_vcard::write(&contacts).into_bytes())?;
    Ok(with(r, "contacts", json!(contacts.len())))
}

fn info(s: &mut Session, _: &Value) -> CmdResult {
    let mb = &s.mailbox;
    Ok(json!({
        "path": s.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        "dirty": s.dirty,
        "accounts": mb.accounts.iter().map(|a| json!({"id": a.id, "name": a.name, "email": a.email, "kind": a.kind})).collect::<Vec<_>>(),
        "folders": mb.folders.len(),
        "messages": mb.messages.len(),
        "unread": mb.messages.values().filter(|m| !m.read).count(),
        "contacts": mb.contacts.len(),
        "calendars": mb.calendars.len(),
        "events": mb.events.len(),
        "tasks": mb.tasks.len(),
        "rules": mb.rules.len(),
        "categories": mb.categories.len(),
        "size": mb.messages.values().map(|m| m.size()).sum::<u64>(),
        "syncStatus": s.sync_status,
    }))
}

fn weekday(text: &str) -> Result<Weekday, crate::CmdError> {
    Weekday::parse(text).or_else(|| Weekday::from_ical(text)).ok_or_else(|| bad(format!("\"{text}\" isn't a day of the week")))
}

fn options(s: &mut Session, v: &Value) -> CmdResult {
    let mut prefs = s.prefs.clone();
    if let Some(b) = p::bool(v, "twelveHour") {
        (prefs.twelve_hour, prefs.clock_chosen) = (b, Some(true));
    }
    if let Some(d) = p::str(v, "weekStart") {
        (prefs.week_start, prefs.week_start_chosen) = (weekday(d)?, Some(true));
    }
    if let Some(days) = p::strings(v, "workDays") {
        let mut list = days.iter().map(|d| weekday(d)).collect::<Result<Vec<_>, _>>()?;
        list.sort();
        list.dedup();
        if list.is_empty() {
            return Err(bad("`workDays` needs at least one day"));
        }
        prefs.work_days = list;
    }
    if let Some(h) = p::u64(v, "workStart") {
        prefs.work_start = h.min(23) as u8;
    }
    if let Some(h) = p::u64(v, "workEnd") {
        prefs.work_end = h.clamp(1, 24) as u8;
    }
    if prefs.work_end <= prefs.work_start {
        return Err(bad("the working day ends after it starts"));
    }
    if let Some(b) = p::bool(v, "markReadOnOpen") {
        prefs.mark_read_on_open = b;
    }
    match v.get("defaultReminder") {
        Some(Value::Null) => prefs.default_reminder = None,
        Some(_) => {
            prefs.default_reminder =
                Some(p::u64(v, "defaultReminder").ok_or_else(|| bad("`defaultReminder` is minutes, or null"))?.min(40_320) as u32)
        }
        None => {}
    }
    if let Some(b) = p::bool(v, "quoteOnReply") {
        prefs.quote_on_reply = b;
    }
    if let Some(n) = p::u64(v, "previewLines") {
        prefs.preview_lines = n.min(3) as u8;
    }
    if let Some(n) = p::u64(v, "syncLimit") {
        prefs.sync_limit = n.min(1_000_000) as u32;
    }
    if let Some(n) = p::u64(v, "syncMinutes") {
        prefs.sync_minutes = n.min(1440) as u32;
    }
    s.prefs = prefs;
    serde_json::to_value(&s.prefs).map_err(|e| fail(e.to_string()))
}
