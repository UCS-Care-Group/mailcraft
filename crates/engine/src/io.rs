// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading and writing files: the mailbox as one self-contained file, and the formats mail,
//! calendars and contacts travel in (`.eml`, `.mbox`, `.ics`, `.vcf`). How a mailbox is kept on
//! disk while it is worked on is `crate::store`.

use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use crate::cmd::{bad, fail};
use crate::{CmdError, Session};

/// Largest file read (512 MB).
pub const MAX_FILE: u64 = 512 << 20;

/// The mailbox file's extension.
pub const MAILBOX_EXT: &str = "mailcraft";

/// How a mailbox file in format 3 begins: the form a desktop program keeps it in, a log of
/// records beside a folder of content (`crate::store`). One self-contained file is JSON.
pub const INDEX_MAGIC: &[u8; 16] = b"MailCraft index\n";

/// How every mailbox file that isn't JSON begins, whatever its form: format 3's
/// [`INDEX_MAGIC`], and whatever a newer MailCraft writes. A file that begins so but isn't a form
/// this program reads was written by a newer one, and is said to be.
pub const FAMILY: &[u8] = b"MailCraft ";

/// What the owner is told about a mailbox file of a form a newer MailCraft writes.
pub const NEWER: &str = "this mailbox was written by a newer MailCraft";

/// The message for a mailbox file that keeps its content in a folder beside it, read without it.
const KEPT_BESIDE: &str = "this mailbox file keeps its messages' content in a folder beside it; open it where it is kept, or save it as one file first (File › Open & Export › Save Mailbox As)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A MailCraft mailbox (`.mailcraft`: one JSON file, or a log of records with a content folder
    /// beside it, `crate::store`).
    Mailbox,
    /// One Internet message.
    Eml,
    /// Many messages in one file.
    Mbox,
    /// iCalendar events and to-dos.
    Ics,
    /// vCard contacts.
    Vcf,
}

impl Format {
    pub fn from_extension(name: &str) -> Option<Format> {
        match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
            Some("mailcraft" | "json") => Some(Format::Mailbox),
            Some("eml" | "msg822") => Some(Format::Eml),
            Some("mbox" | "mbx") => Some(Format::Mbox),
            Some("ics" | "ical" | "ifb" | "vcs") => Some(Format::Ics),
            Some("vcf" | "vcard") => Some(Format::Vcf),
            _ => None,
        }
    }

    /// The format by name, else by what the bytes start with. A mailbox file in format 3 is a
    /// mailbox whatever it is called (none of the other formats starts like one).
    pub fn detect(name: &str, bytes: &[u8]) -> Option<Format> {
        if bytes.starts_with(INDEX_MAGIC) {
            return Some(Format::Mailbox);
        }
        if let Some(f) = Format::from_extension(name) {
            return Some(f);
        }
        let head = String::from_utf8_lossy(bytes.get(..bytes.len().min(512)).unwrap_or(&[]))
            .trim_start_matches('\u{feff}')
            .trim_start()
            .to_ascii_uppercase();
        if head.starts_with("BEGIN:VCALENDAR") {
            Some(Format::Ics)
        } else if head.starts_with("BEGIN:VCARD") {
            Some(Format::Vcf)
        } else if head.starts_with("FROM ") {
            Some(Format::Mbox)
        } else if head.starts_with('{') {
            Some(Format::Mailbox)
        } else if head
            .lines()
            .next()
            .is_some_and(|l| l.split_once(':').is_some_and(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')))
        {
            Some(Format::Eml)
        } else {
            None
        }
    }
}

/// A mailbox from one self-contained file's bytes (an export, the browser's storage). Whatever
/// the file holds, the result is consistent (`Mailbox::repair`).
///
/// A mailbox a desktop program works on keeps its content in a folder beside its file
/// (`crate::store`); the file alone isn't the mailbox, and is opened by path instead
/// ([`open_path`]).
pub fn read_mailbox(bytes: &[u8]) -> Result<Mailbox, String> {
    if bytes.starts_with(INDEX_MAGIC) {
        return Err(KEPT_BESIDE.into());
    }
    if bytes.starts_with(FAMILY) {
        return Err(NEWER.into());
    }
    let mut mb: Mailbox = serde_json::from_slice(bytes).map_err(|e| format!("this isn't a MailCraft mailbox ({e})"))?;
    match mb.version {
        0 | 1 => {}
        2 => return Err(KEPT_BESIDE.into()),
        newer => return Err(format!("this mailbox was written by a newer MailCraft (format {newer})")),
    }
    for line in mb.repair() {
        log::warn!("mailbox repaired: {line}");
    }
    Ok(mb)
}

/// The mailbox as one self-contained file: everything in it, attachments as base64.
pub fn write_mailbox(mb: &Mailbox) -> Result<Vec<u8>, String> {
    serde_json::to_vec(mb).map_err(|e| e.to_string())
}

/// Open a mailbox file by path: one self-contained file, or a file with its content folder
/// beside it (`crate::store`), with every body and attachment in memory: what comes back is the
/// whole mailbox, to use without its file. A program that works on the mailbox opens it with
/// `store::open` instead and reads what stays on disk when it wants it (`Session::content`).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_path(path: &std::path::Path) -> Result<Mailbox, String> {
    let mut mailbox = crate::store::open(path)?.mailbox;
    crate::store::load_all(path, &mut mailbox)?;
    Ok(mailbox)
}

