// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The public folders' part of a directory run (#1263): what is listed, what is read, and that
//! nothing of them is asked for unless the owner picked or asked, and nothing is written.

use std::future::{Future, ready};
use std::sync::Mutex;

use mailcraft_model::sync::{EwsDirectoryRequest, EwsDirectoryResult};
use mailcraft_model::{PublicFolder, PublicFolderKind, Rights};

use super::*;
use crate::http::{HttpExchange, HttpRequest, HttpResponse, TransportError};
use crate::standin::{StandIn, response};

/// The account's own mailbox is the stand-in's; the public folders are answered here: a folder
/// Team (mail) holding the calendar Holidays and the folder Archive, which the owner may not look
/// into, and the address book Phone list at the top.
struct PublicServer {
    standin: StandIn,
    /// Every request's body, in order.
    asked: Mutex<Vec<String>>,
    /// Answer `FindFolder` below any folder with one more folder, without end (a hostile server).
    endless: bool,
}

impl PublicServer {
    fn new() -> Self {
        Self { standin: StandIn::new(), asked: Mutex::new(Vec::new()), endless: false }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

fn folder(id: &str, name: &str, class: &str, children: u32) -> String {
    format!(
        "<t:Folder><t:FolderId Id=\"{id}\" ChangeKey=\"AQ==\"/><t:DisplayName>{name}</t:DisplayName><t:ChildFolderCount>{children}</t:ChildFolderCount>\
         <t:FolderClass>{class}</t:FolderClass></t:Folder>"
    )
}

fn folders(xml: &str) -> String {
    response(
        "FindFolder",
        &format!(
            "<m:FindFolderResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>\
             <m:RootFolder IndexedPagingOffset=\"0\" TotalItemsInView=\"1\" IncludesLastItemInRange=\"true\"><t:Folders>{xml}</t:Folders></m:RootFolder>\
             </m:FindFolderResponseMessage>"
        ),
    )
}

fn items(xml: &str) -> String {
    response(
        "FindItem",
        &format!(
            "<m:FindItemResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>\
             <m:RootFolder IndexedPagingOffset=\"0\" TotalItemsInView=\"1\" IncludesLastItemInRange=\"true\"><t:Items>{xml}</t:Items></m:RootFolder>\
             </m:FindItemResponseMessage>"
        ),
    )
}

fn refused(operation: &str, code: &str) -> String {
    response(operation, &crate::standin::error_message(operation, code))
}

impl HttpExchange for PublicServer {
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>> {
        let body = String::from_utf8_lossy(request.body()).into_owned();
        self.asked.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(body.clone());
        let find_folder = body.contains("<m:FindFolder");
        let find_item = body.contains("<m:FindItem");
        let answer = if find_folder && self.endless && (body.contains("publicfoldersroot") || body.contains("Id=\"pf-")) {
            let n = self.asked().len();
            Some(folders(&folder(&format!("pf-{n}"), &format!("Level {n}"), "IPF.Appointment", 1)))
        } else if find_folder && body.contains("publicfoldersroot") {
            Some(folders(&(folder("pf-team", "Team", "IPF.Note", 2) + &folder("pf-phones", "Phone list", "IPF.Contact", 0))))
        } else if find_folder && body.contains("Id=\"pf-team\"") {
            Some(folders(&(folder("pf-holidays", "Holidays", "IPF.Appointment", 0) + &folder("pf-archive", "Archive", "IPF.Note", 1))))
        } else if find_folder && body.contains("Id=\"pf-archive\"") {
            Some(refused("FindFolder", "ErrorAccessDenied"))
        } else if find_item && body.contains("Id=\"pf-holidays\"") {
            Some(items(
                "<t:CalendarItem><t:ItemId Id=\"ev-1\" ChangeKey=\"AQ==\"/><t:Subject>Summer break</t:Subject>\
                 <t:Start>2026-07-01T08:00:00Z</t:Start><t:End>2026-07-01T09:00:00Z</t:End><t:CalendarItemType>Single</t:CalendarItemType>\
                 <t:UID>uid-1</t:UID></t:CalendarItem>",
            ))
        } else if find_item && body.contains("Id=\"pf-phones\"") {
            Some(items(
                "<t:Contact><t:ItemId Id=\"c-1\" ChangeKey=\"AQ==\"/><t:DisplayName>Front desk</t:DisplayName>\
                 <t:EmailAddresses><t:Entry Key=\"EmailAddress1\">desk@example.org</t:Entry></t:EmailAddresses></t:Contact>",
            ))
        } else if find_item && body.contains("Id=\"pf-gone\"") {
            Some(refused("FindItem", "ErrorFolderNotFound"))
        } else {
            None
        };
        let answered = match answer {
            Some(body) => Ok(HttpResponse::new(200, body.into_bytes())),
            None => crate::standin::block_on(self.standin.exchange(request)).unwrap_or(Err(TransportError::Other)),
        };
        ready(answered)
    }
}

fn request() -> EwsDirectoryRequest {
    EwsDirectoryRequest {
        account: 1,
        email: "ada@example.org".into(),
        url: "http://127.0.0.1/EWS/Exchange.asmx".into(),
        calendars: true,
        contacts: true,
        from: 1,
        to: 2_000_000_000,
        ..Default::default()
    }
}

fn run(server: &PublicServer, request: &EwsDirectoryRequest) -> EwsDirectoryResult {
    let client = crate::Client::new("http://127.0.0.1/EWS/Exchange.asmx", crate::Secret::new(crate::standin::AUTHORIZATION)).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    crate::standin::block_on(crate::sync_directory(&client, server, request, &cancel, &mut |_| true)).unwrap()
}

fn picked(id: &str, name: &str, kind: PublicFolderKind) -> PublicFolder {
    PublicFolder { id: id.into(), name: name.into(), kind }
}

/// Nothing that makes, changes, deletes, moves or sends anything was asked.
fn nothing_written(server: &PublicServer) {
    for body in server.asked() {
        for write in [
            "CreateItem",
            "UpdateItem",
            "DeleteItem",
            "MoveItem",
            "CopyItem",
            "SendItem",
            "CreateFolder",
            "UpdateFolder",
            "DeleteFolder",
            "EmptyFolder",
        ] {
            assert!(!body.contains(&format!("<m:{write}")), "{write} was asked");
        }
    }
    assert!(server.standin.state().writes.is_empty());
}

#[test]
fn the_public_folders_calendars_and_address_books_are_listed_with_the_folders_they_lie_in() {
    let server = PublicServer::new();
    let result = run(&server, &EwsDirectoryRequest { browse_public: true, ..request() });
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(result.public_browsed);
    let listing = result.public_listing.unwrap();
    let seen: Vec<(&str, &str, PublicFolderKind)> = listing.iter().map(|f| (f.id.as_str(), f.name.as_str(), f.kind)).collect();
    // (The mail folders are walked through and not listed; the one the owner may not look into
    // is passed over without a word.)
    assert_eq!(seen, vec![("pf-phones", "Phone list", PublicFolderKind::Contacts), ("pf-holidays", "Team / Holidays", PublicFolderKind::Calendar)]);
    // The listing asks one level at a time, and never in another mailbox.
    let public: Vec<String> = server.asked().into_iter().filter(|b| b.contains("publicfoldersroot") || b.contains("Id=\"pf-")).collect();
    assert!(public.iter().all(|b| b.contains("Traversal=\"Shallow\"") && !b.contains("<t:Mailbox>")), "{public:?}");
    nothing_written(&server);
}

#[test]
fn nothing_of_the_public_folders_is_asked_for_unless_the_owner_picked_one_or_asked_for_the_list() {
    let server = PublicServer::new();
    let result = run(&server, &request());
    assert!(!result.public_browsed && result.public_listing.is_none());
    assert!(server.asked().iter().all(|b| !b.contains("publicfoldersroot") && !b.contains("Id=\"pf-")));
}

#[test]
fn a_public_calendar_and_address_book_picked_are_read_beside_the_accounts_own_and_read_only() {
    let server = PublicServer::new();
    let public =
        vec![picked("pf-holidays", "Team / Holidays", PublicFolderKind::Calendar), picked("pf-phones", "Phone list", PublicFolderKind::Contacts)];
    let result = run(&server, &EwsDirectoryRequest { public, ..request() });
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let calendars = result.calendars.unwrap();
    let holidays = calendars.calendars.iter().find(|c| c.id == "pf-holidays").unwrap();
    assert_eq!(
        (holidays.name.as_str(), holidays.group.as_str(), holidays.rights, holidays.main),
        ("Team / Holidays", "Public Folders", Rights::Read, false)
    );
    assert!(calendars.calendars.iter().any(|c| c.main), "the account's own calendar is listed with it");
    let events = result.events.iter().find(|d| d.calendar == "pf-holidays").unwrap();
    assert_eq!(events.account, 1);
    assert_eq!(events.events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), vec!["Summer break"]);
    let contacts = result.contacts.unwrap();
    let book = contacts.books.iter().find(|b| b.id == "pf-phones").unwrap();
    assert_eq!((book.name.as_str(), book.rights), ("Phone list", Rights::Read));
    let desk = contacts.contacts.iter().find(|c| c.book_id == "pf-phones").unwrap();
    assert_eq!((desk.display_name.as_str(), desk.book.as_str()), ("Front desk", "Phone list"));
    assert!(contacts.complete);
    nothing_written(&server);
}

#[test]
fn a_public_folder_that_cant_be_read_says_so_and_takes_nothing_away() {
    let server = PublicServer::new();
    let public = vec![picked("pf-gone", "Old calendar", PublicFolderKind::Calendar), picked("pf-gone", "Old book", PublicFolderKind::Contacts)];
    let result = run(&server, &EwsDirectoryRequest { public, ..request() });
    assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
    // The calendar is still listed (so it isn't removed) and has no download (so none of its
    // events are); the contacts are no longer all there are (so none are removed).
    assert!(result.calendars.unwrap().calendars.iter().any(|c| c.id == "pf-gone"));
    assert!(!result.events.iter().any(|d| d.calendar == "pf-gone"));
    assert!(!result.contacts.unwrap().complete);
}

#[test]
fn a_public_folder_tree_without_end_is_walked_only_so_far() {
    let server = PublicServer { endless: true, ..PublicServer::new() };
    let result = run(&server, &EwsDirectoryRequest { browse_public: true, ..request() });
    let listing = result.public_listing.unwrap();
    assert!(listing.len() <= MAX_DEPTH, "{}", listing.len());
    let public = server.asked().iter().filter(|b| b.contains("publicfoldersroot") || b.contains("Id=\"pf-")).count();
    assert!(public <= MAX_DEPTH, "{public} requests");
}

#[test]
fn a_folder_class_is_a_calendar_or_an_address_book_only_by_its_whole_name_or_a_kind_of_it() {
    assert_eq!(kind_of("IPF.Appointment"), Some(PublicFolderKind::Calendar));
    assert_eq!(kind_of(" ipf.appointment.Birthday "), Some(PublicFolderKind::Calendar));
    assert_eq!(kind_of("IPF.Contact"), Some(PublicFolderKind::Contacts));
    assert_eq!(kind_of("IPF.Contactish"), None);
    assert_eq!(kind_of("IPF.Note"), None);
    assert_eq!(kind_of("IPF.Task"), None);
    assert_eq!(kind_of(""), None);
    assert_eq!(kind_of("IPF.Appointmentäöü"), None);
}

#[test]
fn a_public_folders_name_is_one_line_and_capped() {
    assert_eq!(path_name("", "  Holidays\r\n"), "Holidays");
    assert_eq!(path_name("Team", "Holi\u{0}days"), "Team / Holi days");
    assert_eq!(path_name("Team", ""), "Team / Public folder");
    let long = path_name(&"ä".repeat(500), "x");
    assert_eq!(long.chars().count(), mailcraft_model::MAX_PUBLIC_NAME);
}

#[test]
fn a_public_calendar_is_read_for_its_own_stretch_and_the_accounts_for_the_one_on_screen() {
    let server = PublicServer::new();
    let public = vec![picked("pf-holidays", "Team / Holidays", PublicFolderKind::Calendar)];
    let (public_from, public_to) = (1_786_000_000, 1_826_000_000);
    let result = run(&server, &EwsDirectoryRequest { public, public_from, public_to, from: 1_794_000_000, to: 1_796_000_000, ..request() });
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let events = result.events.iter().find(|d| d.calendar == "pf-holidays").unwrap();
    assert_eq!((events.from, events.to), (public_from, public_to));
    let views: Vec<String> = server.asked().into_iter().filter(|body| body.contains("CalendarView")).collect();
    let start = |at: i64| format!("StartDate=\"{}\"", mailcraft_time::format_iso_utc(at));
    assert!(views.iter().any(|body| body.contains("pf-holidays") && body.contains(&start(public_from))), "{views:?}");
    assert!(
        views.iter().filter(|body| !body.contains("pf-holidays")).all(|body| body.contains(&start(1_794_000_000))),
        "the account's own calendar is read for the stretch on screen: {views:?}"
    );
    nothing_written(&server);
}
