// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public folders of an Exchange server account's organisation (#1263): the calendars and
//! address books in them, listed for the owner to pick from, and the ones picked, read.
//!
//! Both go with the owner's own sign-in to the account's own server, as the rest of a run does:
//! the server answers only what the owner may see. Nothing is written: a public calendar or
//! address book is read-only here, whatever the server would let the owner do.
//!
//! **The listing** walks the tree from `publicfoldersroot` one level at a time (`FindFolder`,
//! `Shallow`: a server need not allow a deep search of the public folders), within caps on the
//! folders looked at, the depth and the requests, and keeps the calendars and the address books
//! with the names of the folders they lie in. It is made only when the owner asks for it.
//!
//! **A folder picked** is read as one of the account's own is: its events in the stretch asked
//! for, or its contacts whole, beside the account's own, so that it is kept and removed with them.
//!
//! The requests carry no routing headers (`X-AnchorMailbox`, `X-PublicFolderMailbox`): an
//! organisation's own Exchange Server sends a request on to the public folder mailbox that has
//! the folder, and Exchange Online, which wants them, is never reached through Exchange Web
//! Services with a password.

use std::collections::VecDeque;

use mailcraft_model::remote::{EventsDownload, RemoteAddressBook, RemoteCalendar};
use mailcraft_model::sync::{EwsDirectoryRequest, EwsDirectoryResult};
use mailcraft_model::{MAX_PUBLIC_FOLDERS, MAX_PUBLIC_LISTING, MAX_PUBLIC_NAME, PublicFolder, PublicFolderKind, Rights};

use crate::directory::{self, Item, Kind};
use crate::error::ResponseCode;
use crate::folder::{DistinguishedFolder, FindFolder, FolderId, FolderInfo};
use crate::http::HttpExchange;
use crate::run::{Asked, Run};

/// Most folders a listing looks at, how deep it goes, and how many requests it makes.
const MAX_LOOKED_AT: usize = 2_000;
const MAX_DEPTH: usize = 8;
const MAX_REQUESTS: usize = 200;
/// Folders asked for in one page.
const PAGE: u32 = 200;
/// What a calendar or an address book is called whose folder has no name.
const UNNAMED: &str = "Public folder";
/// The group public calendars are listed in.
const GROUP: &str = "Public Folders";
/// Most contacts a run takes in, as the rest of a directory run does.
const MAX_CONTACTS: usize = mailcraft_model::remote::MAX_CONTACTS;
/// How many pages of one folder's items are followed.
const MAX_PAGES: usize = 200;

/// What a folder of `class` is, when it is one a listing keeps.
pub(crate) fn kind_of(class: &str) -> Option<PublicFolderKind> {
    let class = class.trim();
    let is = |base: &str| {
        class.len() >= base.len()
            && class.get(..base.len()).is_some_and(|start| start.eq_ignore_ascii_case(base))
            && class.get(base.len()..).is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    };
    if is("IPF.Appointment") {
        Some(PublicFolderKind::Calendar)
    } else if is("IPF.Contact") {
        Some(PublicFolderKind::Contacts)
    } else {
        None
    }
}

/// A folder's name with the names of the folders it lies in: one line, capped.
pub(crate) fn path_name(path: &str, name: &str) -> String {
    let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let name = name.trim();
    let name = if name.is_empty() { UNNAMED } else { name };
    let whole = if path.is_empty() { name.to_owned() } else { format!("{path} / {name}") };
    whole.chars().take(MAX_PUBLIC_NAME).collect::<String>().trim_end().to_owned()
}

