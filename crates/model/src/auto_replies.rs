// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Automatic replies (File › Info › Automatic Replies, the "out of office" of a mailbox), as
//! plain data like [`crate::remote`]: the setting as a Microsoft account's server keeps it
//! (`automaticRepliesSetting` of its mailbox settings, in Microsoft Graph's own names), which a
//! host with sockets brings down and takes up; the engine says what is to be asked
//! (`account.automaticReplies`, `account.setAutomaticReplies`) and takes what came back in
//! (`sync.applyAutomaticReplies`).
//!
//! All of it is hostile: it comes from a server, or from a command's parameters.
//! [`AutoReplies::tidy`] cuts it to size before any of it is kept. The two replies are HTML on
//! the server: what comes down is shown as plain text, never as a page, and what goes up is
//! the owner's plain text written as simple HTML.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Longest reply taken from a server or sent to one, in bytes of its HTML. Exchange's own limit
/// is about this; a reply longer than that is cut.
pub const MAX_REPLY_HTML: usize = 128 * 1024;
/// Longest reply the owner may type, in characters of plain text (written as HTML it grows).
pub const MAX_REPLY_TEXT: usize = 16 * 1024;
/// Longest time and name of a time zone taken.
const MAX_TIME: usize = 40;
const MAX_ZONE: usize = 100;

/// Whether the mailbox answers mail by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoReplyStatus {
    /// Don't send automatic replies.
    #[default]
    Disabled,
    /// Send them until they are turned off.
    AlwaysEnabled,
    /// Send them only between a start and an end.
    Scheduled,
}

impl AutoReplyStatus {
    pub const ALL: [AutoReplyStatus; 3] = [AutoReplyStatus::Disabled, AutoReplyStatus::AlwaysEnabled, AutoReplyStatus::Scheduled];

    /// Graph's name for it.
    pub fn code(self) -> &'static str {
        match self {
            AutoReplyStatus::Disabled => "disabled",
            AutoReplyStatus::AlwaysEnabled => "alwaysEnabled",
            AutoReplyStatus::Scheduled => "scheduled",
        }
    }

    /// From Graph's name, whatever its case.
    pub fn parse(s: &str) -> Option<AutoReplyStatus> {
        Self::ALL.into_iter().find(|x| x.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// Who outside the owner's organisation gets the reply for outside it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExternalAudience {
    /// Nobody outside the organisation.
    None,
    /// Only the owner's contacts.
    ContactsOnly,
    /// Anyone outside the organisation.
    #[default]
    All,
}

impl ExternalAudience {
    pub const ALL: [ExternalAudience; 3] = [ExternalAudience::None, ExternalAudience::ContactsOnly, ExternalAudience::All];

    /// Graph's name for it.
    pub fn code(self) -> &'static str {
        match self {
            ExternalAudience::None => "none",
            ExternalAudience::ContactsOnly => "contactsOnly",
            ExternalAudience::All => "all",
        }
    }

    /// From Graph's name, whatever its case.
    pub fn parse(s: &str) -> Option<ExternalAudience> {
        Self::ALL.into_iter().find(|x| x.code().eq_ignore_ascii_case(s.trim()))
    }
}

/// A wall-clock time and the zone it is by, as Graph writes one (`dateTimeTimeZone`):
/// `2026-11-02T08:00:00.0000000` and `Pacific Standard Time`, `Europe/Berlin` or `UTC`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZonedTime {
    pub date_time: String,
    pub time_zone: String,
}

impl ZonedTime {
    /// On one line and no longer than a time and a zone's name are.
    fn tidy(&mut self) {
        self.date_time = one_line(&self.date_time, MAX_TIME);
        self.time_zone = one_line(&self.time_zone, MAX_ZONE);
    }
}

/// The automatic replies of a mailbox, in Graph's names. In a download every field is the
/// server's; in a change only what is to change is there (Graph changes what it is told and
/// leaves the rest).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoReplies {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<AutoReplyStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_start_date_time: Option<ZonedTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduled_end_date_time: Option<ZonedTime>,
    /// The reply for people inside the organisation, as HTML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub internal_reply_message: Option<String>,
    /// The reply for people outside it, as HTML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_reply_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_audience: Option<ExternalAudience>,
}

impl AutoReplies {
    /// Cut everything to the size that is taken: the replies to [`MAX_REPLY_HTML`] bytes (at a
    /// character's boundary), the times and zones to a line.
    pub fn tidy(&mut self) {
        for reply in [&mut self.internal_reply_message, &mut self.external_reply_message].into_iter().flatten() {
            *reply = cut(reply, MAX_REPLY_HTML);
        }
        for time in [&mut self.scheduled_start_date_time, &mut self.scheduled_end_date_time].into_iter().flatten() {
            time.tidy();
        }
    }

    /// Nothing is to change.
    pub fn is_empty(&self) -> bool {
        *self == AutoReplies::default()
    }

    /// The automatic replies in a server's answer (Graph's JSON): the setting itself, or the one
    /// under `automaticRepliesSetting` when the answer is all of the mailbox's settings. What
    /// isn't what it should be (a status or an audience that is none of them, a time that isn't
    /// an object with a time in it, a reply that isn't text) is left out, and the rest is tidied.
    pub fn from_graph(answer: &Value) -> AutoReplies {
        let setting = answer.get("automaticRepliesSetting").filter(|inner| inner.is_object()).unwrap_or(answer);
        let text = |of: &Value, name: &str| of.get(name).and_then(Value::as_str).unwrap_or_default().to_string();
        let time = |name: &str| {
            let at = setting.get(name).filter(|at| at.is_object())?;
            let date_time = text(at, "dateTime");
            (!date_time.trim().is_empty()).then(|| ZonedTime { date_time, time_zone: text(at, "timeZone") })
        };
        let reply = |name: &str| setting.get(name).and_then(Value::as_str).map(str::to_string);
        let mut read = AutoReplies {
            status: AutoReplyStatus::parse(&text(setting, "status")),
            scheduled_start_date_time: time("scheduledStartDateTime"),
            scheduled_end_date_time: time("scheduledEndDateTime"),
            internal_reply_message: reply("internalReplyMessage"),
            external_reply_message: reply("externalReplyMessage"),
            external_audience: ExternalAudience::parse(&text(setting, "externalAudience")),
        };
        read.tidy();
        read
    }
}

/// `text` cut to at most `max` bytes at a character's boundary.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_string()
}

/// `text` on one line, without control characters, at most `max` characters.
fn one_line(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}
