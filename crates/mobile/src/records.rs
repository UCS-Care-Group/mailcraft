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
    /// `microsoft`, `imap`, `local`, `jmap`, `exchange` or `other`. The phone app v1 shows
    /// `microsoft` accounts only.
    pub kind: String,
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
    /// The ids of those messages (the first 200 at most), newest first, so that a command on the row
    /// names them and never depends on the session's own view of the list.
    pub member_ids: Vec<u64>,
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
    /// Somebody else organizes it and the owner is invited: it can be answered.
    pub invited: bool,
    /// The owner's answer to it: `none`, `accepted`, `tentative` or `declined`.
    pub answer: String,
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
    /// What has not ended yet, from now through the next seven days (at most ten, none
    /// cancelled), in order: the card of next events.
    pub next_events: Vec<EventRecord>,
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
    /// The whole message is on the phone. When not, only its headers and preview are (the body
    /// shows the preview): `Core::open_message` fetches it.
    pub whole: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarRecord {
    pub id: u64,
    /// The account whose server keeps it; 0 for a calendar on this phone only.
    pub account: u64,
    pub name: String,
    /// `0xRRGGBB`.
    pub color: u32,
    pub visible: bool,
    /// Events in it can be made and changed here.
    pub editable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CalendarList {
    pub rev: u64,
    pub calendars: Vec<CalendarRecord>,
    /// The calendar new events go into (0 when there is none).
    pub current: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AttendeeRecord {
    pub name: String,
    pub address: String,
    pub optional: bool,
    /// `none`, `accepted`, `tentative` or `declined`.
    pub response: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RecurrenceRecord {
    /// `daily`, `weekly`, `monthly` or `yearly`.
    pub freq: String,
    pub interval: u32,
    /// `mon` to `sun`.
    pub by_day: Vec<String>,
    pub nth: Option<i8>,
    pub count: Option<u32>,
    /// `YYYY-MM-DD`.
    pub until: Option<String>,
}

/// One event whole, as the editor and the detail page need it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EventDetail {
    pub rev: u64,
    pub event: u64,
    pub calendar: u64,
    pub title: String,
    pub location: String,
    pub description: String,
    /// `YYYY-MM-DDTHH:MM:SS`, or for a whole-day event `YYYY-MM-DD` (the last day included).
    pub start: String,
    pub end: String,
    pub all_day: bool,
    pub organizer: Option<AddressRecord>,
    pub attendees: Vec<AttendeeRecord>,
    /// `attendees` is not the whole list (more people than are handed over, or an address cut to
    /// size): the editor must not send it back as the list, which would drop whoever is missing.
    pub people_cut: bool,
    pub recurrence: Option<RecurrenceRecord>,
    pub reminder_minutes: Option<u32>,
    pub show_as: String,
    pub meeting: bool,
    pub invited: bool,
    pub answer: String,
    pub cancelled: bool,
    pub editable: bool,
    /// An account's server keeps it: a change to a meeting with people invited has to say
    /// whether they are written to (`send`).
    pub on_server: bool,
}

/// One event whole with the start and end of one of its times, both read in one look at the
/// calendar, so the times and the series they belong to always agree.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct EventOccurrence {
    pub detail: EventDetail,
    /// `YYYY-MM-DDTHH:MM:SS` of the time asked for, as the range gives it; `None` when the event
    /// has no time on that day (it was moved or deleted since the row was read).
    pub start: Option<String>,
    pub end: Option<String>,
}

/// What a window of the message list is asked for: where, how filtered, in which of the Inbox
/// split's tabs, with or without conversations, and under which search.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ListQuery {
    pub folder: u64,
    /// `all`, `unread`, `flagged`, `attachments` or `toMe`.
    pub filter: String,
    /// The code of one of the Inbox split's tabs (`primary`, `social`…), from [`InboxSplit`];
    /// empty for the whole folder.
    pub split: String,
    /// One row for each conversation (its newest message), instead of one for each message.
    pub conversations: bool,
    /// Text to find in what is on this phone (sender, subject and preview); empty for none.
    pub search: String,
    /// A search looks through every account's mail, not the folder alone.
    pub everywhere: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SplitTab {
    /// `primary`, `social`, `promotions`, `updates` or `forums`.
    pub code: String,
    /// Its name in English: the app translates it with `t`.
    pub name: String,
    pub unread: u32,
    pub total: u32,
}

/// The row of tabs over an Inbox that its server divides (Gmail's categories): empty `tabs` for
/// any other folder, and for an Inbox that has nothing to sort.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct InboxSplit {
    pub rev: u64,
    pub folder: u64,
    pub tabs: Vec<SplitTab>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CategoryRecord {
    pub name: String,
    /// The category's colour, `0xRRGGBB`.
    pub color: u32,
}

/// The categories the mail of an account can be given, with their colours.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CategoryList {
    pub rev: u64,
    pub categories: Vec<CategoryRecord>,
}

