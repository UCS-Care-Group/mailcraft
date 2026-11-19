// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server for an Exchange server or a JMAP account (#1725): what the host is
//! asked for a page [`super::search_server::requests`] named.
//!
//! The page says which mailbox, which folder and which search; a host that has to connect needs
//! to know where the server is, and each protocol searches in its own way:
//!
//! - **Exchange Web Services** searches a folder at a time, so the request names the folders
//!   ([`ews`]): the one in view, the folder in view and what is under it, or every folder of the
//!   account but Deleted Items, Junk E-mail and what is under them, the Inbox first.
//! - **JMAP** searches the account, so the request names what stays out of it ([`jmap`]: Deleted
//!   Items and Junk E-mail, for a search of the mailbox or of everything) and the mailboxes this
//!   computer has, by which a found email is put in its place.
//!
//! Both are pure: nothing is sent from here, and the secret that opens the server is the
//! window's to add.

use std::collections::HashSet;

use mailcraft_model::sync::{EwsSearchRequest, GraphSearchRequest, JmapSearchRequest, MAX_SEARCH_FOLDERS, SearchPlace};
use mailcraft_model::{Folder, FolderKind, Id, Mailbox};

use super::ews_mail::ews_account;
use super::jmap_mail::jmap_account;
use crate::{CmdError, Scope, Session};

/// The folders of `account` whose mail stays out of a search of the mailbox or of everything:
/// Deleted Items, Junk E-mail and what is under them.
fn left_out(mb: &Mailbox, account: Id) -> HashSet<Id> {
    mb.folders
        .iter()
        .filter(|f| f.account == account && matches!(f.kind, FolderKind::Deleted | FolderKind::Junk))
        .flat_map(|f| mb.subtree(f.id))
        .collect()
}

/// The folders of `account` its server knows, with the id it knows each by.
fn on_server(mb: &Mailbox, account: Id) -> impl Iterator<Item = (&Folder, &str)> {
    mb.folders.iter().filter(move |f| f.account == account).filter_map(|f| f.server.as_ref().map(|server| (f, server.id.as_str())))
}

/// What the host of an Exchange server account is asked for `page`.
pub fn ews(s: &Session, page: &GraphSearchRequest) -> Result<EwsSearchRequest, CmdError> {
    let mb = &s.mailbox;
    let (a, server) = ews_account(mb, page.account)?;
    let folders: Vec<String> = if page.folder.is_empty() {
        // A search of a folder and what is under it asks those; one of the mailbox, or of
        // everything, every folder but the ones whose mail stays out of it. (The Outbox is
        // MailCraft's own, and has no id on the server.)
        let under: Option<HashSet<Id>> = (s.view.scope == Scope::Subfolders).then(|| mb.subtree(s.view.folder).into_iter().collect());
        let out = if under.is_some() { HashSet::new() } else { left_out(mb, a.id) };
        let mut asked: Vec<(&Folder, &str)> =
            on_server(mb, a.id).filter(|(f, _)| under.as_ref().is_none_or(|ids| ids.contains(&f.id)) && !out.contains(&f.id)).collect();
        // The Inbox first: what the owner looks for is there more often than anywhere else.
        asked.sort_by_key(|(f, _)| f.kind != FolderKind::Inbox);
        asked.into_iter().map(|(_, id)| id.to_owned()).take(MAX_SEARCH_FOLDERS).collect()
    } else {
        vec![page.folder.clone()]
    };
    Ok(EwsSearchRequest {
        account: a.id,
        email: a.email.clone(),
        url: server.url.clone(),
        username: server.username.clone(),
        trusted: server.trusted.clone(),
        ticket: page.ticket,
        folders,
        query: page.query.clone(),
        next: page.next.clone(),
    })
}

/// What the host of a JMAP account is asked for `page`.
pub fn jmap(s: &Session, page: &GraphSearchRequest) -> Result<JmapSearchRequest, CmdError> {
    let mb = &s.mailbox;
    let (a, server) = jmap_account(mb, page.account)?;
    let outside = match s.view.scope {
        Scope::Mailbox | Scope::Everywhere if page.folder.is_empty() => {
            let out = left_out(mb, a.id);
            on_server(mb, a.id).filter(|(f, _)| out.contains(&f.id)).map(|(_, id)| id.to_owned()).take(MAX_SEARCH_FOLDERS).collect()
        }
        _ => Vec::new(),
    };
    let places = on_server(mb, a.id).map(|(f, id)| SearchPlace { id: id.to_owned(), kind: f.kind }).take(MAX_SEARCH_FOLDERS).collect();
    Ok(JmapSearchRequest {
        account: a.id,
        email: a.email.clone(),
        session: server.session.clone(),
        bearer: server.bearer,
        ticket: page.ticket,
        folder: page.folder.clone(),
        outside,
        places,
        query: page.query.clone(),
        next: page.next.clone(),
    })
}

#[cfg(test)]
#[path = "search_requests_tests.rs"]
mod tests;
