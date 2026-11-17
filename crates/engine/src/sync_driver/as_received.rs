// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save Message As writes a message as it was received (#1561), with no I/O: which bytes those
//! are, when its server is to be asked for them first, and what the answer brought.
//!
//! The mailbox keeps the bytes a message came as only for one that is Digitally signed or
//! Encrypted (#30, [`Message::original`]); of every other message it keeps what was read of them
//! (its text, files and header lines), and a message written from that is not the one that came.
//! So the bytes are, in this order:
//! - the bytes it came as, when the mailbox keeps them;
//! - else, for a message of a Microsoft, Exchange server or JMAP account that its server has, the
//!   server's own copy, asked for alone for the save: the window hands the host the fetch a message
//!   is fetched whole with (Graph's `$value`, `GetItem` with its MIME content, the message's blob),
//!   and what comes back for the save is held here until the save writes it, and no longer. It is
//!   never kept with the message, nor written anywhere else.
//!
//! Anything else (a draft, a message a part was removed from here, mail no server has, an IMAP
//! account's, whose server isn't asked yet, or a copy the server didn't send) is written from what
//! this computer keeps, and the save says so ([`Rebuilt`]): never as if it were the message that
//! came. A message only part of which is here is never saved at all ([`partial`]).

use mailcraft_model::sync::GraphFetchResult;
use mailcraft_model::{AccountKind, Id, Message};

use crate::cmd::graph_down;
use crate::{CmdError, Session};

/// What came of a server's copy asked for for a save.
#[derive(Clone, PartialEq, Eq)]
pub enum ServerCopy {
    /// Asked for; the answer hasn't come.
    Asked,
    /// The message as its server holds it.
    Came(Vec<u8>),
    /// The server answered without it (gone, refused, too large, unreachable).
    NotSent,
}

/// A server's copy of a message asked for for a save, under one opening of the mailbox.
#[derive(Clone, PartialEq, Eq)]
pub struct ForSave {
    pub message: Id,
    /// [`Session::openings`] when it was asked: in another mailbox it is nobody's.
    pub opening: u64,
    pub copy: ServerCopy,
}

impl std::fmt::Debug for ForSave {
    // Never the message's bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let copy = match &self.copy {
            ServerCopy::Asked => "asked".to_string(),
            ServerCopy::Came(bytes) => format!("{} bytes", bytes.len()),
            ServerCopy::NotSent => "not sent".to_string(),
        };
        write!(f, "ForSave({}, opening {}, {copy})", self.message, self.opening)
    }
}

/// Why a saved message was written from what this computer keeps, not as it was received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebuilt {
    /// A draft: it was never received.
    Draft,
    /// A part of it was removed here, so the bytes it came as aren't this message any more.
    PartsRemoved,
    /// No server this computer can ask has it (mail on this computer, imported, gone from its
    /// server).
    NoServer,
    /// Its server wasn't asked: an IMAP account's (not yet), or the caller doesn't ask first (an
    /// agent, the command line).
    NotAsked,
    /// Its server was asked and didn't send it.
    NotSent,
}

impl Rebuilt {
    /// As a save's answer says it (`why`).
    pub fn code(self) -> &'static str {
        match self {
            Rebuilt::Draft => "draft",
            Rebuilt::PartsRemoved => "partsRemoved",
            Rebuilt::NoServer => "noServer",
            Rebuilt::NotAsked => "notAsked",
            Rebuilt::NotSent => "notSent",
        }
    }
}

/// Whether the host asks the server of an account of `kind` for a message as it holds it.
fn server_gives_copy(kind: Option<AccountKind>) -> bool {
    matches!(kind, Some(AccountKind::Microsoft | AccountKind::Exchange | AccountKind::Jmap))
}

/// Whether the bytes `m` came as are what a save writes: kept here, and nothing taken out of it.
fn keeps_original(m: &Message) -> bool {
    m.has_original() && !m.parts_removed
}

/// Whether `m`, of an account of `kind`, is one a save writes as its server holds it, asked for
/// first: it keeps no bytes it came as, isn't a draft and had nothing taken out here, and its
/// account's server gives a message's copy and has this one.
fn server_has_copy(kind: Option<AccountKind>, m: &Message) -> bool {
    server_gives_copy(kind)
        && !keeps_original(m)
        && !m.draft
        && m.server_draft.is_none()
        && !m.parts_removed
        && !m.incomplete
        && m.moving_out.is_none()
        && m.server.as_ref().is_some_and(|s| !s.gone && !s.id.is_empty())
}

/// Whether Save Message As of `m` waits for its server's copy first: it is one the server is asked
/// for ([`server_has_copy`]), and no answer for it is held under this opening of the mailbox.
pub fn to_ask(s: &Session, m: &Message) -> bool {
    let answered = s.driver.fetch.for_save.as_ref().is_some_and(|f| f.message == m.id && f.opening == s.openings() && f.copy != ServerCopy::Asked);
    !answered && server_has_copy(s.mailbox.account(m.account).map(|a| a.kind), m)
}

