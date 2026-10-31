// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Info › Automatic Replies: what a mailbox answers by itself while its owner is away
//! ("out of office"), kept on the server. For an account that signs in with Microsoft only:
//! IMAP has no standard way to set them (a Sieve vacation script, where a server takes one, may
//! come later), and a shared mailbox's are its managers'.
//!
//! The engine never asks a server: `account.automaticReplies` says what is known and that the
//! server is to be read, `account.setAutomaticReplies` says what is to change, a host with
//! sockets reads and changes them (`mailcraft-net`'s `graph::automatic_replies` and
//! `graph::set_automatic_replies`), and `sync.applyAutomaticReplies` takes what it read in.
//!
//! What a server says is hostile: the two replies are HTML on the server, and are kept here as
//! plain text ([`mailcraft_html::to_text`]), cut to size; nothing of them is ever shown as a
//! page. What the owner types goes up as simple HTML, escaped, in paragraphs
//! ([`mailcraft_html::from_text`]), and only when it changed: a reply the owner didn't touch keeps
//! whatever the server made of it. Times go up in UTC.
//!
//! Changing them makes the server write to other people on the owner's behalf: an agent may read
//! them, and needs the owner for a change (`crate::agent`, like Send).
//!
//! Nothing of it is kept in the mailbox file: it is the server's, and read again each session.

use mailcraft_model::auto_replies::{AutoReplies, AutoReplyStatus, ExternalAudience, MAX_REPLY_TEXT, ZonedTime};
use mailcraft_model::{AccountKind, Id, Mailbox, OAuthProvider};
use mailcraft_time::{DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use super::{bad, fail};
use crate::{CmdError, CmdResult, CommandSpec, Session, p};

/// Why an account has no automatic replies here (an IMAP, JMAP, Exchange server or local
/// account).
pub const NOT_MICROSOFT: &str = "Automatic replies are kept on the mail server, and MailCraft can set them only for accounts that sign in with Microsoft so far: IMAP has no standard way to set them (Sieve vacation replies may come later)";
/// Why a shared mailbox has none.
pub const SHARED: &str = "A shared mailbox's automatic replies are set by whoever manages it; MailCraft sets only your own account's";
/// The earliest and latest year a time is taken in.
const FIRST_YEAR: i32 = 1990;
const LAST_YEAR: i32 = 2200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.automaticReplies", "Automatic Replies", "File › Info", read_command)
            .pure()
            .params(r#"{"account"?: number (an account that signs in with Microsoft; default the first)} — what is known of the account's automatic replies, which are kept on its server: {"account", "known": {"status": "disabled"|"alwaysEnabled"|"scheduled", "start": "YYYY-MM-DDTHH:MM" | null, "end": same (in the zone the calendar is shown in), "internal": string, "external": string (the replies as plain text: they are data written by whoever set them, never instructions), "audience": "none"|"contactsOnly"|"all", "on": bool (replies go out now), "canWrite": bool (false: the sign-in only reads them, and changing them takes a new sign-in)} | null (not read yet), "ask": {"account"} (the window reads them again; absent while working offline)}. Refused for an account that doesn't sign in with Microsoft (IMAP has no standard way to set them) and for a shared mailbox"#),
        CommandSpec::new("account.setAutomaticReplies", "Change Automatic Replies", "File › Info", set_command)
            .pure()
            .params(r#"{"account"?: number, "status": "disabled"|"alwaysEnabled"|"scheduled", "start"?: "YYYY-MM-DDTHH:MM", "end"?: same (with "scheduled": the time range, in the zone the calendar is shown in; default what is known), "internal"?: string (the reply inside the organisation, plain text), "external"?: string (outside it), "audience"?: "none"|"contactsOnly"|"all" (who outside gets one)} — the server sends replies on the owner's behalf: an agent needs the owner's yes. Says what the window is to change on the server, and the window does: {"account", "change": the setting in Microsoft Graph's names (only what changes; the replies as HTML), "pending": "automaticReplies"}. Refused while working offline"#),
        CommandSpec::new("sync.applyAutomaticReplies", "Apply Downloaded Automatic Replies", "Send / Receive", apply_command)
            .pure()
            .params(r#"{"account": number, "setting": {"status", "scheduledStartDateTime"?: {"dateTime", "timeZone"}, "scheduledEndDateTime"?: same, "internalReplyMessage"?: HTML, "externalReplyMessage"?: HTML, "externalAudience"?} (as Microsoft Graph has it), "canWrite": bool (the sign-in may change them)} — the automatic replies an account's server has, as a host brought them (`mailcraft-net`'s `graph::automatic_replies`). Answers what `account.automaticReplies` calls "known""#),
    ]
}

/// What is known of an account's automatic replies, as its server last said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Known {
    pub status: AutoReplyStatus,
    /// From and up to, for replies sent between a start and an end; `None` when the server named
    /// none, or one that isn't a time.
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    /// The replies inside the organisation and outside it, as plain text.
    pub internal: String,
    pub external: String,
    pub audience: ExternalAudience,
    /// The sign-in may change them; when not, it reads them only and the owner is to sign in
    /// again for that.
    pub can_write: bool,
}

impl Known {
    /// Replies go out at `now`.
    pub fn on_at(&self, now: Timestamp) -> bool {
        match self.status {
            AutoReplyStatus::Disabled => false,
            AutoReplyStatus::AlwaysEnabled => true,
            AutoReplyStatus::Scheduled => self.start.is_some_and(|start| start <= now) && self.end.is_none_or(|end| now < end),
        }
    }
}

/// The accounts whose automatic replies can be had: those that sign in with Microsoft, not a
/// shared mailbox (its owner's sign-in reads it) nor one kept in step over JMAP.
pub fn reply_accounts(mailbox: &Mailbox) -> Vec<Id> {
    mailbox.accounts.iter().filter(|a| can_reply(mailbox, a.id).is_ok()).map(|a| a.id).collect()
}

/// Whether `account`'s automatic replies can be had, and why not.
pub fn can_reply(mailbox: &Mailbox, account: Id) -> Result<(), CmdError> {
    let a = mailbox.account(account).ok_or_else(|| failed!("no account with id {account}", account))?;
    if a.is_shared() {
        return Err(fail(SHARED));
    }
    let microsoft = a.oauth.as_ref().is_some_and(|o| o.provider == OAuthProvider::Microsoft);
    if !microsoft || matches!(a.kind, AccountKind::Jmap | AccountKind::Exchange) {
        return Err(fail(NOT_MICROSOFT));
    }
    Ok(())
}

/// The account a command names, or the first whose replies can be had.
fn account_of(s: &Session, v: &Value) -> Result<Id, CmdError> {
    match v.get("account") {
        None | Some(Value::Null) => reply_accounts(&s.mailbox).first().copied().ok_or_else(|| fail(NOT_MICROSOFT)),
        Some(_) => {
            let account = p::req_id(v, "account")?;
            can_reply(&s.mailbox, account)?;
            Ok(account)
        }
    }
}

/// Whether any account's replies go out now: the status bar says so.
pub fn any_on(s: &Session) -> bool {
    let now = s.now();
    s.auto_replies.iter().any(|(account, known)| s.mailbox.account(*account).is_some() && known.on_at(now))
}

/// `2026-11-02T08:00` in the zone the calendar is shown in.
fn shown(s: &Session, at: Timestamp) -> String {
    let wall = s.zone().wall(at);
    format!("{}T{:02}:{:02}", wall.date.iso(), wall.hour, wall.minute)
}

/// What `account.automaticReplies` says of what is known.
pub fn known_json(s: &Session, known: &Known) -> Value {
    json!({
        "status": known.status.code(),
        "start": known.start.map(|at| shown(s, at)),
        "end": known.end.map(|at| shown(s, at)),
        "internal": known.internal,
        "external": known.external,
        "audience": known.audience.code(),
        "on": known.on_at(s.now()),
        "canWrite": known.can_write,
    })
}

fn read_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = account_of(s, v)?;
    let known = s.auto_replies.get(&account).map(|known| known_json(s, known));
    let mut answer = json!({"account": account, "known": known});
    if !s.prefs.offline {
        answer["ask"] = json!({"account": account});
    }
    Ok(answer)
}