/// How far opening a mailbox has got, shared between the thread that reads it and the window that
/// shows it (#519): the bytes of the file read, of how many, and whether the window gave up
/// waiting, which stops the reading at its next read.
#[derive(Debug, Default)]
pub struct OpenProgress {
    read: std::sync::atomic::AtomicU64,
    total: std::sync::atomic::AtomicU64,
    cancelled: std::sync::atomic::AtomicBool,
}

impl OpenProgress {
    const ORDER: std::sync::atomic::Ordering = std::sync::atomic::Ordering::Relaxed;

    /// Bytes read so far.
    pub fn read(&self) -> u64 {
        self.read.load(Self::ORDER)
    }

    /// Bytes there are to read (0: not known, or not known yet).
    pub fn total(&self) -> u64 {
        self.total.load(Self::ORDER)
    }

    pub fn set_total(&self, total: u64) {
        self.total.store(total, Self::ORDER);
    }

    pub fn add_read(&self, n: u64) {
        let _ = self.read.try_update(Self::ORDER, Self::ORDER, |r| Some(r.saturating_add(n)));
    }

    /// Every byte is read: what is left is making a mailbox of them.
    pub fn reading_done(&self) -> bool {
        let total = self.total();
        total > 0 && self.read() >= total
    }

    /// Stop reading: the reading ends with an error at its next read.
    pub fn cancel(&self) {
        self.cancelled.store(true, Self::ORDER);
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Self::ORDER)
    }
}

/// A host's way of keeping a mailbox file up to date without whoever changed the mailbox waiting
/// for the disk: the desktop app hands each state to a background thread (`store::Saver`).
pub trait BackgroundSave {
    /// The mailbox file at `path` is kept up to date through [`BackgroundSave::submit`] from now
    /// on. Whatever replaces the mailbox in its session (File › Open, File › New) writes the last
    /// changes first, so they can't fall between two saves.
    fn follow(&self, path: &std::path::Path);
    /// Write this state of the mailbox (the session's `rev`) to `path` soon, without waiting. A
    /// state still waiting its turn gives way to this newer one. The mailbox is copied (a cheap
    /// copy: its items are shared) only when it is to be written: the same state handed in again
    /// while it is on its way costs nothing.
    fn submit(&self, path: &std::path::Path, mailbox: &Mailbox, rev: u64);
    /// What became of the states handed in, since this was last asked.
    fn reports(&self) -> Vec<SaveReport>;
    /// How far the writing has got, for the window to show: `None` when nothing handed in is
    /// waiting or being written, and the file isn't busy otherwise ([`SaveStage::Sweeping`]).
    fn progress(&self) -> Option<SaveProgress> {
        None
    }
    /// Wait until nothing handed in is waiting or being written (the program is closing), for a
    /// while at most: never forever. A save cut short leaves the file as the last one left it.
    fn flush(&self);
}

