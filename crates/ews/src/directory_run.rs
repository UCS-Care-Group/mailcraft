// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A directory run for an Exchange server account: the calendar changes, contact changes and
//! to-do changes made here go up, then the account's calendars, address books and to-do lists
//! (and those it was given by address) come down ([MS-OXWSCORE], [MS-OXWSCAL], [MS-OXWSCONT],
//! [MS-OXWSTASK]).
//!
//! It speaks through a [`Client`] over any [`HttpExchange`] and reads no clock and starts no
//! thread. A busy server is waited for as in a mail run ([`crate::run::sync_mail`]).
//!
//! **The calendar** is read as a `CalendarView` for the stretch asked for: a recurring series
//! comes as its occurrences, and the series itself is read beside them in a second pass so that
//! a rule the mailbox can keep is kept. **The contacts and to-dos** are read whole, a page at a
//! time.
//!
//! **Another mailbox's calendar or address book** is read with the owner's sign-in by putting its
//! address in every folder request (`t:Mailbox`, [MS-OXWSFOLD]), never by an administrator's
//! right: the server answers only what the owner was given access to.

use std::sync::atomic::AtomicBool;

use mailcraft_model::CalendarOwner;
use mailcraft_model::remote::{
    CalendarsDownload, ContactOutcome, ContactsDownload, ContactsUploaded, EventOutcome, EventResult, EventsDownload, EventsUploaded,
    RemoteAddressBook, RemoteCalendar, RemoteContact, RemoteEvent, RemoteEventKind,
};
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList, TaskOpKind, TaskOutcome, TaskResult, TasksDownload, TasksUploaded};

use crate::client::Client;
use crate::directory::{self, Answer, Item, ItemUp, Kind};
use crate::error::{Error, ResponseCode};
use crate::folder::{DistinguishedFolder, FindFolder, FolderInfo, GetFolder};
use crate::http::HttpExchange;
use crate::run::{Asked, Run, Stop};
use crate::soap::Element;

/// How many pages of one listing are followed, and how many items one run takes in.
const MAX_PAGES: usize = 200;
const MAX_ITEMS: usize = 20_000;
/// Most changes one run carries of each kind.
const MAX_CHANGES: usize = 5_000;

/// Read the directory of `request`: the changes first, then the folders and what they hold.
pub async fn sync_directory<E: HttpExchange>(
    client: &Client,
    exchange: &E,
    request: &mailcraft_model::sync::EwsDirectoryRequest,
    cancel: &AtomicBool,
    wait: &mut dyn FnMut(u32) -> bool,
) -> mailcraft_model::sync::EwsDirectoryResult {
    let mut run = Run {
        client,
        exchange,
        cancel,
        wait,
        shape: crate::items::Shape::Full,
        flag_lacking: false,
        read_unsaid: false,
        own: Default::default(),
        reread_folders: false,
    };
    let mut out = mailcraft_model::sync::EwsDirectoryResult {
        account: request.account,
        email: request.email.clone(),
        url: request.url.clone(),
        ..Default::default()
    };
    if let Err(stop) = run.directory(request, &mut out).await {
        match stop {
            Asked::Stop(why) => stop_line(why, &mut out),
            Asked::Code(_) | Asked::Bad | Asked::Lost | Asked::Unreadable => {
                out.errors.push("The server's answer for the calendar, contacts or to-dos couldn't be used".to_owned());
            }
        }
    }
    out.errors.truncate(mailcraft_model::sync::MAX_EWS_ERRORS);
    out
}

/// What a stop says, in words that name nothing of the server.
fn stop_line(stop: Stop, out: &mut mailcraft_model::sync::EwsDirectoryResult) {
    match stop {
        Stop::Cancelled => {}
        Stop::Auth => out.auth_refused = true,
        Stop::Busy(seconds) => out.busy_for = Some(seconds.max(1)),
        Stop::Tls => out.errors.push("Couldn't set up a secure connection with the server".to_owned()),
        Stop::Unreachable => out.errors.push("Couldn't reach the server".to_owned()),
        Stop::Line(text) => out.errors.push(text.to_owned()),
    }
}

