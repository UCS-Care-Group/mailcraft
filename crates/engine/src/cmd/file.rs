//! File: the mailbox file, importing and exporting, options.

use mailcraft_model::{Id, Mailbox};
use mailcraft_time::Weekday;
use serde_json::{Value, json};

use super::{bad, deliver, fail, file_name, source};
use crate::io::{Format, MAILBOX_EXT};
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
            .params(r#"{"path": string} | {"name": string, "data": base64}, "folder"?: number — .eml, .msg, .mbox, .ics, .vcf"#),
        CommandSpec::new("file.exportMessage", "Save Message As", "File › Save As", export_message).pure().params(r#"{"id"?: number, "path"?: string} — one message as .eml"#),
        CommandSpec::new("file.exportFolder", "Export Folder", "File › Open & Export", export_folder).pure().params(r#"{"id"?: number, "path"?: string} — a folder's messages as .mbox"#),
        CommandSpec::new("file.exportCalendar", "Save Calendar", "File › Save Calendar", export_calendar).pure().params(r#"{"calendar"?: number, "id"?: number (one event), "path"?: string} — .ics"#),
        CommandSpec::new("file.exportContacts", "Export Contacts", "File › Open & Export", export_contacts).pure().params(r#"{"ids"?: [number], "path"?: string} — .vcf"#),
        CommandSpec::new("file.info", "Info", "File › Info", info).pure(),
        CommandSpec::new("file.options", "Options", "File", options).pure().params(
            r#"{"twelveHour"?: bool, "weekStart"?: "sun"|"mon"|…, "workDays"?: ["mon",…], "workStart"?: 0-23, "workEnd"?: 1-24, "markReadOnOpen"?: bool, "defaultReminder"?: minutes | null, "quoteOnReply"?: bool, "composePlain"?: bool (write messages as plain text rather than HTML), "previewLines"?: 0-3, "syncLimit"?: number, "syncMinutes"?: 0-1440 (0: only when asked), "readReceipts"?: "ask"|"always"|"never" (what to do when an opened message asks for a read receipt; default ask)}"#,
        ),
    ]
}

/// Before another mailbox takes this one's place. Where a host keeps the mailbox file up to date
/// by itself (the desktop app's background saver), what it hasn't written yet is written now:
/// replacing the mailbox must not lose the changes that fall between two saves. Anywhere else,
/// leaving a mailbox unsaved is the caller's business, as it always was.
#[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
fn settle(s: &Session) -> Result<(), crate::CmdError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(path) = &s.path
            && crate::store::followed(path)
        {
            crate::store::save(path, &s.mailbox).map_err(|e| fail(format!("the mailbox couldn't be saved first, so it stays open ({e})")))?;
        }
    }
    Ok(())
}

fn new(s: &mut Session, v: &Value) -> CmdResult {
    settle(s)?;
    let mailbox =
        if p::bool(v, "sample") == Some(true) { crate::sample::mailbox(s.now(), s.offset_at(s.now())) } else { Mailbox::with_local_account("", "") };
    s.replace_mailbox(mailbox);
    s.path = None;
    Ok(json!({"sample": p::bool(v, "sample") == Some(true)}))
}

/// The mailbox `file.open` is pointed at, and how many of its bodies and attachments couldn't be
/// read back.
fn read(v: &Value) -> Result<(Mailbox, usize), crate::CmdError> {
    // A file on disk is opened where it is: it may keep its content in a folder beside it.
    #[cfg(not(target_arch = "wasm32"))]
    {
        if v.get("data").is_none()
            && let Some(path) = p::str(v, "path")
        {
            if Format::from_extension(path).is_some_and(|f| f != Format::Mailbox) {
                return Err(fail(format!("{path} isn't a mailbox; bring it in with File › Import")));
            }
            let opened = crate::store::open(std::path::Path::new(path)).map_err(fail)?;
            return Ok((opened.mailbox, opened.unavailable));
        }
    }
    let (name, bytes) = source(v)?;
    match Format::detect(&name, &bytes) {
        Some(Format::Mailbox) => {}
        Some(_) => return Err(fail(format!("{name} isn't a mailbox; bring it in with File › Import"))),
        None => return Err(fail(format!("{name} isn't a MailCraft mailbox"))),
    }
    Ok((crate::io::read_mailbox(&bytes).map_err(fail)?, 0))
}

fn open(s: &mut Session, v: &Value) -> CmdResult {
    // First, so that opening the file the mailbox is kept in reads what was just written.
    settle(s)?;
    let (mailbox, unavailable) = read(v)?;
    s.replace_mailbox(mailbox);
    // Only a real file is a place to save back to.
    s.path = if v.get("data").is_some() { None } else { p::str(v, "path").map(Into::into) };
    Ok(json!({
        "accounts": s.mailbox.accounts.len(),
        "messages": s.mailbox.messages.len(),
        "contacts": s.mailbox.contacts.len(),
        "events": s.mailbox.events.len(),
        "tasks": s.mailbox.tasks.len(),
        "unavailable": unavailable,
    }))
}

/// The mailbox as one self-contained file: what a caller without a place for files gets.
fn single_file(s: &Session) -> Result<Vec<u8>, crate::CmdError> {
    crate::io::write_mailbox(&s.mailbox).map_err(fail)
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
        let saved = crate::store::save(std::path::Path::new(&path), &s.mailbox).map_err(fail)?;
        json!({"path": path, "bytes": saved.bytes, "blobs": saved.blobs})
    };
    #[cfg(target_arch = "wasm32")]
    let r = deliver(&json!({"path": path}), "", single_file(s)?)?;
    if r.get("path").is_some() {
        s.path = Some(path.into());
        s.dirty = false;
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
        let bytes = crate::store::save_single(std::path::Path::new(&path), &s.mailbox).map_err(fail)?;
        json!({"path": path, "bytes": bytes})
    };
    #[cfg(target_arch = "wasm32")]
    let r = deliver(v, "", single_file(s)?)?;
    let home = s.path.as_ref().is_none_or(|kept| kept.as_path() == std::path::Path::new(&path));
    if r.get("path").is_some() && home {
        s.path = Some(path.into());
        s.dirty = false;
    }
    Ok(with(r, "copy", json!(!home)))
}