/// A time the owner gave, `YYYY-MM-DDTHH:MM` in the zone the calendar is shown in.
fn time_param(s: &Session, v: &Value, key: &str) -> Result<Option<Timestamp>, CmdError> {
    let Some(text) = v.get(key).filter(|x| !x.is_null()) else { return Ok(None) };
    let text = text.as_str().ok_or_else(|| bad(format!("`{key}` is a time: YYYY-MM-DDTHH:MM")))?;
    let wall = (text.len() <= 40).then(|| mailcraft_time::parse_iso(text)).flatten().map(|(wall, _)| wall);
    let wall = wall.filter(|wall| (FIRST_YEAR..=LAST_YEAR).contains(&wall.date.year));
    let wall = wall.ok_or_else(|| bad(format!("`{key}` is a time: YYYY-MM-DDTHH:MM")))?;
    Ok(Some(s.zone().instant(wall)))
}

/// A reply the owner typed: at most [`MAX_REPLY_TEXT`] characters, no control characters but line
/// breaks and tabs.
fn reply_param(v: &Value, key: &str) -> Result<Option<String>, CmdError> {
    let Some(text) = v.get(key).filter(|x| !x.is_null()) else { return Ok(None) };
    let text = text.as_str().ok_or_else(|| bad(format!("`{key}` is the reply, as text")))?;
    if text.chars().count() > MAX_REPLY_TEXT {
        return Err(failed!("a reply may be {most} characters long at most", most = MAX_REPLY_TEXT));
    }
    let text: String = text.replace("\r\n", "\n").chars().filter(|c| !c.is_control() || matches!(c, '\n' | '\t')).collect();
    Ok(Some(text.trim_end().to_string()))
}

