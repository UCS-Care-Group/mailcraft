// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars, address books and to-do lists over Exchange Web Services ([MS-OXWSCORE],
//! [MS-OXWSCAL], [MS-OXWSCONT], [MS-OXWSTASK]): `FindFolder`, `FindItem`, `CreateItem`,
//! `UpdateItem` and `DeleteItem`.
//!
//! The shapes are the service's own: a calendar item is a `t:CalendarItem`, a contact a
//! `t:Contact`, a to-do a `t:Task`. What is read goes into the model's own remote shapes
//! ([`RemoteEvent`], [`RemoteContact`], [`RemoteTask`]) so that the engine merges it exactly as a
//! Microsoft account's calendar, contacts and to-dos are merged.
//!
//! **What the server says is hostile.** Every text is cut as it is read, every list is capped,
//! every id is held to what an id may be, and a folder or item that can't be read is passed over
//! rather than taken for something else. Nothing here runs, fetches or follows anything.
//!
//! A folder is read in the mailbox the request names (another person's calendar shared with the
//! owner, or a shared mailbox's address book): `t:DistinguishedFolderId` carries a `t:Mailbox`
//! and every item is then read in that mailbox with the owner's own sign-in.

use mailcraft_model::remote::{RemoteContact, RemoteEvent, RemoteEventKind};
use mailcraft_model::remote_tasks::{RemoteTask, RemoteTaskList};
use mailcraft_model::{Address, Attendee, Freq, Importance, Labeled, PostalAddress, Recurrence, Response, ResponseOptions, ShowAs};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday};

use crate::error::Error;
use crate::folder::FolderInfo;
use crate::reply::Reply;
use crate::soap::{Element, MESSAGES, TYPES};
use crate::xml::Node;

/// The most items one page asks for.
pub const MAX_PAGE: u32 = 512;
/// The longest text kept in a field that is one line, in characters.
const MAX_LINE: usize = 500;
/// The longest description or note kept.
const MAX_TEXT: usize = 8_000;
const MAX_PEOPLE: usize = 500;
const MAX_ENTRIES: usize = 50;
const MAX_CATEGORIES: usize = 50;
const MAX_CATEGORY: usize = 255;

/// Which kind of item a folder holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Calendar items (appointments and meetings).
    Calendar,
    /// Contacts.
    Contacts,
    /// To-dos.
    Tasks,
}

impl Kind {
    /// The folder class the service gives a folder of this kind.
    pub fn class(self) -> &'static str {
        match self {
            Self::Calendar => "IPF.Appointment",
            Self::Contacts => "IPF.Contact",
            Self::Tasks => "IPF.Task",
        }
    }

    /// The distinguished name of the account's default folder of this kind.
    pub fn distinguished(self) -> crate::folder::DistinguishedFolder {
        match self {
            Self::Calendar => crate::folder::DistinguishedFolder::Calendar,
            Self::Contacts => crate::folder::DistinguishedFolder::Contacts,
            Self::Tasks => crate::folder::DistinguishedFolder::Tasks,
        }
    }
}

/// One item a `FindItem` brought.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// A calendar item.
    Event(Box<RemoteEvent>),
    /// A contact.
    Contact(Box<RemoteContact>),
    /// A to-do.
    Task(Box<RemoteTask>),
}

/// A page of a folder's items.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ItemsPage {
    /// The items of the page.
    pub items: Vec<Item>,
    /// This page reaches the end (`IncludesLastItemInRange`).
    pub last: bool,
}

impl std::fmt::Debug for ItemsPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemsPage").field("items", &self.items.len()).field("last", &self.last).finish()
    }
}

/// The request `FindItem` for a folder, with all of an item's properties.
#[derive(Clone, PartialEq, Eq)]
pub struct FindItems {
    kind: Kind,
    folder: String,
    /// A calendar stretch (from, up to), as instants: read as a `CalendarView`.
    view: Option<(Timestamp, Timestamp)>,
    offset: u32,
    max: u32,
    /// Ask for no properties beyond the shape's own: a server that answers a calendar view of
    /// another mailbox's folder with nothing when it is asked for added properties (Grommunio
    /// does) still gives the items.
    plain: bool,
}

impl std::fmt::Debug for FindItems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FindItems").field("kind", &self.kind).field("view", &self.view.is_some()).field("offset", &self.offset).finish()
    }
}

impl FindItems {
    /// Reads `max` items of `folder` (its server id) from `offset`.
    pub fn new(kind: Kind, folder: &str, offset: u32, max: u32) -> Self {
        Self { kind, folder: folder.to_owned(), view: None, offset, max: max.clamp(1, MAX_PAGE), plain: false }
    }

    /// Asks for nothing beyond `BaseShape` (for a folder of another mailbox, where a server may
    /// answer an added property with an empty page).
    #[must_use]
    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }

    /// Reads the calendar stretch `from` to `to` instead of a page: a `CalendarView` expands a
    /// recurring series into the occurrences that fall in it.
    #[must_use]
    pub fn view(mut self, from: Timestamp, to: Timestamp) -> Self {
        self.view = Some((from, to.max(from.saturating_add(1))));
        self.offset = 0;
        self
    }

    /// The operation as the schema has it.
    pub fn element(&self) -> Element {
        let mut shape = Element::new("m:ItemShape").child(Element::new("t:BaseShape").text("AllProperties"));
        // `AllProperties` does not bring everything the mailbox keeps (Grommunio's answer leaves
        // out the body, the people invited and a contact's places): ask for them by name.
        let fields: &[&str] = match self.kind {
            Kind::Calendar => &[
                "item:Body",
                "item:Categories",
                "item:ReminderIsSet",
                "item:ReminderMinutesBeforeStart",
                "item:Sensitivity",
                "item:HasAttachments",
                "item:LastModifiedTime",
                "calendar:RequiredAttendees",
                "calendar:OptionalAttendees",
                "calendar:StartTimeZone",
                "calendar:EndTimeZone",
                "calendar:OriginalStart",
                "calendar:Recurrence",
                "calendar:IsResponseRequested",
                "calendar:AllowNewTimeProposal",
                "calendar:MyResponseType",
                "calendar:IsCancelled",
            ],
            Kind::Contacts => &[
                "item:Body",
                "item:Categories",
                "item:LastModifiedTime",
                "contacts:PhysicalAddresses",
                "contacts:BusinessHomePage",
                "contacts:Birthday",
                "contacts:Department",
                "contacts:Nickname",
            ],
            Kind::Tasks => &[
                "item:Body",
                "item:Categories",
                "item:ReminderIsSet",
                "item:ReminderDueBy",
                "item:LastModifiedTime",
                "task:IsComplete",
                "task:CompleteDate",
            ],
        };
        if !fields.is_empty() && !self.plain {
            shape = shape.child(
                fields.iter().fold(Element::new("t:AdditionalProperties"), |list, uri| list.child(Element::new("t:FieldURI").attr("FieldURI", uri))),
            );
        }
        let page = match self.view {
            Some((from, to)) => Element::new("m:CalendarView")
                .attr("StartDate", &mailcraft_time::format_iso_utc(from))
                .attr("EndDate", &mailcraft_time::format_iso_utc(to))
                .attr("MaxEntriesReturned", &self.max.to_string()),
            None => Element::new("m:IndexedPageItemView")
                .attr("MaxEntriesReturned", &self.max.to_string())
                .attr("Offset", &self.offset.to_string())
                .attr("BasePoint", "Beginning"),
        };
        let folder = Element::new("t:FolderId").attr("Id", &self.folder);
        Element::new("m:FindItem").attr("Traversal", "Shallow").child(shape).child(page).child(Element::new("m:ParentFolderIds").child(folder))
    }

    /// The server's answer.
    pub fn answer(&self, reply: &Reply) -> Result<ItemsPage, Error> {
        let messages = reply.messages("FindItem")?;
        let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
        let node = message.ok()?;
        let root = node.child_in(MESSAGES, "RootFolder").ok_or(Error::Unexpected)?;
        let mut page = ItemsPage::default();
        if let Some(items) = root.child_in(TYPES, "Items") {
            for item in &items.children {
                if *item.namespace != *TYPES {
                    continue;
                }
                let read = match self.kind {
                    Kind::Calendar => read_calendar(item).map(|event| Item::Event(Box::new(event))),
                    Kind::Contacts => read_contact(item).map(|contact| Item::Contact(Box::new(contact))),
                    Kind::Tasks => read_task(item).map(|task| Item::Task(Box::new(task))),
                };
                if let Some(read) = read {
                    page.items.push(read);
                }
            }
        }
        page.last = root.attribute("IncludesLastItemInRange").is_none_or(|last| !last.trim().eq_ignore_ascii_case("false"));
        Ok(page)
    }
}