fn import(s: &mut Session, v: &Value) -> CmdResult {
    let (name, bytes) = source(v)?;
    crate::io::import(s, &name, &bytes, p::id(v, "folder"))
}

fn export_message(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| fail(format!("no message with id {id}")))?;
    // A draft keeps its Bcc line; a sent or received message never had one to show.
    let opts = mailcraft_mime::WriteOptions { include_bcc: m.draft, offset_minutes: s.offset_at(m.date) };
    let name = file_name(if m.subject.trim().is_empty() { "message" } else { &m.subject }, "eml");
    deliver(v, &name, mailcraft_mime::write(m, &opts))
}

fn export_folder(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").unwrap_or(s.view.folder);
    let folder = s.mailbox.folder(id).ok_or_else(|| fail(format!("no folder with id {id}")))?;
    let mut messages: Vec<&mailcraft_model::Message> = s.mailbox.messages_in(id).map(|m| m.as_ref()).collect();
    messages.sort_by_key(|m| (m.date, m.id));
    let opts = mailcraft_mime::WriteOptions { include_bcc: false, offset_minutes: s.offset_at(s.now()) };
    let bytes = mailcraft_mime::write_mbox(&messages, &opts);
    let r = deliver(v, &file_name(&folder.name, "mbox"), bytes)?;
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
        let e = s.mailbox.event(id).ok_or_else(|| fail(format!("no event with id {id}")))?;
        let text = mailcraft_ical::write_in(&[e], &[], &opts, zone);
        return deliver(v, &file_name(if e.title.is_empty() { "event" } else { &e.title }, "ics"), text.into_bytes());
    }
    let calendar = p::id(v, "calendar");
    if let Some(c) = calendar
        && s.mailbox.calendar(c).is_none()
    {
        return Err(fail(format!("no calendar with id {c}")));
    }
    let events: Vec<&mailcraft_model::Event> =
        s.mailbox.events.values().map(|e| e.as_ref()).filter(|e| calendar.is_none_or(|c| e.calendar == c)).collect();
    let tasks: Vec<&mailcraft_model::Task> = if calendar.is_none() { s.mailbox.tasks.values().map(|t| t.as_ref()).collect() } else { Vec::new() };
    let name = calendar.and_then(|c| s.mailbox.calendar(c)).map_or("calendar", |c| c.name.as_str());
    let r = deliver(v, &file_name(name, "ics"), mailcraft_ical::write_in(&events, &tasks, &opts, zone).into_bytes())?;
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
        [one] => file_name(&one.name(), "vcf"),
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
        prefs.twelve_hour = b;
    }
    if let Some(d) = p::str(v, "weekStart") {
        prefs.week_start = weekday(d)?;
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
    if let Some(b) = p::bool(v, "composePlain") {
        prefs.compose_plain = b;
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
    if let Some(r) = p::str(v, "readReceipts") {
        prefs.read_receipts = crate::session::parse_enum(r).ok_or_else(|| bad("`readReceipts` is ask, always or never"))?;
    }
    s.prefs = prefs;
    serde_json::to_value(&s.prefs).map_err(|e| fail(e.to_string()))
}
