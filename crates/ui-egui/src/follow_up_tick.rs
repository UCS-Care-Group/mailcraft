// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Follow-up reminders come due (#948): `mail.settleReminders` runs once when the window starts
//! (so that one that fell due while MailCraft was closed is raised at once, and once) and then at
//! most every half minute while it runs: it raises what is due, cancels "if nobody replies"
//! reminders that somebody replied to, and ends those whose flag was cleared elsewhere. What it
//! raised is shown by the Reminders window ([`crate::reminders`]).

use serde_json::json;

use crate::MailApp;

/// Seconds between two looks.
pub(crate) const EVERY: i64 = 30;

impl MailApp {
    /// Settle the reminders: at the first call, and then every [`EVERY`] seconds (a clock put back
    /// starts the interval again). Cheap in between, and when no message has a reminder.
    pub(crate) fn follow_up_tick(&mut self) {
        // A mailbox still opening has no reminders yet: the first look waits for it.
        if self.opening.is_some() {
            return;
        }
        let now = self.session.now();
        if self.follow_up_last != 0 && now >= self.follow_up_last && now - self.follow_up_last < EVERY {
            return;
        }
        self.follow_up_last = now;
        // Only when something is due or ended: a run that has nothing to do copies the mailbox
        // for nothing.
        if self.session.mailbox.follow_up_work(now) {
            let _ = self.execute("mail.settleReminders", json!({}));
        }
    }
}

#[cfg(test)]
#[path = "follow_up_tick_tests.rs"]
mod tests;
