// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The attachments of a message: what may be done with each (the engine's opening verdict, the
//! desktop's), its bytes for saving, sharing and previewing, opening it in another app, and
//! bringing a calendar file into the calendar. Nothing here runs or renders an attachment, and a
//! name or a size from a message is never trusted: names are made safe by the engine, sizes are
//! capped, and a picture is a picture by its own bytes.

use mailcraft_engine::Part;
use mailcraft_engine::opening::{self, Verdict};
use serde_json::json;

use crate::message::sniff;
use crate::{Core, CoreError};

/// The most bytes of one attachment handed over: the desktop's cap for one file.
pub const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;
/// The least a small device may still be given, whatever its budget says.
const MIN_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
/// The most bytes of a calendar file that is read.
const MAX_CALENDAR_BYTES: usize = 8 * 1024 * 1024;
/// The most characters of a name that are looked at, from its end (a longer name is cut at its start before anything reads it).
const LOOKED_AT: usize = 1_024;

/// What may be done with an attachment of a kind: the engine's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum OpenVerdict {
    /// Open it in another app.
    Open,
    /// An Office file that can carry macros: ask first.
    Macros,
    /// A web page or a drawing that can carry scripts: ask first.
    Scripts,
    /// A program, a script, an installer, a package or a file of no known kind: never opened or
    /// shared from mail; it can be saved.
    Blocked,
}

impl From<Verdict> for OpenVerdict {
    fn from(v: Verdict) -> OpenVerdict {
        match v {
            Verdict::Open => OpenVerdict::Open,
            Verdict::Macros => OpenVerdict::Macros,
            Verdict::Scripts => OpenVerdict::Scripts,
            Verdict::Blocked => OpenVerdict::Blocked,
        }
    }
}

/// An attachment's name as it may be written, and what may be done with it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentCheck {
    pub verdict: OpenVerdict,
    /// The name it is shown and written under to be opened: one segment, nothing a folder can't
    /// have, no look-alike tricks, never empty.
    pub name: String,
    /// The name it is saved under (with `.bin` when it has no short kind).
    pub save_name: String,
    /// The owner's words for a blocked kind; empty otherwise.
    pub blocked: String,
    /// Whether it is a calendar file (`.ics`) that can be brought into the calendar.
    pub calendar: bool,
}