/// One page of a `FindFolder`, with the folders of this kind only.
pub fn folders_of_kind(page: &crate::folder::FoldersPage, kind: Kind) -> Vec<FolderInfo> {
    page.folders.iter().filter(|folder| folder.class.eq_ignore_ascii_case(kind.class())).cloned().collect()
}

/// `text` on one line, `most` characters at the most.
fn line(text: &str, most: usize) -> String {
    let mut out = String::new();
    for word in text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= most {
            break;
        }
    }
    out.chars().take(most).collect()
}

/// `text` with its control characters other than line breaks taken out and cut to `most`
/// characters: a description or a note keeps its lines.
fn text(text: &str, most: usize) -> String {
    text.chars().filter(|c| *c == '\n' || *c == '\t' || !c.is_control()).take(most).collect()
}

/// The text of a child element, trimmed.
fn child_text<'n>(node: &'n Node, name: &str) -> &'n str {
    node.child_in(TYPES, name).map_or("", |child| child.text.trim())
}

/// Whether a child element says true.
fn child_true(node: &Node, name: &str) -> bool {
    node.child_in(TYPES, name).is_some_and(|child| child.text.trim().eq_ignore_ascii_case("true"))
}

/// The first number a child element says.
fn child_number<T: std::str::FromStr>(node: &Node, name: &str) -> Option<T> {
    node.child_in(TYPES, name).and_then(|child| child.text.trim().parse().ok())
}

/// An instant a child element says.
fn child_time(node: &Node, name: &str) -> Option<Timestamp> {
    node.child_in(TYPES, name).and_then(|child| mailcraft_time::iso_to_timestamp(child.text.trim(), 0))
}

/// The day a date-time says, without its time or zone.
fn day_of(text: &str) -> Option<Date> {
    let text = text.trim();
    let mut parts = text.get(..10)?.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    Date::new(year, month, day)
}

/// A child's day.
fn child_day(node: &Node, name: &str) -> Option<Date> {
    node.child_in(TYPES, name).and_then(|child| day_of(&child.text))
}

/// The name a `Mailbox` element gives, and its address when it is an SMTP one.
fn mailbox_of(node: &Node) -> Option<Address> {
    let name = line(child_text(node, "Name"), 256);
    let routing = node.child_in(TYPES, "RoutingType").map_or("SMTP", |n| n.text.trim());
    let email = if routing.eq_ignore_ascii_case("SMTP") { line(child_text(node, "EmailAddress"), 320) } else { String::new() };
    (!name.is_empty() || !email.is_empty()).then_some(Address { name, email })
}

/// A `t:Mailbox` or `t:SingleRecipient` element's `Mailbox`.
fn recipient(node: &Node) -> Option<Address> {
    if node.is(TYPES, "Mailbox") { mailbox_of(node) } else { node.child_in(TYPES, "Mailbox").and_then(mailbox_of) }
}

/// The one person of `Organizer`.
fn organizer(node: &Node) -> Option<Address> {
    node.child_in(TYPES, "Organizer").and_then(recipient)
}

/// The people in a recipient list (`RequiredAttendees`), with whether each is optional and what
/// they answered.
fn attendees(node: &Node, name: &str, optional: bool) -> Vec<Attendee> {
    let Some(list) = node.child_in(TYPES, name) else { return Vec::new() };
    list.children
        .iter()
        .take(MAX_PEOPLE)
        .filter_map(|holder| {
            let address = recipient(holder)?;
            let response = match holder.child_in(TYPES, "ResponseType").map_or("", |n| n.text.trim()) {
                "Accept" | "Accepted" => Response::Accepted,
                "Tentative" => Response::Tentative,
                "Decline" | "Declined" => Response::Declined,
                _ => Response::None,
            };
            Some(Attendee { address, optional, response })
        })
        .collect()
}

/// The categories a `Categories` element names.
fn categories(node: &Node) -> Vec<String> {
    let Some(list) = node.child_in(TYPES, "Categories") else { return Vec::new() };
    list.children.iter().take(MAX_CATEGORIES).map(|child| line(&child.text, MAX_CATEGORY)).filter(|name| !name.is_empty()).collect()
}

/// The body of an item as plain text.
fn body_text(node: &Node) -> String {
    if let Some(text_body) = node.child_in(TYPES, "TextBody")
        && !text_body.text.trim().is_empty()
    {
        return text(&text_body.text, MAX_TEXT);
    }
    let Some(body) = node.child_in(TYPES, "Body") else { return String::new() };
    let is_html = body.attribute("BodyType").is_some_and(|kind| kind.eq_ignore_ascii_case("HTML"));
    if is_html {
        // (The text of an HTML body is not read here: the engine's calendar keeps plain text, and
        // a message's HTML is nobody's to run. What plain text the server kept is above.)
        String::new()
    } else {
        text(&body.text, MAX_TEXT)
    }
}

/// The importance an item says.
fn importance(node: &Node) -> Importance {
    match child_text(node, "Importance") {
        "Low" => Importance::Low,
        "High" => Importance::High,
        _ => Importance::Normal,
    }
}

/// The id and change key of an item.
fn item_id(node: &Node) -> Option<(String, String)> {
    let id = node.child_in(TYPES, "ItemId")?;
    let value = id.attribute("Id").filter(|id| crate::folder::valid_id(id))?.to_owned();
    let key = id.attribute("ChangeKey").filter(|key| crate::folder::valid_id(key)).unwrap_or_default().to_owned();
    Some((value, key))
}

