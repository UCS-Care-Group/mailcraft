// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the directory operations read: the shapes of `FindItem`, `FindFolder` and the three kinds
//! of item, held to what the model keeps.

use mailcraft_model::remote::RemoteEventKind;

use super::*;
use crate::reply::Reply;
use crate::soap::{MESSAGES, SOAP, TYPES};

fn answer(operation: &str, message: &str) -> Reply {
    let body = format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"{SOAP}\" xmlns:m=\"{MESSAGES}\" xmlns:t=\"{TYPES}\"><s:Body><m:{operation}Response>\
         <m:ResponseMessages><m:{operation}ResponseMessage ResponseClass=\"Success\"><m:ResponseCode>NoError</m:ResponseCode>{message}\
         </m:{operation}ResponseMessage></m:ResponseMessages></m:{operation}Response></s:Body></s:Envelope>"
    );
    Reply::parse(body.as_bytes()).unwrap()
}

fn find_items(items: &str) -> ItemsPage {
    let reply = answer(
        "FindItem",
        &format!("<m:RootFolder IncludesLastItemInRange=\"true\" TotalItemsInView=\"1\"><t:Items>{items}</t:Items></m:RootFolder>"),
    );
    FindItems::new(Kind::Calendar, "folder", 0, 100).answer(&reply).unwrap()
}

#[test]
fn a_calendar_item_is_read_as_an_event() {
    let page = find_items(
        "<t:CalendarItem><t:ItemId Id=\"event-1\" ChangeKey=\"key-1\"/><t:Subject>Budget review</t:Subject>\
         <t:Body BodyType=\"Text\">Numbers</t:Body><t:Start>2026-10-12T13:00:00Z</t:Start><t:End>2026-10-12T14:00:00Z</t:End>\
         <t:IsAllDayEvent>false</t:IsAllDayEvent><t:Location>Room 1</t:Location><t:LegacyFreeBusyStatus>Busy</t:LegacyFreeBusyStatus>\
         <t:IsResponseRequested>true</t:IsResponseRequested><t:AllowNewTimeProposal>true</t:AllowNewTimeProposal>\
         <t:CalendarItemType>Single</t:CalendarItemType><t:UID>uid-1</t:UID><t:Categories><t:String>Work</t:String></t:Categories>\
         <t:RequiredAttendees><t:Attendee><t:Mailbox><t:Name>Cy</t:Name><t:EmailAddress>cy@example.org</t:EmailAddress><t:RoutingType>SMTP</t:RoutingType></t:Mailbox><t:ResponseType>Accept</t:ResponseType></t:Attendee></t:RequiredAttendees>\
         </t:CalendarItem>",
    );
    assert!(page.last);
    let Item::Event(event) = &page.items[0] else { panic!("not an event") };
    assert_eq!(event.id, "event-1");
    assert_eq!(event.title, "Budget review");
    assert_eq!(event.start, mailcraft_time::iso_to_timestamp("2026-10-12T13:00:00Z", 0).unwrap());
    assert_eq!(event.end, mailcraft_time::iso_to_timestamp("2026-10-12T14:00:00Z", 0).unwrap());
    assert_eq!(event.organizer, None);
    assert_eq!(event.attendees.len(), 1);
    assert_eq!(event.attendees[0].address.email, "cy@example.org");
    assert_eq!(event.categories, ["Work"]);
    assert_eq!(event.kind, RemoteEventKind::Single);
}

#[test]
fn a_contact_and_a_to_do_are_read() {
    let reply = answer(
        "FindItem",
        "<m:RootFolder><t:Items><t:Contact><t:ItemId Id=\"c-1\" ChangeKey=\"k\"/><t:DisplayName>Grace Hopper</t:DisplayName>\
         <t:GivenName>Grace</t:GivenName><t:Surname>Hopper</t:Surname><t:CompanyName>Navy</t:CompanyName>\
         <t:EmailAddresses><t:Entry Key=\"EmailAddress1\">grace@example.org</t:Entry></t:EmailAddresses>\
         <t:PhoneNumbers><t:Entry Key=\"MobilePhone\">+1 555 0100</t:Entry></t:PhoneNumbers>\
         <t:PhysicalAddresses><t:Entry Key=\"Business\"><t:Street>1 Navy Way</t:Street><t:City>Arlington</t:City></t:Entry></t:PhysicalAddresses>\
         </t:Contact></t:Items></m:RootFolder>",
    );
    let page = FindItems::new(Kind::Contacts, "folder", 0, 100).answer(&reply).unwrap();
    let Item::Contact(contact) = &page.items[0] else { panic!("not a contact") };
    assert_eq!(contact.display_name, "Grace Hopper");
    assert_eq!(contact.emails[0].value, "grace@example.org");
    assert_eq!(contact.phones[0].label, "mobile");
    assert_eq!(contact.addresses[0].city, "Arlington");

    let reply = answer(
        "FindItem",
        "<m:RootFolder><t:Items><t:Task><t:ItemId Id=\"t-1\" ChangeKey=\"k\"/><t:Subject>Send the report</t:Subject>\
         <t:StartDate>2026-10-01T00:00:00Z</t:StartDate><t:DueDate>2026-10-03T00:00:00Z</t:DueDate>\
         <t:Status>Completed</t:Status><t:CompleteDate>2026-10-02T12:00:00Z</t:CompleteDate><t:Importance>High</t:Importance>\
         </t:Task></t:Items></m:RootFolder>",
    );
    let page = FindItems::new(Kind::Tasks, "folder", 0, 100).answer(&reply).unwrap();
    let Item::Task(task) = &page.items[0] else { panic!("not a to-do") };
    assert_eq!(task.title, "Send the report");
    assert!(task.completed.is_some());
    assert_eq!(task.importance, mailcraft_model::Importance::High);
}

#[test]
fn a_folder_listing_keeps_its_class_and_rights() {
    let reply = answer(
        "FindFolder",
        "<m:RootFolder IncludesLastItemInRange=\"true\"><t:Folders><t:CalendarFolder><t:FolderId Id=\"cal-2\"/><t:DisplayName>Team</t:DisplayName>\
         <t:FolderClass>IPF.Appointment</t:FolderClass><t:EffectiveRights><t:Read>true</t:Read><t:Modify>false</t:Modify></t:EffectiveRights>\
         </t:CalendarFolder></t:Folders></m:RootFolder>",
    );
    let page = crate::folder::FindFolder::new(crate::folder::DistinguishedFolder::MessageRoot, true, 0, 100).answer(&reply).unwrap();
    assert!(page.last);
    assert_eq!(page.folders[0].display_name, "Team");
    assert_eq!(page.folders[0].class, "IPF.Appointment");
    assert_eq!(page.folders[0].rights, mailcraft_model::Rights::Read);
}

#[test]
fn an_empty_answer_has_no_items() {
    let page = find_items("");
    assert!(page.items.is_empty());
    assert!(page.last);
}
