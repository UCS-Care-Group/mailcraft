// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #70: calendars opened by address: the commands, what a list from the host does with them, and
//! that nothing but the owner's own list can bring one in.

use mailcraft_model::remote::{CalendarsDownload, EventsDownload, OpenedRefusal, OpenedWhy, RemoteCalendar, RemoteEvent, opened_id};
use mailcraft_model::{CalendarOwner, Directory, Id, MAX_OPENED_CALENDARS, Rights};
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Access, Class, access};
use crate::cmd::remote;

const OWNER: &str = "ada@example.org";
const NOW: i64 = 1_791_993_600;

/// A session with a work account that signs in with Microsoft, and its id.
fn served() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft", "provider": "microsoft", "tenant": "organizations"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn main_calendar() -> RemoteCalendar {
    RemoteCalendar { id: "c-main".into(), name: "Calendar".into(), main: true, rights: Rights::Edit, ..Default::default() }
}

fn opened(address: &str, name: &str, rights: Rights) -> RemoteCalendar {
    RemoteCalendar {
        id: opened_id(address),
        name: name.into(),
        rights,
        owner: Some(CalendarOwner { address: address.into(), name: name.into() }),
        ..Default::default()
    }
}

fn list(s: &mut Session, account: Id, more: Vec<RemoteCalendar>, refused: Vec<OpenedRefusal>) -> Value {
    let mut calendars = vec![main_calendar()];
    calendars.extend(more);
    let download = CalendarsDownload { account, email: OWNER.into(), complete: true, calendars, refused };
    remote::apply_calendars(s, &download).unwrap()
}

fn open(s: &mut Session, address: &str) -> Result<Value, String> {
    s.run("calendar.openShared", &json!({"address": address})).map_err(|e| e.to_string())
}

fn named(s: &Session, name: &str) -> Option<mailcraft_model::Calendar> {
    s.mailbox.calendars.iter().find(|c| c.name == name).cloned()
}

#[test]
fn an_address_is_opened_and_the_next_list_brings_the_calendar_under_shared_calendars() {
    let (mut s, account) = served();
    let answer = open(&mut s, "cy@example.org").unwrap();
    assert_eq!((answer["address"].as_str(), answer["pending"].as_bool()), (Some("cy@example.org"), Some(true)));
    // Nothing is a calendar yet: the host reads it with the next job.
    assert!(named(&s, "Cy Colleague").is_none());
    assert_eq!(s.mailbox.account(account).unwrap().opened_calendars.len(), 1);
    let picked = s.mailbox.account(account).unwrap().opened_calendars[0].color;
    list(&mut s, account, vec![opened("cy@example.org", "Cy Colleague", Rights::Read)], vec![]);
    let calendar = named(&s, "Cy Colleague").unwrap();
    assert_eq!((calendar.rights, calendar.origin.id.as_str(), calendar.visible, calendar.color), (Rights::Read, "user:cy@example.org", true, picked));
    let groups = crate::cmd::calendar_settings::calendar_groups(&s);
    let shared = groups.iter().find(|g| g.title == "Shared Calendars").expect("a group of Shared Calendars");
    assert_eq!(shared.calendars, [calendar.id]);
    // Its name, colour and tick are kept on the owner's list too.
    let kept = &s.mailbox.account(account).unwrap().opened_calendars[0];
    assert_eq!((kept.name.as_str(), kept.ticked), ("Cy Colleague", true));
    // Changing what is shown, as for any calendar, is kept there as well after the next list.
    s.run("calendar.showCalendar", &json!({"id": calendar.id, "value": false})).unwrap();
    list(&mut s, account, vec![opened("cy@example.org", "Cy Colleague", Rights::Read)], vec![]);
    assert!(!s.mailbox.account(account).unwrap().opened_calendars[0].ticked);
    assert!(!named(&s, "Cy Colleague").unwrap().visible);
}

