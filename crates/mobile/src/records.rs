// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The records the typed queries answer with (not the display blocks: `blocks.rs`). Each answer
//! names the revision it was read at (`rev`), so the app can tell a stale window from a current
//! one when [`crate::Event::RevChanged`] arrives. Times are seconds since 1970 (UTC) where they
//! are instants, and `YYYY-MM-DDTHH:MM:SS` or `YYYY-MM-DD` in the owner's zone where they are
//! calendar times.

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AccountRecord {
    pub id: u64,
    pub name: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FolderRecord {
    pub id: u64,
    pub account: u64,
    pub parent: Option<u64>,
    pub name: String,
    /// `inbox`, `drafts`, `sent`, `deleted`, `junk`, `archive`, `outbox` or `custom`.
    pub kind: String,
    pub favorite: bool,
    pub total: u32,
    pub unread: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FolderList {
    pub rev: u64,
    pub accounts: Vec<AccountRecord>,
    pub folders: Vec<FolderRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MessageRow {
    /// The message the row shows (the newest of its conversation, when conversations are on).
    pub id: u64,
    pub account: u64,
    pub folder: u64,
    pub from_name: String,
    pub from_address: String,
    pub subject: String,
    pub preview: String,
    /// When it arrived (or was sent), seconds since 1970 UTC.
    pub when: i64,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    pub important: bool,
    pub answered: bool,
    pub forwarded: bool,
    pub draft: bool,
    pub pinned: bool,
    pub categories: Vec<String>,
    /// Messages the row stands for (1 outside a conversation).
    pub members: u32,
    /// The heading above the row when it starts a group (`Today`, `Last Week`), in English: the
    /// app translates it with `t`, unless `group_is_category` says it is a name the owner chose.
    pub group: Option<String>,
    /// Whether `group` is one of the owner's categories (shown as spelt, never translated).
    pub group_is_category: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MessageWindow {
    pub rev: u64,
    pub folder: u64,
    /// How many rows the whole list has.
    pub total: u32,
    pub offset: u32,
    pub rows: Vec<MessageRow>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EventRecord {
    pub event: u64,
    pub calendar: u64,
    /// The calendar's colour, `0xRRGGBB`.
    pub color: u32,
    pub title: String,
    pub location: String,
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub repeats: bool,
    /// `free`, `tentative`, `busy`, `outOfOffice` or `workingElsewhere`.
    pub show_as: String,
    pub meeting: bool,
    pub cancelled: bool,
    pub categories: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarRange {
    pub rev: u64,
    pub from: String,
    pub to: String,
    pub events: Vec<EventRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PersonRecord {
    pub id: u64,
    pub name: String,
    pub company: String,
    pub title: String,
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub favorite: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PeopleWindow {
    pub rev: u64,
    pub total: u32,
    pub offset: u32,
    pub people: Vec<PersonRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskListRecord {
    /// The account whose To Do keeps the list; 0 for tasks that live on this phone only.
    pub account: u64,
    /// The list's id on the server (empty for the phone's own).
    pub list: String,
    pub open: u32,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskLists {
    pub rev: u64,
    pub lists: Vec<TaskListRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskRecord {
    pub id: u64,
    pub title: String,
    pub due: Option<String>,
    pub start: Option<String>,
    pub completed: bool,
    pub important: bool,
    pub account: u64,
    pub list: String,
    pub categories: Vec<String>,
    /// The message the task stands for, when it was made from one.
    pub message: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskWindow {
    pub rev: u64,
    pub total: u32,
    pub offset: u32,
    pub tasks: Vec<TaskRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TodayRecord {
    pub rev: u64,
    /// The owner's today, `YYYY-MM-DD`.
    pub date: String,
    pub events: Vec<EventRecord>,
    /// Tasks not completed that are due today or were due before.
    pub tasks: Vec<TaskRecord>,
    /// Unread messages in the Inboxes of all accounts.
    pub unread: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AddressRecord {
    pub name: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttachmentRecord {
    /// Its index among the message's attachments.
    pub index: u32,
    pub name: String,
    pub mime: String,
    pub size: u64,
    /// Shown inside the body (a picture of it) rather than listed.
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PictureBytes {
    pub rev: u64,
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`, told from the bytes themselves.
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MessageView {
    pub rev: u64,
    pub id: u64,
    pub account: u64,
    pub folder: u64,
    pub from: AddressRecord,
    pub to: Vec<AddressRecord>,
    pub cc: Vec<AddressRecord>,
    pub subject: String,
    pub when: i64,
    pub unread: bool,
    pub flagged: bool,
    pub categories: Vec<String>,
    pub attachments: Vec<AttachmentRecord>,
    /// The body, read for the width and theme asked for (its `@media` rules tested against them).
    pub blocks: Vec<crate::blocks::BlockView>,
    /// The window is dark and the message brings its own design for dark windows.
    pub dark_design: bool,
    /// Pictures on the web the message asks for (none is fetched): what the banner offering to
    /// download them counts.
    pub remote_pictures: u32,
}
