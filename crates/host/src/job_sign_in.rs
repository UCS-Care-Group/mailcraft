// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose sign-in a calendar or To Do job's answer came with, judged once for every front end (the
//! desktop's window and the phone's core alike) so that the rule can't drift between them.
//!
//! A job renews its sign-in for itself, and the tokens it gets name who signed in. When they name
//! somebody else than the account the sign-in is for (a shared mailbox's calendars go with the
//! sign-in of the account they are opened through), nothing of the answer is that account's: it
//! is refused whole. Nothing of it is taken into the mailbox, the token is not kept, and the front
//! end tells the owner in its own words, as it does for a sign-in of somebody else's anywhere
//! (#103). The jobs themselves apply it as soon as the sign-in is renewed, before anything goes
//! up or is read ([`crate::tasks::run_with`], and the Graph run for a request that names its
//! [`crate::graph::GraphRequest::signer`]); [`crate::calendar_job::CalendarJobs::poll`] and
//! [`crate::tasks_job::TasksJobs::poll`] apply it again before anything is handed on.
//!
//! The address the tokens name is for the screen alone: never logged, never in a `Debug` output.

use mailcraft_model::Account;
use mailcraft_model::sync::Tokens;

/// The most characters of the address a refused sign-in named that are handed on to be shown.
const LONGEST_ADDRESS: usize = 320;

/// A job's answer that came with somebody else's sign-in: refused whole.
#[derive(Clone, PartialEq, Eq)]
pub struct ForeignSignIn {
    /// Who the tokens say signed in, cut to a sensible length: said on screen, never logged.
    pub signed_in_as: String,
}

impl std::fmt::Debug for ForeignSignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForeignSignIn").finish_non_exhaustive()
    }
}

/// Whether the `tokens` a job came back with are somebody else's than `signer`'s, the account whose
/// sign-in the job went with ([`mailcraft_engine::sync_driver::signed_in_as_other`]: a Google
/// identity must be the account's address; tokens that name nobody are the account's, as they
/// always were). `None` when they are the account's own or when there are none.
pub fn foreign_sign_in(signer: &Account, tokens: Option<&Tokens>) -> Option<ForeignSignIn> {
    let tokens = tokens?;
    if !mailcraft_engine::sync_driver::signed_in_as_other(signer, tokens) {
        return None;
    }
    Some(ForeignSignIn { signed_in_as: tokens.email.trim().chars().take(LONGEST_ADDRESS).collect() })
}
