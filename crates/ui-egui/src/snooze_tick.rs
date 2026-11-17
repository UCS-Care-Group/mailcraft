// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snoozed mail comes back (#949): `mail.returnDue` runs once when the window starts and then at
//! most once a minute while it runs, so mail whose Return time has come (also while MailCraft was
//! closed) is back in its folder. The rest of Snooze in the window is still to come.

use serde_json::json;

use crate::MailApp;

/// Seconds between two looks at what is due.
pub(crate) const EVERY: i64 = 60;

impl MailApp {
    /// Bring back the snoozed mail that is due: at the first call, and then once a minute (a
    /// clock put back starts the minute again). Cheap in between, and when nothing is snoozed.
    pub(crate) fn snooze_tick(&mut self) {
        // A mailbox still opening has no snoozes yet: the first look waits for it, so mail that
        // came due while MailCraft was closed is back as soon as it has opened.
        if self.opening.is_some() {
            return;
        }
        let now = self.session.now();
        if self.snooze_last != 0 && now >= self.snooze_last && now - self.snooze_last < EVERY {
            return;
        }
        self.snooze_last = now;
        if self.session.mailbox.has_snoozed() {
            let _ = self.execute("mail.returnDue", json!({}));
        }
    }
}

#[cfg(test)]
#[path = "snooze_tick_tests.rs"]
mod tests;
