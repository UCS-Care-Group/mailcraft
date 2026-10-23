// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft iCalendar: events and to-dos as RFC 5545 text (`.ics` files, meeting invitations).
//!
//! [`parse`] reads `VEVENT`s and `VTODO`s, [`write()`] writes them, and [`parse_rrule`] and
//! [`format_rrule`] turn a recurrence rule into the model's [`Recurrence`](mailcraft_model::Recurrence)
//! and back.
//!
//! The model keeps event times as wall-clock times in the zone the calendar is shown in, so the
//! reader converts on the way in: a UTC time (`…Z`) and a time in a zone (`TZID`) become the time
//! there through the caller's `offset_at`, and a date becomes an all-day event. What a `TZID`
//! means comes from the `VTIMEZONE` the text brings, else from the zone of that name (`zones.rs`
//! has the order, and what happens to a zone nobody knows). An event in a zone with another
//! clock keeps that zone, and a series repeats by it. A time without any zone is taken as it is
//! written.
//!
//! The writer writes times without a zone ([`write()`]) or, told which zone the calendar is shown
//! in ([`write_in`]), times with their zone and the `VTIMEZONE` that defines it, which is what
//! other programs read right.
//!
//! Input is hostile. Nothing here panics on any text: line lengths and counts are capped, and a
//! component that makes no sense is skipped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod lines;
mod read;
mod rrule;
#[cfg(test)]
mod tests;
mod values;
mod write;
#[cfg(test)]
mod zone_budget_tests;
#[cfg(test)]
mod zone_tests;
mod zones;

use mailcraft_model::{Event, Task};
use mailcraft_time::Timestamp;

pub use read::{parse, parse_events};
pub use rrule::{format_rrule, parse_rrule};
pub use write::{write, write_in};

/// What an iCalendar text holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IcsFile {
    pub events: Vec<Event>,
    pub tasks: Vec<Task>,
    /// `METHOD` (`REQUEST`, `REPLY`, `CANCEL`, `PUBLISH`…), upper-cased; empty when absent.
    pub method: String,
    /// What there is to tell the owner about how the text was read, in sentences: a time zone
    /// nobody knows (its times are kept as written, or at the offset its name gives), a zone the
    /// text defines otherwise than its name says. A few at most; empty when everything was
    /// taken at its word.
    pub notes: Vec<String>,
}

/// What [`write()`] puts around the events and to-dos.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// `METHOD` to write; empty for none.
    pub method: String,
    /// `DTSTAMP` (UTC).
    pub now: Timestamp,
}