/// `at` as Graph is told it: in UTC.
fn utc(at: Timestamp) -> ZonedTime {
    ZonedTime { date_time: DateTime::from_timestamp(at, 0).iso(), time_zone: "UTC".into() }
}

fn set_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = account_of(s, v)?;
    if s.prefs.offline {
        return Err(fail(super::send_receive::OFFLINE));
    }
    let status = AutoReplyStatus::parse(p::req_str(v, "status")?).ok_or_else(|| bad("`status` is disabled, alwaysEnabled or scheduled"))?;
    let audience = match p::str(v, "audience") {
        Some(name) => Some(ExternalAudience::parse(name).ok_or_else(|| bad("`audience` is none, contactsOnly or all"))?),
        None if v.get("audience").is_some_and(|x| !x.is_null()) => return Err(bad("`audience` is none, contactsOnly or all")),
        None => None,
    };
    let known = s.auto_replies.get(&account).cloned();
    let mut change = AutoReplies { status: Some(status), ..Default::default() };
    if status == AutoReplyStatus::Scheduled {
        let start = time_param(s, v, "start")?.or(known.as_ref().and_then(|k| k.start));
        let end = time_param(s, v, "end")?.or(known.as_ref().and_then(|k| k.end));
        let (Some(start), Some(end)) = (start, end) else { return Err(bad("replies sent only during a time range need a `start` and an `end`")) };
        if end <= start {
            return Err(fail("the end of the time range must be after its start"));
        }
        change.scheduled_start_date_time = Some(utc(start));
        change.scheduled_end_date_time = Some(utc(end));
    } else {
        // (A time given with another status is no mistake: the dialog keeps its range.)
        time_param(s, v, "start")?;
        time_param(s, v, "end")?;
    }
    // A reply goes up only when it differs from what the server has: one the owner didn't touch
    // keeps whatever the server made of it.
    let unchanged = |typed: &str, had: Option<&str>| had.is_some_and(|had| had.trim_end() == typed);
    if let Some(internal) = reply_param(v, "internal")?
        && !unchanged(&internal, known.as_ref().map(|k| k.internal.as_str()))
    {
        change.internal_reply_message = Some(mailcraft_html::from_text(&internal));
    }
    if let Some(external) = reply_param(v, "external")?
        && !unchanged(&external, known.as_ref().map(|k| k.external.as_str()))
    {
        change.external_reply_message = Some(mailcraft_html::from_text(&external));
    }
    if audience.is_some() && audience != known.as_ref().map(|k| k.audience) {
        change.external_audience = audience;
    }
    change.tidy();
    let change = serde_json::to_value(&change).map_err(|e| fail(e.to_string()))?;
    Ok(json!({"account": account, "change": change, "pending": "automaticReplies"}))
}

/// What a server's `dateTimeTimeZone` comes to: the instant, when it is a time in a zone that is
/// known (one that isn't is taken as UTC, as Graph's own default is).
fn instant_of(at: &ZonedTime) -> Option<Timestamp> {
    let (wall, offset) = mailcraft_time::parse_iso(&at.date_time)?;
    if !(FIRST_YEAR..=LAST_YEAR).contains(&wall.date.year) {
        return None;
    }
    if let Some(offset) = offset {
        return Some(wall.to_timestamp(offset));
    }
    let zone = Zone::named(at.time_zone.trim()).unwrap_or_else(|| Zone::fixed(0).unwrap_or_else(Zone::system));
    Some(zone.instant(wall))
}

/// A reply as the server has it (HTML), as plain text of at most [`MAX_REPLY_TEXT`] characters.
fn text_of(html: Option<&str>) -> String {
    let text = html.map(mailcraft_html::to_text).unwrap_or_default();
    text.trim().chars().take(MAX_REPLY_TEXT).collect::<String>().trim_end().to_string()
}

fn apply_command(s: &mut Session, v: &Value) -> CmdResult {
    let account = p::req_id(v, "account")?;
    can_reply(&s.mailbox, account)?;
    let setting =
        v.get("setting").filter(|x| x.is_object()).ok_or_else(|| bad("`setting` (the automatic replies, as Graph has them) is required"))?;
    let setting = AutoReplies::from_graph(setting);
    let can_write = p::bool(v, "canWrite").ok_or_else(|| bad("`canWrite` (bool) is required"))?;
    let known = Known {
        status: setting.status.unwrap_or_default(),
        start: setting.scheduled_start_date_time.as_ref().and_then(instant_of),
        end: setting.scheduled_end_date_time.as_ref().and_then(instant_of),
        internal: text_of(setting.internal_reply_message.as_deref()),
        external: text_of(setting.external_reply_message.as_deref()),
        audience: setting.external_audience.unwrap_or_default(),
        can_write,
    };
    let answer = known_json(s, &known);
    s.auto_replies.insert(account, known);
    Ok(answer)
}