/// A picture downloaded from the web: its kind told from its own bytes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct FetchedPicture {
    /// `image/png`, `image/jpeg`, `image/gif` or `image/webp`; `image/x-icon` for a sender's picture only.
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What the owner's rules for a sender's picture need to know about a message, from the mailbox
/// and the server's word and nothing the sender wrote: who sent it, whether it is in Junk E-mail
/// (or under it), whether the receiving server vouches for the sender (DMARC), and whether the
/// owner knows the sender (a contact, or somebody mail in Sent Items went to).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SenderFacts {
    pub address: String,
    pub junk: bool,
    pub vouched: bool,
    pub known: bool,
}

/// What a link is and where it goes, as the owner is shown it and as it is launched.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LinkTarget {
    pub kind: LinkKind,
    /// The address written as it is shown and launched: for a web address, the host in lower case
    /// with international labels as Punycode, so that the host shown is the host reached.
    pub shown: String,
}

/// The only kinds of link that are followed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LinkKind {
    /// `http` or `https`: to the browser.
    Web,
    /// `mailto`: to the composer.
    Mail,
    /// `tel`: to the dialer.
    Phone,
    /// `cid`: a part of the message itself; nothing is followed.
    Part,
}

/// What "remove account and data" came to (`Core::delete_local_data`).
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum DataRemoved {
    /// The mail and every sign-in kept are gone.
    All,
    /// The mail and every sign-in that could be found are gone, but the list of the sign-ins
    /// kept was damaged (it was set aside): older ones may still be in the phone's credential
    /// store. `message`, in the owner's language, says so and how to remove them; once it has
    /// been shown, `Core::sign_ins_notice_shown` keeps later removals from saying it again.
    SignInsMayRemain { message: String },
}

/// What `Core::forget_sign_ins` came to.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SignInsForgotten {
    /// How many entries were looked for.
    pub looked_for: u64,
    /// Set when the list of the sign-ins kept was damaged (it was set aside), so older ones may
    /// still be in the phone's credential store: the words to show the owner.
    pub may_remain: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct LabeledRecord {
    /// What the contact's card calls it (`Work`, `Mobile`), as written there.
    pub label: String,
    pub value: String,
    /// The account's server has no place for it: it is kept on this phone only (#1917).
    pub local_only: bool,
}

/// One person (a contact) as the detail page shows it. Read-only: the app offers no editing.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PersonDetail {
    pub rev: u64,
    pub id: u64,
    pub name: String,
    pub company: String,
    pub title: String,
    pub department: String,
    pub emails: Vec<LabeledRecord>,
    pub phones: Vec<LabeledRecord>,
    /// Postal addresses, each on one line.
    pub addresses: Vec<LabeledRecord>,
    pub favorite: bool,
    /// The contacts folder it is kept in; 0 for this phone.
    pub folder: u64,
    /// That folder as the card names it ("Contacts (ada@example.org)"); empty for this phone.
    pub folder_name: String,
    /// Whose the folder is: `own`, `mailbox` (a shared mailbox's), `person` (shared by a
    /// person), or empty for this phone.
    pub folder_kind: String,
    /// The account may only read that folder: the person can't be changed or deleted here.
    pub read_only: bool,
    /// A change to the person waits to go to the account's server.
    pub not_sent: bool,
}

/// A contacts folder a new person can go in.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ContactFolderRecord {
    /// What `people.new`'s `folder` takes; `None` for the account's main folder not made yet
    /// (`people.new` without `folder` makes it, when it is [`ContactFolders::default_index`]).
    pub id: Option<u64>,
    pub account: u64,
    /// As the card names it ("Contacts (ada@example.org)").
    pub name: String,
    /// `own`, `mailbox` or `person`, as [`PersonDetail::folder_kind`].
    pub kind: String,
}

/// Where a new person can go: this phone, or a contacts folder the account may write.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ContactFolders {
    pub rev: u64,
    pub folders: Vec<ContactFolderRecord>,
    /// Where `people.new` puts a person when it isn't told: an index into `folders`, or
    /// `None` for this phone.
    pub default_index: Option<u32>,
}

/// Somebody a contacts folder has already, with an e-mail address of a person about to be moved
/// there (#1144): the picker asks whether to open that one or move anyway.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AlreadyThere {
    pub id: u64,
    pub name: String,
}

/// One task as the editor shows it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskDetail {
    pub rev: u64,
    pub task: TaskRecord,
    pub notes: String,
    /// The notes are longer than what is handed over: an app must not write them back as they
    /// stand, or the rest would be lost.
    pub notes_cut: bool,
    /// When its reminder goes off, `YYYY-MM-DDTHH:MM:SS` in the owner's zone.
    pub reminder: Option<String>,
    /// The account keeps it only on this phone (made here and not sent, or kept here by choice).
    pub only_here: bool,
}

/// Where tasks can live, and what to ask about the ones that exist only on this phone.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TaskSetup {
    pub rev: u64,
    /// The accounts whose To Do is kept in step, in the owner's order.
    pub accounts: Vec<AccountRecord>,
    /// Tasks that exist only on this phone and could be sent to an account's To Do. Nothing is
    /// sent until the owner has said yes (`tasks.sendLocal`).
    pub local_waiting: u32,
    /// The owner has been asked and said "Not now" or yes (the question is then not asked
    /// again, only offered).
    pub answered: bool,
    /// The account the owner sent their local tasks to, when they did.
    pub sent_to: Option<u64>,
}