/// An attachment with its bytes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentFile {
    pub rev: u64,
    pub index: u32,
    pub check: AttachmentCheck,
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp` when the bytes themselves say so
    /// (the only kinds previewed inside the app); empty otherwise.
    pub picture: String,
    pub bytes: Vec<u8>,
}

/// What a calendar file brought in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarImported {
    pub events: u32,
    pub tasks: u32,
}

fn is_calendar(name: &str, mime: &str) -> bool {
    matches!(opening::extension(name).as_str(), "ics" | "ical" | "ifb" | "vcs") || mime.trim().to_ascii_lowercase().starts_with("text/calendar")
}

/// The last `max` characters of `s`: the end of a name is where its kind is.
fn tail(s: &str, max: usize) -> &str {
    let count = s.chars().count();
    match count.checked_sub(max).and_then(|skip| s.char_indices().nth(skip)) {
        Some((at, _)) => s.get(at..).unwrap_or_default(),
        None => s,
    }
}

fn check_of(name: &str, mime: &str) -> AttachmentCheck {
    let name = tail(name, LOOKED_AT);
    let verdict = opening::verdict(name);
    AttachmentCheck {
        verdict: verdict.into(),
        name: opening::file_name(name),
        save_name: opening::save_name(name),
        blocked: if verdict == Verdict::Blocked { opening::blocked_message(name) } else { String::new() },
        calendar: is_calendar(name, mime),
    }
}

/// What may be done with an attachment called `name` (any text: it is cut and made safe here).
#[uniffi::export]
pub fn attachment_check(name: String) -> AttachmentCheck {
    check_of(&name, "")
}

/// Whether a file of this check may be handed to another app: never a blocked kind, and a kind that
/// can carry macros or scripts only once the owner said yes.
pub(crate) fn refuse_unless_openable(check: &AttachmentCheck, confirmed: bool) -> Result<(), CoreError> {
    match check.verdict {
        OpenVerdict::Blocked => Err(CoreError::Failed { reason: check.blocked.clone() }),
        OpenVerdict::Macros | OpenVerdict::Scripts if !confirmed => {
            Err(bad("the owner has to be asked before this kind of file is opened"))
        }
        _ => Ok(()),
    }
}

fn bad(reason: &str) -> CoreError {
    CoreError::BadRequest { reason: reason.to_string() }
}

impl Core {
    /// The most bytes of one attachment this device is given: the desktop's cap, lower on a
    /// device whose memory budget is small.
    fn attachment_cap(&self) -> usize {
        let budget = usize::try_from(self.memory_budget() / 4).unwrap_or(usize::MAX);
        budget.clamp(MIN_ATTACHMENT_BYTES, MAX_ATTACHMENT_BYTES)
    }

    /// Reads attachment `index` of message `id` once the size is known to be within `cap`.
    async fn read_attachment(
        &self,
        id: u64,
        index: u32,
        cap: usize,
        only: Option<fn(&AttachmentCheck) -> bool>,
    ) -> Result<AttachmentFile, CoreError> {
        let at = usize::try_from(index).map_err(|_| bad("the message has no such attachment"))?;
        self.call(move |s| {
            let m = s
                .loaded_message_where(id, |_| false)
                .map_err(|_| CoreError::Failed { reason: s.interface_language.t("the message could not be read").to_string() })?;
            let a = m.attachments.get(at).ok_or_else(|| bad("the message has no such attachment"))?;
            let check = check_of(&a.name, &a.mime);
            if let Some(allowed) = only
                && !allowed(&check)
            {
                return Err(bad("that attachment is not of a kind this can be done with"));
            }
            if a.size() > cap {
                return Err(CoreError::Failed { reason: s.interface_language.t("the attachment is too large").to_string() });
            }
            let bytes = s
                .content(id, Part::Attachment(at))
                .map_err(|_| CoreError::Failed { reason: s.interface_language.t("the attachment could not be read").to_string() })?;
            if bytes.len() > cap {
                return Err(CoreError::Failed { reason: s.interface_language.t("the attachment is too large").to_string() });
            }
            let picture = sniff(&bytes).unwrap_or_default().to_string();
            Ok(AttachmentFile { rev: s.rev(), index, check, picture, bytes: bytes.to_vec() })
        })
        .await
    }
}

#[uniffi::export]
impl Core {
    /// Attachment `index` of message `id` with its bytes, for saving, sharing and previewing: at
    /// most 64 MB (less on a device with little memory).
    pub async fn attachment(&self, id: u64, index: u32) -> Result<AttachmentFile, CoreError> {
        self.read_attachment(id, index, self.attachment_cap(), None).await
    }

    /// Opens attachment `index` of message `id` in another app, under the opening verdict as on
    /// the desktop: a blocked kind is refused (it can be saved), and a kind that can carry macros or
    /// scripts is opened only when `confirmed` says the owner was asked and said yes.
    pub async fn open_attachment(&self, id: u64, index: u32, confirmed: bool) -> Result<(), CoreError> {
        let file = self.read_attachment_for_opening(id, index, confirmed).await?;
        self.platform
            .open_file(file.check.name, file.bytes)
            .map_err(|_| CoreError::Failed { reason: self.lang().t("No app on this phone can open that file.").to_string() })
    }

    /// Brings calendar attachment `index` of message `id` into the calendar (`file.import`):
    /// nothing is sent to anybody.
    pub async fn import_calendar_attachment(&self, id: u64, index: u32) -> Result<CalendarImported, CoreError> {
        let file = self.read_attachment(id, index, MAX_CALENDAR_BYTES.min(self.attachment_cap()), Some(|c| c.calendar)).await?;
        let params = json!({"name": file.check.name, "data": mailcraft_engine::cmd::base64_encode(&file.bytes)});
        self.call(move |s| {
            let done = s.run("file.import", &params)?;
            let n = |key: &str| done.get(key).and_then(serde_json::Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX));
            Ok(CalendarImported { events: n("events"), tasks: n("tasks") })
        })
        .await
    }
}

impl Core {
    async fn read_attachment_for_opening(&self, id: u64, index: u32, confirmed: bool) -> Result<AttachmentFile, CoreError> {
        // The verdict is read before a byte is: a refused kind is never read.
        refuse_unless_openable(&self.attachment_verdict_of(id, index).await?, confirmed)?;
        let file = self.read_attachment(id, index, self.attachment_cap(), None).await?;
        // And again for what was read: the message may have been replaced between the two calls.
        refuse_unless_openable(&file.check, confirmed)?;
        Ok(file)
    }

    async fn attachment_verdict_of(&self, id: u64, index: u32) -> Result<AttachmentCheck, CoreError> {
        let at = usize::try_from(index).map_err(|_| bad("the message has no such attachment"))?;
        self.call(move |s| {
            let m = s
                .loaded_message_where(id, |_| false)
                .map_err(|_| CoreError::Failed { reason: s.interface_language.t("the message could not be read").to_string() })?;
            let a = m.attachments.get(at).ok_or_else(|| bad("the message has no such attachment"))?;
            Ok(check_of(&a.name, &a.mime))
        })
        .await
    }
}