#[test]
fn a_refusal_adds_nothing_and_a_calendar_that_was_read_before_stays() {
    let (mut s, account) = served();
    open(&mut s, "gus@example.org").unwrap();
    let said = list(&mut s, account, vec![], vec![OpenedRefusal { address: "gus@example.org".into(), why: OpenedWhy::NoRights }]);
    assert_eq!(said["refused"][0]["removed"], true);
    assert_eq!(said["refused"][0]["why"], "noRights");
    assert!(s.mailbox.account(account).unwrap().opened_calendars.is_empty());
    assert!(named(&s, "gus@example.org").is_none());
    // Read once, then refused (the grant was taken back): what is here stays, and so does the entry.
    open(&mut s, "hal@example.org").unwrap();
    list(&mut s, account, vec![opened("hal@example.org", "Hal", Rights::Read)], vec![]);
    let said = list(&mut s, account, vec![], vec![OpenedRefusal { address: "hal@example.org".into(), why: OpenedWhy::NoRights }]);
    assert_eq!(said["refused"][0]["removed"], false);
    assert!(named(&s, "Hal").is_some());
    assert_eq!(s.mailbox.account(account).unwrap().opened_calendars.len(), 1);
    // A refusal about an address that isn't on the list does nothing.
    let said = list(&mut s, account, vec![], vec![OpenedRefusal { address: "stranger@example.org".into(), why: OpenedWhy::NoRights }]);
    assert_eq!(said["refused"].as_array().unwrap().len(), 0);
}

#[test]
fn a_list_that_leaves_an_opened_calendar_out_removes_nothing_and_a_servers_list_cannot_bring_one() {
    let (mut s, account) = served();
    open(&mut s, "cy@example.org").unwrap();
    list(&mut s, account, vec![opened("cy@example.org", "Cy", Rights::Edit)], vec![]);
    // (A complete list without it: the host couldn't read it this time.)
    let said = list(&mut s, account, vec![], vec![]);
    assert_eq!(said["removed"], 0);
    assert!(named(&s, "Cy").is_some());
    // An address the owner didn't open is not brought by a list, whoever says so.
    list(&mut s, account, vec![opened("victim@example.org", "Victim", Rights::Edit)], vec![]);
    assert!(named(&s, "Victim").is_none());
    // Nor one whose "address" isn't one.
    let bogus = RemoteCalendar { id: "user:a/../b@example.org".into(), name: "Bogus".into(), ..Default::default() };
    list(&mut s, account, vec![bogus], vec![]);
    assert!(named(&s, "Bogus").is_none());
}

#[test]
fn removing_one_takes_its_events_with_it_and_nothing_else() {
    let (mut s, account) = served();
    open(&mut s, "cy@example.org").unwrap();
    open(&mut s, "dee@example.org").unwrap();
    list(&mut s, account, vec![opened("cy@example.org", "Cy", Rights::Read), opened("dee@example.org", "Dee", Rights::Read)], vec![]);
    let cy = named(&s, "Cy").unwrap().id;
    // Events came down for each.
    for (address, title) in [("cy@example.org", "Cy's review"), ("dee@example.org", "Dee's lunch")] {
        let event = RemoteEvent { id: format!("e-{address}"), title: title.into(), start: NOW + 3_600, end: NOW + 7_200, ..Default::default() };
        let download = EventsDownload {
            account,
            email: OWNER.into(),
            calendar: opened_id(address),
            from: NOW,
            to: NOW + 86_400,
            complete: true,
            events: vec![event],
            ..Default::default()
        };
        remote::apply_events(&mut s, &download).unwrap();
    }
    let titles = |s: &Session| s.mailbox.events.values().filter(|e| e.origin.id.starts_with("e-")).map(|e| e.title.clone()).collect::<Vec<_>>();
    assert_eq!(titles(&s).len(), 2);
    let answer = s.run("calendar.removeShared", &json!({"id": cy})).unwrap();
    assert_eq!(answer["events"], 1);
    assert_eq!(titles(&s), ["Dee's lunch"]);
    assert_eq!(answer["removed"], "cy@example.org");
    assert!(named(&s, "Cy").is_none() && named(&s, "Dee").is_some());
    assert_eq!(s.mailbox.account(account).unwrap().opened_calendars.len(), 1);
    // By address, case apart.
    s.run("calendar.removeShared", &json!({"address": "DEE@example.org"})).unwrap();
    assert!(named(&s, "Dee").is_none());
    assert!(s.mailbox.account(account).unwrap().opened_calendars.is_empty());
    // And the account's own calendar is never one that can be removed this way.
    let own = s.mailbox.calendars.iter().find(|c| c.origin.account == account && c.origin.id.is_empty()).map(|c| c.id).unwrap();
    assert!(s.run("calendar.removeShared", &json!({"id": own})).is_err());
    assert!(s.run("calendar.removeShared", &json!({"address": "nobody@example.org"})).is_err());
}

