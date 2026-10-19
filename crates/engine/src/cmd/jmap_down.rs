//! Mail down from a JMAP server (RFC 8621).
//!
//! A JMAP run reads, for each mailbox it looks at, its newest messages: a *window*
//! ([`MessagePage`], one per mailbox). A window that holds every message of its mailbox is a
//! complete listing (`afresh`): what this mailbox had there and the window doesn't is marked gone
//! at once, and removed when the run ends whole, as a Graph folder listed afresh is. A window that
//! was cut short only adds and updates.
//!
//! - **A new message** comes as its headers and preview, not fetched ([`graph_down::new_message`]).
//! - **A message this mailbox has** comes as its keywords: read, flagged, answered and forwarded
//!   take the server's value, but for one changed here since the server last said (that goes up
//!   with the run's changes). It follows the server into the mailbox the window is of, unless it
//!   was moved here since.
//! - **A message that is its marks alone and that this mailbox does not have** (the request said it did, and
//!   it was deleted here since) is passed over, not added blank.
//! - **A message the server knows by a new id** (some servers name a message by where it is, so a
//!   move gives it a new id): one listed under an id this account doesn't know is first matched to
//!   a message of the same folder that lost its id (moved here, or marked gone by this very
//!   window), by its `Message-ID`. Exactly one such message is that one; two or more, or none,
//!   and it is a new message.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_model::sync::{GraphMessage, MessagePage, ServerMessage};
use mailcraft_model::{Address, Id, Mailbox};

use super::graph_down::{self, Down, ServerIndex};

/// The longest server id taken in. RFC 8620 allows 255 octets; ids also go into requests and
/// addresses, so a longer one is refused rather than carried.
pub const MAX_ID: usize = 512;

/// Whether `id` can be a JMAP id (RFC 8620 §1.2: `A-Za-z0-9`, `-` and `_`).
pub fn plausible_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A `Message-ID` as it is compared: without blanks and angle brackets.
fn message_id(raw: &str) -> &str {
    raw.trim().trim_start_matches('<').trim_end_matches('>')
}

/// Take in one mailbox's window. A window for a mailbox this account doesn't have (yet) is
/// passed over: the next run brings it again.
pub fn take_window(mb: &mut Mailbox, index: &mut ServerIndex, account: Id, page: &MessagePage, down: &mut Down) {
    if !plausible_id(&page.folder) {
        return;
    }
    let Some(folder) = mb.folder_by_server(account, &page.folder) else { return };
    if page.afresh {
        for m in mb.messages.values_mut() {
            if m.account == account
                && m.server.as_ref().is_some_and(|s| s.folder == page.folder && !s.gone)
                && let Some(s) = Arc::make_mut(m).server.as_mut()
            {
                s.gone = true;
            }
        }
    }
    for id in page.removed.iter().filter(|id| plausible_id(id)) {
        let Some(m) = index.find(mb, account, id) else { continue };
        if let Ok(m) = mb.message_mut(m)
            && let Some(s) = m.server.as_mut().filter(|s| s.folder == page.folder && !s.gone)
        {
            s.gone = true;
            down.gone += 1;
        }
    }
    // The messages of this folder that lost their server id, by `Message-ID`: those a listed id
    // nobody knows may be.
    let mut lost: HashMap<String, Vec<Id>> = HashMap::new();
    for m in mb.messages.values() {
        let lost_here = m.account == account && m.folder == folder && m.server.as_ref().is_some_and(|s| s.gone || s.id.is_empty());
        if lost_here && !message_id(&m.message_id).is_empty() {
            lost.entry(message_id(&m.message_id).to_string()).or_default().push(m.id);
        }
    }
    for there in &page.messages {
        let id = there.server.id.as_str();
        if !plausible_id(id) {
            continue;
        }
        let here = index.find(mb, account, id).or_else(|| match lost.get_mut(message_id(&there.message_id)) {
            // One message it can be: that one, once.
            Some(candidates) if candidates.len() == 1 => candidates.pop(),
            _ => None,
        });
        match here {
            Some(m) => {
                if update(mb, account, m, folder, &page.folder, there) {
                    down.changed += 1;
                }
                index.note(mb, account, id, m);
            }
            // A message the host sent as its marks alone, because the request said this mailbox has it, and that
            // it no longer does (it was deleted here since): no message to add. It would stay blank, as every
            // later run would send it as one the mailbox has. The next run asks for it whole.
            None if marks_only(there) => {}
            None => {
                if let Ok(m) = mb.add_message(graph_down::new_message(folder, &page.folder, there)) {
                    index.note(mb, account, id, m);
                    down.new += 1;
                }
            }
        }
    }
}

