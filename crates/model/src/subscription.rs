// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Subscribed calendars (Outlook's Internet Calendars): a calendar of its own, read-only, whose
//! events are a copy of an `.ics` file that a web address serves, brought down again from time to
//! time.
//!
//! The address can hold a secret (the private addresses that calendar services hand out carry a
//! token in the path), so it is kept like a password: the host puts it in the system's credential
//! store, or keeps it in memory for the session, and never in the mailbox file. All that the
//! mailbox keeps of it is the host's name, for showing ([`Subscription::host`]), and what the
//! server said about the copy that was taken (its validators), so that the next look can ask
//! "has it changed?".
//!
//! Plain data, like [`crate::remote`]: the engine checks an address as text ([`check_address`])
//! and says what a look at the web should ask ([`SubscriptionAsk`]); a host with sockets
//! (`mailcraft-net`) does the asking and brings back a [`SubscriptionOutcome`], which the engine
//! takes in (`sync.applySubscription`). All of it is hostile: it is somebody else's file.

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Id;

/// Longest address that is taken, in bytes.
pub const MAX_ADDRESS: usize = 2_048;
/// Shortest and longest time between two looks, in minutes (a quarter of an hour, a day), and
/// the usual one.
pub const MIN_INTERVAL: u32 = 15;
pub const MAX_INTERVAL: u32 = 24 * 60;
pub const DEFAULT_INTERVAL: u32 = 60;
/// Most subscribed calendars, and most events in one of them and in all of them: what is beyond
/// is left out, and File › Info says so.
pub const MAX_SUBSCRIBED: usize = 20;
pub const MAX_EVENTS: usize = 20_000;
pub const MAX_EVENTS_TOTAL: usize = 100_000;
/// Most text a look brings, in bytes (what the network crate takes at most).
pub const MAX_TEXT: usize = 8 * 1024 * 1024;
/// Longest validator (`ETag`, `Last-Modified`) that is kept and sent back.
pub const MAX_VALIDATOR: usize = 256;
/// Where a subscribed calendar is listed.
pub const GROUP: &str = "Other Calendars";

/// What the mailbox keeps of a subscribed calendar (everything but its address).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Subscription {
    /// The host of the address, for showing; never more of the address than that.
    pub host: String,
    /// When it was subscribed to (seconds): with the host and the calendar's id, it names the
    /// entry that holds the address in the credential store ([`Subscription::secret_name`]).
    pub since: Timestamp,
    /// Minutes between two looks ([`MIN_INTERVAL`] to [`MAX_INTERVAL`]).
    pub interval_minutes: u32,
    /// The server's marks of the copy that was taken, sent back to ask whether it changed.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub etag: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub last_modified: String,
    /// When the server last answered a look (seconds); 0 for never.
    pub checked: Timestamp,
    /// Why the last look brought nothing (a [`Failure`]'s code); empty when it went well. What
    /// was there before stays.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub failure: String,
    /// How many events the last copy holds.
    pub events: usize,
    /// The file held more than may be kept: the events beyond the limit were left out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub capped: bool,
    /// The owner asked for a look now (Send/Receive, Update Now): not kept.
    #[serde(skip)]
    pub asked: bool,
}

impl Subscription {
    /// A subscription to `host` made at `since`, looked at every [`DEFAULT_INTERVAL`] minutes.
    pub fn new(host: &str, since: Timestamp) -> Subscription {
        Subscription { host: host.to_string(), since, interval_minutes: DEFAULT_INTERVAL, ..Default::default() }
    }

    /// The minutes between two looks, within bounds whatever a file says.
    pub fn interval(&self) -> u32 {
        match self.interval_minutes {
            0 => DEFAULT_INTERVAL,
            minutes => minutes.clamp(MIN_INTERVAL, MAX_INTERVAL),
        }
    }

    /// The name of the entry that holds the calendar's address in the system's credential store:
    /// bound to the host, so that an address is never handed to another host, and to the
    /// calendar, so that two mailboxes' calendars with the same number don't share one.
    pub fn secret_name(&self, calendar: Id) -> String {
        format!("subscription:{calendar}|host={}|since={}", self.host.trim().to_ascii_lowercase(), self.since)
    }

