//! Reading and writing files: the mailbox as one self-contained file, and the formats mail,
//! calendars and contacts travel in (`.eml`, Outlook's `.msg`, `.mbox`, `.ics`, `.vcf`). How a
//! mailbox is kept on disk while it is worked on is `crate::store`.

use mailcraft_model::{FolderKind, Id, Mailbox};
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

/// The message for a mailbox file that keeps its content in a folder beside it, read without it.
const KEPT_BESIDE: &str = "this mailbox file keeps its messages' content in a folder beside it; open it where it is kept, or save it as one file first (File › Open & Export › Save Mailbox As)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// A MailCraft mailbox (`.mailcraft`: one JSON file, or a log of records with a content folder
    /// beside it, `crate::store`).
    Mailbox,
    /// One Internet message.
    Eml,
    /// One message saved from Outlook (a compound file).
    Msg,
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
            Some("msg") => Some(Format::Msg),
            Some("mbox" | "mbx") => Some(Format::Mbox),
            Some("ics" | "ical" | "ifb" | "vcs") => Some(Format::Ics),
            Some("vcf" | "vcard") => Some(Format::Vcf),
            _ => None,
        }
    }

    /// The format by name, else by what the bytes start with. A compound file is an Outlook
    /// message whatever it is called (none of the other formats starts like one), and a file
    /// called `.msg` that isn't one is taken for what its bytes are, if they are anything.
    pub fn detect(name: &str, bytes: &[u8]) -> Option<Format> {
        if mailcraft_msg::is_compound(bytes) {
            return Some(Format::Msg);
        }
        if bytes.starts_with(INDEX_MAGIC) {
            return Some(Format::Mailbox);
        }
        let by_name = Format::from_extension(name);
        if let Some(f) = by_name
            && f != Format::Msg
        {
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
            by_name
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
/// beside it (`crate::store`).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_path(path: &std::path::Path) -> Result<Mailbox, String> {
    crate::store::open(path).map(|opened| opened.mailbox)
}

/// A host's way of keeping a mailbox file up to date without whoever changed the mailbox waiting
/// for the disk: the desktop app hands each state to a background thread (`store::Saver`).
pub trait BackgroundSave {
    /// The mailbox file at `path` is kept up to date through [`BackgroundSave::submit`] from now
    /// on. Whatever replaces the mailbox in its session (File › Open, File › New) writes the last
    /// changes first, so they can't fall between two saves.
    fn follow(&self, path: &std::path::Path);
    /// Write this state of the mailbox (the session's `rev`) to `path` soon, without waiting. A
    /// state still waiting its turn gives way to this newer one.
    fn submit(&self, path: &std::path::Path, mailbox: Mailbox, rev: u64);
    /// What became of the states handed in, since this was last asked.
    fn reports(&self) -> Vec<SaveReport>;
    /// Wait until nothing handed in is waiting or being written (the program is closing).
    fn flush(&self);
}

/// How writing one state of a mailbox went ([`BackgroundSave::reports`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    pub path: std::path::PathBuf,
    /// The session's `rev` of the state that was handed in.
    pub rev: u64,
    /// `Err`: why it couldn't be written. The file is as it was.
    pub result: Result<(), String>,
}

/// Bring a file's contents into the mailbox: messages go to `folder` (default: the folder in
/// view, or the Inbox), events to the default calendar, contacts and to-dos to their lists.
pub fn import(s: &mut Session, name: &str, bytes: &[u8], folder: Option<Id>) -> Result<Value, CmdError> {
    let format = Format::detect(name, bytes).ok_or_else(|| fail(format!("{name}: not a message, calendar or contact file MailCraft reads")))?;
    let folder = match folder {
        Some(f) => s.mailbox.folder(f).map(|f| f.id).ok_or_else(|| fail(format!("no folder with id {f}")))?,
        None => s.mailbox.folder(s.view.folder).map(|f| f.id).or_else(|| s.mailbox.inbox()).ok_or_else(|| fail("the mailbox has no folders"))?,
    };
    let now = s.now();
    match format {
        Format::Mailbox => Err(bad(format!("{name} is a whole mailbox; open it with File › Open"))),
        Format::Eml | Format::Msg | Format::Mbox => {
            let messages = match format {
                Format::Eml => vec![mailcraft_mime::parse(bytes)],
                Format::Msg => vec![mailcraft_msg::parse(bytes).map_err(|e| fail(format!("{name}: {e}")))?],
                _ => mailcraft_mime::parse_mbox(bytes),
            };
            if messages.is_empty() {
                return Err(fail(format!("{name} has no messages")));
            }
            let drafts = s.mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Drafts);
            let mut ids = Vec::new();
            for mut m in messages {
                m.id = 0;
                m.uid = 0;
                m.folder = folder;
                m.draft = drafts;
                if m.received == 0 {
                    m.received = if m.date != 0 { m.date } else { now };
                }
                ids.push(s.mailbox.add_message(m)?);
            }
            if let Some(last) = ids.last() {
                s.sel.messages = vec![*last];
            }
            let format = match format {
                Format::Eml => "eml",
                Format::Msg => "msg",
                _ => "mbox",
            };
            Ok(json!({"format": format, "messages": ids.len(), "ids": ids, "folder": folder}))
        }
        Format::Ics => {
            let text = String::from_utf8_lossy(bytes);
            // (Times in a zone become the times of the zone the calendar is shown in.)
            let file = mailcraft_ical::parse(&text, &crate::cmd::remote::offset_of(s));
            if file.events.is_empty() && file.tasks.is_empty() {
                return Err(fail(format!("{name} has no events or tasks")));
            }
            let notes = file.notes;
            let (mut events, mut tasks, mut updated) = (Vec::new(), Vec::new(), 0);
            for mut e in file.events {
                e.id = 0;
                // The same event again (same UID) replaces the one already here. One that a server
                // keeps is not the owner's to replace (the next download would bring it back as
                // well): the imported event is added beside it.
                let own = |x: &&std::sync::Arc<mailcraft_model::Event>| x.uid == e.uid && x.origin.is_local();
                let existing = if e.uid.is_empty() { None } else { s.mailbox.events.values().find(own).map(|x| (x.id, x.calendar)) };
                match existing {
                    Some((id, calendar)) => {
                        e.id = id;
                        e.calendar = calendar;
                        *s.mailbox.event_mut(id)? = e;
                        updated += 1;
                        events.push(id);
                    }
                    None => events.push(s.mailbox.add_event(e)?),
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
            let contacts = mailcraft_vcard::parse(&String::from_utf8_lossy(bytes));
            if contacts.is_empty() {
                return Err(fail(format!("{name} has no contacts")));
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