impl Session {
    /// Save Message As of `message` waits for its server's copy: the fetch for it asks for the
    /// message whatever else it would (whole already), and the answer is held for the save.
    pub(crate) fn ask_for_save(&mut self, message: Id) {
        self.driver.fetch.for_save = Some(ForSave { message, opening: self.openings(), copy: ServerCopy::Asked });
    }

    /// Whether a fetch of `message` is asked for a save ([`Session::ask_for_save`]), so that its
    /// request names it though it is whole already.
    pub fn asked_for_save(&self, message: Id) -> bool {
        self.driver.fetch.for_save.as_ref().is_some_and(|f| f.message == message && f.opening == self.openings() && f.copy == ServerCopy::Asked)
    }

    /// The copy for the save out of the answer to a fetch of `messages`, before the answer is taken
    /// in: what came for the message the save waits for, taken out of the answer when nothing else
    /// takes it in (it is whole here already), else copied. `None` when the answer isn't to a fetch
    /// of it; an answer that is and doesn't bring it says it didn't come, so that the save goes on
    /// rather than asking again.
    pub(crate) fn copy_for_save(&self, messages: &[Id], result: &mut GraphFetchResult) -> Option<ServerCopy> {
        let message = self.driver.fetch.for_save.as_ref().filter(|f| f.copy == ServerCopy::Asked && f.opening == self.openings())?.message;
        if !messages.contains(&message) {
            return None;
        }
        let Some(m) = self.mailbox.message(message).filter(|m| m.account == result.account) else { return Some(ServerCopy::NotSent) };
        let Some(on_server) = m.server.as_ref().map(|s| s.id.as_str()) else { return Some(ServerCopy::NotSent) };
        let Some(at) = result.fetched.iter().position(|f| f.id == on_server) else { return Some(ServerCopy::NotSent) };
        let taken_in = graph_down::still_to_fetch(&self.mailbox, m) || graph_down::wants_from_source(m);
        let bytes = if taken_in { result.fetched.get(at).map(|f| f.mime.clone()).unwrap_or_default() } else { result.fetched.remove(at).mime };
        Some(if bytes.is_empty() { ServerCopy::NotSent } else { ServerCopy::Came(bytes) })
    }

    /// What came for the save, once the answer it came in was taken in (`taken`: it wasn't
    /// refused, as an answer for another account or server is).
    pub(crate) fn answered_for_save(&mut self, copy: ServerCopy, taken: bool) {
        if let Some(f) = self.driver.fetch.for_save.as_mut() {
            f.copy = if taken { copy } else { ServerCopy::NotSent };
        }
    }

    /// The answer held for a save of `message`, given up to it: the server's copy, or that it
    /// didn't come. `None` when nothing was asked for it under this opening.
    pub(crate) fn take_for_save(&mut self, message: Id) -> Option<ServerCopy> {
        let opening = self.openings();
        let held = self.driver.fetch.for_save.take_if(|f| f.message == message && f.opening == opening && f.copy != ServerCopy::Asked);
        held.map(|f| f.copy)
    }

    /// A save that waited for its server's copy was called off (its dialog cancelled): the copy
    /// isn't held any longer.
    pub fn forget_for_save(&mut self) {
        self.driver.fetch.for_save = None;
    }
}

/// Why `m`, of an account of `kind`, is written from what this computer keeps rather than as it
/// was received; `copy`: what its server sent for the save, if it was asked.
pub fn rebuilt_why(kind: Option<AccountKind>, m: &Message, copy: Option<&ServerCopy>) -> Rebuilt {
    if m.draft || m.server_draft.is_some() {
        Rebuilt::Draft
    } else if m.parts_removed {
        Rebuilt::PartsRemoved
    } else if copy.is_some() {
        Rebuilt::NotSent
    } else if server_has_copy(kind, m) || (kind == Some(AccountKind::Imap) && m.uid != 0) {
        Rebuilt::NotAsked
    } else {
        Rebuilt::NoServer
    }
}

/// Why `m` can't be saved at all: only a part of it is here, and a file of that part would pass
/// for the message. Judged on what the mailbox says of it, before anything is read in.
pub fn partial(m: &Message) -> Option<CmdError> {
    let subject = crate::cmd::compose::subject_of(m);
    if m.incomplete {
        return Some(failed!("only the headers of “{subject}” are here, so it can't be saved as a message", subject));
    }
    if m.unfetched {
        return Some(failed!("“{subject}” isn't downloaded yet: open it once you're online, then save it", subject));
    }
    if m.not_downloaded > 0 {
        return Some(failed!("“{subject}” was too large to download: only its header is here, so it can't be saved as a message", subject));
    }
    if m.kept.as_deref().is_some_and(|k| k.failed) {
        return Some(failed!("the text of “{subject}” can't be read on this computer, so it can't be saved as a message", subject));
    }
    None
}