    /// Whether a look is due at `now`: never looked at, or the interval has gone by (a clock that
    /// went back counts as due).
    pub fn due(&self, now: Timestamp) -> bool {
        self.asked || self.checked <= 0 || now < self.checked || now.saturating_sub(self.checked) >= i64::from(self.interval()).saturating_mul(60)
    }

    /// Why the last look brought nothing, if it didn't.
    pub fn failed(&self) -> Option<Failure> {
        (!self.failure.is_empty()).then(|| Failure::parse(&self.failure))
    }
}

/// Why a look brought nothing. What each says reads on after "Couldn't update:".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The address isn't one that is read (not `https`, too long, a user name in it).
    Address,
    /// The address is this computer's, or of a network that is not the public internet.
    NotPublic,
    TooManyRedirects,
    /// The server answered something other than the file (401, 403, 404, 5xx...).
    Status(u16),
    TooLarge,
    TimedOut,
    /// It came packed in a way that can't be read.
    Packed,
    /// No connection, or its encryption couldn't be set up.
    Network,
    /// What came isn't an iCalendar file.
    NotCalendar,
    /// Anything else (a code from a newer version).
    Other,
}

impl Failure {
    pub fn code(self) -> String {
        match self {
            Failure::Address => "address".into(),
            Failure::NotPublic => "notPublic".into(),
            Failure::TooManyRedirects => "redirects".into(),
            Failure::Status(status) => format!("status:{status}"),
            Failure::TooLarge => "tooLarge".into(),
            Failure::TimedOut => "timedOut".into(),
            Failure::Packed => "packed".into(),
            Failure::Network => "network".into(),
            Failure::NotCalendar => "notCalendar".into(),
            Failure::Other => "other".into(),
        }
    }

    pub fn parse(code: &str) -> Failure {
        match code {
            "address" => Failure::Address,
            "notPublic" => Failure::NotPublic,
            "redirects" => Failure::TooManyRedirects,
            "tooLarge" => Failure::TooLarge,
            "timedOut" => Failure::TimedOut,
            "packed" => Failure::Packed,
            "network" => Failure::Network,
            "notCalendar" => Failure::NotCalendar,
            code => code.strip_prefix("status:").and_then(|n| n.parse().ok()).map_or(Failure::Other, Failure::Status),
        }
    }

    /// In words for the owner. (Nothing of the address is in it.)
    pub fn message(self) -> String {
        match self {
            Failure::Address => "its address isn't one that calendars are read from".into(),
            Failure::NotPublic => "its address is on this computer or a private network".into(),
            Failure::TooManyRedirects => "its address led on to too many others".into(),
            Failure::Status(401 | 403) => format!("the server won't hand it out ({})", self.status()),
            Failure::Status(404 | 410) => format!("it is no longer there ({})", self.status()),
            Failure::Status(429) => "the server is being asked too often just now (429)".into(),
            Failure::Status(500..=599) => format!("the server has trouble of its own ({})", self.status()),
            Failure::Status(status) => format!("the server answered {status} instead of a calendar"),
            Failure::TooLarge => "the file is larger than 8 MB".into(),
            Failure::TimedOut => "the server took too long".into(),
            Failure::Packed => "the server sent it packed in a way that can't be read".into(),
            Failure::Network => "the server couldn't be reached or its encryption failed".into(),
            Failure::NotCalendar => "what the server sent isn't a calendar".into(),
            Failure::Other => "it didn't work".into(),
        }
    }

    fn status(self) -> u16 {
        match self {
            Failure::Status(status) => status,
            _ => 0,
        }
    }
}

/// What the host is to ask the web for: one calendar's file. The address is not here: the window
/// keeps it (it may hold a secret) and puts it with the request.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SubscriptionAsk {
    pub calendar: Id,
    pub host: String,
    pub etag: String,
    pub last_modified: String,
}

/// What a look at the web brought.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SubscriptionOutcome {
    /// The file, as text, with the server's marks of it.
    Changed {
        #[serde(default)]
        text: String,
        #[serde(default)]
        etag: String,
        #[serde(default, rename = "lastModified")]
        last_modified: String,
    },
    /// The server said the copy taken is still current (304).
    NotModified,
    /// Nothing came: a [`Failure`]'s code.
    Failed {
        #[serde(default)]
        failure: String,
    },
}

