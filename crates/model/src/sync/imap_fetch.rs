// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Messages of an IMAP account fetched whole by their UID (#653): what mail to keep offline left
//! as its headers and preview here, when it is opened, replied to or forwarded, and what a
//! longer window covers again, in the background. A message is named by its server mailbox, that
//! mailbox's `UIDVALIDITY` and its UID there ([`ImapFetchKey`]): never by its Message-ID, which
//! the copies Gmail keeps under each label and in All Mail share. What comes back is a
//! [`super::GraphFetchResult`], each message under its key.

use serde::{Deserialize, Serialize};

use crate::{Account, Id};

/// Which messages of an IMAP account a host should fetch whole. The secret the server is shown
/// (the password, or an OAuth access token) is not in it: the host hands it over beside it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImapFetchRequest {
    /// The account, for its incoming server and the name it signs in with.
    pub account: Account,
    /// The messages, as [`ImapFetchKey::write`] writes them, in the order they are wanted.
    pub ids: Vec<String>,
    /// Nobody is waiting: it comes after what the owner asked for.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub background: bool,
}

impl ImapFetchRequest {
    /// The account's id.
    pub fn id(&self) -> Id {
        self.account.id
    }
}

/// One message of an IMAP account, as the server knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ImapFetchKey {
    /// The path of its server mailbox ([`crate::Folder::remote`]).
    pub remote: String,
    /// That mailbox's `UIDVALIDITY` when the UID was given: under another one, the UID is another
    /// message's, or nobody's.
    pub uid_validity: u32,
    pub uid: u32,
}

impl ImapFetchKey {
    /// `uidvalidity/uid/remote`: the path last, for it may hold a `/`.
    pub fn write(&self) -> String {
        format!("{}/{}/{}", self.uid_validity, self.uid, self.remote)
    }

    /// What [`ImapFetchKey::write`] wrote, or `None` for anything else (no UID, no path).
    pub fn read(text: &str) -> Option<ImapFetchKey> {
        let mut parts = text.splitn(3, '/');
        let uid_validity = parts.next()?.parse().ok()?;
        let uid: u32 = parts.next()?.parse().ok()?;
        let remote = parts.next()?;
        (uid != 0 && !remote.is_empty()).then(|| ImapFetchKey { remote: remote.to_string(), uid_validity, uid })
    }
}
