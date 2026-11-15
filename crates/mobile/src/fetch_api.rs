// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the apps call to have mail fetched (MB3.17): a message the owner opens, one of its
//! attachments, and a search on the server. The work is `fetch.rs`'s; these only hand it over and
//! wait. Message text, attachments and what a search finds are the sender's, hostile: nothing of
//! them runs here, and an agent that reads them is told they are data, never instructions.

use mailcraft_engine::opening::{self, Verdict};
use mailcraft_engine::{Part, Session};
use mailcraft_model::Id;

use crate::fetch::{FetchFailure, OpenState, ServerSearchAnswer};
use crate::text;
use crate::{Core, CoreError};

/// The largest attachment handed over.
pub(crate) const MAX_ATTACHMENT_BYTES: usize = 64 * 1024 * 1024;

/// What may be done with an attachment, by its kind (the desktop's rules, `mail.openAttachment`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AttachmentVerdict {
    /// Open it with the phone's own app for it.
    Open,
    /// An Office file that can carry macros: ask the owner first.
    AskMacros,
    /// A web page or a drawing that can carry scripts: ask the owner first.
    AskScripts,
    /// A program, a script, a shortcut, an installer, a disk image or a file of no known kind:
    /// never opened from mail; it may only be saved.
    SaveOnly,
}

/// One attachment's file.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentFile {
    pub rev: u64,
    /// The name to write it under: the sender's, made safe (no folders, nothing hidden, not too
    /// long).
    pub name: String,
    /// What the message says it is (a hint only; the verdict goes by the name).
    pub mime: String,
    pub verdict: AttachmentVerdict,
    pub bytes: Vec<u8>,
}

fn verdict_of(name: &str) -> AttachmentVerdict {
    match opening::verdict(name) {
        Verdict::Open => AttachmentVerdict::Open,
        Verdict::Macros => AttachmentVerdict::AskMacros,
        Verdict::Scripts => AttachmentVerdict::AskScripts,
        Verdict::Blocked => AttachmentVerdict::SaveOnly,
    }
}

fn words(text: &'static str) -> &'static str {
    text
}

/// Attachment `index` of message `id`, which is on the phone whole.
fn attachment_file(s: &Session, id: Id, index: u32) -> Result<AttachmentFile, CoreError> {
    let lang = s.interface_language;
    let m = s
        .loaded_message_where(id, |_| false)
        .map_err(|_| CoreError::Failed { reason: lang.t(words("the message could not be read")).to_string() })?;
    let at = usize::try_from(index).map_err(|_| CoreError::BadRequest { reason: "the message has no such attachment".to_string() })?;
    let a = m.attachments.get(at).ok_or_else(|| CoreError::BadRequest { reason: "the message has no such attachment".to_string() })?;
    if a.size() > MAX_ATTACHMENT_BYTES {
        return Err(CoreError::Failed { reason: lang.t(words("The attachment is too large to open on the phone.")).to_string() });
    }
    let bytes = s
        .content(id, Part::Attachment(at))
        .map_err(|_| CoreError::Failed { reason: lang.t(words("the attachment could not be read")).to_string() })?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(CoreError::Failed { reason: lang.t(words("The attachment is too large to open on the phone.")).to_string() });
    }
    Ok(AttachmentFile {
        rev: s.rev(),
        name: opening::file_name(&a.name),
        mime: text::clip(&a.mime, text::MAX_NAME),
        verdict: verdict_of(&a.name),
        bytes: bytes.to_vec(),
    })
}

/// What a message that couldn't come is said as, to a call that waited for it.
fn failure_error(lang: mailcraft_i18n::Lang, why: FetchFailure) -> CoreError {
    if why == FetchFailure::Cancelled {
        return CoreError::Cancelled;
    }
    CoreError::Failed { reason: crate::fetch::failure_words(lang, why) }
}

/// [`attachment_file`], for the mailbox that was open when the message was asked for: another
/// mailbox has a message of that number too, and it is not this one.
pub(crate) fn attachment_in(s: &Session, id: Id, index: u32, opening: u64) -> Result<AttachmentFile, CoreError> {
    if s.openings() != opening {
        return Err(CoreError::Cancelled);
    }
    attachment_file(s, id, index)
}

/// What the first part of `fetch_attachment` found.
enum First {
    Ready(AttachmentFile),
    /// What to wait on, and the opening of the mailbox it was asked in.
    Wait(crate::fetch::MessageWaiter, u64),
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

    /// The bytes of attachment `index` of message `id` (its index in `MessageView::attachments`),
    /// with the name to write it under and what may be done with it: programs, scripts and files
    /// of no known kind are never to be opened, only saved. When the message isn't on the phone
    /// whole, it is fetched first, and this waits for it. Attachments over 64 MB aren't handed over.
    pub async fn fetch_attachment(&self, id: u64, index: u32) -> Result<AttachmentFile, CoreError> {
        let lang = self.lang();
        let sync = self.sync.clone();
        let first = self
            .call(move |s| match sync.wait_for(s, id)? {
                None => attachment_file(s, id, index).map(First::Ready),
                Some(waiter) => Ok(First::Wait(waiter, s.openings())),
            })
            .await?;
        let (waiter, opening) = match first {
            First::Ready(file) => return Ok(file),
            First::Wait(waiter, opening) => (waiter, opening),
        };
        match waiter.next().await {
            // (Another mailbox may be open by now: its message of that number is not this one.)
            Some(Ok(())) => self.call(move |s| attachment_in(s, id, index, opening)).await,
            Some(Err(why)) => Err(failure_error(lang, why)),
            None => Err(CoreError::Cancelled),
        }
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
