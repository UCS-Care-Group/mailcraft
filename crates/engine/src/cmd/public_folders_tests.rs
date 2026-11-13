// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Picking calendars and address books of the public folders (#1263).

use mailcraft_model::remote::{CalendarsDownload, ContactsDownload, EventsDownload, RemoteAddressBook, RemoteCalendar, RemoteContact, RemoteEvent};
use mailcraft_model::sync::EwsDirectoryResult;
use mailcraft_model::{Account, PublicFolder, PublicFolderKind, Rights};
use serde_json::json;

use crate::Session;

fn session() -> (Session, u64) {
    let mut s = Session::empty();
    let added = s
        .run(
            "account.add",
            &json!({"email": "ada@example.org", "kind": "exchange", "ews": {"url": "https://exchange.example.org/EWS/Exchange.asmx"}}),
        )
        .unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn listing() -> Vec<PublicFolder> {
    vec![
        PublicFolder { id: "pf-holidays".into(), name: "Team / Holidays".into(), kind: PublicFolderKind::Calendar },
        PublicFolder { id: "pf-phones".into(), name: "Phone list".into(), kind: PublicFolderKind::Contacts },
    ]
}

/// A run's result for `account`: its own main calendar and contacts, and what `public` adds.
fn result(account: u64, browsed: Option<Vec<PublicFolder>>, public: bool) -> EwsDirectoryResult {
    let mut calendars = vec![RemoteCalendar { id: "own-main".into(), name: String::new(), main: true, ..Default::default() }];
    let mut events = Vec::new();
    let mut books = vec![RemoteAddressBook { id: "own-contacts".into(), name: "Contacts".into(), rights: Rights::Edit }];
    let mut contacts = Vec::new();
    if public {
        calendars.push(RemoteCalendar {
            id: "pf-holidays".into(),
            name: "Team / Holidays".into(),
            group: "Public Folders".into(),
            rights: Rights::Read,
            ..Default::default()
        });
        events.push(EventsDownload {
            account,
            email: "ada@example.org".into(),
            calendar: "pf-holidays".into(),
            from: 1,
            to: 2_000_000_000,
            complete: true,
            events: vec![RemoteEvent {
                id: "ev-1".into(),
                title: "Summer break".into(),
                start: 1_782_892_800,
                end: 1_782_896_400,
                ..Default::default()
            }],
            ..Default::default()
        });
        books.push(RemoteAddressBook { id: "pf-phones".into(), name: "Phone list".into(), rights: Rights::Read });
        contacts.push(RemoteContact {
            id: "c-1".into(),
            display_name: "Front desk".into(),
            book: "Phone list".into(),
            book_id: "pf-phones".into(),
            ..Default::default()
        });
    }
    EwsDirectoryResult {
        account,
        email: "ada@example.org".into(),
        calendars: Some(CalendarsDownload { account, email: "ada@example.org".into(), complete: true, calendars, ..Default::default() }),
        events,
        contacts: Some(ContactsDownload { account, email: "ada@example.org".into(), complete: true, books, contacts }),
        public_browsed: browsed.is_some(),
        public_listing: browsed,
        ..Default::default()
    }
}

#[test]
fn a_listing_is_asked_for_by_the_next_run_and_answered_when_it_comes() {
    let (mut s, account) = session();
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(1), Some(2), false).unwrap();
    assert!(!request.browse_public && request.public.is_empty());
    let asked = s.run("publicFolders.list", &json!({"account": account})).unwrap();
    assert_eq!(asked["pending"], true);
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(1), Some(2), false).unwrap();
    assert!(request.browse_public);
    let waiting = s.run("publicFolders.listed", &json!({"account": account})).unwrap();
    assert_eq!((waiting["pending"].clone(), waiting["listed"].clone()), (json!(true), json!(false)));
    // A run that wasn't asked for it (started before) leaves the asking as it was.
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, None, false)).unwrap();
    assert_eq!(s.run("publicFolders.listed", &json!({"account": account})).unwrap()["pending"], true);
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, Some(listing()), false)).unwrap();
    let listed = s.run("publicFolders.listed", &json!({"account": account})).unwrap();
    assert_eq!(listed["pending"], false);
    assert_eq!(listed["folders"][0], json!({"id": "pf-holidays", "name": "Team / Holidays", "kind": "calendar", "open": false}));
    assert_eq!(listed["folders"][1]["kind"], "contacts");
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(1), Some(2), false).unwrap();
    assert!(!request.browse_public, "a listing is made once for each asking");
    // Nothing of the listing is in the mailbox file.
    assert!(!serde_json::to_string(&s.mailbox).unwrap().contains("pf-holidays"));
}