/// A calendar item as a server has it.
fn read_calendar(node: &Node) -> Option<RemoteEvent> {
    if !node.is(TYPES, "CalendarItem") {
        return None;
    }
    let (id, change_key) = item_id(node)?;
    let all_day = child_true(node, "IsAllDayEvent");
    let start_text = node.child_in(TYPES, "Start").map(|n| n.text.trim()).unwrap_or("");
    let end_text = node.child_in(TYPES, "End").map(|n| n.text.trim()).unwrap_or("");
    let start_zone = node
        .child_in(TYPES, "StartTimeZone")
        .and_then(|zone| zone.attribute("Id"))
        .or_else(|| node.child_in(TYPES, "StartTimeZoneId").map(|n| n.text.trim()))
        .unwrap_or("")
        .to_owned();
    let end_zone = node
        .child_in(TYPES, "EndTimeZone")
        .and_then(|zone| zone.attribute("Id"))
        .or_else(|| node.child_in(TYPES, "EndTimeZoneId").map(|n| n.text.trim()))
        .unwrap_or("")
        .to_owned();
    let (start, end) = if all_day {
        let first = day_of(start_text)?;
        let after = day_of(end_text).unwrap_or(first.add_days(1));
        (DateTime::at_midnight(first).to_timestamp(0), DateTime::at_midnight(after).to_timestamp(0))
    } else {
        let start = mailcraft_time::iso_to_timestamp(start_text, 0)?;
        let end = mailcraft_time::iso_to_timestamp(end_text, 0).unwrap_or(start);
        (start, end.max(start))
    };
    let kind = match child_text(node, "CalendarItemType") {
        "Occurrence" => RemoteEventKind::Occurrence,
        "Exception" => RemoteEventKind::Exception,
        "RecurringMaster" => RemoteEventKind::SeriesMaster,
        _ => RemoteEventKind::Single,
    };
    let response = match child_text(node, "MyResponseType") {
        "Accept" | "Accepted" => Response::Accepted,
        "Tentative" => Response::Tentative,
        "Decline" | "Declined" => Response::Declined,
        _ => Response::None,
    };
    let show_as = match child_text(node, "LegacyFreeBusyStatus") {
        "Free" => ShowAs::Free,
        "Tentative" => ShowAs::Tentative,
        "OOF" => ShowAs::OutOfOffice,
        "WorkingElsewhere" => ShowAs::WorkingElsewhere,
        _ => ShowAs::Busy,
    };
    let reminder_minutes = if child_true(node, "ReminderIsSet") {
        child_number::<u32>(node, "ReminderMinutesBeforeStart").map(|minutes| minutes.min(40_320))
    } else {
        None
    };
    let recurrence = child_in_types(node, "Recurrence").and_then(read_recurrence);
    let mut event = RemoteEvent {
        id,
        change_key,
        uid: line(child_text(node, "UID"), 1_024),
        title: line(child_text(node, "Subject"), MAX_LINE),
        location: line(child_text(node, "Location"), MAX_LINE),
        description: body_text(node),
        start,
        end,
        all_day,
        start_zone: line(&start_zone, 64),
        end_zone: line(&end_zone, 64),
        organizer: organizer(node),
        attendees: attendees(node, "RequiredAttendees", false).into_iter().chain(attendees(node, "OptionalAttendees", true)).collect(),
        response,
        show_as,
        private: child_text(node, "Sensitivity") == "Private",
        cancelled: child_true(node, "IsCancelled"),
        kind,
        reminder_minutes,
        modified: child_time(node, "LastModifiedTime").unwrap_or(0),
        is_organizer: Some(child_text(node, "MyResponseType") == "Organizer"),
        recurrence,
        series_day: child_day(node, "Start"),
        original: child_time(node, "OriginalStart"),
        categories: categories(node),
        importance: importance(node),
        options: ResponseOptions {
            request_responses: node.child_in(TYPES, "IsResponseRequested").is_none_or(|n| n.text.trim() != "false"),
            allow_proposals: child_true(node, "AllowNewTimeProposal"),
        },
        attachments: child_true(node, "HasAttachments"),
        ..RemoteEvent::default()
    };
    if event.kind == RemoteEventKind::SeriesMaster {
        event.series_day = event.series_day.or_else(|| day_of(start_text));
    } else {
        event.series_day = None;
    }
    Some(event)
}

/// A child element in the types' namespace.
fn child_in_types<'n>(node: &'n Node, name: &str) -> Option<&'n Node> {
    node.child_in(TYPES, name)
}

/// The rule a `Recurrence` element says, when the mailbox can keep it.
fn read_recurrence(node: &Node) -> Option<Recurrence> {
    let pattern = node.children.first()?;
    let range = node.children.get(1);
    let interval = |p: &Node| p.child_in(TYPES, "Interval").and_then(|n| n.text.trim().parse::<u32>().ok()).unwrap_or(1).clamp(1, 999);
    let mut rule = Recurrence { interval: interval(pattern), ..Recurrence::default() };
    let mut named = false;
    match pattern.name.as_str() {
        "DailyRecurrence" => {
            rule.freq = Freq::Daily;
            named = true;
        }
        "WeeklyRecurrence" => {
            rule.freq = Freq::Weekly;
            named = true;
        }
        "AbsoluteMonthlyRecurrence" => {
            rule.freq = Freq::Monthly;
            named = true;
        }
        "RelativeMonthlyRecurrence" | "RelativeYearlyRecurrence" => {
            rule.freq = if pattern.name == "RelativeYearlyRecurrence" { Freq::Yearly } else { Freq::Monthly };
            rule.by_day = days_of_week(child_text(pattern, "DaysOfWeek"));
            rule.nth = Some(match child_text(pattern, "DayOfWeekIndex") {
                "First" => 1,
                "Second" => 2,
                "Third" => 3,
                "Fourth" => 4,
                "Last" => -1,
                _ => return None,
            });
            named = true;
        }
        _ => {}
    }
    if !named {
        return None;
    }
    if let Some(range) = range {
        match range.name.as_str() {
            "EndDateRecurrence" => rule.until = child_day(range, "EndDate"),
            "NumberedRecurrence" => {
                rule.count = range.child_in(TYPES, "NumberOfOccurrences").and_then(|n| n.text.trim().parse::<u32>().ok()).filter(|n| *n > 0)
            }
            _ => {}
        }
    }
    Some(rule)
}

/// The weekdays a `DaysOfWeek` text names (`Monday Wednesday`, or `Monday,Wednesday`).
fn days_of_week(text: &str) -> Vec<Weekday> {
    text.split([' ', ',', ';'])
        .filter_map(|day| match day.trim() {
            "Sunday" => Some(Weekday::Sun),
            "Monday" => Some(Weekday::Mon),
            "Tuesday" => Some(Weekday::Tue),
            "Wednesday" => Some(Weekday::Wed),
            "Thursday" => Some(Weekday::Thu),
            "Friday" => Some(Weekday::Fri),
            "Saturday" => Some(Weekday::Sat),
            _ => None,
        })
        .collect()
}