impl<E: HttpExchange> Run<'_, E> {
    async fn directory(
        &mut self,
        request: &mailcraft_model::sync::EwsDirectoryRequest,
        out: &mut mailcraft_model::sync::EwsDirectoryResult,
    ) -> Result<(), Asked> {
        let mailbox = request.mailbox.trim();
        let mailbox = (!mailbox.is_empty()).then_some(mailbox);
        // What was changed here goes first, as mail does.
        if let Some(upload) = &request.changes {
            let mut uploaded = EventsUploaded { account: request.account, email: request.email.clone(), ..Default::default() };
            for op in upload.ops.iter().take(MAX_CHANGES) {
                uploaded.outcomes.push(self.push_event(op, &upload.zone).await);
            }
            out.uploaded = Some(uploaded);
        }
        if let Some(upload) = &request.contact_changes {
            let mut uploaded = ContactsUploaded { account: request.account, email: request.email.clone(), ..Default::default() };
            for op in upload.ops.iter().take(MAX_CHANGES) {
                uploaded.outcomes.push(self.push_contact(op).await);
            }
            out.contacts_uploaded = Some(uploaded);
        }
        if let Some(upload) = &request.task_changes {
            let mut uploaded = TasksUploaded { account: request.account, email: request.email.clone(), ..Default::default() };
            for op in upload.ops.iter().take(MAX_CHANGES) {
                uploaded.outcomes.push(self.push_task(op).await);
            }
            out.tasks_uploaded = Some(uploaded);
        }
        // The account's own calendars, books and to-dos.
        if request.calendars {
            match self.read_calendars(request, mailbox, None, false).await {
                Ok((list, events)) => {
                    out.calendars = Some(list);
                    out.events.extend(events);
                }
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => out.errors.push("Couldn't read the calendars from the server".to_owned()),
            }
        }
        if request.contacts {
            match self.read_contacts(request.account, &request.email, mailbox, None).await {
                Ok(download) => out.contacts = Some(download),
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => out.errors.push("Couldn't read the contacts from the server".to_owned()),
            }
        }
        if request.tasks {
            match self.read_tasks(request.account, &request.email, mailbox).await {
                Ok(download) => out.tasks = Some(download),
                Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                Err(_) => out.errors.push("Couldn't read the to-dos from the server".to_owned()),
            }
        }
        // Calendars and address books given by another mailbox's address.
        for shared in &request.shared {
            let address = shared.address.trim();
            if address.is_empty() {
                continue;
            }
            if shared.calendar {
                match self.read_calendars(request, Some(address), Some(address), true).await {
                    Ok((list, events)) => {
                        if let Some(mut list) = Some(list) {
                            // (A shared mailbox's calendars are listed beside the account's own.)
                            out.calendars.get_or_insert_with(|| CalendarsDownload {
                                account: request.account,
                                email: request.email.clone(),
                                complete: true,
                                ..Default::default()
                            });
                            if let Some(own) = out.calendars.as_mut() {
                                own.calendars.append(&mut list.calendars);
                            }
                        }
                        out.events.extend(events);
                    }
                    Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                    Err(_) => out.errors.push("Couldn't read a calendar that was shared with the account".to_owned()),
                }
            }
            if shared.contacts {
                match self.read_contacts(request.account, &request.email, Some(address), Some(address)).await {
                    Ok(download) => match out.contacts.as_mut() {
                        Some(all) => {
                            all.books.extend(download.books);
                            all.contacts.extend(download.contacts);
                        }
                        None => out.contacts = Some(download),
                    },
                    Err(Asked::Stop(stop)) => return Err(Asked::Stop(stop)),
                    Err(_) => out.errors.push("Couldn't read an address book that was shared with the account".to_owned()),
                }
            }
        }
        Ok(())
    }

    /// The folder lists of a kind: the default folder, and every folder of that class below the
    /// top of the mailbox.
    async fn directory_folders(&mut self, kind: Kind, mailbox: Option<&str>) -> Result<(Option<FolderInfo>, Vec<FolderInfo>), Asked> {
        let mut get = GetFolder::new(&[kind.distinguished()]).full();
        if let Some(address) = mailbox {
            get = get.in_mailbox(address);
        }
        let default = match self.ask(&get.element(), &|reply| get.answer(reply)).await {
            Ok(mut answers) => answers.pop().and_then(Result::ok),
            Err(Asked::Code(ResponseCode::FolderNotFound | ResponseCode::NonExistentMailbox)) => None,
            Err(other) => return Err(other),
        };
        let mut all: Vec<FolderInfo> = Vec::new();
        let mut offset = 0u32;
        for _ in 0..MAX_PAGES {
            let mut find = FindFolder::new(DistinguishedFolder::MessageRoot, true, offset, 512);
            if let Some(address) = mailbox {
                find = find.in_mailbox(address);
            }
            let page = match self.ask(&find.element(), &|reply| find.answer(reply)).await {
                Ok(page) => page,
                // (A server that lets the owner into the folder it is given but not list the
                // mailbox — Grommunio does — still has that folder.)
                Err(Asked::Code(ResponseCode::AccessDenied | ResponseCode::FolderNotFound | ResponseCode::NonExistentMailbox))
                    if mailbox.is_some() && default.is_some() =>
                {
                    let default = default.clone().into_iter().collect::<Vec<_>>();
                    return Ok((default.first().cloned(), default));
                }
                Err(other) => return Err(other),
            };
            let before = page.folders.len();
            all.extend(page.folders.into_iter().filter(|folder| folder.class.eq_ignore_ascii_case(kind.class())));
            if page.last || before == 0 {
                break;
            }
            offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
        }
        if let Some(default) = &default
            && !all.iter().any(|folder| folder.id == default.id)
        {
            all.insert(0, default.clone());
        }
        Ok((default, all))
    }

    async fn read_calendars(
        &mut self,
        request: &mailcraft_model::sync::EwsDirectoryRequest,
        mailbox: Option<&str>,
        owner_address: Option<&str>,
        shared: bool,
    ) -> Result<(CalendarsDownload, Vec<EventsDownload>), Asked> {
        let (default, folders) = self.directory_folders(Kind::Calendar, mailbox).await?;
        let mut list = CalendarsDownload { account: request.account, email: request.email.clone(), complete: true, ..Default::default() };
        let mut events = Vec::new();
        for folder in folders.iter().take(mailcraft_model::remote::MAX_REMOTE_CALENDARS) {
            let main = default.as_ref().is_some_and(|default| default.id == folder.id);
            let remote_id = match (owner_address, main) {
                (Some(address), true) => mailcraft_model::remote::opened_id(address),
                (Some(_), false) => folder.id.as_str().to_owned(),
                (None, true) => String::new(),
                (None, false) => folder.id.as_str().to_owned(),
            };
            let name = if main && !shared { String::new() } else { clean_name(&folder.display_name, owner_address) };
            // (The list names a calendar by a real id even when its events come as the account's
            // main one, whose remote id is empty; `apply_calendars` still makes the main one's
            // origin empty.)
            let list_id = if main && !shared { folder.id.as_str().to_owned() } else { remote_id.clone() };
            list.calendars.push(RemoteCalendar {
                id: list_id,
                name,
                group: if shared { "Shared Calendars".to_owned() } else { "My Calendars".to_owned() },
                rights: folder.rights,
                owner: owner_address.map(|address| CalendarOwner { address: address.to_owned(), name: address.to_owned() }),
                main: !shared && main,
                ..Default::default()
            });
            let from = request.from.max(1);
            let to = request.to.max(from.saturating_add(1));
            let (mut download, complete) = self.read_events(folder, &remote_id, from, to, shared).await?;
            if !complete {
                list.complete = false;
            }
            download.account = request.account;
            download.email = request.email.clone();
            download.has_categories = true;
            download.has_options = true;
            events.push(download);
        }
        Ok((list, events))
    }

    async fn read_events(&mut self, folder: &FolderInfo, remote_id: &str, from: i64, to: i64, plain: bool) -> Result<(EventsDownload, bool), Asked> {
        let mut masters: Vec<RemoteEvent> = Vec::new();
        let mut by_uid: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        // The series themselves: `FindItem` without a view returns them, and their occurrences
        // are the `CalendarView` below.
        let mut offset = 0u32;
        let mut masters_complete = false;
        for _ in 0..MAX_PAGES {
            let find = directory::FindItems::new(Kind::Calendar, folder.id.as_str(), offset, 512);
            let find = if plain { find.plain() } else { find };
            let page = self.ask(&find.element(), &|reply| find.answer(reply)).await?;
            let before = page.items.len();
            let last = page.last;
            for item in page.items {
                if let Item::Event(event) = item
                    && event.kind == RemoteEventKind::SeriesMaster
                {
                    if !event.uid.is_empty() {
                        by_uid.insert(event.uid.clone(), event.id.clone());
                    }
                    masters.push(*event);
                }
            }
            if last || before == 0 {
                masters_complete = true;
                break;
            }
            offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
        }
        let mut download = EventsDownload {
            account: 0,
            email: String::new(),
            calendar: remote_id.to_owned(),
            from,
            to,
            complete: masters_complete,
            events: Vec::new(),
            has_categories: true,
            has_options: true,
            ..Default::default()
        };
        download.events.extend(masters.into_iter().filter(|event| event.recurrence.is_some()));
        let mut cursor = from;
        let mut view_complete = false;
        for _ in 0..MAX_PAGES {
            let find = directory::FindItems::new(Kind::Calendar, folder.id.as_str(), 0, 512).view(cursor, to);
            let find = if plain { find.plain() } else { find };
            let page = self.ask(&find.element(), &|reply| find.answer(reply)).await?;
            let before = page.items.len();
            let last = page.last;
            let mut latest = cursor;
            for item in page.items {
                if let Item::Event(mut event) = item {
                    latest = latest.max(event.end.max(event.start));
                    if matches!(event.kind, RemoteEventKind::Occurrence | RemoteEventKind::Exception) {
                        if let Some(master) = by_uid.get(&event.uid) {
                            event.series.clone_from(master);
                        } else {
                            // (A series master that could not be read: what is here is an event of
                            // its own, so that it is never taken for a day of a series that isn't
                            // known here.)
                            event.kind = RemoteEventKind::Single;
                        }
                    }
                    if event.kind != RemoteEventKind::SeriesMaster {
                        download.events.push(*event);
                    }
                }
                if download.events.len() >= MAX_ITEMS {
                    return Ok((download, false));
                }
            }
            if last || before == 0 || latest <= cursor {
                view_complete = true;
                break;
            }
            cursor = latest;
        }
        download.complete &= view_complete;
        let complete = download.complete;
        Ok((download, complete))
    }

    async fn read_contacts(
        &mut self,
        account: mailcraft_model::Id,
        email: &str,
        mailbox: Option<&str>,
        owner_address: Option<&str>,
    ) -> Result<ContactsDownload, Asked> {
        let (_, folders) = self.directory_folders(Kind::Contacts, mailbox).await?;
        let mut download = ContactsDownload { account, email: email.to_owned(), complete: true, books: Vec::new(), contacts: Vec::new() };
        for folder in folders.iter().take(mailcraft_model::remote::MAX_REMOTE_CALENDARS) {
            let book = clean_name(&folder.display_name, owner_address);
            download.books.push(RemoteAddressBook { id: folder.id.as_str().to_owned(), name: book.clone(), rights: folder.rights });
            let mut offset = 0u32;
            for _ in 0..MAX_PAGES {
                let find = directory::FindItems::new(Kind::Contacts, folder.id.as_str(), offset, 512);
                let find = if mailbox.is_some() { find.plain() } else { find };
                let page = self.ask(&find.element(), &|reply| find.answer(reply)).await?;
                let before = page.items.len();
                let last = page.last;
                for item in page.items {
                    if let Item::Contact(mut contact) = item {
                        contact.book = book.clone();
                        contact.book_id = folder.id.as_str().to_owned();
                        download.contacts.push(*contact);
                        if download.contacts.len() >= MAX_ITEMS {
                            download.complete = false;
                            return Ok(download);
                        }
                    }
                }
                if last || before == 0 {
                    break;
                }
                offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
            }
        }
        Ok(download)
    }

    async fn read_tasks(&mut self, account: mailcraft_model::Id, email: &str, mailbox: Option<&str>) -> Result<TasksDownload, Asked> {
        let (_, folders) = self.directory_folders(Kind::Tasks, mailbox).await?;
        let mut download = TasksDownload { account, email: email.to_owned(), complete: true, lists: Vec::new(), tasks: Vec::new() };
        for folder in folders.iter().take(mailcraft_model::remote_tasks::MAX_TASK_LISTS) {
            download.lists.push(RemoteTaskList { id: folder.id.as_str().to_owned(), complete: true, shared: false });
            let mut offset = 0u32;
            for _ in 0..MAX_PAGES {
                let find = directory::FindItems::new(Kind::Tasks, folder.id.as_str(), offset, 512);
                let find = if mailbox.is_some() { find.plain() } else { find };
                let page = self.ask(&find.element(), &|reply| find.answer(reply)).await?;
                let before = page.items.len();
                let last = page.last;
                for item in page.items {
                    if let Item::Task(mut task) = item {
                        task.list = folder.id.as_str().to_owned();
                        download.tasks.push(*task);
                        if download.tasks.len() >= mailcraft_model::remote_tasks::MAX_TASKS {
                            download.complete = false;
                            return Ok(download);
                        }
                    }
                }
                if last || before == 0 {
                    break;
                }
                offset = offset.saturating_add(u32::try_from(before).unwrap_or(u32::MAX));
            }
        }
        Ok(download)
    }

    async fn push_event(&mut self, op: &mailcraft_model::remote::EventOp, zone: &str) -> EventOutcome {
        let zone = op.fields.zones.as_ref().map(|zones| zones.start.as_str()).filter(|zone| !zone.trim().is_empty()).unwrap_or(zone);
        let mut outcome = EventOutcome { op: op.clone(), ..Default::default() };
        if op.remote.is_empty() && op.kind != mailcraft_model::remote::EventChangeKind::Create {
            outcome.result = EventResult::Refused;
            outcome.error = "the server's copy of this event is not known here".to_owned();
            return outcome;
        }
        let result = match op.kind {
            mailcraft_model::remote::EventChangeKind::Respond => {
                if op.remote.is_empty() {
                    Err(Asked::Bad)
                } else {
                    let answer = match op.response {
                        mailcraft_model::Response::Accepted => Answer::Accept,
                        mailcraft_model::Response::Tentative => Answer::Tentative,
                        mailcraft_model::Response::Declined => Answer::Decline,
                        mailcraft_model::Response::None => Answer::Tentative,
                    };
                    let element = directory::meeting_answer(answer, &op.remote, &op.comment);
                    self.ask_once(&element, &|reply| directory::answered(reply)).await.map(|()| (op.remote.clone(), String::new()))
                }
            }
            mailcraft_model::remote::EventChangeKind::Forward => {
                if op.remote.is_empty() || op.to.is_empty() {
                    Err(Asked::Bad)
                } else {
                    let element = forward_item(&op.remote, &op.to, &op.comment);
                    self.ask_once(&element, &|reply| directory::answered(reply)).await.map(|()| (op.remote.clone(), String::new()))
                }
            }
            mailcraft_model::remote::EventChangeKind::Delete => {
                let element = directory::delete_item(Kind::Calendar, &op.remote, "", op.cancel, false);
                self.ask(&element, &|reply| directory::deleted(reply)).await.map(|()| (String::new(), String::new()))
            }
            mailcraft_model::remote::EventChangeKind::Create | mailcraft_model::remote::EventChangeKind::Update => {
                let event = event_from_fields(op, zone);
                if op.kind == mailcraft_model::remote::EventChangeKind::Create {
                    let folder = (!op.calendar.is_empty()).then_some(op.calendar.as_str());
                    let element = directory::create_item(Kind::Calendar, folder, directory::calendar_item(&event, zone), op.send);
                    self.ask_once(&element, &|reply| directory::saved(reply, "CreateItem")).await
                } else {
                    let element = directory::update_event(&op.fields, &op.remote, "", op.send, zone);
                    self.ask(&element, &|_reply| Ok::<_, Error>(())).await.map(|()| (String::new(), String::new()))
                }
            }
        };
        match result {
            Ok((id, key)) => {
                outcome.result = EventResult::Done;
                if !id.is_empty() {
                    outcome.remote = id;
                } else if op.kind == mailcraft_model::remote::EventChangeKind::Create {
                    // (A create always gives an id; without one nothing can find it again.)
                    outcome.result = EventResult::Failed;
                    outcome.error = "the server didn't say which event it made".to_owned();
                }
                outcome.change_key = key;
            }
            Err(Asked::Code(ResponseCode::ItemNotFound | ResponseCode::FolderNotFound)) => outcome.result = EventResult::Gone,
            Err(Asked::Code(ResponseCode::AccessDenied)) => outcome.result = EventResult::Denied,
            Err(Asked::Code(ResponseCode::ServerBusy)) => outcome.result = EventResult::Failed,
            Err(Asked::Stop(Stop::Busy(seconds))) => {
                outcome.result = EventResult::Failed;
                outcome.retry_after = Some(seconds.max(1));
            }
            Err(Asked::Stop(Stop::Auth)) => {
                outcome.result = EventResult::Failed;
                outcome.error = "the server turned the sign-in down".to_owned();
            }
            Err(Asked::Stop(Stop::Unreachable | Stop::Tls)) => {
                outcome.result = EventResult::Failed;
                outcome.error = "the server couldn't be reached".to_owned();
            }
            Err(Asked::Code(_)) => {
                outcome.result = EventResult::Refused;
                outcome.error = "the server won't do it".to_owned();
            }
            Err(_) => {
                outcome.result = EventResult::Failed;
                outcome.error = "the server's answer couldn't be used".to_owned();
            }
        }
        outcome
    }

    async fn push_contact(&mut self, op: &mailcraft_model::remote::ContactOp) -> ContactOutcome {
        let mut outcome = ContactOutcome { op: op.clone(), ..Default::default() };
        // (A deletion carries no card: it names the server's copy by its id alone.)
        let contact = if op.kind == mailcraft_model::remote::ContactChangeKind::Delete {
            RemoteContact::default()
        } else {
            match contact_from_card(&op.card) {
                Some(contact) => contact,
                None => {
                    outcome.result = EventResult::Refused;
                    outcome.error = "the contact couldn't be read".to_owned();
                    return outcome;
                }
            }
        };
        let result = match op.kind {
            mailcraft_model::remote::ContactChangeKind::Delete => {
                if op.remote.is_empty() {
                    Err(Asked::Bad)
                } else {
                    let element = directory::delete_item(Kind::Contacts, &op.remote, "", false, false);
                    self.ask(&element, &|reply| directory::deleted(reply)).await.map(|()| (String::new(), String::new()))
                }
            }
            mailcraft_model::remote::ContactChangeKind::Create => {
                let folder = (!op.book.is_empty()).then_some(op.book.as_str());
                let element = directory::create_item(Kind::Contacts, folder, directory::contact_item(&contact), false);
                self.ask_once(&element, &|reply| directory::saved(reply, "CreateItem")).await
            }
            mailcraft_model::remote::ContactChangeKind::Update => {
                if op.remote.is_empty() {
                    Err(Asked::Bad)
                } else {
                    let element = directory::update_item(Kind::Contacts, &ItemUp::Contact(Box::new(contact)), &op.remote, "", false, "");
                    self.ask(&element, &|_reply| Ok::<_, Error>(())).await.map(|()| (op.remote.clone(), String::new()))
                }
            }
        };
        match result {
            Ok((id, key)) => {
                outcome.result = EventResult::Done;
                if !id.is_empty() {
                    outcome.remote = id;
                } else if op.kind == mailcraft_model::remote::ContactChangeKind::Create {
                    outcome.result = EventResult::Failed;
                    outcome.error = "the server didn't say which contact it made".to_owned();
                }
                let _ = key;
            }
            Err(Asked::Code(ResponseCode::ItemNotFound | ResponseCode::FolderNotFound)) => outcome.result = EventResult::Gone,
            Err(Asked::Code(ResponseCode::AccessDenied)) => outcome.result = EventResult::Denied,
            Err(Asked::Stop(Stop::Busy(seconds))) => {
                outcome.result = EventResult::Failed;
                outcome.error = format!("the server is busy for {seconds} s");
            }
            Err(Asked::Code(_)) => {
                outcome.result = EventResult::Refused;
                outcome.error = "the server won't do it".to_owned();
            }
            Err(_) => {
                outcome.result = EventResult::Failed;
                outcome.error = "the server's answer couldn't be used".to_owned();
            }
        }
        outcome
    }

    async fn push_task(&mut self, op: &mailcraft_model::remote_tasks::TaskOp) -> TaskOutcome {
        let mut outcome = TaskOutcome { kind: op.kind, task: op.task, patch: op.patch.clone(), ..Default::default() };
        if op.id.is_empty() && op.kind != TaskOpKind::Create {
            outcome.result = TaskResult::Refused;
            outcome.error = "the server's copy of this to-do is not known here".to_owned();
            return outcome;
        }
        let task = task_from_patch(op);
        let result = match op.kind {
            TaskOpKind::Delete => {
                let element = directory::delete_item(Kind::Tasks, &op.id, "", false, false);
                self.ask(&element, &|reply| directory::deleted(reply)).await.map(|()| (String::new(), String::new()))
            }
            TaskOpKind::Create => {
                let folder = (!op.list.is_empty()).then_some(op.list.as_str());
                let element = directory::create_item(Kind::Tasks, folder, directory::task_item(&task), false);
                self.ask_once(&element, &|reply| directory::saved(reply, "CreateItem")).await
            }
            TaskOpKind::Update => {
                let element = directory::update_task(&op.patch, &op.id, "");
                self.ask(&element, &|_reply| Ok::<_, Error>(())).await.map(|()| (String::new(), String::new()))
            }
        };
        match result {
            Ok((id, key)) => {
                outcome.result = TaskResult::Done;
                if !id.is_empty() {
                    outcome.id = id;
                } else if op.kind == TaskOpKind::Create {
                    outcome.result = TaskResult::Failed;
                    outcome.error = "the server didn't say which to-do it made".to_owned();
                }
                outcome.change_key = key;
                // (A to-do made in the server's default list has no id of its own for the list
                // here; the next download gives it the folder's id.)
                outcome.list = if op.list.is_empty() { "tasks".to_owned() } else { op.list.clone() };
            }
            Err(Asked::Code(ResponseCode::ItemNotFound | ResponseCode::FolderNotFound)) => outcome.result = TaskResult::Gone,
            Err(Asked::Code(ResponseCode::AccessDenied)) => outcome.result = TaskResult::Denied,
            Err(Asked::Stop(Stop::Busy(_))) => outcome.result = TaskResult::Failed,
            Err(Asked::Code(_)) => {
                outcome.result = TaskResult::Refused;
                outcome.error = "the server won't do it".to_owned();
            }
            Err(_) => {
                outcome.result = TaskResult::Failed;
                outcome.error = "the server's answer couldn't be used".to_owned();
            }
        }
        outcome
    }
}

