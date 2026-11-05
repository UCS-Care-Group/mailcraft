// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of an IMAP account fetched whole by their UID (#653): mail older than the account's
//! mail to keep offline is kept here as its headers and a preview, and is asked for again when it
//! is opened, or in the background once a longer window covers it.
//!
//! - **Only read.** Each mailbox is opened with EXAMINE, and a message is asked for with
//!   `UID FETCH … BODY.PEEK[]`, which never sets `\Seen`: nothing on the server changes.
//! - **By mailbox, `UIDVALIDITY` and UID** ([`ImapFetchKey`]), never by Message-ID: Gmail keeps a
//!   copy of a message under each of its labels. A mailbox numbered anew since is not asked for
//!   its UIDs, which are other messages' now: they are said to be refused, and the next
//!   Send/Receive reads the mailbox afresh.
//! - **Bounded**: a message larger than a synchronisation takes whole is refused (it stays as it
//!   is here); nothing is downloaded whole while the server doesn't say how large it is, and is
//!   asked for again; and one request brings at most so many bytes; what it didn't get to is
//!   asked for again.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ImapFetchKey, ImapFetchRequest};
use mailcraft_proto::imap::uid_sets;

use crate::sync::{MAX_MESSAGE, open_imap_with, shown};
use crate::transport::Patience;
use crate::{NetError, kinds, scrub};

/// Most messages one request asks for.
const MOST: usize = 100;
/// Most bytes of messages one request brings; the rest waits for the next.
const MOST_BYTES: usize = 64 * 1024 * 1024;

/// Fetch the messages `request` names, whole, signing in with `secret` (the password, or the
/// access token of an account that signs in with OAuth). Never fails as a whole: what couldn't
/// be had is said in the answer, and a server that couldn't be reached makes it
/// [`offline`](GraphFetchResult::offline). Setting `cancel` stops it before the next message.
pub fn fetch_imap(request: &ImapFetchRequest, secret: &str, cancel: &AtomicBool) -> GraphFetchResult {
    let mut out = GraphFetchResult { account: request.account.id, email: request.account.email.clone(), ..Default::default() };
    fetch_into(request, secret, cancel, &mut out);
    for line in &mut out.errors {
        *line = scrub(line, secret);
    }
    out
}