#[test]
fn an_address_that_could_not_name_a_mailbox_is_refused_before_anything_is_kept() {
    let (mut s, account) = served();
    for hostile in [
        "",
        "plain",
        "a/../b@example.org",
        "a@example.org/../../me",
        "a b@example.org",
        "a@b@example.org",
        "\u{0}@example.org",
        "a%2f@example.org",
        "a?x@example.org",
        "é@example.org",
        "x@example.org\r\nBcc: y@example.org",
        &"x".repeat(100_000),
    ] {
        assert!(open(&mut s, hostile).is_err(), "{hostile:?}");
    }
    assert!(s.mailbox.account(account).unwrap().opened_calendars.is_empty());
    for params in [json!({}), json!({"address": 5}), json!({"address": null}), json!({"address": "a@example.org", "account": 99_999})] {
        assert!(s.run("calendar.openShared", &params).is_err(), "{params}");
    }
}

#[test]
fn only_a_work_or_school_account_kept_through_graph_can_open_one_and_not_for_itself() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    assert!(open(&mut s, "cy@example.org").is_err(), "no account yet");
    s.run("account.add", &json!({"email": "pw@example.org"})).unwrap();
    assert!(open(&mut s, "cy@example.org").is_err(), "an account with a password");
    let personal = s.run("account.add", &json!({"email": "me@outlook.com", "kind": "microsoft", "provider": "microsoft"})).unwrap();
    assert!(open(&mut s, "cy@example.org").is_err(), "a personal account");
    let id = personal["id"].as_u64().unwrap();
    assert!(s.run("calendar.openShared", &json!({"address": "cy@example.org", "account": id})).is_err());
    // An account that said it is personal, whatever its address.
    let (mut s, account) = served();
    if let Some(oauth) = s.mailbox.account_mut(account).unwrap().oauth.as_mut() {
        oauth.directory = Directory::Personal;
    }
    assert!(open(&mut s, "cy@example.org").is_err());
    // The account's own address, a shared mailbox's, one that is open already.
    let (mut s, account) = served();
    assert!(open(&mut s, "ADA@example.org").unwrap_err().contains("own address"));
    s.run("account.addSharedMailbox", &json!({"address": "support@example.org"})).unwrap();
    assert!(open(&mut s, "Support@example.org").is_err());
    open(&mut s, "cy@example.org").unwrap();
    assert!(open(&mut s, "CY@example.org").is_err());
    assert_eq!(s.mailbox.account(account).unwrap().opened_calendars.len(), 1);
}

#[test]
fn at_most_the_cap_are_opened() {
    let (mut s, account) = served();
    for n in 0..MAX_OPENED_CALENDARS {
        open(&mut s, &format!("p{n}@example.org")).unwrap();
    }
    assert!(open(&mut s, "one-more@example.org").is_err());
    assert_eq!(s.mailbox.account(account).unwrap().opened_calendars.len(), MAX_OPENED_CALENDARS);
}

#[test]
fn a_person_the_server_lists_too_is_noted_for_the_owner_to_decide() {
    let (mut s, account) = served();
    open(&mut s, "cy@example.org").unwrap();
    list(&mut s, account, vec![opened("cy@example.org", "Cy", Rights::Read)], vec![]);
    // Later the invitation is accepted in Outlook: the server lists Cy's calendar as well.
    let listed = RemoteCalendar {
        id: "AAMk-cy".into(),
        name: "Cy".into(),
        group: "Other Calendars".into(),
        rights: Rights::Read,
        owner: Some(CalendarOwner { address: "Cy@example.org".into(), name: "Cy".into() }),
        ..Default::default()
    };
    let said = list(&mut s, account, vec![opened("cy@example.org", "Cy", Rights::Read), listed], vec![]);
    let dups = said["duplicates"].as_array().unwrap();
    assert_eq!(dups.len(), 1);
    assert_eq!(dups[0]["address"], "cy@example.org");
    // Opening one the server lists already is refused with the reason.
    assert!(open(&mut s, "cy@example.org").is_err());
    // Removing the copy opened by address leaves the server's.
    s.run("calendar.removeShared", &json!({"address": "cy@example.org"})).unwrap();
    assert!(s.mailbox.calendars.iter().any(|c| c.origin.id == "AAMk-cy"));
    assert!(!s.mailbox.calendars.iter().any(|c| c.origin.id == "user:cy@example.org"));
}

