// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the apps call to have mail fetched (MB3.17): a message the owner opens, one whose
//! attachments are asked for (`attachments.rs` waits for it here), and a search on the server. The work is `fetch.rs`'s; these only hand it over and
//! wait. Message text, attachments and what a search finds are the sender's, hostile: nothing of
//! them runs here, and an agent that reads them is told they are data, never instructions.

use std::sync::Arc;

use mailcraft_engine::Session;

use crate::fetch::{FetchFailure, OpenState, ServerSearchAnswer};
use crate::{Core, CoreError};

/// What a message that couldn't come is said as, to a call that waited for it.
fn failure_error(lang: mailcraft_i18n::Lang, why: FetchFailure) -> CoreError {
    if why == FetchFailure::Cancelled {
        return CoreError::Cancelled;
    }
    CoreError::Failed { reason: crate::fetch::failure_words(lang, why) }
}

/// `read`, for the mailbox that was open when the message was asked for: another mailbox has a
/// message of that number too, and it is not this one.
pub(crate) fn in_opening<T>(s: &Session, opening: u64, read: impl FnOnce(&Session) -> Result<T, CoreError>) -> Result<T, CoreError> {
    if s.openings() != opening {
        return Err(CoreError::Cancelled);
    }
    read(s)
}

/// What the first part of [`Core::when_whole`] found.
enum First<T> {
    Ready(T),
    /// What to wait on, and the opening of the mailbox it was asked in.
    Wait(crate::fetch::MessageWaiter, u64),
}

impl Core {
    /// `read` of message `id` once it is on the phone whole: when only its headers and preview are
    /// here, it is fetched first (as opening it does) and this waits for it. Used for what needs
    /// the message's attachments.
    pub(crate) async fn when_whole<T, F>(&self, id: u64, read: F) -> Result<T, CoreError>
    where
        T: Send + 'static,
        F: Fn(&Session) -> Result<T, CoreError> + Send + Sync + 'static,
    {
        let lang = self.lang();
        let sync = self.sync.clone();
        let read = Arc::new(read);
        let now = read.clone();
        let first = self
            .call(move |s| match sync.wait_for(s, id)? {
                None => now(s).map(First::Ready),
                Some(waiter) => Ok(First::Wait(waiter, s.openings())),
            })
            .await?;
        let (waiter, opening) = match first {
            First::Ready(found) => return Ok(found),
            First::Wait(waiter, opening) => (waiter, opening),
        };
        match waiter.next().await {
            // (Another mailbox may be open by now: its message of that number is not this one.)
            Some(Ok(())) => self.call(move |s| in_opening(s, opening, |s| read(s))).await,
            Some(Err(why)) => Err(failure_error(lang, why)),
            None => Err(CoreError::Cancelled),
        }
    }
}

#[uniffi::export]
impl Core {
    /// The owner opened message `id` (the reading pane shows it): keeping within the phone's
    /// limits leaves it alone, and when only its headers and preview are on the phone it is
    /// fetched whole, ahead of the background, one message at a time per account. `Fetching` is
    /// followed by `MessageFetched` or `FetchFailed` (with why). Nothing is fetched while there is
    /// no network or Work Offline is on (`Offline`).
    pub async fn open_message(&self, id: u64) -> Result<OpenState, CoreError> {
        let sync = self.sync.clone();
        self.call(move |s| sync.open(s, id)).await
    }

    /// Search `account`'s mailbox on its server for `query` (the words a search on the phone
    /// takes; what the server's search doesn't understand is left out), for mail older than what
    /// the phone keeps. What it finds comes onto the phone as headers in its folders, kept for the
    /// session whatever the phone's limits say; opening one fetches it whole. The answer names the
    /// messages found, newest first, those already on the phone among them.
    pub async fn search_server(&self, account: u64, query: String) -> Result<ServerSearchAnswer, CoreError> {
        if query.len() > crate::fetch::MAX_QUERY {
            return Err(CoreError::BadRequest { reason: "the search is too long".to_string() });
        }
        let sync = self.sync.clone();
        let held = query.len();
        let reply = self.call_holding(held, move |s| sync.search(s, account, Some(query))).await?;
        reply.next().await.unwrap_or(Err(CoreError::Cancelled))
    }

    /// The next page of the last `search_server` of `account`, when its answer said `more`.
    pub async fn search_server_more(&self, account: u64) -> Result<ServerSearchAnswer, CoreError> {
        let sync = self.sync.clone();
        let reply = self.call(move |s| sync.search(s, account, None)).await?;
        reply.next().await.unwrap_or(Err(CoreError::Cancelled))
    }
}