fn fetch_into(request: &ImapFetchRequest, secret: &str, cancel: &AtomicBool, out: &mut GraphFetchResult) {
    // What is asked for, by mailbox, in the order asked; a name that isn't a key is nobody's.
    let mut by_mailbox: Vec<(String, Vec<ImapFetchKey>)> = Vec::new();
    for id in request.ids.iter().take(MOST) {
        let Some(key) = ImapFetchKey::read(id) else {
            out.failed.push(id.clone());
            continue;
        };
        match by_mailbox.iter_mut().find(|(remote, _)| *remote == key.remote) {
            Some((_, keys)) => keys.push(key),
            None => by_mailbox.push((key.remote.clone(), vec![key])),
        }
    }
    if by_mailbox.is_empty() || cancel.load(Ordering::Relaxed) {
        return;
    }
    let mut imap = match open_imap_with(&request.account, secret, &Patience::default(), Some(cancel)) {
        Ok(imap) => imap,
        Err(e) => {
            out.offline = e.is_unreachable();
            out.errors.push(format!("Couldn't download the message: {}", e.without(secret)));
            return;
        }
    };
    let mailboxes = match imap.list() {
        Ok(list) => list,
        Err(e) => {
            out.errors.push(format!("Couldn't read the list of folders: {}", NetError::from(e)));
            return;
        }
    };
    let mut bytes = 0usize;
    'mailboxes: for (remote, keys) in by_mailbox {
        let Some(mailbox) = mailboxes.iter().find(|m| kinds::selectable(m) && kinds::remote_name(m) == remote) else {
            // Gone from the server: so are its messages, as the next Send/Receive will say.
            out.missing.extend(keys.iter().map(ImapFetchKey::write));
            continue;
        };
        let selected = match imap.select(&mailbox.raw, true) {
            Ok(selected) => selected,
            Err(e) => {
                let e = NetError::from(e);
                out.errors.push(format!("Couldn't open \"{}\": {e}", shown(&remote)));
                if e.is_fatal() {
                    out.offline = e.is_unreachable();
                    break;
                }
                out.failed.extend(keys.iter().map(ImapFetchKey::write));
                continue;
            }
        };
        // Numbered anew: these UIDs are not the messages that were asked for.
        let (renumbered, same): (Vec<ImapFetchKey>, Vec<ImapFetchKey>) =
            keys.into_iter().partition(|k| k.uid_validity != 0 && selected.uid_validity != 0 && k.uid_validity != selected.uid_validity);
        out.failed.extend(renumbered.iter().map(ImapFetchKey::write));
        let uids: Vec<u32> = same.iter().map(|k| k.uid).collect();
        let sizes = match sizes(&mut imap, &uids) {
            Ok(sizes) => sizes,
            // Not refused: nothing is known of these messages, and they are asked for again.
            Err(e) => {
                let e = NetError::from(e);
                out.errors.push(format!("Couldn't read the sizes of the messages in \"{}\": {e}", shown(&remote)));
                if e.is_fatal() {
                    out.offline = e.is_unreachable();
                    break;
                }
                continue;
            }
        };
        for key in same {
            if cancel.load(Ordering::Relaxed) {
                break 'mailboxes;
            }
            let size = match sizes.get(&key.uid) {
                Some(Some(size)) => *size,
                // Asked, and not there: the server says the size of every message it has.
                Some(None) => {
                    out.missing.push(key.write());
                    continue;
                }
                // Not in an answer that names it (it can't be, but if it were): no size within
                // the bounds, refused as one too large is, and asked for again when it is next
                // opened.
                None => {
                    out.failed.push(key.write());
                    continue;
                }
            };
            if size > MAX_MESSAGE {
                out.failed.push(key.write());
                continue;
            }
            // One that would take the request past what it brings waits for the next.
            if bytes > 0 && bytes.saturating_add(usize::try_from(size).unwrap_or(usize::MAX)) > MOST_BYTES {
                break 'mailboxes;
            }
            match imap.uid_fetch_message(key.uid) {
                Ok(Some(fetched)) => {
                    let mime = fetched.body.unwrap_or_default();
                    bytes = bytes.saturating_add(mime.len());
                    out.fetched.push(GraphFetched { id: key.write(), mime });
                }
                Ok(None) => out.missing.push(key.write()),
                Err(e) => {
                    let e = NetError::from(e);
                    out.errors.push(format!("Couldn't download a message from \"{}\": {e}", shown(&remote)));
                    if e.is_fatal() {
                        out.offline = e.is_unreachable();
                        break 'mailboxes;
                    }
                    out.failed.push(key.write());
                }
            }
        }
    }
    imap.get_mut().leave();
    let _ = imap.logout();
}

#[cfg(test)]
#[path = "imap_fetch_tests.rs"]
mod tests;

/// The sizes of `uids` in the open mailbox, as the server says them: `Some(None)` for one it was
/// asked for and has not (gone). An error when the server wouldn't say them, or the connection
/// broke while it did.
fn sizes(
    imap: &mut mailcraft_proto::imap::Imap<crate::transport::Stream<'_>>,
    uids: &[u32],
) -> Result<BTreeMap<u32, Option<u64>>, mailcraft_proto::ProtoError> {
    let mut out = BTreeMap::new();
    for (set, named) in uid_sets(uids) {
        let sizes = imap.uid_fetch_sizes(&set)?;
        out.extend(named.into_iter().map(|uid| (uid, None)));
        out.extend(sizes.into_iter().map(|(uid, size)| (uid, Some(size))));
    }
    Ok(out)
}