#[test]
fn an_older_mailbox_and_a_saved_one_keep_the_list() {
    let (mut s, account) = served();
    open(&mut s, "cy@example.org").unwrap();
    list(&mut s, account, vec![opened("cy@example.org", "Cy", Rights::Edit)], vec![]);
    let text = serde_json::to_string(&s.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&text).unwrap();
    assert_eq!(back.account(account).unwrap().opened_calendars, s.mailbox.account(account).unwrap().opened_calendars);
    let calendar = back.calendars.iter().find(|c| c.name == "Cy").unwrap();
    assert_eq!((calendar.rights, calendar.origin.id.as_str()), (Rights::Edit, "user:cy@example.org"));
}

#[test]
fn an_agent_is_asked_first() {
    for id in ["calendar.openShared", "calendar.removeShared", "calendar.hideListedCopy"] {
        assert_eq!(access(id), Some(Access::Owner(Class::Accounts)), "{id}");
    }
    let (s, _) = served();
    let said = crate::agent::describe(&s, "calendar.openShared", &json!({"address": "cy@example.org"}));
    assert!(said.contains("cy@example.org"), "{said}");
    let said = crate::agent::describe(&s, "calendar.hideListedCopy", &json!({"address": "ops@example.org"}));
    assert!(said.starts_with("hide") && said.contains("ops@example.org"), "{said}");
    let said = crate::agent::describe(&s, "calendar.hideListedCopy", &json!({"address": "ops@example.org", "hide": false}));
    assert!(said.starts_with("show again"), "{said}");
}

const OPS: &str = "operations@example.org";

/// The copy of the shared mailbox's calendar the server lists among the account's own (an
/// invitation accepted in Outlook), named as the server names it.
fn ops_copy() -> RemoteCalendar {
    RemoteCalendar {
        id: "AAMk-ops".into(),
        name: "Example Operations".into(),
        group: "Other Calendars".into(),
        rights: Rights::Edit,
        owner: Some(CalendarOwner { address: "Operations@Example.org".into(), name: "Example Operations".into() }),
        ..Default::default()
    }
}

fn copy_event(s: &mut Session, account: Id) {
    let event = RemoteEvent { id: "e-ops".into(), title: "Ops stand-up".into(), start: NOW + 3_600, end: NOW + 7_200, ..Default::default() };
    let download = EventsDownload {
        account,
        email: OWNER.into(),
        calendar: "AAMk-ops".into(),
        from: NOW,
        to: NOW + 86_400,
        complete: true,
        events: vec![event],
        ..Default::default()
    };
    remote::apply_events(s, &download).unwrap();
}

fn has_copy(s: &Session) -> bool {
    s.mailbox.calendars.iter().any(|c| c.origin.id == "AAMk-ops")
}