/// A contact as a server has it.
fn read_contact(node: &Node) -> Option<RemoteContact> {
    if !node.is(TYPES, "Contact") {
        return None;
    }
    let (id, change_key) = item_id(node)?;
    let labeled = |name: &str, key: &str, map: &dyn Fn(&str) -> String| -> Vec<Labeled> {
        let Some(dict) = node.child_in(TYPES, name) else { return Vec::new() };
        dict.children
            .iter()
            .take(MAX_ENTRIES)
            .filter_map(|entry| {
                let value = line(entry.text.trim(), MAX_LINE);
                if value.is_empty() {
                    return None;
                }
                let key = entry.attribute(key).unwrap_or("");
                Some(Labeled { label: map(key), value })
            })
            .collect()
    };
    let email_label = |key: &str| {
        match key {
            "EmailAddress1" => "work",
            "EmailAddress2" => "home",
            "EmailAddress3" => "other",
            _ => "other",
        }
        .to_string()
    };
    let phone_label = |key: &str| {
        match key {
            "BusinessPhone" | "BusinessPhone2" => "business",
            "HomePhone" | "HomePhone2" => "home",
            "MobilePhone" => "mobile",
            "BusinessFax" => "business fax",
            "HomeFax" => "home fax",
            "OtherFax" => "other fax",
            "Pager" => "pager",
            "AssistantPhone" => "assistant",
            "CompanyMainPhone" => "company",
            _ => "other",
        }
        .to_string()
    };
    let addresses = node
        .child_in(TYPES, "PhysicalAddresses")
        .map(|dict| {
            dict.children
                .iter()
                .take(MAX_ENTRIES)
                .map(|entry| PostalAddress {
                    label: match entry.attribute("Key").unwrap_or("") {
                        "Business" => "business",
                        "Home" => "home",
                        _ => "other",
                    }
                    .to_string(),
                    street: text(child_text(entry, "Street"), MAX_LINE),
                    city: line(child_text(entry, "City"), MAX_LINE),
                    region: line(child_text(entry, "State"), MAX_LINE),
                    postal_code: line(child_text(entry, "PostalCode"), MAX_LINE),
                    country: line(child_text(entry, "CountryOrRegion"), MAX_LINE),
                })
                .filter(|address| !address.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let websites: Vec<String> =
        node.child_in(TYPES, "BusinessHomePage").map(|n| line(&n.text, MAX_LINE)).filter(|site| !site.is_empty()).into_iter().collect();
    Some(RemoteContact {
        id,
        change_key,
        uid: String::new(),
        display_name: line(child_text(node, "DisplayName"), MAX_LINE),
        first: line(child_text(node, "GivenName"), MAX_LINE),
        middle: line(child_text(node, "MiddleName"), MAX_LINE),
        last: line(child_text(node, "Surname"), MAX_LINE),
        nickname: line(child_text(node, "Nickname"), MAX_LINE),
        company: line(child_text(node, "CompanyName"), MAX_LINE),
        title: line(child_text(node, "JobTitle"), MAX_LINE),
        department: line(child_text(node, "Department"), MAX_LINE),
        emails: labeled("EmailAddresses", "Key", &email_label),
        phones: labeled("PhoneNumbers", "Key", &phone_label),
        addresses,
        websites,
        birthday: child_day(node, "Birthday"),
        notes: body_text(node),
        modified: child_time(node, "LastModifiedTime").unwrap_or(0),
        ..RemoteContact::default()
    })
}

/// A to-do as a server has it.
fn read_task(node: &Node) -> Option<RemoteTask> {
    if !node.is(TYPES, "Task") {
        return None;
    }
    let (id, change_key) = item_id(node)?;
    let completed =
        if child_true(node, "IsComplete") || child_text(node, "Status") == "Completed" { child_time(node, "CompleteDate").or(Some(1)) } else { None };
    let reminder = child_true(node, "ReminderIsSet").then(|| child_time(node, "ReminderDueBy")).flatten();
    Some(RemoteTask {
        id,
        list: String::new(),
        change_key,
        title: line(child_text(node, "Subject"), MAX_LINE),
        notes: body_text(node),
        start: child_day(node, "StartDate"),
        due: child_day(node, "DueDate"),
        completed,
        importance: importance(node),
        categories: categories(node),
        reminder,
    })
}

/// The to-do lists a folder listing names.
pub fn task_lists(folders: &[FolderInfo]) -> Vec<RemoteTaskList> {
    folders
        .iter()
        .take(mailcraft_model::remote_tasks::MAX_TASK_LISTS)
        .map(|folder| RemoteTaskList { id: folder.id.as_str().to_owned(), complete: true, shared: false })
        .collect()
}

// ---- what goes up ----

/// What is sent as one item.
#[derive(Clone, PartialEq, Eq)]
pub enum ItemUp {
    /// A calendar item.
    Event(Box<RemoteEvent>),
    /// A contact.
    Contact(Box<RemoteContact>),
    /// A to-do.
    Task(Box<RemoteTask>),
}

impl std::fmt::Debug for ItemUp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Event(_) => "ItemUp::Event(..)",
            Self::Contact(_) => "ItemUp::Contact(..)",
            Self::Task(_) => "ItemUp::Task(..)",
        })
    }
}

/// The `t:CalendarItem` element of an event: the whole of it, for a new one or a change.
pub fn calendar_item(event: &RemoteEvent, zone: &str) -> Element {
    let mut item = Element::new("t:CalendarItem")
        .child(Element::new("t:Subject").text(&line(&event.title, MAX_LINE)))
        .child(Element::new("t:Body").attr("BodyType", "Text").text(&text(&event.description, MAX_TEXT)))
        .child(Element::new("t:Start").text(&mailcraft_time::format_iso_utc(event.start)))
        .child(Element::new("t:End").text(&mailcraft_time::format_iso_utc(event.end.max(event.start))))
        .child(Element::new("t:IsAllDayEvent").text(if event.all_day { "true" } else { "false" }))
        .child(Element::new("t:Location").text(&line(&event.location, MAX_LINE)));
    let show = match event.show_as {
        ShowAs::Free => "Free",
        ShowAs::Tentative => "Tentative",
        ShowAs::OutOfOffice => "OOF",
        ShowAs::WorkingElsewhere => "WorkingElsewhere",
        ShowAs::Busy => "Busy",
    };
    item = item.child(Element::new("t:LegacyFreeBusyStatus").text(show));
    if event.private {
        item = item.child(Element::new("t:Sensitivity").text("Private"));
    }
    let importance = match event.importance {
        Importance::Low => "Low",
        Importance::Normal => "Normal",
        Importance::High => "High",
    };
    item = item.child(Element::new("t:Importance").text(importance));
    if let Some(minutes) = event.reminder_minutes {
        item =
            item.child(Element::new("t:ReminderIsSet").text("true")).child(Element::new("t:ReminderMinutesBeforeStart").text(&minutes.to_string()));
    }
    if !event.categories.is_empty() {
        item = item.child(
            event
                .categories
                .iter()
                .take(MAX_CATEGORIES)
                .fold(Element::new("t:Categories"), |list, name| list.child(Element::new("t:String").text(&line(name, MAX_CATEGORY)))),
        );
    }
    item = item.child(Element::new("t:IsResponseRequested").text(if event.options.request_responses { "true" } else { "false" }));
    item = item.child(Element::new("t:AllowNewTimeProposal").text(if event.options.allow_proposals { "true" } else { "false" }));
    let required: Vec<&Attendee> = event.attendees.iter().filter(|a| !a.optional).collect();
    let optional: Vec<&Attendee> = event.attendees.iter().filter(|a| a.optional).collect();
    for (name, people) in [("t:RequiredAttendees", required), ("t:OptionalAttendees", optional)] {
        if people.iter().any(|a| !a.address.email.trim().is_empty()) {
            item = item.child(people.into_iter().fold(Element::new(name), |list, attendee| list.child(attendee_element(&attendee.address))));
        }
    }
    if !zone.trim().is_empty() {
        item = item.child(Element::new("t:StartTimeZone").attr("Id", zone.trim())).child(Element::new("t:EndTimeZone").attr("Id", zone.trim()));
    }
    if let Some(rule) = &event.recurrence {
        item = item.child(recurrence_element(rule, event));
    }
    item
}

fn attendee_element(address: &Address) -> Element {
    let mut mailbox = Element::new("t:Mailbox");
    if !address.name.trim().is_empty() {
        mailbox = mailbox.child(Element::new("t:Name").text(&line(&address.name, 256)));
    }
    mailbox = mailbox.child(Element::new("t:EmailAddress").text(address.email.trim())).child(Element::new("t:RoutingType").text("SMTP"));
    Element::new("t:Attendee").child(mailbox)
}