/// Whether a message of a window is its marks and nothing else: no `Message-ID`, subject, date, sender,
/// recipients, preview or size. That is how a host sends a message the request said this mailbox has (a
/// message the server has whole always has a size, if nothing else).
fn marks_only(m: &GraphMessage) -> bool {
    m.server.size == 0
        && m.message_id.is_empty()
        && m.subject.is_empty()
        && m.preview.is_empty()
        && m.sent == 0
        && m.received == 0
        && m.from == Address::default()
        && m.to.is_empty()
        && m.cc.is_empty()
}

/// Bring a message this mailbox has in step with what the server says of it now: its keywords,
/// its id and its mailbox. Whether anything changed.
fn update(mb: &mut Mailbox, account: Id, id: Id, folder: Id, server_folder: &str, there: &GraphMessage) -> bool {
    let Some(m) = mb.message(id) else { return false };
    // Listed again by a complete window, a message is no longer gone: that alone is no change.
    let mut before = m.clone();
    if let Some(s) = before.server.as_mut() {
        s.gone = false;
    }
    let last = m.server.clone().unwrap_or_default();
    // Where the server last had it, as this mailbox has that folder: a message still there here
    // follows the server; one moved here since stays where it was put (that move goes up).
    let was_here = mb.folder_by_server(account, &last.folder);
    let follows = last.gone || last.id.is_empty() || was_here.is_none_or(|was| m.folder == was);
    let Ok(m) = mb.message_mut(id) else { return false };
    let now = &there.server;
    if m.read == last.read {
        m.read = now.read;
    }
    // JMAP has `$flagged` and nothing more: a follow-up flag marked complete here stays so until
    // the server's changes, and a due date is kept while the message stays flagged.
    if m.flag == last.flag {
        m.flag = now.flag;
    }
    if m.answered == last.answered {
        m.answered = now.answered;
    }
    if m.forwarded == last.forwarded {
        m.forwarded = now.forwarded;
    }
    if follows {
        m.folder = folder;
    }
    m.server = Some(ServerMessage {
        id: now.id.clone(),
        folder: server_folder.to_string(),
        read: now.read,
        flag: now.flag,
        answered: now.answered,
        forwarded: now.forwarded,
        gone: false,
        // Its size and the rest are what was learnt of it before, unless the window says.
        size: if now.size > 0 { now.size } else { last.size },
        ..last
    });
    *m != before
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_what_rfc_8620_allows_and_not_too_long() {
        for id in ["a", "Mabc_-09", "E1V2Zx", &"x".repeat(MAX_ID)] {
            assert!(plausible_id(id), "{id:?}");
        }
        for id in ["", " a", "a b", "a/b", "a.b", "é", "a\n", &"x".repeat(MAX_ID + 1)] {
            assert!(!plausible_id(id), "{id:?}");
        }
    }

    #[test]
    fn a_message_id_is_compared_without_brackets() {
        assert_eq!(message_id(" <abc@example.org> "), "abc@example.org");
        assert_eq!(message_id("abc@example.org"), "abc@example.org");
        assert_eq!(message_id("<>"), "");
    }
}
