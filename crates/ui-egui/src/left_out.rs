// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages left out for failing to download (#1581): File › Info says so for as long as it is so,
//! with a way to try them again (`sync.retryFailed`).

use mailcraft_model::Id;
use serde_json::json;

use crate::MailApp;

impl MailApp {
    /// What File › Info says of an account's messages that were left out, when there are any.
    pub(crate) fn left_out_line(&self, account: Id) -> Option<String> {
        let n = self.session.mailbox.left_out(account);
        (n > 0).then(|| {
            tln!(n, "{n} message was left out because it failed to download.", "{n} messages were left out because they failed to download.")
        })
    }

    /// The owner asks for them again: the next Send / Receive tries each as if it were new.
    pub(crate) fn retry_left_out(&mut self, account: Id) {
        let _ = self.run("sync.retryFailed", json!({"account": account}));
    }
}