/// The `t:Recurrence` element of a rule, when the mailbox can say it.
fn recurrence_element(rule: &Recurrence, event: &RemoteEvent) -> Element {
    let start = if event.all_day {
        day_of(&mailcraft_time::format_iso_utc(event.start)).map(|day| DateTime::at_midnight(day).to_timestamp(0)).unwrap_or(event.start)
    } else {
        event.start
    };
    let interval = rule.interval.clamp(1, 999).to_string();
    let mut pattern = match rule.freq {
        Freq::Daily => Element::new("t:DailyRecurrence").child(Element::new("t:Interval").text(&interval)),
        Freq::Weekly => Element::new("t:WeeklyRecurrence")
            .child(Element::new("t:Interval").text(&interval))
            .child(Element::new("t:DaysOfWeek").text(&weekdays(rule, event))),
        Freq::Monthly if rule.nth.is_some() => Element::new("t:RelativeMonthlyRecurrence")
            .child(Element::new("t:Interval").text(&interval))
            .child(Element::new("t:DaysOfWeek").text(&weekdays(rule, event)))
            .child(Element::new("t:DayOfWeekIndex").text(nth_name(rule.nth.unwrap_or(1)))),
        Freq::Yearly if rule.nth.is_some() => Element::new("t:RelativeYearlyRecurrence")
            .child(Element::new("t:DaysOfWeek").text(&weekdays(rule, event)))
            .child(Element::new("t:DayOfWeekIndex").text(nth_name(rule.nth.unwrap_or(1))))
            .child(Element::new("t:Month").text(month_name(day_of(&mailcraft_time::format_iso_utc(event.start))))),
        Freq::Monthly => Element::new("t:AbsoluteMonthlyRecurrence")
            .child(Element::new("t:Interval").text(&interval))
            .child(Element::new("t:DayOfMonth").text(&day_of(&mailcraft_time::format_iso_utc(event.start)).map_or(1, |d| d.day).to_string())),
        Freq::Yearly => Element::new("t:AbsoluteYearlyRecurrence")
            .child(Element::new("t:Interval").text(&interval))
            .child(Element::new("t:DayOfMonth").text(&day_of(&mailcraft_time::format_iso_utc(event.start)).map_or(1, |d| d.day).to_string()))
            .child(Element::new("t:Month").text(month_name(day_of(&mailcraft_time::format_iso_utc(event.start))))),
    };
    if pattern.name().is_empty() {
        pattern = Element::new("t:DailyRecurrence").child(Element::new("t:Interval").text(&interval));
    }
    let range = if let Some(count) = rule.count {
        Element::new("t:NumberedRecurrence")
            .child(Element::new("t:StartDate").text(&mailcraft_time::format_iso_utc(start)))
            .child(Element::new("t:NumberOfOccurrences").text(&count.to_string()))
    } else if let Some(until) = rule.until {
        Element::new("t:EndDateRecurrence")
            .child(Element::new("t:StartDate").text(&mailcraft_time::format_iso_utc(start)))
            .child(Element::new("t:EndDate").text(&mailcraft_time::format_iso_utc(DateTime::at_midnight(until).to_timestamp(0))))
    } else {
        Element::new("t:NoEndRecurrence").child(Element::new("t:StartDate").text(&mailcraft_time::format_iso_utc(start)))
    };
    Element::new("t:Recurrence").child(pattern).child(range)
}