#[test]
fn a_listing_that_couldnt_be_made_says_so() {
    let (mut s, account) = session();
    s.run("publicFolders.list", &json!({"account": account})).unwrap();
    let mut failed = result(account, None, false);
    failed.public_browsed = true;
    failed.errors.push("Couldn't list the public folders".into());
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &failed).unwrap();
    let listed = s.run("publicFolders.listed", &json!({"account": account})).unwrap();
    assert_eq!((listed["pending"].clone(), listed["failed"].clone(), listed["listed"].clone()), (json!(false), json!(true), json!(false)));
}

#[test]
fn a_public_calendar_and_book_picked_are_read_shown_read_only_and_removed_whole() {
    let (mut s, account) = session();
    s.run("publicFolders.list", &json!({"account": account})).unwrap();
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, Some(listing()), false)).unwrap();
    // Only a folder of the listing can be picked.
    assert!(s.run("publicFolders.open", &json!({"account": account, "id": "pf-elsewhere"})).is_err());
    let opened = s.run("publicFolders.open", &json!({"account": account, "id": "pf-holidays"})).unwrap();
    assert_eq!((opened["name"].clone(), opened["kind"].clone()), (json!("Team / Holidays"), json!("calendar")));
    assert!(s.run("publicFolders.open", &json!({"account": account, "id": "pf-holidays"})).is_err(), "once");
    s.run("publicFolders.open", &json!({"account": account, "id": "pf-phones"})).unwrap();
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(1), Some(2), false).unwrap();
    assert_eq!(request.public.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["pf-holidays", "pf-phones"]);
    // The account's list is kept in the mailbox file.
    assert!(serde_json::to_string(&s.mailbox).unwrap().contains("pf-holidays"));

    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, None, true)).unwrap();
    let calendar = s.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id == "pf-holidays").unwrap();
    assert!(calendar.visible, "a public calendar picked is shown");
    assert_eq!((calendar.rights, calendar.group.as_str()), (Rights::Read, "Public Folders"));
    let calendar = calendar.id;
    assert_eq!(s.mailbox.events.values().filter(|e| e.calendar == calendar).count(), 1);
    assert!(s.mailbox.contacts.values().any(|c| c.book_id == "pf-phones"));
    assert!(s.mailbox.contact_books.iter().any(|b| b.id == "pf-phones" && b.rights == Rights::Read));
    // Hidden by the owner, it stays hidden at the next run.
    s.mailbox.calendars.iter_mut().find(|c| c.id == calendar).unwrap().visible = false;
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, None, true)).unwrap();
    assert!(!s.mailbox.calendar(calendar).unwrap().visible);

    let closed = s.run("publicFolders.close", &json!({"id": "pf-holidays"})).unwrap();
    assert_eq!((closed["removed"].clone(), closed["events"].clone()), (json!("Team / Holidays"), json!(1)));
    assert!(s.mailbox.calendar(calendar).is_none());
    let closed = s.run("publicFolders.close", &json!({"id": "pf-phones"})).unwrap();
    assert_eq!(closed["contacts"], 1);
    assert!(!s.mailbox.contacts.values().any(|c| c.book_id == "pf-phones"));
    assert!(!s.mailbox.contact_books.iter().any(|b| b.id == "pf-phones"));
    assert!(s.mailbox.account(account).unwrap().opened_public.is_empty());
    // The account's own calendar and book stay.
    assert!(s.mailbox.calendars.iter().any(|c| c.origin.account == account && c.origin.id.is_empty()));
    assert!(s.run("publicFolders.close", &json!({"id": "pf-phones"})).is_err());
}

