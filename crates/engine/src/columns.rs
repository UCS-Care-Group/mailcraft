// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Saved message-list columns. Geometry is in bounded, whole interface points.

use crate::{CmdResult, Session, SortBy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Column {
    From,
    #[default]
    Subject,
    Date,
    Size,
    Importance,
    Attachments,
    Flag,
    Categories,
    Folder,
    #[serde(other)]
    Unknown,
}

impl Column {
    pub const ALL: [Self; 9] =
        [Self::From, Self::Subject, Self::Date, Self::Size, Self::Importance, Self::Attachments, Self::Flag, Self::Categories, Self::Folder];

    pub fn sort(self) -> Option<SortBy> {
        match self {
            Self::From => Some(SortBy::From),
            Self::Subject => Some(SortBy::Subject),
            Self::Date => Some(SortBy::Date),
            Self::Size => Some(SortBy::Size),
            Self::Importance => Some(SortBy::Importance),
            Self::Attachments => Some(SortBy::Attachments),
            Self::Flag => Some(SortBy::Flag),
            Self::Categories => Some(SortBy::Categories),
            Self::Folder | Self::Unknown => None,
        }
    }

    pub fn width(self) -> u16 {
        match self {
            Self::From => 160,
            Self::Subject => 280,
            Self::Date => 140,
            Self::Size => 90,
            Self::Categories | Self::Folder => 140,
            _ => 48,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedSpec", rename_all = "camelCase")]
pub struct ColumnSpec {
    pub column: Column,
    pub width: u16,
}

/// A column as ui.json has it: one without a width gets its own column's.
#[derive(Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SavedSpec {
    column: Column,
    width: Option<u16>,
}

impl Default for SavedSpec {
    fn default() -> Self {
        Self { column: Column::Subject, width: None }
    }
}

impl From<SavedSpec> for ColumnSpec {
    fn from(saved: SavedSpec) -> Self {
        Self { column: saved.column, width: saved.width.unwrap_or(saved.column.width()) }
    }
}

impl Default for ColumnSpec {
    fn default() -> Self {
        Self::new(Column::Subject)
    }
}

impl ColumnSpec {
    pub fn new(column: Column) -> Self {
        Self { column, width: column.width() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Columns {
    pub enabled: bool,
    pub items: Vec<ColumnSpec>,
}

impl Default for Columns {
    fn default() -> Self {
        Self {
            enabled: false,
            items: [Column::Importance, Column::Attachments, Column::From, Column::Subject, Column::Date, Column::Flag]
                .into_iter()
                .map(ColumnSpec::new)
                .collect(),
        }
    }
}

impl Columns {
    /// Old/future settings cannot hide every useful field or create unbounded geometry.
    pub fn sanitize(&mut self) {
        let mut seen = Vec::new();
        self.items.truncate(32);
        self.items.retain_mut(|item| {
            if item.column == Column::Unknown || seen.contains(&item.column) {
                return false;
            }
            seen.push(item.column);
            item.width = item.width.clamp(36, 800);
            true
        });
        if !seen.contains(&Column::Subject) {
            self.items.push(ColumnSpec::new(Column::Subject));
        }
    }
}

/// Whether a folder holds mail going out: Sent Items, Drafts and the Outbox. Its rows name who
/// the mail is to, and arranging by From arranges by them.
pub fn is_outgoing(f: &mailcraft_model::Folder) -> bool {
    matches!(f.kind, mailcraft_model::FolderKind::Sent | mailcraft_model::FolderKind::Drafts | mailcraft_model::FolderKind::Outbox)
}

/// What a row of mail going out to nobody yet is headed by, as the row says it.
pub const NO_RECIPIENTS: &str = "(no recipients)";

/// The name a row is arranged by under From, and its heading: the recipients in a folder of
/// mail going out, else the sender.
pub fn person(mb: &mailcraft_model::Mailbox, m: &mailcraft_model::Message) -> String {
    if mb.folder(m.folder).is_some_and(is_outgoing) {
        let names: Vec<&str> = m.to.iter().map(|a| a.display()).collect();
        if names.is_empty() { NO_RECIPIENTS.to_string() } else { names.join("; ") }
    } else {
        m.from.display().to_string()
    }
}

/// [`person`] in lower case, to sort by.
pub fn person_key(mb: &mailcraft_model::Mailbox, m: &mailcraft_model::Message) -> String {
    person(mb, m).to_lowercase()
}

/// Whether a message has files, as its row shows them: what its server says, when it has said
/// ([`server_answer`]); a draft as it sorts ([`sorts_with_files`]); otherwise the calendar part of meeting mail is not one (the row wears the
/// meeting's mark for it), any other calendar file is, and a message not fetched yet goes by the
/// server.
pub fn has_files(m: &mailcraft_model::Message) -> bool {
    if let Some(has) = server_answer(m) {
        return has;
    }
    // A draft as it sorts: its calendar file is written here, and wears no mark either way.
    if m.draft {
        return sorts_with_files(m);
    }
    by_parts(m)
}

/// Whether a message has files by its parts, while their bytes are here: the calendar part of
/// meeting mail is not one (the row wears the meeting's mark for it), any other calendar file
/// is, and a message not fetched yet goes by the server.
pub(crate) fn by_parts(m: &mailcraft_model::Message) -> bool {
    match m.attachments.iter().position(crate::session::is_calendar_part) {
        Some(index) if is_meeting_mail(&m.attachments[index].data) => m.listed_attachments().any(|(other, _)| other != index),
        _ => m.has_attachments(),
    }
}

/// Whether a message sorts among those with files (Arrange By Attachments). Opening a message, or
/// letting its parts go again, never moves it in the list (#575 review). A message whose server
/// has said goes by that ([`server_answer`]): a meeting invitation's own calendar part isn't a
/// file, a calendar file attached to a mail is, as the target has it. Any other goes only by its
/// list of parts, never by a part's bytes or the HTML; the price there: no calendar part counts,
/// so a plain `.ics` file sorts with the messages without files, and every inline picture with a
/// `Content-ID` is taken to be one the HTML shows, as the attachment list takes it while the
/// HTML is kept outside memory.
pub fn sorts_with_files(m: &mailcraft_model::Message) -> bool {
    server_answer(m).unwrap_or_else(|| {
        (m.unfetched && m.attachments_on_server)
            || m.attachments.iter().any(|a| !crate::session::is_calendar_part(a) && !(a.inline && !a.content_id.is_empty()))
    })
}

/// What the message's server last said of whether it has attachments (or, once some were removed
/// here, what its parts said then), while that speaks for it: not for a draft, which is written
/// here and goes up later, so that the server's answer lags behind its files.
fn server_answer(m: &mailcraft_model::Message) -> Option<bool> {
    m.attachments_changed_here.or(m.server_attachments).filter(|_| !m.draft)
}

/// Whether a calendar part is meeting mail (a request, a counter-proposal, a cancellation or an
/// answer), from its `METHOD` in its first lines: the window's glance at it for the row's mark.
pub fn is_meeting_mail(data: &[u8]) -> bool {
    let head = &data[..data.len().min(16 * 1024)];
    let method = head.split(|byte| *byte == b'\n').take(400).find_map(|line| {
        let line = line.trim_ascii();
        line.get(..7).filter(|start| start.eq_ignore_ascii_case(b"METHOD:")).map(|_| line[7..].trim_ascii())
    });
    method.is_some_and(|method| [&b"REQUEST"[..], b"COUNTER", b"CANCEL", b"REPLY"].iter().any(|word| method.eq_ignore_ascii_case(word)))
}

/// Query with `{}`; change the view with `enabled` and/or an ordered `items` array.
pub(crate) fn command(s: &mut Session, v: &Value) -> CmdResult {
    use crate::cmd::bad;
    let mut next = s.prefs.columns.clone();
    if let Some(enabled) = v.get("enabled") {
        next.enabled = enabled.as_bool().ok_or_else(|| bad("`enabled` is a boolean"))?;
    }
    if let Some(items) = v.get("items") {
        let items = items
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= Column::ALL.len())
            .ok_or_else(|| bad("`items` is a nonempty list of at most nine columns"))?;
        let mut out = Vec::new();
        for item in items {
            let spec: ColumnSpec =
                serde_json::from_value(item.clone()).map_err(|_| bad("each item has a column name and a width in whole points"))?;
            if item.get("column").is_none()
                || item.get("width").is_none()
                || spec.column == Column::Unknown
                || out.iter().any(|x: &ColumnSpec| x.column == spec.column)
            {
                return Err(bad("each column must be known and occur once"));
            }
            if !(36..=800).contains(&spec.width) {
                return Err(bad("column width is between 36 and 800 points"));
            }
            out.push(spec);
        }
        if !out.iter().any(|i| i.column == Column::Subject) {
            return Err(bad("the Subject column must remain visible"));
        }
        next.items = out;
    }
    next.sanitize();
    s.prefs.columns = next;
    Ok(json!(s.prefs.columns))
}

#[cfg(test)]
#[path = "columns_tests.rs"]
mod columns_tests;