fn weekdays(rule: &Recurrence, event: &RemoteEvent) -> String {
    let mut days = rule.by_day.clone();
    if days.is_empty()
        && let Some(day) = day_of(&mailcraft_time::format_iso_utc(event.start))
    {
        days.push(day.weekday());
    }
    days.iter()
        .map(|day| match day {
            Weekday::Sun => "Sunday",
            Weekday::Mon => "Monday",
            Weekday::Tue => "Tuesday",
            Weekday::Wed => "Wednesday",
            Weekday::Thu => "Thursday",
            Weekday::Fri => "Friday",
            Weekday::Sat => "Saturday",
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn nth_name(nth: i8) -> &'static str {
    match nth {
        1 => "First",
        2 => "Second",
        3 => "Third",
        4 => "Fourth",
        -1 => "Last",
        _ => "First",
    }
}

fn month_name(day: Option<Date>) -> &'static str {
    match day.map_or(1, |d| d.month) {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        _ => "December",
    }
}

/// The `t:Contact` element of a contact.
pub fn contact_item(contact: &RemoteContact) -> Element {
    let mut item = Element::new("t:Contact")
        .child(Element::new("t:DisplayName").text(&line(&contact.display_name, MAX_LINE)))
        .child(Element::new("t:GivenName").text(&line(&contact.first, MAX_LINE)))
        .child(Element::new("t:MiddleName").text(&line(&contact.middle, MAX_LINE)))
        .child(Element::new("t:Surname").text(&line(&contact.last, MAX_LINE)))
        .child(Element::new("t:Nickname").text(&line(&contact.nickname, MAX_LINE)))
        .child(Element::new("t:CompanyName").text(&line(&contact.company, MAX_LINE)))
        .child(Element::new("t:JobTitle").text(&line(&contact.title, MAX_LINE)))
        .child(Element::new("t:Department").text(&line(&contact.department, MAX_LINE)));
    if !contact.notes.trim().is_empty() {
        item = item.child(Element::new("t:Body").attr("BodyType", "Text").text(&text(&contact.notes, MAX_TEXT)));
    }
    if !contact.emails.is_empty() {
        item = item.child(contact.emails.iter().take(3).enumerate().fold(Element::new("t:EmailAddresses"), |list, (n, entry)| {
            let key = match n {
                0 => "EmailAddress1",
                1 => "EmailAddress2",
                _ => "EmailAddress3",
            };
            list.child(Element::new("t:Entry").attr("Key", key).text(&line(&entry.value, MAX_LINE)))
        }));
    }
    if !contact.phones.is_empty() {
        item = item.child(contact.phones.iter().take(MAX_ENTRIES).fold(Element::new("t:PhoneNumbers"), |list, entry| {
            let key = match entry.label.to_ascii_lowercase().as_str() {
                "business" | "work" => "BusinessPhone",
                "home" => "HomePhone",
                "mobile" | "cell" => "MobilePhone",
                "business fax" => "BusinessFax",
                "home fax" => "HomeFax",
                "pager" => "Pager",
                "assistant" => "AssistantPhone",
                "company" => "CompanyMainPhone",
                _ => "OtherTelephone",
            };
            list.child(Element::new("t:Entry").attr("Key", key).text(&line(&entry.value, MAX_LINE)))
        }));
    }
    let with_addresses: Vec<_> = contact.addresses.iter().take(MAX_ENTRIES).collect();
    if !with_addresses.is_empty() {
        item = item.child(with_addresses.into_iter().fold(Element::new("t:PhysicalAddresses"), |list, address| {
            let key = match address.label.to_ascii_lowercase().as_str() {
                "business" | "work" => "Business",
                "home" => "Home",
                _ => "Other",
            };
            list.child(
                Element::new("t:Entry")
                    .attr("Key", key)
                    .child(Element::new("t:Street").text(&text(&address.street, MAX_LINE)))
                    .child(Element::new("t:City").text(&line(&address.city, MAX_LINE)))
                    .child(Element::new("t:State").text(&line(&address.region, MAX_LINE)))
                    .child(Element::new("t:PostalCode").text(&line(&address.postal_code, MAX_LINE)))
                    .child(Element::new("t:CountryOrRegion").text(&line(&address.country, MAX_LINE))),
            )
        }));
    }
    if let Some(site) = contact.websites.first().filter(|site| !site.trim().is_empty()) {
        item = item.child(Element::new("t:BusinessHomePage").text(&line(site, MAX_LINE)));
    }
    if let Some(day) = contact.birthday {
        item = item.child(Element::new("t:Birthday").text(&format!("{}T00:00:00Z", day.iso())));
    }
    item
}

/// The `t:Task` element of a to-do.
pub fn task_item(task: &RemoteTask) -> Element {
    let mut item = Element::new("t:Task")
        .child(Element::new("t:Subject").text(&line(&task.title, MAX_LINE)))
        .child(Element::new("t:Body").attr("BodyType", "Text").text(&text(&task.notes, MAX_TEXT)));
    if let Some(start) = task.start {
        item = item.child(Element::new("t:StartDate").text(&format!("{}T00:00:00Z", start.iso())));
    }
    if let Some(due) = task.due {
        item = item.child(Element::new("t:DueDate").text(&format!("{}T00:00:00Z", due.iso())));
    }
    match task.completed {
        Some(when) => {
            item = item
                .child(Element::new("t:Status").text("Completed"))
                .child(Element::new("t:PercentComplete").text("100"))
                .child(Element::new("t:IsComplete").text("true"))
                .child(Element::new("t:CompleteDate").text(&mailcraft_time::format_iso_utc(when)));
        }
        None => {
            item = item.child(Element::new("t:Status").text("NotStarted")).child(Element::new("t:PercentComplete").text("0"));
        }
    }
    let importance = match task.importance {
        Importance::Low => "Low",
        Importance::Normal => "Normal",
        Importance::High => "High",
    };
    item = item.child(Element::new("t:Importance").text(importance));
    if !task.categories.is_empty() {
        item = item.child(
            task.categories
                .iter()
                .take(MAX_CATEGORIES)
                .fold(Element::new("t:Categories"), |list, name| list.child(Element::new("t:String").text(&line(name, MAX_CATEGORY)))),
        );
    }
    if let Some(when) = task.reminder {
        item = item
            .child(Element::new("t:ReminderIsSet").text("true"))
            .child(Element::new("t:ReminderDueBy").text(&mailcraft_time::format_iso_utc(when)));
    }
    item
}

/// `CreateItem` with one item in `folder` (`None`: the server's default folder for its kind).
pub fn create_item(kind: Kind, folder: Option<&str>, item: Element, send: bool) -> Element {
    let mut operation = Element::new("m:CreateItem");
    if kind == Kind::Calendar {
        operation = operation.attr("SendMeetingInvitations", if send { "SendToAllAndSaveCopy" } else { "SendToNone" });
    }
    let folder = folder.map_or_else(
        || Element::new("m:SavedItemFolderId").child(Element::new("t:DistinguishedFolderId").attr("Id", default_folder(kind))),
        |id| Element::new("m:SavedItemFolderId").child(Element::new("t:FolderId").attr("Id", id)),
    );
    operation.child(folder).child(Element::new("m:Items").child(item))
}

/// The field URI a child element of an item is set through.
fn field_uri(kind: Kind, name: &str) -> Option<&'static str> {
    let uri = match (kind, name) {
        (_, "Subject") => "item:Subject",
        (_, "Body") => "item:Body",
        (_, "Importance") => "item:Importance",
        (_, "Categories") => "item:Categories",
        (_, "ReminderIsSet") => "item:ReminderIsSet",
        (Kind::Calendar, "Start") => "calendar:Start",
        (Kind::Calendar, "End") => "calendar:End",
        (Kind::Calendar, "IsAllDayEvent") => "calendar:IsAllDayEvent",
        (Kind::Calendar, "Location") => "calendar:Location",
        (Kind::Calendar, "LegacyFreeBusyStatus") => "calendar:LegacyFreeBusyStatus",
        (Kind::Calendar, "Sensitivity") => "item:Sensitivity",
        (Kind::Calendar, "ReminderMinutesBeforeStart") => "item:ReminderMinutesBeforeStart",
        (Kind::Calendar, "IsResponseRequested") => "calendar:IsResponseRequested",
        (Kind::Calendar, "AllowNewTimeProposal") => "calendar:AllowNewTimeProposal",
        (Kind::Calendar, "RequiredAttendees") => "calendar:RequiredAttendees",
        (Kind::Calendar, "OptionalAttendees") => "calendar:OptionalAttendees",
        (Kind::Calendar, "StartTimeZone") => "calendar:StartTimeZone",
        (Kind::Calendar, "EndTimeZone") => "calendar:EndTimeZone",
        (Kind::Calendar, "Recurrence") => "calendar:Recurrence",
        (Kind::Contacts, "DisplayName") => "contacts:DisplayName",
        (Kind::Contacts, "GivenName") => "contacts:GivenName",
        (Kind::Contacts, "MiddleName") => "contacts:MiddleName",
        (Kind::Contacts, "Surname") => "contacts:Surname",
        (Kind::Contacts, "Nickname") => "contacts:Nickname",
        (Kind::Contacts, "CompanyName") => "contacts:CompanyName",
        (Kind::Contacts, "JobTitle") => "contacts:JobTitle",
        (Kind::Contacts, "Department") => "contacts:Department",
        (Kind::Contacts, "EmailAddresses") => "contacts:EmailAddresses",
        (Kind::Contacts, "PhoneNumbers") => "contacts:PhoneNumbers",
        (Kind::Contacts, "PhysicalAddresses") => "contacts:PhysicalAddresses",
        (Kind::Contacts, "BusinessHomePage") => "contacts:BusinessHomePage",
        (Kind::Contacts, "Birthday") => "contacts:Birthday",
        (Kind::Tasks, "StartDate") => "task:StartDate",
        (Kind::Tasks, "DueDate") => "task:DueDate",
        (Kind::Tasks, "Status") => "task:Status",
        (Kind::Tasks, "PercentComplete") => "task:PercentComplete",
        (Kind::Tasks, "IsComplete") => "task:IsComplete",
        (Kind::Tasks, "CompleteDate") => "task:CompleteDate",
        (Kind::Tasks, "ReminderDueBy") => "item:ReminderDueBy",
        _ => return None,
    };
    Some(uri)
}

/// The fields that are cleared when an item's whole state doesn't name them.
fn clearable(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Calendar => &[
            "Sensitivity",
            "ReminderIsSet",
            "ReminderMinutesBeforeStart",
            "RequiredAttendees",
            "OptionalAttendees",
            "Recurrence",
            "StartTimeZone",
            "EndTimeZone",
        ],
        Kind::Contacts => &["EmailAddresses", "PhoneNumbers", "PhysicalAddresses", "BusinessHomePage", "Birthday"],
        Kind::Tasks => &["StartDate", "DueDate", "CompleteDate", "Categories", "ReminderIsSet", "ReminderDueBy"],
    }
}