#[test]
fn a_shared_mailboxs_calendar_listed_twice_is_offered_until_answered_and_hidden_stays_hidden() {
    let (mut s, account) = served();
    // Without the shared mailbox open here, it is somebody's calendar like any other.
    assert!(list(&mut s, account, vec![ops_copy()], vec![])["listedTwice"].as_array().unwrap().is_empty());
    assert!(s.run("calendar.hideListedCopy", &json!({"address": OPS})).is_err(), "no shared mailbox is open");
    let shared = s.run("account.addSharedMailbox", &json!({"address": OPS})).unwrap()["id"].as_u64().unwrap();
    let said = list(&mut s, account, vec![ops_copy()], vec![]);
    let twice = said["listedTwice"].as_array().unwrap();
    assert_eq!(twice.len(), 1);
    assert_eq!(
        (twice[0]["address"].as_str(), twice[0]["name"].as_str(), twice[0]["mailbox"].as_u64()),
        (Some(OPS), Some("Example Operations"), Some(shared))
    );
    // Not answered yet: said again with the next list.
    assert_eq!(list(&mut s, account, vec![ops_copy()], vec![])["listedTwice"].as_array().unwrap().len(), 1);
    copy_event(&mut s, account);
    assert!(s.mailbox.events.values().any(|e| e.title == "Ops stand-up"));

    // Hidden: the copy and its events go, the shared mailbox's own calendar stays, and an
    // address of another case is the same mailbox.
    let hid = s.run("calendar.hideListedCopy", &json!({"address": "OPERATIONS@example.org"})).unwrap();
    assert_eq!((hid["hidden"].as_bool(), hid["calendars"].as_u64(), hid["events"].as_u64()), (Some(true), Some(1), Some(1)));
    assert!(!has_copy(&s) && !s.mailbox.events.values().any(|e| e.title == "Ops stand-up"));
    assert!(s.mailbox.calendars.iter().any(|c| c.origin.account == shared), "the shared mailbox's own calendar stays");
    // The next list brings it no more, and doesn't ask again.
    let said = list(&mut s, account, vec![ops_copy()], vec![]);
    assert!(said["listedTwice"].as_array().unwrap().is_empty() && !has_copy(&s));
    // A save and a reload keep it hidden.
    let back = mailcraft_model::from_json(&serde_json::to_vec(&s.mailbox).unwrap(), 1_000_000).unwrap();
    assert_eq!(back.account(account).unwrap().listed_twice_answer(OPS), Some(true));

    // Undone, as another calendar setting is: the copy and its events are back.
    s.run("edit.undo", &json!({})).unwrap();
    assert!(has_copy(&s) && s.mailbox.events.values().any(|e| e.title == "Ops stand-up"));
    assert_eq!(s.mailbox.account(account).unwrap().listed_twice_answer(OPS), None);
    s.run("edit.redo", &json!({})).unwrap();
    assert!(!has_copy(&s));

    // Shown again: the next list brings it back, and asks nothing.
    let shown = s.run("calendar.hideListedCopy", &json!({"address": OPS, "hide": false})).unwrap();
    assert_eq!(shown["hidden"].as_bool(), Some(false));
    let said = list(&mut s, account, vec![ops_copy()], vec![]);
    assert!(has_copy(&s) && said["listedTwice"].as_array().unwrap().is_empty());
}

#[test]
fn keeping_both_is_remembered_and_closing_the_shared_mailbox_forgets_it() {
    let (mut s, account) = served();
    let shared = s.run("account.addSharedMailbox", &json!({"address": OPS})).unwrap()["id"].as_u64().unwrap();
    list(&mut s, account, vec![ops_copy()], vec![]);
    s.run("calendar.hideListedCopy", &json!({"address": OPS, "hide": false})).unwrap();
    let said = list(&mut s, account, vec![ops_copy()], vec![]);
    assert!(has_copy(&s) && said["listedTwice"].as_array().unwrap().is_empty(), "offered once");
    // Junk is refused before anything changes.
    for junk in
        [json!({}), json!({"address": 5}), json!({"address": OPS, "hide": "yes"}), json!({"address": "nobody"}), json!({"address": "cy@example.org"})]
    {
        assert!(s.run("calendar.hideListedCopy", &junk).is_err(), "{junk}");
    }
    // The owner's own account's address isn't a shared mailbox.
    assert!(s.run("calendar.hideListedCopy", &json!({"address": OWNER})).is_err());
    s.run("calendar.hideListedCopy", &json!({"address": OPS})).unwrap();
    // Closed: nothing is hidden for it any more, and opened again it is offered again.
    s.run("account.removeSharedMailbox", &json!({"id": shared})).unwrap();
    assert!(s.mailbox.account(account).unwrap().listed_twice.is_empty());
    list(&mut s, account, vec![ops_copy()], vec![]);
    assert!(has_copy(&s));
    s.run("account.addSharedMailbox", &json!({"address": OPS})).unwrap();
    assert_eq!(list(&mut s, account, vec![ops_copy()], vec![])["listedTwice"].as_array().unwrap().len(), 1);
}