/// How far the saving of a mailbox has got ([`BackgroundSave::progress`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveProgress {
    pub stage: SaveStage,
    /// How many of the stage's `total` are done.
    pub done: u64,
    pub total: u64,
    /// How long the state being written has been at it, in milliseconds (0 while it waits): a
    /// save of a moment is not worth telling.
    pub millis: u64,
}

/// What a save is at ([`SaveProgress`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SaveStage {
    /// Handed in, waiting its turn.
    #[default]
    Waiting,
    /// Going through the messages: what each comes to in the mailbox file.
    Messages,
    /// Writing the bodies and attachments that aren't on disk yet, one file each.
    Content,
    /// Nothing of the mailbox is being written, but its file is busy: the content files it no
    /// longer names are being looked for and removed (or are about to be). Whatever replaces the
    /// mailbox waits for this too, so as not to find the file taken.
    Sweeping,
}

/// Where a host gets the content of a mailbox that isn't in memory: the attachments, and the
/// bodies of mail older than the memory the mailbox was given when it was opened, which the
/// desktop's `store` keeps in the content folder beside the mailbox file ([`ContentRef`]). The
/// engine asks for a part when something needs its bytes ([`Session::content`]); nothing is
/// fetched for a message nobody looks at. The browser keeps every mailbox in memory and has no
/// source.
pub trait ContentSource: Send + Sync {
    /// The bytes kept under `r` for the mailbox file at `mailbox`, checked against the length and
    /// SHA-256 the reference names (anything else is "missing or damaged", never bytes).
    fn fetch(&self, mailbox: &std::path::Path, r: &ContentRef) -> Result<std::sync::Arc<Vec<u8>>, String>;
}

pub use mailcraft_model::ContentRef;

/// How writing one state of a mailbox went ([`BackgroundSave::reports`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    pub path: std::path::PathBuf,
    /// The session's `rev` of the state that was handed in.
    pub rev: u64,
    /// `Err`: why it couldn't be written. The file is as it was.
    pub result: Result<(), String>,
}

/// Most messages one import takes from an mbox file; the rest are left.
pub const MAX_IMPORT_MESSAGES: usize = 200_000;

/// Why a file named from outside wasn't read ([`read_regular_file`]).
#[derive(Debug)]
pub enum ReadError {
    /// Not a regular file: a folder, a device, a pipe.
    NotAFile,
    /// Larger than it may be.
    TooLarge,
    Io(std::io::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NotAFile => f.write_str("not a file"),
            ReadError::TooLarge => write!(f, "larger than {} MB", MAX_FILE >> 20),
            ReadError::Io(e) => e.fmt(f),
        }
    }
}

/// A file named from outside, opened once and checked through that handle to be a regular file
/// (so nothing put in its place after a check is read instead). On Unix it is opened without
/// waiting (`O_NONBLOCK`, which a regular file ignores), so a pipe put where a file was named
/// can't hold whoever opens it. Windows has no such flag: a named pipe's path
/// (`\\.\pipe\…`) is not covered there, and is refused only once it is open.
#[cfg(not(target_arch = "wasm32"))]
pub fn regular_file(path: &std::path::Path) -> Result<std::fs::File, ReadError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = options.open(path).map_err(ReadError::Io)?;
    if !file.metadata().map_err(ReadError::Io)?.is_file() {
        return Err(ReadError::NotAFile);
    }
    Ok(file)
}