/// A name a folder is shown under; a shared one says whose it is.
fn clean_name(name: &str, owner: Option<&str>) -> String {
    let name = name.chars().filter(|c| !c.is_control()).take(255).collect::<String>();
    match owner {
        Some(address) if !name.is_empty() => format!("{name} ({address})"),
        Some(address) => address.to_owned(),
        None if name.is_empty() => "Calendar".to_owned(),
        None => name,
    }
}

/// An event from the fields an upload says.
fn event_from_fields(op: &mailcraft_model::remote::EventOp, _zone: &str) -> RemoteEvent {
    let fields = &op.fields;
    let (start, end, all_day) = fields.time.map_or((0, 0, false), |time| (time.start, time.end.max(time.start), time.all_day));
    let recurrence = fields.repeat.as_ref().and_then(|repeat| repeat.rule.clone());
    RemoteEvent {
        id: op.remote.clone(),
        token: op.token.clone(),
        title: fields.title.clone().unwrap_or_default(),
        location: fields.location.clone().unwrap_or_default(),
        description: fields.description.clone().unwrap_or_default(),
        start,
        end,
        all_day,
        show_as: fields.show_as.unwrap_or_default(),
        private: fields.private.unwrap_or(false),
        reminder_minutes: fields.reminder.as_ref().map(|reminder| reminder.minutes),
        recurrence,
        series_day: fields.repeat.as_ref().map(|repeat| repeat.first),
        attendees: fields.attendees.clone().unwrap_or_default(),
        categories: fields.categories.clone().unwrap_or_default(),
        importance: fields.importance.unwrap_or_default(),
        options: mailcraft_model::ResponseOptions {
            request_responses: fields.request_responses.unwrap_or(true),
            allow_proposals: fields.allow_proposals.unwrap_or(true),
        },
        ..RemoteEvent::default()
    }
}