/// `UpdateItem` of an item, with the whole of what the item is now set.
pub fn update_item(kind: Kind, item: &ItemUp, id: &str, change_key: &str, send: bool, zone: &str) -> Element {
    let mut operation = Element::new("m:UpdateItem");
    if kind == Kind::Calendar {
        operation = operation.attr("SendMeetingInvitationsOrCancellations", if send { "SendToAllAndSaveCopy" } else { "SendToNone" });
    }
    let item_id = match change_key.trim().is_empty() {
        true => Element::new("t:ItemId").attr("Id", id),
        false => Element::new("t:ItemId").attr("Id", id).attr("ChangeKey", change_key),
    };
    let holder = match kind {
        Kind::Calendar => "t:CalendarItem",
        Kind::Contacts => "t:Contact",
        Kind::Tasks => "t:Task",
    };
    let full = match item {
        ItemUp::Event(event) => calendar_item(event, zone),
        ItemUp::Contact(contact) => contact_item(contact),
        ItemUp::Task(task) => task_item(task),
    };
    let mut updates = Element::new("t:Updates");
    let mut named: Vec<&str> = Vec::new();
    for part in full.parts() {
        let Some(uri) = field_uri(kind, part.name().trim_start_matches("t:")) else { continue };
        named.push(part.name().trim_start_matches("t:"));
        updates = updates.child(
            Element::new("t:SetItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri)).child(Element::new(holder).child(part.clone())),
        );
    }
    for name in clearable(kind) {
        if named.contains(name) {
            continue;
        }
        if let Some(uri) = field_uri(kind, name) {
            updates = updates.child(Element::new("t:DeleteItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri)));
        }
    }
    let change = Element::new("t:ItemChange").child(item_id).child(updates);
    operation.attr("ConflictResolution", "AlwaysOverwrite").child(Element::new("m:ItemChanges").child(change))
}

fn default_folder(kind: Kind) -> &'static str {
    match kind {
        Kind::Calendar => "calendar",
        Kind::Contacts => "contacts",
        Kind::Tasks => "tasks",
    }
}

/// A `SetItemField` of the item's own type.
fn set_field(holder: &str, uri: &str, field: Element) -> Element {
    Element::new("t:SetItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri)).child(Element::new(holder).child(field))
}

/// A `DeleteItemField`.
fn delete_field(uri: &str) -> Element {
    Element::new("t:DeleteItemField").child(Element::new("t:FieldURI").attr("FieldURI", uri))
}

/// `UpdateItem` of an event, with only the fields the upload says (`None` leaves a field as the
/// server has it).
pub fn update_event(fields: &mailcraft_model::remote::EventFields, id: &str, change_key: &str, send: bool, zone: &str) -> Element {
    let mut updates = Element::new("t:Updates");
    let mut put = |field: Element| updates = std::mem::take(&mut updates).child(field);
    if let Some(title) = &fields.title {
        put(set_field("t:CalendarItem", "item:Subject", Element::new("t:Subject").text(&line(title, MAX_LINE))));
    }
    if let Some(location) = &fields.location {
        put(set_field("t:CalendarItem", "calendar:Location", Element::new("t:Location").text(&line(location, MAX_LINE))));
    }
    if let Some(description) = &fields.description {
        put(set_field("t:CalendarItem", "item:Body", Element::new("t:Body").attr("BodyType", "Text").text(&text(description, MAX_TEXT))));
    }
    if let Some(time) = fields.time {
        put(set_field("t:CalendarItem", "calendar:Start", Element::new("t:Start").text(&mailcraft_time::format_iso_utc(time.start))));
        put(set_field("t:CalendarItem", "calendar:End", Element::new("t:End").text(&mailcraft_time::format_iso_utc(time.end.max(time.start)))));
        put(set_field("t:CalendarItem", "calendar:IsAllDayEvent", Element::new("t:IsAllDayEvent").text(if time.all_day { "true" } else { "false" })));
        let zone = fields.zones.as_ref().map(|zones| zones.start.as_str()).filter(|zone| !zone.trim().is_empty()).unwrap_or(zone);
        if !zone.trim().is_empty() {
            put(set_field("t:CalendarItem", "calendar:StartTimeZone", Element::new("t:StartTimeZone").attr("Id", zone.trim())));
            put(set_field("t:CalendarItem", "calendar:EndTimeZone", Element::new("t:EndTimeZone").attr("Id", zone.trim())));
        }
    }
    if let Some(show_as) = fields.show_as {
        let show = match show_as {
            ShowAs::Free => "Free",
            ShowAs::Tentative => "Tentative",
            ShowAs::OutOfOffice => "OOF",
            ShowAs::WorkingElsewhere => "WorkingElsewhere",
            ShowAs::Busy => "Busy",
        };
        put(set_field("t:CalendarItem", "calendar:LegacyFreeBusyStatus", Element::new("t:LegacyFreeBusyStatus").text(show)));
    }
    if let Some(private) = fields.private {
        put(set_field("t:CalendarItem", "item:Sensitivity", Element::new("t:Sensitivity").text(if private { "Private" } else { "Normal" })));
    }
    if let Some(reminder) = &fields.reminder {
        if reminder.on {
            put(set_field("t:CalendarItem", "item:ReminderIsSet", Element::new("t:ReminderIsSet").text("true")));
            put(set_field(
                "t:CalendarItem",
                "item:ReminderMinutesBeforeStart",
                Element::new("t:ReminderMinutesBeforeStart").text(&reminder.minutes.min(40_320).to_string()),
            ));
        } else {
            put(delete_field("item:ReminderIsSet"));
            put(delete_field("item:ReminderMinutesBeforeStart"));
        }
    }
    if let Some(repeat) = &fields.repeat {
        match &repeat.rule {
            Some(rule) => {
                let (start, end, all_day) = fields.time.map_or((0, 0, false), |time| (time.start, time.end, time.all_day));
                let event = RemoteEvent { start, end, all_day, ..RemoteEvent::default() };
                put(set_field("t:CalendarItem", "calendar:Recurrence", recurrence_element(rule, &event)));
            }
            None => put(delete_field("calendar:Recurrence")),
        }
    }
    if let Some(people) = &fields.attendees {
        let required: Vec<&Attendee> = people.iter().filter(|a| !a.optional).collect();
        let optional: Vec<&Attendee> = people.iter().filter(|a| a.optional).collect();
        for (name, list) in [("t:RequiredAttendees", required), ("t:OptionalAttendees", optional)] {
            let field = if list.is_empty() {
                delete_field(if name == "t:RequiredAttendees" { "calendar:RequiredAttendees" } else { "calendar:OptionalAttendees" })
            } else {
                set_field(
                    "t:CalendarItem",
                    if name == "t:RequiredAttendees" { "calendar:RequiredAttendees" } else { "calendar:OptionalAttendees" },
                    list.into_iter().fold(Element::new(name), |list, attendee| list.child(attendee_element(&attendee.address))),
                )
            };
            put(field);
        }
    }
    if let Some(categories) = &fields.categories {
        if categories.is_empty() {
            put(delete_field("item:Categories"));
        } else {
            put(set_field(
                "t:CalendarItem",
                "item:Categories",
                categories
                    .iter()
                    .take(MAX_CATEGORIES)
                    .fold(Element::new("t:Categories"), |list, name| list.child(Element::new("t:String").text(&line(name, MAX_CATEGORY)))),
            ));
        }
    }
    if let Some(importance) = fields.importance {
        let name = match importance {
            Importance::Low => "Low",
            Importance::Normal => "Normal",
            Importance::High => "High",
        };
        put(set_field("t:CalendarItem", "item:Importance", Element::new("t:Importance").text(name)));
    }
    if let Some(request) = fields.request_responses {
        put(set_field(
            "t:CalendarItem",
            "calendar:IsResponseRequested",
            Element::new("t:IsResponseRequested").text(if request { "true" } else { "false" }),
        ));
    }
    if let Some(allow) = fields.allow_proposals {
        put(set_field(
            "t:CalendarItem",
            "calendar:AllowNewTimeProposal",
            Element::new("t:AllowNewTimeProposal").text(if allow { "true" } else { "false" }),
        ));
    }
    let item_id = match change_key.trim().is_empty() {
        true => Element::new("t:ItemId").attr("Id", id),
        false => Element::new("t:ItemId").attr("Id", id).attr("ChangeKey", change_key),
    };
    Element::new("m:UpdateItem")
        .attr("SendMeetingInvitationsOrCancellations", if send { "SendToAllAndSaveCopy" } else { "SendToNone" })
        .attr("ConflictResolution", "AlwaysOverwrite")
        .child(Element::new("m:ItemChanges").child(Element::new("t:ItemChange").child(item_id).child(updates)))
}

/// `UpdateItem` of a to-do, with only the fields its patch says.
pub fn update_task(patch: &mailcraft_model::TaskPatch, id: &str, change_key: &str) -> Element {
    let mut updates = Element::new("t:Updates");
    let mut put = |field: Element| updates = std::mem::take(&mut updates).child(field);
    if let Some(title) = &patch.title {
        put(set_field("t:Task", "item:Subject", Element::new("t:Subject").text(&line(title, MAX_LINE))));
    }
    if let Some(notes) = &patch.notes {
        put(set_field("t:Task", "item:Body", Element::new("t:Body").attr("BodyType", "Text").text(&text(notes, MAX_TEXT))));
    }
    for (field, name, uri) in [(patch.start, "t:StartDate", "task:StartDate"), (patch.due, "t:DueDate", "task:DueDate")] {
        match field {
            Some(Some(day)) => put(set_field("t:Task", uri, Element::new(name).text(&format!("{}T00:00:00Z", day.iso())))),
            Some(None) => put(delete_field(uri)),
            None => {}
        }
    }
    if let Some(completed) = patch.completed {
        match completed {
            Some(when) => {
                put(set_field("t:Task", "task:Status", Element::new("t:Status").text("Completed")));
                put(set_field("t:Task", "task:PercentComplete", Element::new("t:PercentComplete").text("100")));
                put(set_field("t:Task", "task:IsComplete", Element::new("t:IsComplete").text("true")));
                put(set_field("t:Task", "task:CompleteDate", Element::new("t:CompleteDate").text(&mailcraft_time::format_iso_utc(when))));
            }
            None => {
                put(set_field("t:Task", "task:Status", Element::new("t:Status").text("NotStarted")));
                put(set_field("t:Task", "task:PercentComplete", Element::new("t:PercentComplete").text("0")));
                put(set_field("t:Task", "task:IsComplete", Element::new("t:IsComplete").text("false")));
                put(delete_field("task:CompleteDate"));
            }
        }
    }
    if let Some(importance) = patch.importance {
        let name = match importance {
            Importance::Low => "Low",
            Importance::Normal => "Normal",
            Importance::High => "High",
        };
        put(set_field("t:Task", "item:Importance", Element::new("t:Importance").text(name)));
    }
    if let Some(categories) = &patch.categories {
        if categories.is_empty() {
            put(delete_field("item:Categories"));
        } else {
            put(set_field(
                "t:Task",
                "item:Categories",
                categories
                    .iter()
                    .take(MAX_CATEGORIES)
                    .fold(Element::new("t:Categories"), |list, name| list.child(Element::new("t:String").text(&line(name, MAX_CATEGORY)))),
            ));
        }
    }
    if let Some(reminder) = patch.reminder {
        match reminder {
            Some(when) => {
                put(set_field("t:Task", "item:ReminderIsSet", Element::new("t:ReminderIsSet").text("true")));
                put(set_field("t:Task", "item:ReminderDueBy", Element::new("t:ReminderDueBy").text(&mailcraft_time::format_iso_utc(when))));
            }
            None => {
                put(delete_field("item:ReminderIsSet"));
                put(delete_field("item:ReminderDueBy"));
            }
        }
    }
    let item_id = match change_key.trim().is_empty() {
        true => Element::new("t:ItemId").attr("Id", id),
        false => Element::new("t:ItemId").attr("Id", id).attr("ChangeKey", change_key),
    };
    Element::new("m:UpdateItem")
        .attr("ConflictResolution", "AlwaysOverwrite")
        .child(Element::new("m:ItemChanges").child(Element::new("t:ItemChange").child(item_id).child(updates)))
}

/// `DeleteItem` of an item: to Deleted Items, or (a calendar item) with a cancellation.
pub fn delete_item(kind: Kind, id: &str, change_key: &str, send: bool, soft: bool) -> Element {
    let delete_type = if soft { "SoftDelete" } else { "MoveToDeletedItems" };
    let mut operation = Element::new("m:DeleteItem").attr("DeleteType", delete_type);
    if kind == Kind::Calendar {
        operation = operation.attr("SendMeetingCancellations", if send { "SendToAllAndSaveCopy" } else { "SendToNone" });
    }
    let item_id = match change_key.trim().is_empty() {
        true => Element::new("t:ItemId").attr("Id", id),
        false => Element::new("t:ItemId").attr("Id", id).attr("ChangeKey", change_key),
    };
    operation.child(Element::new("m:ItemIds").child(item_id))
}

/// The id a `CreateItem`/`UpdateItem` answer gives the item.
pub fn saved(reply: &Reply, operation: &str) -> Result<(String, String), Error> {
    let messages = reply.messages(operation)?;
    let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
    let node = message.ok()?;
    let items = node.child_in(MESSAGES, "Items").ok_or(Error::Unexpected)?;
    let item = items.children.iter().find(|c| *c.namespace == *TYPES).ok_or(Error::Unexpected)?;
    item_id(item).ok_or(Error::Unexpected)
}

/// Whether a `DeleteItem` answer says it was done.
pub fn deleted(reply: &Reply) -> Result<(), Error> {
    let messages = reply.messages("DeleteItem")?;
    let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
    message.ok().map(|_| ())
}

/// The response the owner makes to somebody else's invitation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Accept it.
    Accept,
    /// Accept it tentatively.
    Tentative,
    /// Decline it.
    Decline,
}

/// `CreateItem` with an `AcceptItem`/`TentativeItem`/`DeclineItem` for a meeting the owner was
/// invited to, with the note that goes to its organizer.
pub fn meeting_answer(answer: Answer, id: &str, comment: &str) -> Element {
    let name = match answer {
        Answer::Accept => "AcceptItem",
        Answer::Tentative => "TentativeItem",
        Answer::Decline => "DeclineItem",
    };
    let mut item = Element::new(&format!("t:{name}")).child(Element::new("t:ReferenceItemId").attr("Id", id));
    if !comment.trim().is_empty() {
        item = item.child(Element::new("t:Body").attr("BodyType", "Text").text(&text(comment, 2_000)));
    }
    Element::new("m:CreateItem").child(Element::new("m:Items").child(item))
}

/// The id a response to an invitation was saved under (some servers answer with none).
pub fn answered(reply: &Reply) -> Result<(), Error> {
    let messages = reply.messages("CreateItem")?;
    let [message] = messages.as_slice() else { return Err(Error::Unexpected) };
    message.ok().map(|_| ())
}

#[cfg(test)]
#[path = "directory_tests.rs"]
mod tests;