/// The bytes of a file named from outside ([`regular_file`]), at most [`MAX_FILE`] of them.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_regular_file(path: &std::path::Path) -> Result<Vec<u8>, ReadError> {
    use std::io::Read as _;
    let file = regular_file(path)?;
    if file.metadata().map_err(ReadError::Io)?.len() > MAX_FILE {
        return Err(ReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE.saturating_add(1)).read_to_end(&mut bytes).map_err(ReadError::Io)?;
    if u64::try_from(bytes.len()).map_or(true, |n| n > MAX_FILE) {
        return Err(ReadError::TooLarge);
    }
    Ok(bytes)
}

/// The header a message opened from a file (#505) keeps the SHA-256 of that file in, so that the
/// same file opened again finds it. One that came in the file itself is dropped.
pub const OPENED_FROM: &str = mailcraft_mime::OPENED_FILE_HEADER;

/// The SHA-256 of a file, in hex (none in the browser, which opens no files from the system).
fn file_sha256(bytes: &[u8]) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
        Some(digest.as_ref().iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = bytes;
        None
    }
}

/// A message file the system asked MailCraft to open (a double click, a later start; #505):
/// brought into the local folder Imported (never a server's folder, Drafts, the Outbox, Sent,
/// Junk or Deleted Items), as a received message that is read, never a draft. Opened again, the
/// same message (by its Message-ID in that folder, else by the file's SHA-256) is the one there,
/// and nothing is added. Says `opened` (the message) and `already`.
pub fn import_opened(s: &mut Session, name: &str, bytes: &[u8]) -> Result<Value, CmdError> {
    if Format::detect(name, bytes) != Some(Format::Eml) {
        return Err(failed!("{name}: not a message file", name));
    }
    let folder = s.mailbox.imported_folder(false)?;
    let mut m = mailcraft_mime::parse(bytes);
    m.headers.retain(|(key, _)| !key.eq_ignore_ascii_case(OPENED_FROM));
    let hash = file_sha256(bytes);
    // The same Message-ID (none for both, too), and, when the copy there keeps the SHA-256 of the
    // file it came from, the same file; a copy without one (imported before) by its Message-ID
    // alone, when there is one.
    let same = |x: &mailcraft_model::Message| {
        let stored = x.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(OPENED_FROM)).map(|(_, value)| value.as_str());
        x.message_id == m.message_id
            && match stored {
                Some(stored) => hash.as_deref() == Some(stored),
                None => !m.message_id.is_empty(),
            }
    };
    let there = s.mailbox.messages_in(folder).find(|x| !x.draft && same(x)).map(|x| x.id);
    if let Some(id) = there {
        show_opened(s, id);
        return Ok(json!({"format": "eml", "messages": 0, "ids": [id], "folder": folder, "opened": id, "already": true}));
    }
    if let Some(hash) = hash {
        m.headers.push((OPENED_FROM.to_string(), hash));
    }
    (m.id, m.uid, m.folder, m.draft, m.read) = (0, 0, folder, false, true);
    (m.server, m.synced) = (None, None);
    if m.received == 0 {
        m.received = if m.date != 0 { m.date } else { s.now() };
    }
    let id = s.mailbox.add_message(m)?;
    show_opened(s, id);
    Ok(json!({"format": "eml", "messages": 1, "ids": [id], "folder": folder, "opened": id, "already": false}))
}

/// A message file read to be looked at and not kept (#515): the message as `import_opened` would
/// file it (a received, read message, never a draft, with no number, folder or server of its own,
/// and no hash of its own claimed), and nothing is added to the mailbox. Parsed as every message
/// is; the caller caps the size of `bytes`.
pub fn parse_unfiled(name: &str, bytes: &[u8]) -> Result<mailcraft_model::Message, CmdError> {
    if Format::detect(name, bytes) != Some(Format::Eml) {
        return Err(failed!("{name}: not a message file", name));
    }
    let mut m = mailcraft_mime::parse(bytes);
    m.headers.retain(|(key, _)| !key.eq_ignore_ascii_case(OPENED_FROM));
    (m.id, m.uid, m.folder, m.draft, m.read) = (0, 0, 0, false, true);
    (m.server, m.synced) = (None, None);
    if m.received == 0 {
        m.received = m.date;
    }
    Ok(m)
}