#[test]
fn public_folders_are_for_an_exchange_server_accounts_own_mailbox_only() {
    let mut s = Session::empty();
    assert!(s.run("publicFolders.list", &json!({})).is_err());
    let id = s.run("account.add", &json!({"email": "ada@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert!(s.run("publicFolders.list", &json!({"account": id})).is_err());
}

#[test]
fn an_account_keeps_only_public_folders_that_can_be() {
    let mut account = Account {
        opened_public: vec![
            PublicFolder { id: "pf-1".into(), name: "One\nline".into(), kind: PublicFolderKind::Calendar },
            PublicFolder { id: "pf-1".into(), name: "Twice".into(), kind: PublicFolderKind::Calendar },
            PublicFolder { id: String::new(), name: "No id".into(), kind: PublicFolderKind::Contacts },
            PublicFolder { id: "x".repeat(2_000), name: "Too long".into(), kind: PublicFolderKind::Contacts },
        ],
        ..Default::default()
    };
    account.opened_public.extend((0..40).map(|n| PublicFolder {
        id: format!("pf-more-{n}"),
        name: format!("More {n}"),
        kind: PublicFolderKind::Calendar,
    }));
    account.clamp_opened_calendars();
    assert_eq!(account.opened_public.len(), mailcraft_model::MAX_PUBLIC_FOLDERS);
    assert_eq!(account.opened_public[0].name, "One line");
    assert_eq!(account.opened_public[1].id, "pf-more-0");
    // A list in a file this version can't read in part keeps what it can.
    let file =
        json!({"id": 1, "kind": "exchange", "openedPublic": [{"id": "pf-a", "name": "A", "kind": "calendar"}, {"id": "pf-b", "kind": "notes"}]});
    let read: Account = serde_json::from_value(file).unwrap();
    assert_eq!(read.opened_public.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), vec!["pf-a"]);
}

#[test]
fn a_public_calendar_is_read_months_back_and_a_year_ahead_with_the_stretch_on_screen() {
    use crate::cmd::ews_directory::public_stretch;
    const DAY: i64 = 86_400;
    let now = 1_794_528_000; // 13 November 2026
    // A month on screen near today: the stretch around today, with the month in it.
    let (from, to) = public_stretch(now, now - 30 * DAY, now + 20 * DAY);
    assert_eq!((from, to), (now - 92 * DAY, now + 366 * DAY));
    // A month on screen a little further back than three months is added to it.
    let (from, to) = public_stretch(now, now - 200 * DAY, now - 160 * DAY);
    assert_eq!((from, to), (now - 200 * DAY, now + 366 * DAY));
    // A month years away is read alone, so a calendar view is never too long for the server.
    let (from, to) = public_stretch(now, now - 1_200 * DAY, now - 1_160 * DAY);
    assert_eq!((from, to), (now - 1_200 * DAY, now - 1_160 * DAY));

    // Only a request with a public folder picked carries one.
    let (mut s, account) = session();
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(now), Some(now + DAY), false).unwrap();
    assert_eq!((request.public_from, request.public_to), (0, 0));
    s.run("publicFolders.list", &json!({"account": account})).unwrap();
    crate::cmd::ews_directory::apply_ews_directory(&mut s, &result(account, Some(listing()), false)).unwrap();
    s.run("publicFolders.open", &json!({"account": account, "id": "pf-holidays"})).unwrap();
    let request = crate::cmd::ews_directory::ews_directory_request(&s, account, Some(now), Some(now + DAY), false).unwrap();
    assert!(request.public_from < request.from && request.public_to > request.to, "{request:?}");
    assert_eq!((request.from, request.to), (now, now + DAY), "the account's own calendar keeps the stretch on screen");
}