/// A contact from the vCard the engine wrote for it.
fn contact_from_card(card: &str) -> Option<RemoteContact> {
    let contacts = mailcraft_vcard::parse(card);
    let contact = contacts.first()?;
    Some(RemoteContact {
        display_name: contact.display_name.clone(),
        first: contact.first.clone(),
        middle: contact.middle.clone(),
        last: contact.last.clone(),
        nickname: contact.nickname.clone(),
        company: contact.company.clone(),
        title: contact.title.clone(),
        department: contact.department.clone(),
        emails: contact.emails.clone(),
        phones: contact.phones.clone(),
        addresses: contact.addresses.clone(),
        websites: contact.websites.clone(),
        birthday: contact.birthday,
        notes: contact.notes.clone(),
        ..RemoteContact::default()
    })
}

/// A to-do from the patch an upload says.
fn task_from_patch(op: &mailcraft_model::remote_tasks::TaskOp) -> RemoteTask {
    let patch = &op.patch;
    RemoteTask {
        id: op.id.clone(),
        list: op.list.clone(),
        title: patch.title.clone().unwrap_or_default(),
        notes: patch.notes.clone().unwrap_or_default(),
        start: patch.start.flatten(),
        due: patch.due.flatten(),
        completed: patch.completed.flatten(),
        importance: patch.importance.unwrap_or_default(),
        categories: patch.categories.clone().unwrap_or_default(),
        reminder: patch.reminder.flatten(),
        change_key: String::new(),
    }
}

/// `CreateItem` with a `ForwardItem` for a meeting sent on to other people.
fn forward_item(id: &str, to: &[mailcraft_model::Address], comment: &str) -> Element {
    let mut item = Element::new("t:ForwardItem").child(Element::new("t:ReferenceItemId").attr("Id", id));
    let recipients = to.iter().filter(|address| !address.email.trim().is_empty()).fold(Element::new("t:ToRecipients"), |list, address| {
        let mut mailbox = Element::new("t:Mailbox");
        if !address.name.trim().is_empty() {
            mailbox = mailbox.child(Element::new("t:Name").text(address.name.trim()));
        }
        list.child(mailbox.child(Element::new("t:EmailAddress").text(address.email.trim())))
    });
    item = item.child(recipients);
    if !comment.trim().is_empty() {
        item = item.child(Element::new("t:Body").attr("BodyType", "Text").text(comment));
    }
    Element::new("m:CreateItem").child(Element::new("m:Items").child(item))
}

#[cfg(test)]
#[path = "directory_run_tests.rs"]
mod tests;