/// The opened message selected and in the list, as an import's last message is (#485): what a
/// search in view finds is asked afresh, and its folder is shown when the list leaves it out.
fn show_opened(s: &mut Session, id: Id) {
    s.list_due();
    s.sel.messages = vec![id];
    s.show_selected();
}

/// Bring a file's contents into the mailbox: messages go to `folder` (default: the folder in
/// view, or the Inbox), events to the default calendar, contacts and to-dos to their lists.
pub fn import(s: &mut Session, name: &str, bytes: &[u8], folder: Option<Id>) -> Result<Value, CmdError> {
    let format = Format::detect(name, bytes).ok_or_else(|| failed!("{name}: not a message, calendar or contact file MailCraft reads", name))?;
    let explicit = folder.is_some();
    let folder = match folder {
        Some(f) => s.mailbox.folder(f).map(|f| f.id).ok_or_else(|| failed!("no folder with id {id}", id = f))?,
        None => s.mailbox.folder(s.view.folder).map(|f| f.id).or_else(|| s.mailbox.inbox()).ok_or_else(|| fail("the mailbox has no folders"))?,
    };
    // Mail brought in from a file is mail the owner has not written: it never lands where the next
    // Send/Receive would send it. With an Outbox in view it goes to the account's Drafts instead
    // (or its Inbox); an Outbox named outright is refused.
    let folder = if matches!(format, Format::Eml | Format::Mbox) && s.mailbox.is_send_queue(folder) {
        if explicit {
            return Err(fail("Mail can't be imported into an Outbox: it would be sent. Choose another folder."));
        }
        let account = s.mailbox.folder(folder).map_or(0, |f| f.account);
        [FolderKind::Drafts, FolderKind::Inbox]
            .into_iter()
            .find_map(|kind| s.mailbox.folder_of_kind(account, kind))
            .filter(|id| !s.mailbox.is_send_queue(*id))
            .ok_or_else(|| fail("Mail can't be imported into an Outbox: it would be sent."))?
    } else {
        folder
    };
    let now = s.now();
    match format {
        Format::Mailbox => Err(bad(format!("{name} is a whole mailbox; open it with File › Open"))),
        Format::Eml | Format::Mbox => {
            // At most MAX_IMPORT_MESSAGES are taken from a file; the one after them tells whether
            // the file went on ("stopped after N").
            let mut messages: Vec<_> = if format == Format::Eml {
                vec![mailcraft_mime::parse(bytes)]
            } else {
                mailcraft_mime::mbox_messages(bytes).take(MAX_IMPORT_MESSAGES + 1).collect()
            };
            let stopped = messages.len() > MAX_IMPORT_MESSAGES;
            messages.truncate(MAX_IMPORT_MESSAGES);
            if messages.is_empty() {
                return Err(failed!("{name} has no messages", name));
            }
            // Graph can't put received mail into a Microsoft account's folders: a local folder
            // has it instead, and is shown (R3 spec S30).
            let microsoft = s.mailbox.folder(folder).and_then(|f| s.mailbox.account(f.account)).is_some_and(|a| a.kind == AccountKind::Microsoft);
            let folder = if microsoft {
                let drafts = s.mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Drafts);
                let local = s.mailbox.imported_folder(drafts)?;
                s.view.folder = local;
                s.view.module = crate::Module::Mail;
                s.view.search.clear();
                local
            } else {
                folder
            };
            let drafts = s.mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Drafts);
            let mut ids = Vec::new();
            for mut m in messages {
                m.id = 0;
                m.uid = 0;
                m.folder = folder;
                m.draft = drafts;
                // Only a message opened from a file keeps that file's SHA-256 (`import_opened`).
                m.headers.retain(|(key, _)| !key.eq_ignore_ascii_case(OPENED_FROM));
                if m.received == 0 {
                    m.received = if m.date != 0 { m.date } else { now };
                }
                ids.push(s.mailbox.add_message(m)?);
            }
            if let Some(last) = ids.last() {
                // What a search in view finds is asked of the mailbox with them in it.
                s.list_due();
                s.sel.messages = vec![*last];
                // Selected, it is in the list (#485): its folder's, whole, if the list in view
                // leaves it out.
                s.show_selected();
            }
            Ok(
                json!({"format": if format == Format::Eml { "eml" } else { "mbox" }, "messages": ids.len(), "ids": ids, "folder": folder, "local": microsoft, "stoppedAfter": stopped.then_some(ids.len())}),
            )
        }
        Format::Ics => {
            let text = String::from_utf8_lossy(bytes);
            // (Times in a zone become the times of the zone the calendar is shown in.)
            let file = mailcraft_ical::parse(&text, &crate::cmd::remote::offset_of(s));
            if file.events.is_empty() && file.tasks.is_empty() {
                return Err(failed!("{name} has no events or tasks", name));
            }
            let notes = file.notes;
            let (mut events, mut tasks, mut updated) = (Vec::new(), Vec::new(), 0);
            // The events of this computer's own, by UID: looked up once, not once for every event.
            let mut own_uids: std::collections::HashMap<String, (mailcraft_model::Id, mailcraft_model::Id)> = std::collections::HashMap::new();
            for x in s.mailbox.events.values() {
                if !x.uid.is_empty() && x.origin.is_local() {
                    own_uids.entry(x.uid.clone()).or_insert((x.id, x.calendar));
                }
            }
            let offset_at = crate::cmd::remote::offset_of(s);
            for e in file.events {
                // What a file says is cut to size as an invitation's is.
                let mut e = crate::cmd::calendar::tidy_imported(e, &offset_at);
                // The same event again (same UID, same organizer) replaces the one already here.
                // A file can carry any UID: one that names another organizer, or none, is a
                // different meeting and is added beside it. One that a server keeps is not the
                // owner's to replace (the next download would bring it back as well) either.
                let existing = if e.uid.is_empty() { None } else { own_uids.get(&e.uid).copied() };
                // (Two that name nobody are the owner's own appointment, exported and imported again.)
                let existing = existing.filter(|(id, _)| s.mailbox.event(*id).is_some_and(|have| crate::cmd::calendar::same_organizer(have, &e)));
                match existing {
                    Some((id, calendar)) => {
                        e.id = id;
                        e.calendar = calendar;
                        *s.mailbox.event_mut(id)? = e;
                        updated += 1;
                        events.push(id);
                    }
                    None => {
                        let uid = e.uid.clone();
                        let (calendar, id) = (e.calendar, s.mailbox.add_event(e)?);
                        if !uid.is_empty() {
                            own_uids.entry(uid).or_insert((id, calendar));
                        }
                        events.push(id);
                    }
                }
            }
            for mut t in file.tasks {
                t.id = 0;
                tasks.push(s.mailbox.add_task(t));
            }
            if let Some(first) = events.first().and_then(|id| s.mailbox.event(*id)) {
                s.sel.event_day = Some(first.start.date);
                s.view.date = first.start.date;
                s.sel.event = Some(first.id);
            }
            // (`notes`: what was unclear about a time zone in the file, for the owner to read.)
            Ok(json!({"format": "ics", "events": events.len(), "tasks": tasks.len(), "updated": updated, "method": file.method, "notes": notes}))
        }
        Format::Vcf => {
            let contacts = mailcraft_vcard::parse_bytes(bytes);
            if contacts.is_empty() {
                return Err(failed!("{name} has no contacts", name));
            }
            let mut ids = Vec::new();
            for mut c in contacts {
                c.id = 0;
                ids.push(s.mailbox.add_contact(c));
            }
            s.sel.contact = ids.first().copied();
            Ok(json!({"format": "vcf", "contacts": ids.len(), "ids": ids}))
        }
    }
}