// (The file may be megabytes: never printed.)
impl std::fmt::Debug for SubscriptionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubscriptionOutcome::Changed { text, .. } => write!(f, "Changed({} bytes)", text.len()),
            SubscriptionOutcome::NotModified => f.write_str("NotModified"),
            SubscriptionOutcome::Failed { failure } => write!(f, "Failed({failure})"),
        }
    }
}

/// Why an address isn't taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddressProblem {
    Empty,
    /// More than [`MAX_ADDRESS`] bytes.
    TooLong,
    /// Not an `https://` or `webcal://` address.
    NotWeb,
    /// `http://`: nothing is read unencrypted. The same address over `https://` may do
    /// ([`try_https`]).
    Plain,
    /// A user name (and password) in front of the host.
    UserInfo,
    /// Spaces or control characters, no host, or a port that isn't one.
    Malformed,
}

impl AddressProblem {
    /// In words for the owner. (None of it quotes the address.)
    pub fn message(self) -> &'static str {
        match self {
            AddressProblem::Empty => "Type the address of the calendar.",
            AddressProblem::TooLong => "That address is too long to be a calendar's.",
            AddressProblem::NotWeb => "A calendar's address starts with https:// or webcal://.",
            AddressProblem::Plain => {
                "MailCraft reads calendars over an encrypted connection only: an address starting with http:// is refused. Try the same address with https://."
            }
            AddressProblem::UserInfo => "An address with a user name in it isn't taken: a calendar that needs a sign-in can't be subscribed to here.",
            AddressProblem::Malformed => "That doesn't look like a web address.",
        }
    }
}

/// An address that is taken: written as `https://`, and its host.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedAddress {
    pub url: String,
    pub host: String,
}

// (The address may hold a secret.)
impl std::fmt::Debug for CheckedAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CheckedAddress({})", self.host)
    }
}

/// Check an address as text, before anything is sent: `https://`, or `webcal://` (and `webcals://`),
/// which is read as `https://`; no user name; at most [`MAX_ADDRESS`] bytes; a host. Nothing is
/// looked up.
pub fn check_address(text: &str) -> Result<CheckedAddress, AddressProblem> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AddressProblem::Empty);
    }
    if text.len() > MAX_ADDRESS {
        return Err(AddressProblem::TooLong);
    }
    let (scheme, rest) = text.split_once("://").ok_or(AddressProblem::NotWeb)?;
    match scheme.to_ascii_lowercase().as_str() {
        "https" | "webcal" | "webcals" => {}
        "http" => return Err(AddressProblem::Plain),
        _ => return Err(AddressProblem::NotWeb),
    }
    if rest.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '\\' | '"' | '<' | '>')) {
        return Err(AddressProblem::Malformed);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or("");
    if authority.contains('@') {
        return Err(AddressProblem::UserInfo);
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(inside) => {
            let (host, after) = inside.split_once(']').ok_or(AddressProblem::Malformed)?;
            (host, after.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if let Some(port) = port
        && (port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u32>().is_ok_and(|p| p == 0 || p > 65_535))
    {
        return Err(AddressProblem::Malformed);
    }
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host_ok = !host.is_empty() && host.len() <= 253 && !host.contains(['/', '?', '#', '@', '[', ']', '%']) && !host.starts_with('.');
    if !host_ok {
        return Err(AddressProblem::Malformed);
    }
    Ok(CheckedAddress { url: format!("https://{rest}"), host })
}

/// The same address over `https://`, for the offer that follows a refused `http://` address;
/// `None` when it isn't an `http://` address.
pub fn try_https(text: &str) -> Option<String> {
    let text = text.trim();
    let (scheme, rest) = text.split_once("://")?;
    scheme.eq_ignore_ascii_case("http").then(|| format!("https://{rest}"))
}

/// A header value that is safe to send back as a validator: printable ASCII, not too long;
/// empty when it is anything else.
pub fn clean_validator(value: &str) -> String {
    let value = value.trim();
    if value.len() <= MAX_VALIDATOR && value.bytes().all(|b| (0x20..0x7f).contains(&b)) { value.to_string() } else { String::new() }
}

#[cfg(test)]
#[path = "subscription_tests.rs"]
mod tests;
