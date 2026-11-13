// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline (#143), the decisions with no I/O: when mail outside an account's window
//! is taken back to its headers and preview. It is done when a mailbox is opened, once a day
//! while it stays open, and when the owner shortens the window, a few hundred messages at a time
//! (`account.thinOffline` with `most`), so that a first thinning of years of mail never holds a
//! front end up. A front end calls [`Session::offline_tick`] when it has a moment.

use serde_json::json;

use crate::Session;

/// Messages taken back to their headers and preview in one step.
pub const THIN_BATCH: u64 = 400;
/// How often a mailbox that stays open is thinned again (milliseconds).
pub const DAY_MS: f64 = 86_400_000.0;

/// When mail was last taken back, and for which opening of a mailbox.
#[derive(Debug, Default)]
pub struct Thinning {
    /// [`Session::openings`] when it was last done; `None` before the first.
    opening: Option<u64>,
    /// When it was last done (milliseconds).
    pub at: f64,
    /// More to do at once: messages left past the last batch, or a window just shortened.
    pub now: bool,
}

impl Thinning {
    /// Whether thinning is due at `now` for the mailbox opening `opening`.
    pub fn due(&self, opening: u64, now: f64) -> bool {
        self.now || self.opening != Some(opening) || now - self.at >= DAY_MS
    }

    /// A thinning step was started at `now` for `opening`.
    pub fn started(&mut self, opening: u64, now: f64) {
        (self.opening, self.at, self.now) = (Some(opening), now, false);
    }
}

/// What one [`Session::offline_tick`] came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Thinned {
    /// Nothing was due, or no account has a window.
    Idle,
    /// A batch was taken back and no more is left.
    Done,
    /// A batch was taken back and more is left: ask again at the next step.
    More,
    /// It failed; the text is for the log.
    Failed(String),
}

impl Session {
    /// Take mail outside its window back when it is due at `now` (milliseconds, any clock).
    pub fn offline_tick(&mut self, now: f64) -> Thinned {
        let opening = self.openings();
        if !self.driver.thinning.due(opening, now) {
            return Thinned::Idle;
        }
        self.driver.thinning.started(opening, now);
        if !self.mailbox.accounts.iter().any(|a| a.kind.has_offline_window()) {
            return Thinned::Idle;
        }
        match self.run("account.thinOffline", &json!({"most": THIN_BATCH})) {
            Ok(said) if said.get("left").and_then(serde_json::Value::as_u64).unwrap_or(0) > 0 => {
                self.driver.thinning.now = true;
                Thinned::More
            }
            Ok(_) => Thinned::Done,
            Err(e) => Thinned::Failed(e.to_string()),
        }
    }
}