impl<E: HttpExchange> Run<'_, E> {
    /// The public folders' part of a directory run: the ones picked are read, then the listing is
    /// made when it was asked for.
    pub(crate) async fn public_folders(&mut self, request: &EwsDirectoryRequest, out: &mut EwsDirectoryResult) -> Result<(), Asked> {
        self.read_public(request, out).await?;
        if request.browse_public {
            out.public_browsed = true;
            match self.list_public().await {
                Ok(listing) => out.public_listing = Some(listing),
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => out.errors.push("Couldn't list the public folders: the server may have none, or not let this account see them".to_owned()),
            }
        }
        Ok(())
    }

    /// The calendars and address books in the public folders, as far as the caps go.
    pub(crate) async fn list_public(&mut self) -> Result<Vec<PublicFolder>, Asked> {
        let mut found: Vec<PublicFolder> = Vec::new();
        let mut queue: VecDeque<(Option<FolderId>, String, usize)> = VecDeque::from([(None, String::new(), 0)]);
        let (mut looked_at, mut requests) = (0usize, 0usize);
        while let Some((parent, path, depth)) = queue.pop_front() {
            let mut offset = 0u32;
            loop {
                if requests >= MAX_REQUESTS {
                    return Ok(found);
                }
                requests = requests.saturating_add(1);
                let find = match &parent {
                    None => FindFolder::new(DistinguishedFolder::PublicFoldersRoot, false, offset, PAGE),
                    Some(id) => FindFolder::below(id, offset, PAGE),
                };
                let page = match self.ask(&find.element(), &|reply| find.answer(reply)).await {
                    Ok(page) => page,
                    // (A folder below the top that the owner may not look into is passed over.)
                    Err(Asked::Code(ResponseCode::AccessDenied | ResponseCode::FolderNotFound)) if parent.is_some() => break,
                    Err(other) => return Err(other),
                };
                let before = page.folders.len();
                for folder in page.folders {
                    looked_at = looked_at.saturating_add(1);
                    let name = path_name(&path, &folder.display_name);
                    if let Some(kind) = kind_of(&folder.class)
                        && found.len() < MAX_PUBLIC_LISTING
                        && !found.iter().any(|have| have.id == folder.id.as_str())
                    {
                        found.push(PublicFolder { id: folder.id.as_str().to_owned(), name: name.clone(), kind });
                    }
                    // (A server that doesn't say how many folders lie in one is asked.)
                    let has_more = folder.child_folder_count.is_none_or(|count| count > 0);
                    if has_more && depth.saturating_add(1) < MAX_DEPTH && looked_at < MAX_LOOKED_AT {
                        queue.push_back((Some(folder.id), name, depth.saturating_add(1)));
                    }
                }
                if page.last || before == 0 || looked_at >= MAX_LOOKED_AT {
                    break;
                }
                offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
            }
        }
        Ok(found)
    }

    /// The public calendars and address books the owner picked, beside the account's own: one
    /// whose own list couldn't be read this time waits for the next run, as there is no list to
    /// keep it in.
    async fn read_public(&mut self, request: &EwsDirectoryRequest, out: &mut EwsDirectoryResult) -> Result<(), Asked> {
        for picked in request.public.iter().take(MAX_PUBLIC_FOLDERS) {
            let Some(id) = FolderId::new(&picked.id) else { continue };
            let name = path_name("", &picked.name);
            let folder = FolderInfo {
                id,
                change_key: None,
                display_name: name.clone(),
                total_count: None,
                unread_count: None,
                child_folder_count: None,
                class: String::new(),
                rights: Rights::Read,
            };
            match picked.kind {
                PublicFolderKind::Calendar => {
                    let Some(list) = out.calendars.as_mut() else { continue };
                    list.calendars.push(RemoteCalendar {
                        id: picked.id.clone(),
                        name,
                        group: GROUP.to_owned(),
                        rights: Rights::Read,
                        ..Default::default()
                    });
                    let from = request.from.max(1);
                    let to = request.to.max(from.saturating_add(1));
                    match self.read_events(&folder, &picked.id, from, to, true).await {
                        Ok((download, _)) => out.events.push(EventsDownload {
                            account: request.account,
                            email: request.email.clone(),
                            has_categories: true,
                            has_options: true,
                            ..download
                        }),
                        Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                        // (Its events stay as they were: there is no download to say otherwise.)
                        Err(_) => out
                            .errors
                            .push("Couldn't read a calendar in the public folders: it may be gone, or not shared with you any more".to_owned()),
                    }
                }
                PublicFolderKind::Contacts => {
                    if out.contacts.is_none() {
                        continue;
                    }
                    let read = self.read_public_contacts(&folder, &name).await;
                    let Some(all) = out.contacts.as_mut() else { continue };
                    all.books.push(RemoteAddressBook { id: picked.id.clone(), name, rights: Rights::Read });
                    match read {
                        Ok((contacts, complete)) => {
                            let room = MAX_CONTACTS.saturating_sub(all.contacts.len());
                            all.complete &= complete && contacts.len() <= room;
                            all.contacts.extend(contacts.into_iter().take(room));
                        }
                        Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                        Err(_) => {
                            // (What is here of it stays: the list is no longer all there is.)
                            all.complete = false;
                            out.errors.push(
                                "Couldn't read an address book in the public folders: it may be gone, or not shared with you any more".to_owned(),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// The contacts of one public address book, and whether they are all of them.
    async fn read_public_contacts(&mut self, folder: &FolderInfo, book: &str) -> Result<(Vec<mailcraft_model::remote::RemoteContact>, bool), Asked> {
        let mut contacts = Vec::new();
        let mut offset = 0u32;
        for _ in 0..MAX_PAGES {
            let find = directory::FindItems::new(Kind::Contacts, folder.id.as_str(), offset, 512).plain();
            let page = self.ask(&find.element(), &|reply| find.answer(reply)).await?;
            let before = page.items.len();
            let last = page.last;
            for item in page.items {
                if let Item::Contact(mut contact) = item {
                    book.clone_into(&mut contact.book);
                    folder.id.as_str().clone_into(&mut contact.book_id);
                    contacts.push(*contact);
                    if contacts.len() >= MAX_CONTACTS {
                        return Ok((contacts, false));
                    }
                }
            }
            if last || before == 0 {
                return Ok((contacts, true));
            }
            offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
        }
        Ok((contacts, false))
    }
}

#[cfg(test)]
#[path = "public_run_tests.rs"]
mod tests;
