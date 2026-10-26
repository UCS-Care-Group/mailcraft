// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The session: the mailbox, what is selected, how it is viewed, the message being written, and
//! undo/redo.

use std::path::PathBuf;
use std::sync::Arc;

use mailcraft_model::{Attachment, ContentRef, Id, Importance, Mailbox, Message, RichBody};
use mailcraft_time::{Date, DateTime, Timestamp, Weekday, Zone};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::list::Row;
use crate::{CmdError, Registry};

/// Undo steps kept.
const MAX_HISTORY: usize = 100;

/// How long a search spends bringing the bodies of older mail into memory before it answers with
/// what it has searched (and says what it hasn't).
const SEARCH_TIME: std::time::Duration = std::time::Duration::from_secs(2);

/// Most bytes of bodies and attachments a session keeps in its messages after bringing them in
/// ([`Session::load_where`]), beyond what the mailbox brought in when it opened: past it, what
/// was used longest ago and is safely on disk goes back to being named only (128 MB).
pub const LOADED_MAX: u64 = 128 << 20;

/// The largest calendar part (an invitation's `.ics`) read from a message: one past it is left
/// where it is, unread.
pub(crate) const MAX_CALENDAR_PART: usize = 2 << 20;

/// Messages one load looks at to let go of their content ([`Session::evict`]).
#[cfg(not(target_arch = "wasm32"))]
const EVICT_TRIES: usize = 16;

/// One part of a message whose bytes [`Session::content`] fetches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    Text,
    Html,
    /// An attachment, by its index in `Message::attachments`.
    Attachment(usize),
}

/// The part of the app in front: mail, calendar, people or tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Module {
    #[default]
    Mail,
    Calendar,
    People,
    Tasks,
}

impl Module {
    pub const ALL: [Module; 4] = [Module::Mail, Module::Calendar, Module::People, Module::Tasks];

    pub fn label(self) -> &'static str {
        match self {
            Module::Mail => "Mail",
            Module::Calendar => "Calendar",
            Module::People => "People",
            Module::Tasks => "Tasks",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortBy {
    #[default]
    Date,
    From,
    Subject,
    Size,
    Importance,
    Categories,
}

impl SortBy {
    pub const ALL: [SortBy; 6] = [SortBy::Date, SortBy::From, SortBy::Subject, SortBy::Size, SortBy::Importance, SortBy::Categories];

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Date => "Date",
            SortBy::From => "From",
            SortBy::Subject => "Subject",
            SortBy::Size => "Size",
            SortBy::Importance => "Importance",
            SortBy::Categories => "Categories",
        }
    }

    /// Whether a list arranged this way starts at the top of the scale until it is turned over:
    /// the newest, the largest, the most important. Senders, subjects and categories start at A.
    pub fn starts_high(self) -> bool {
        !matches!(self, SortBy::From | SortBy::Subject | SortBy::Categories)
    }
}

/// How the contact list is arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PeopleSort {
    /// A to Z by the name each is filed as, favourites first, under their first letters.
    #[default]
    Name,
    /// A to Z by the categories each carries, those with none first, then by name.
    Categories,
}

impl PeopleSort {
    pub const ALL: [PeopleSort; 2] = [PeopleSort::Name, PeopleSort::Categories];

    pub fn label(self) -> &'static str {
        match self {
            PeopleSort::Name => "Name",
            PeopleSort::Categories => "Categories",
        }
    }
}

/// How the task list is arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskSort {
    /// By when each is due, under Overdue, Today, Tomorrow and the like.
    #[default]
    DueDate,
    /// Under each of its categories, those with none first, then by when each is due.
    Categories,
}

impl TaskSort {
    pub const ALL: [TaskSort; 2] = [TaskSort::DueDate, TaskSort::Categories];

    pub fn label(self) -> &'static str {
        match self {
            TaskSort::DueDate => "Due Date",
            TaskSort::Categories => "Categories",
        }
    }
}

/// What the message list is narrowed to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Filter {
    #[default]
    All,
    Unread,
    Flagged,
    Attachments,
    ToMe,
}

impl Filter {
    pub const ALL: [Filter; 5] = [Filter::All, Filter::Unread, Filter::Flagged, Filter::Attachments, Filter::ToMe];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Unread => "Unread",
            Filter::Flagged => "Flagged",
            Filter::Attachments => "Has Attachments",
            Filter::ToMe => "To Me",
        }
    }
}

/// Where a search looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// The folder in view.
    #[default]
    Folder,
    /// Every folder of every account, except Deleted Items and Junk.
    Everywhere,
}

/// Where the reading pane sits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Pane {
    #[default]
    Right,
    Bottom,
    Off,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CalView {
    Day,
    WorkWeek,
    #[default]
    Week,
    Month,
}

impl CalView {
    pub const ALL: [CalView; 4] = [CalView::Day, CalView::WorkWeek, CalView::Week, CalView::Month];

    pub fn label(self) -> &'static str {
        match self {
            CalView::Day => "Day",
            CalView::WorkWeek => "Work Week",
            CalView::Week => "Week",
            CalView::Month => "Month",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskFilter {
    #[default]
    Active,
    Today,
    Overdue,
    Completed,
    All,
}

impl TaskFilter {
    pub const ALL: [TaskFilter; 5] = [TaskFilter::Active, TaskFilter::Today, TaskFilter::Overdue, TaskFilter::Completed, TaskFilter::All];

    pub fn label(self) -> &'static str {
        match self {
            TaskFilter::Active => "Active",
            TaskFilter::Today => "Today",
            TaskFilter::Overdue => "Overdue",
            TaskFilter::Completed => "Completed",
            TaskFilter::All => "All",
        }
    }
}

/// An enum from the code commands and JSON use (`workWeek`); the first letter's case doesn't matter.
pub fn parse_enum<T: DeserializeOwned>(s: &str) -> Option<T> {
    let s = s.trim();
    let try_one = |s: String| serde_json::from_value::<T>(Value::String(s)).ok();
    try_one(s.to_string()).or_else(|| {
        let mut c = s.chars();
        let first = c.next()?;
        try_one(first.to_lowercase().chain(c).collect())
    })
}

/// The code of an enum value (`workWeek`).
pub fn enum_code<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

/// View state that commands change (the View tab, the navigation bar, the search box).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ViewState {
    pub module: Module,
    /// The mail folder in view.
    pub folder: Id,
    pub sort: SortBy,
    /// Newest (largest, Z) first.
    pub sort_descending: bool,
    /// Show messages grouped into conversations.
    pub conversations: bool,
    pub filter: Filter,
    pub search: String,
    pub scope: Scope,
    pub reading_pane: Pane,
    pub folder_pane: bool,
    pub calendar: CalView,
    /// The day the calendar shows (its week or month, depending on the view).
    pub date: Date,
    /// How many days the Day view shows from `date` on (`view.days`: Alt+1…0, a drag over the
    /// date navigator); 0 and 1 are the one day.
    pub days: u8,
    /// The calendar the owner works in: where new events go (`calendar.setCurrent`). 0 for
    /// none chosen, which is the account's server calendar when there is one.
    pub in_calendar: Id,
    /// The calendars that lie over one another instead of standing side by side (`view.overlay`):
    /// those of them that are shown share one block, when there are two at least.
    pub overlay: Vec<Id>,
    /// Of the calendars that lie over one another, the one in front (`view.frontCalendar`); 0
    /// for the first of them.
    pub overlay_front: Id,
    pub people_search: String,
    pub people_sort: PeopleSort,
    pub task_filter: TaskFilter,
    pub task_sort: TaskSort,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            module: Module::Mail,
            folder: 0,
            sort: SortBy::Date,
            sort_descending: true,
            conversations: false,
            filter: Filter::All,
            search: String::new(),
            scope: Scope::Folder,
            reading_pane: Pane::Right,
            folder_pane: true,
            calendar: CalView::Week,
            date: Date::default(),
            days: 0,
            in_calendar: 0,
            overlay: Vec::new(),
            overlay_front: 0,
            people_search: String::new(),
            people_sort: PeopleSort::Name,
            task_filter: TaskFilter::Active,
            task_sort: TaskSort::DueDate,
        }
    }
}

/// What is selected in each module.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Selection {
    /// Selected messages; the last one is the one the reading pane shows.
    pub messages: Vec<Id>,
    pub contact: Option<Id>,
    pub event: Option<Id>,
    /// The day of the selected occurrence, for an event that repeats.
    pub event_day: Option<Date>,
    pub task: Option<Id>,
    /// Which of the focused message's rows has the focus, for a message the list shows more than
    /// once (arranged by categories, under each of its categories): the message and the row's
    /// [`copy`](crate::list::Row::copy).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_row: Option<(Id, u16)>,
    /// The row a range (Shift-click) runs from: the message and the row's copy, for one listed
    /// under several categories.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor_row: Option<(Id, u16)>,
}

impl Selection {
    /// The message in the reading pane.
    pub fn focus(&self) -> Option<Id> {
        self.messages.last().copied()
    }

    /// Which of `id`'s rows a range runs from: the anchor's chosen row, else its first.
    pub fn anchor_copy(&self, id: Id) -> u16 {
        match self.anchor_row {
            Some((a, copy)) if a == id => copy,
            _ => 0,
        }
    }

    /// Which of the focused message's rows has the focus: its first unless another was chosen.
    pub fn focus_copy(&self) -> u16 {
        match self.focus_row {
            Some((id, copy)) if self.focus() == Some(id) => copy,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ComposeMode {
    #[default]
    New,
    Reply,
    ReplyAll,
    Forward,
    /// Continuing a saved draft.
    Draft,
}

/// How the message being written is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BodyFormat {
    /// HTML, with the same text as a plain-text part beside it.
    #[default]
    Html,
    /// Plain text only: the formatting stays in the message being written but is not sent.
    Plain,
}

/// The original under a reply or forward that has HTML, carried with its look but without
/// anything in it that runs, embeds or tracks (`mailcraft_html::quotable`): it goes out below the
/// owner's text and is never edited here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Quote {
    /// The original's header lines and text, for the plain-text part.
    pub text: String,
    /// The original's header lines and its HTML made fit to quote (the inside of its `<body>`,
    /// with only the elements and attributes that give it its look), for the HTML part.
    pub html: String,
    /// The original's pictures its HTML shows (`cid:`), sent with a reply so the quote keeps
    /// them. A forward carries every attachment anyway.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pictures: Vec<Attachment>,
}

/// The message being written. Recipient lines are text, as typed (`Ada <ada@example.org>; bob@…`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Compose {
    pub mode: ComposeMode,
    /// The message replied to or forwarded.
    pub source: Option<Id>,
    /// It forwards `source` whole, as an attachment (#315): a Microsoft account's draft of it is
    /// made as a message of its own, not as a forward of `source`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub as_attachment: bool,
    /// The saved draft this message continues.
    pub draft: Option<Id>,
    /// The account it is sent from.
    pub account: Id,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    /// What the owner writes, in rich text, with its selection and its own undo. Stored as plain
    /// text before rich text; that still reads.
    pub body: RichBody,
    /// HTML with a plain-text part, or plain text only.
    pub format: BodyFormat,
    /// The original of a reply or forward in HTML, below the owner's text. A plain-text original
    /// is quoted in the body instead, as text the owner can edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<Quote>,
    pub attachments: Vec<Attachment>,
    pub importance: Importance,
    pub in_reply_to: String,
    pub references: Vec<String>,
    /// The signature put in the body (its paragraphs are marked there): which one, how it lays
    /// out its first picture, and the pictures it carries. `None` when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<InsertedSignature>,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
}

/// The signature in a message being written (R4.5): what went in, so that it can be told whether
/// the owner has edited it, and the pictures it shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InsertedSignature {
    pub name: String,
    pub layout: mailcraft_model::signature::Layout,
    pub pictures: Vec<mailcraft_model::signature::Picture>,
    /// Its text as it went in.
    pub text: mailcraft_model::RichText,
}

/// A signature open in File › Signatures (R4.5.3): edited as a message's body is, by the
/// same commands, until `signature.save` puts it back in the mailbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignatureEdit {
    /// The signature as it was opened (its name, layout and pictures, as they are changed);
    /// its text is `body`'s.
    pub signature: mailcraft_model::signature::Signature,
    pub body: RichBody,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
}

/// The text the editing commands work on ([`Session::doc_mut`]).
pub struct Doc<'a> {
    pub body: &'a mut RichBody,
    pub dirty: &'a mut bool,
    /// A plain-text message: no formatting.
    pub plain: bool,
}

/// Preferences the engine acts on (File › Options); the front end saves them between runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// `3:05 PM` rather than `15:05`.
    pub twelve_hour: bool,
    pub week_start: Weekday,
    /// The owner chose the clock (`file.options`); else it is the interface language's own
    /// ([`Prefs::follow_language`]). `None`: settings saved before this was kept.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_chosen: Option<bool>,
    /// The same for the first day of the week.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub week_start_chosen: Option<bool>,
    pub work_days: Vec<Weekday>,
    /// Working hours, for the calendar's shading (0–24).
    pub work_start: u8,
    pub work_end: u8,
    /// Opening a message in the reading pane marks it read.
    pub mark_read_on_open: bool,
    /// Reminder a new appointment gets, in minutes before it starts.
    pub default_reminder: Option<u32>,
    /// Put the original message under a reply.
    pub quote_on_reply: bool,
    /// Lines of body text under each message in the list (0–3).
    pub preview_lines: u8,
    /// The list has group headings (Today, Yesterday…; `view.groups`). Off: one continuous list.
    pub show_groups: bool,
    /// Each row of the list is one line: sender, subject, time and marks (`view.oneLine`).
    #[serde(skip_serializing_if = "is_false")]
    pub one_line: bool,
    /// Most messages one synchronisation downloads per folder (0: all).
    pub sync_limit: u32,
    /// Minutes between automatic Send/Receive runs (0: only when asked).
    pub sync_minutes: u32,
    /// The calendar's time scale: the minutes one row of its hours stands for (60, 30, 15, 10,
    /// 6 or 5; `view.timeScale`).
    pub time_scale: u8,
    /// The date navigator and the Month view say which week of the year a week is
    /// (`view.weekNumbers`).
    pub week_numbers: bool,
    /// A meeting request (or a cancellation) that was answered from the mail goes to Deleted
    /// Items (`calendar.deleteAnsweredRequests`). Off unless the owner turns it on.
    pub delete_answered_requests: bool,
    /// The working hours above are used even when the account's mailbox has its own
    /// (`view.workHours`).
    pub own_work_hours: bool,
    /// The working hours the account's mailbox has, as a host brought them from its server
    /// (`sync.applyMailboxSettings`); they are the ones in effect unless `own_work_hours`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mailbox_hours: Option<crate::cmd::calendar_settings::MailboxHours>,
    /// The folder pane shows the Favorites that are marked here even for an account whose
    /// server has a list of its own (the ones Outlook shows; `view.favorites`). Off unless the
    /// owner turns it on: the server's list is shown where there is one.
    #[serde(skip_serializing_if = "is_false")]
    pub own_favorites: bool,
    /// The time zone the calendar is shown in, by name (`Europe/Berlin`; `view.timeZone`).
    /// Empty: this computer's own, followed when it changes.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub time_zone: String,
    /// A second time zone the calendar's time bar shows beside the first (`view.timeZone`
    /// with `"second"`); empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub second_zone: String,
    /// What the time bar calls the two zones, when the owner gave them names; else the zones'
    /// own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone_label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub second_zone_label: String,
    /// The zones the owner chose lately, the latest first: a zone picker lists them after the
    /// computer's own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_zones: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            twelve_hour: true,
            week_start: Weekday::Sun,
            clock_chosen: None,
            week_start_chosen: None,
            work_days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
            work_start: 8,
            work_end: 17,
            mark_read_on_open: true,
            default_reminder: Some(15),
            quote_on_reply: true,
            preview_lines: 1,
            show_groups: true,
            one_line: false,
            sync_limit: 500,
            sync_minutes: 5,
            time_scale: 30,
            week_numbers: false,
            delete_answered_requests: false,
            own_work_hours: false,
            mailbox_hours: None,
            own_favorites: false,
            time_zone: String::new(),
            second_zone: String::new(),
            zone_label: String::new(),
            second_zone_label: String::new(),
            recent_zones: Vec::new(),
        }
    }
}

impl Prefs {
    /// The clock and the first day of the week that the owner never chose follow the interface
    /// language (French: 24-hour, Monday); the ones they chose are kept. Settings saved before
    /// this was kept count what differs from the old defaults (12-hour, Sunday) as chosen, so
    /// they behave as they always did.
    pub fn follow_language(&mut self, lang: mailcraft_i18n::Lang) {
        let old = Prefs::default();
        let clock = *self.clock_chosen.get_or_insert(self.twelve_hour != old.twelve_hour);
        let week = *self.week_start_chosen.get_or_insert(self.week_start != old.week_start);
        let own = lang.conventions();
        if !clock {
            self.twelve_hour = own.twelve_hour;
        }
        if !week {
            self.week_start = Weekday::from_index(i64::from(own.week_start));
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// How far the word index a search answers from has got (R6.2; [`Session::word_index`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordIndex {
    /// None is kept: the browser, a mailbox kept in no file, or a host that didn't ask for one
    /// ([`Session::index_words`]). A search reads the messages themselves.
    Off,
    /// Its file is being read, or messages are being read into it in the background (`done` of
    /// `total`; both 0 while there is nothing to count yet): what it hasn't got yet is searched
    /// as without an index, more slowly.
    Building { done: usize, total: usize },
    /// It holds every message as it is.
    Complete,
}

/// The clock commands read; tests and scripts pin it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Clock {
    /// A fixed "now" instead of the system clock.
    pub now: Option<Timestamp>,
    /// A fixed zone offset (minutes east of UTC) instead of the system's zone.
    pub offset: Option<i32>,
    /// A zone instead of the system's: this computer as it would be there. (The calendar is
    /// shown in it unless the options name another, [`Prefs::time_zone`].) `offset` goes before
    /// both.
    pub zone: Option<Zone>,
}

pub(crate) struct Undo {
    label: &'static str,
    pub(crate) mailbox: Mailbox,
    /// What the step did to the Microsoft accounts' mail, for keeping them in step when it is
    /// undone ([`crate::undo`]).
    touched: crate::undo::Touched,
    sel: Selection,
    folder: Id,
}

pub struct Session {
    pub mailbox: Mailbox,
    pub view: ViewState,
    pub sel: Selection,
    /// The message being written, if one is open.
    pub compose: Option<Compose>,
    /// A signature open in File › Signatures. While there is one, the editing commands
    /// work on it instead of the message being written ([`Session::doc`]).
    pub signature_edit: Option<SignatureEdit>,
    pub prefs: Prefs,
    pub registry: Arc<Registry>,
    /// The mailbox file, once opened from or saved to one.
    pub path: Option<PathBuf>,
    /// The mailbox changed since it was opened or saved.
    pub dirty: bool,
    pub clock: Clock,
    /// What the last synchronisation reported, for the status bar.
    pub sync_status: String,
    /// Send/Receive Progress: the run under way, or the last one, account by account. Kept up
    /// to date by `sync.apply` and `sync.applyGraph`; in memory only, never saved.
    pub sync_progress: crate::cmd::sync_progress::SyncProgress,
    /// Messages kept in step by server id (through Graph, over JMAP): kept between the pages of a run.
    pub(crate) graph_index: crate::cmd::graph_down::ServerIndex,
    /// Messages of Microsoft and JMAP accounts the server wouldn't give whole this session (or, for
    /// JMAP, said it no longer has), by account and server id: the background passes them over
    /// (opening one still asks). Never saved.
    pub(crate) graph_unfetchable: std::collections::HashSet<(Id, String)>,
    /// Messages fetched whole this session although they are outside their account's mail to
    /// keep offline (the owner opened, answered or saved one, #143): kept in full until the
    /// mailbox is opened again, then back to their headers and preview. Never saved.
    pub(crate) kept_for_now: std::collections::HashSet<Id>,
    /// How many times a mailbox took the place of the one before ([`Session::replace_mailbox`]).
    openings: u64,
    /// How many times an undo or redo put another version of the mailbox in place.
    restored: u64,
    /// Messages being moved out of a Microsoft account that were whole in the mailbox file when
    /// it was last saved: only for those is the server's copy removed (R3 spec S19). Never saved.
    pub(crate) saved_whole: std::collections::HashSet<Id>,
    /// What the server refused in the Graph run under way, for the status at its end (S20).
    pub(crate) graph_said: Vec<String>,
    /// When each Microsoft account last had every folder read in a Graph run, none passed over
    /// for having the counts it had (#276): the next full sweep is due an hour later. Empty when
    /// a mailbox is opened, so the first run reads everything. Never saved.
    pub(crate) graph_swept: std::collections::HashMap<Id, Timestamp>,
    /// Microsoft and JMAP accounts whose last folder listing was cut short (#276): a folder it left
    /// out wasn't read, so the run that follows is no full sweep (and a JMAP run lists again).
    pub(crate) graph_listed_short: std::collections::HashSet<Id>,
    /// The mailboxes of each JMAP account the run under way took in a window of (M6), and whether
    /// that was the whole of the mailbox, once: at its end, only what those whole windows left out is
    /// removed. Never saved.
    pub(crate) jmap_read: std::collections::HashMap<Id, std::collections::HashMap<String, bool>>,
    /// Set while a command an agent asked for runs (with what the owner allowed it): what the
    /// command did is checked against that ([`crate::agent`]), and undone when it oversteps.
    pub agent: Option<Vec<crate::agent::Class>>,
    /// A copy of the mailbox the window had the host write off its own thread before switching an
    /// account to Microsoft Graph (the account, where the copy is): `account.graphSwitch` takes it
    /// instead of writing one itself ([`crate::cmd::graph_switch::prepare_backup`]). Set only by
    /// the window once the copy is on disk, never by a command's params. Never saved.
    pub graph_switch_backup: Option<(Id, PathBuf)>,
    /// A folder tree of mbox files read and parsed by the host off the window's thread, waiting
    /// for `file.importFolderTree` with `staged` to put it in (#420). Never saved.
    pub staged_tree: Option<crate::import_tree::Parsed>,
    /// More results on the server for the search in view (R6.2): what was asked of the servers
    /// and what they found, never in the mailbox. Never saved.
    pub server_search: crate::cmd::search_server::ServerSearch,
    pub(crate) history: Vec<Undo>,
    pub(crate) redo: Vec<Undo>,
    /// What the last undo or redo couldn't do there (a delete for good, a message sent).
    undo_said: Vec<String>,
    rev: u64,
    list_cache: Option<(String, Arc<Vec<Row>>)>,
    /// Where the content that isn't in memory comes from ([`Session::content`]). `None`: the
    /// content folder beside the mailbox file, where there is one (the desktop); the browser,
    /// which keeps everything in memory, never asks.
    pub content_source: Option<Arc<dyn crate::io::ContentSource>>,
    /// Counts the parts brought into memory (caches that read bodies key on it with `rev`).
    loads: u64,
    /// Messages the last search left out because their bodies weren't in memory and loading them
    /// ran past [`Session::search_time`]; the list says so.
    pub unsearched: usize,
    /// How long one search spends bringing bodies into memory before it goes with what it has.
    pub search_time: std::time::Duration,
    /// How many bytes brought into messages are kept there ([`LOADED_MAX`]).
    pub loaded_budget: u64,
    /// Messages content was brought into, the longest unused first, with the bytes each took.
    resident: std::collections::VecDeque<(Id, u64)>,
    resident_bytes: u64,
    /// Messages whose showing couldn't bring in what it needs (a picture's or a calendar part's
    /// file missing or damaged): not asked for again every frame, until the mailbox is opened anew.
    display_failed: std::collections::HashSet<Id>,
    /// While a command runs: the messages content was brought into or let go of, each as it was
    /// and as it became ([`Session::put_message`]), so that the state the command started from
    /// (its Undo step, and what a failed command goes back to) has them as they are now. Bringing
    /// content in is not a change to the mailbox.
    swaps: Option<Swaps>,
    /// Since a search that reads bodies brought them in: the query, and whether each message
    /// whose body was let go of since matched it ([`Session::load_bodies_for_search`]). Kept
    /// while the list stays as it was searched (`searched_for`), so that a result opened, whose
    /// body comes in, doesn't search everything again.
    search_pass: Option<(mailcraft_model::search::Query, std::collections::HashMap<Id, bool>)>,
    /// What the list was when `search_pass` was made: its key without the loads.
    searched_for: Option<String>,
    /// Keep a word index beside the mailbox file, which a search answers from (R6.2): set by a
    /// host that wants it (the desktop app), never by a command's params. Off, a search reads
    /// the messages themselves, as it always does in the browser. Never saved.
    pub index_words: bool,
    /// The language of the window the owner reads this session in, set by the host (the desktop
    /// and the browser follow the owner's choice; the CLI and a headless MCP server leave it
    /// English). What the engine writes for a person is to follow it; commands, their parameters
    /// and what agents read stay English. Never saved in the mailbox file.
    pub interface_language: mailcraft_i18n::Lang,
    /// The word index of the mailbox file, while [`Session::index_words`] is on and the mailbox
    /// is kept in a file ([`crate::words`]).
    #[cfg(not(target_arch = "wasm32"))]
    words: Option<crate::words::Words>,
}

/// Message `id` was `old` and is `new`, the same message with more or less of it in memory.
struct Swap {
    id: Id,
    old: Arc<Message>,
    new: Arc<Message>,
}

/// The swaps of a command, one a message where they follow on from each other: a message brought
/// in and let go of again many times (a search that reads every body) keeps only what it was
/// first and what it is last, not every body it held between (#173).
#[derive(Default)]
struct Swaps {
    list: Vec<Swap>,
    /// Where in `list` each message's latest swap is.
    last: std::collections::HashMap<Id, usize>,
}

impl Swaps {
    fn push(&mut self, swap: Swap) {
        if let Some(&i) = self.last.get(&swap.id)
            && let Some(prev) = self.list.get_mut(i)
            && Arc::ptr_eq(&prev.new, &swap.old)
        {
            // `old` became `prev.new` and that became `new`: the same as `old` becoming `new`.
            prev.new = swap.new;
            return;
        }
        self.last.insert(swap.id, self.list.len());
        self.list.push(swap);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.list.len()
    }
}

impl Session {
    pub fn new(mailbox: Mailbox) -> Self {
        Session::with_clock(mailbox, Clock::default())
    }

    /// A session that reads the time and the zone from `clock`. (The clock is there before the
    /// mailbox is looked at: what zone its times are in is settled by it, [`Session::settle_zone`].)
    pub fn with_clock(mailbox: Mailbox, clock: Clock) -> Self {
        let mut s = Session {
            mailbox,
            view: ViewState::default(),
            sel: Selection::default(),
            compose: None,
            signature_edit: None,
            prefs: Prefs::default(),
            registry: Arc::new(crate::cmd::registry()),
            path: None,
            dirty: false,
            clock,
            sync_status: String::new(),
            sync_progress: Default::default(),
            graph_index: Default::default(),
            graph_unfetchable: Default::default(),
            kept_for_now: Default::default(),
            openings: 0,
            restored: 0,
            saved_whole: Default::default(),
            graph_said: Vec::new(),
            graph_swept: Default::default(),
            graph_listed_short: Default::default(),
            jmap_read: Default::default(),
            agent: None,
            graph_switch_backup: None,
            staged_tree: None,
            server_search: Default::default(),
            history: Vec::new(),
            redo: Vec::new(),
            undo_said: Vec::new(),
            rev: 0,
            list_cache: None,
            content_source: None,
            loads: 0,
            unsearched: 0,
            search_time: SEARCH_TIME,
            loaded_budget: LOADED_MAX,
            resident: std::collections::VecDeque::new(),
            resident_bytes: 0,
            display_failed: std::collections::HashSet::new(),
            swaps: None,
            search_pass: None,
            searched_for: None,
            index_words: false,
            interface_language: mailcraft_i18n::Lang::English,
            #[cfg(not(target_arch = "wasm32"))]
            words: None,
        };
        s.view.date = s.today();
        s.normalize();
        s
    }

    /// A session on an empty mailbox with one local account.
    pub fn empty() -> Self {
        Session::new(Mailbox::with_local_account("", ""))
    }

    /// A session whose clock stands still at `now`, in a zone `offset` minutes east of UTC.
    pub fn at(mailbox: Mailbox, now: Timestamp, offset: i32) -> Self {
        Session::with_clock(mailbox, Clock { now: Some(now), offset: Some(offset), zone: None })
    }

    /// A session whose clock stands still at `now`, with this computer in `zone`.
    pub fn in_zone(mailbox: Mailbox, now: Timestamp, zone: Zone) -> Self {
        Session::with_clock(mailbox, Clock { now: Some(now), offset: None, zone: Some(zone) })
    }

    // ---- time ----

    pub fn now(&self) -> Timestamp {
        self.clock.now.unwrap_or_else(mailcraft_time::now)
    }

    /// The zone the calendar is shown in, which is the one the mailbox's wall-clock times are
    /// by: the one the options name ([`Prefs::time_zone`]), else this computer's (followed when
    /// it changes). A clock that is pinned ([`Clock`]) says otherwise.
    pub fn zone(&self) -> Zone {
        if let Some(zone) = self.clock.offset.and_then(|offset| Zone::fixed(offset.clamp(-18 * 60, 18 * 60))) {
            return zone;
        }
        Zone::named(&self.prefs.time_zone).filter(|zone| !zone.is_local()).unwrap_or_else(|| self.system_zone())
    }

    /// This computer's own zone, whatever zone the calendar is shown in.
    pub fn system_zone(&self) -> Zone {
        if let Some(zone) = self.clock.offset.and_then(|offset| Zone::fixed(offset.clamp(-18 * 60, 18 * 60))) {
            return zone;
        }
        self.clock.zone.unwrap_or_else(Zone::system)
    }

    /// The offset at `ts` of the zone the calendar is shown in, in minutes east of UTC.
    pub fn offset_at(&self, ts: Timestamp) -> i32 {
        match self.clock.offset {
            Some(offset) => offset,
            None => self.zone().offset_at(ts),
        }
    }

    /// Have the mailbox's wall-clock times be by the clock of the zone the calendar is shown in
    /// ([`Session::zone`]); returns whether anything in the mailbox reads differently for it.
    ///
    /// A mailbox whose times are in that zone already is left alone, which is nearly always. One
    /// that names no zone yet (it was written before zones were kept, or has just got its first
    /// event) is stamped with it, and nothing else changes. One that names another (this
    /// computer was carried to another zone, the owner chose another one in the options, the
    /// file comes from another machine) has its timed events moved to this zone's clock
    /// ([`Mailbox::rebase`](mailcraft_model::Mailbox::rebase)): they stay at their instants,
    /// whole-day events stay on their days, and nothing is noted for a server to be told.
    ///
    /// Called wherever the mailbox or the zone may have changed: when a session is made, before
    /// and after every command, when a mailbox is opened or an undo step brings an older one
    /// back; a front end calls it when it draws, so that the calendar follows the computer's
    /// zone while the program runs.
    pub fn settle_zone(&mut self) -> bool {
        if !self.mailbox.has_times() {
            return false;
        }
        let zone = self.zone();
        if zone.is_local() || self.mailbox.zone == zone.id() {
            return false;
        }
        let stamped = self.mailbox.zone.clone();
        let moved = self.mailbox.rebase(zone);
        if self.mailbox.zone == stamped {
            // A name this program doesn't know: the times stay as they are written.
            return false;
        }
        if moved == 0 && stamped.is_empty() {
            // Only the stamp, which rides along with the next save.
            return false;
        }
        log::info!("calendar: the times of {moved} events are now by the clock of {} (they were by {stamped})", self.mailbox.zone);
        // The day an occurrence was picked by is another day's name now.
        self.sel.event_day = self.sel.event.and_then(|id| self.mailbox.event(id)).map(|event| event.start.date);
        self.rev += 1;
        self.dirty = true;
        true
    }

    pub fn local(&self, ts: Timestamp) -> DateTime {
        DateTime::from_timestamp(ts, self.offset_at(ts))
    }

    /// `at`'s time as the interface language says it, on the owner's clock.
    pub fn time_said(&self, at: DateTime) -> String {
        mailcraft_i18n::date::time(&self.interface_language, at.hour, at.minute, self.prefs.twelve_hour)
    }

    /// `at`'s date (`Sunday, October 11, 2026`) and time as the interface language says them.
    pub fn date_time_said(&self, at: DateTime) -> String {
        let d = at.date;
        let day = mailcraft_i18n::date::Day { year: d.year, month: d.month, day: d.day, weekday: d.weekday().index() };
        format!("{} {}", mailcraft_i18n::date::long(&self.interface_language, day), self.time_said(at))
    }

    pub fn local_now(&self) -> DateTime {
        self.local(self.now())
    }

    pub fn today(&self) -> Date {
        self.local_now().date
    }

    // ---- state ----

    /// Counts every change to the mailbox (caches key on it).
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Changes each time another mailbox (or the same file again) is opened: a host that does
    /// something once a mailbox is opened looks for it to change.
    pub fn openings(&self) -> u64 {
        self.openings
    }

    /// Changes each time an undo or redo puts another version of the mailbox in place: what a
    /// front end keeps up to date from the mailbox change by change is worked out afresh.
    pub fn restored(&self) -> u64 {
        self.restored
    }

    /// The message the reading pane shows.
    /// The text being edited and whether it is plain: a signature open in the options, else the
    /// message being written.
    pub fn doc(&self) -> Option<(&RichBody, bool)> {
        match (&self.signature_edit, &self.compose) {
            (Some(e), _) => Some((&e.body, false)),
            (None, Some(c)) => Some((&c.body, c.format == BodyFormat::Plain)),
            (None, None) => None,
        }
    }

    pub fn doc_mut(&mut self) -> Option<Doc<'_>> {
        match (&mut self.signature_edit, &mut self.compose) {
            (Some(e), _) => Some(Doc { body: &mut e.body, dirty: &mut e.dirty, plain: false }),
            (None, Some(c)) => Some(Doc { plain: c.format == BodyFormat::Plain, body: &mut c.body, dirty: &mut c.dirty }),
            (None, None) => None,
        }
    }

    pub fn focused_message(&self) -> Option<&Message> {
        self.sel.focus().and_then(|id| self.mailbox.message(id))
    }

    /// Counts the times content was brought into memory or let go of ([`Session::load_where`]):
    /// not a change to the mailbox, but what is shown of it (a preview, a search) may differ.
    pub fn loads(&self) -> u64 {
        self.loads
    }

    /// The rows of the message list for the folder, filter, search and sort in view. A search
    /// that reads bodies brings the bodies it lacks into memory first, newest first, for as long
    /// as [`Session::search_time`] allows; what it didn't get to is counted in
    /// [`Session::unsearched`].
    pub fn rows(&mut self) -> Arc<Vec<Row>> {
        let list = |s: &Session| {
            format!(
                "{}|{}|{}|{}|{}",
                s.rev,
                s.today().to_days(),
                s.words_finished(),
                s.prefs.show_groups,
                serde_json::to_string(&s.view).unwrap_or_default()
            )
        };
        let key = |s: &Session| format!("{}|{}", s.loads, list(s));
        if let Some((k, rows)) = &self.list_cache
            && *k == key(self)
        {
            return rows.clone();
        }
        let list_now = list(self);
        #[cfg(not(target_arch = "wasm32"))]
        self.words_in_step(!mailcraft_model::search::Query::parse(&self.view.search).is_empty());
        #[cfg(not(target_arch = "wasm32"))]
        let (decided, unverified) = self.search_words();
        #[cfg(target_arch = "wasm32")]
        let (decided, unverified) = (std::collections::HashMap::new(), 0);
        let searched = match &self.search_pass {
            // Only content came in or went since this list was searched (a result opened): what
            // was found stands, and nothing is brought in for it again.
            Some((_, hits)) if self.searched_for.as_deref() == Some(list_now.as_str()) => hits.clone(),
            _ => {
                let hits = self.load_bodies_for_search(&decided);
                self.unsearched = self.unsearched.saturating_add(unverified);
                self.searched_for = self.search_pass.is_some().then_some(list_now);
                hits
            }
        };
        let key = key(self);
        let rows = Arc::new(crate::list::rows_with(self, &decided, &searched));
        self.list_cache = Some((key, rows.clone()));
        rows
    }

    /// Keep the word index of the mailbox file ([`crate::words`]) while [`Session::index_words`]
    /// is on and the mailbox is kept in a file: the one beside the file it is kept in now (read,
    /// and built where it lacks messages, in the background), brought in step with the mailbox
    /// before a search is answered (`search`), when an account went, and when the background is
    /// done.
    #[cfg(not(target_arch = "wasm32"))]
    fn words_in_step(&mut self, search: bool) {
        let Some(path) = self.path.clone().filter(|_| self.index_words) else {
            self.words = None;
            return;
        };
        let mut words = match self.words.take() {
            Some(words) if words.mailbox() == path => words,
            _ => crate::words::Words::open(&path, self.mailbox.messages.values().cloned().collect()),
        };
        // Also once the background is done: what came while it read is looked at then.
        if search || words.account_gone(&self.mailbox) || (!words.building() && !words.complete()) {
            words.in_step(&self.mailbox, &|m| self.kept_text(m));
        }
        self.words = Some(words);
    }

    /// The search in view, answered from the word index for the messages in view it holds as
    /// they are (R6.2): whether each matches. A word finds the messages holding a word that starts
    /// with it, in the parts its keyword names; what the index doesn't hold (`is:`, dates,
    /// categories) is looked at on the message, which is in memory. A phrase is checked on the
    /// message itself, its text read from disk when it is kept there, within
    /// [`Session::search_time`]: what that leaves unread doesn't match and is counted (the
    /// second number). Messages the index hasn't got (being read in the background) are left out,
    /// to be searched as without an index.
    #[cfg(not(target_arch = "wasm32"))]
    fn search_words(&self) -> (std::collections::HashMap<Id, bool>, usize) {
        use std::cell::Cell;
        let mut decided = std::collections::HashMap::new();
        let query = mailcraft_model::search::Query::parse(&self.view.search);
        let Some(words) = self.words.as_ref().filter(|_| !query.is_empty()) else { return (decided, 0) };
        let mut verify: Vec<Id> = Vec::new();
        {
            let guard = words.lock();
            let Some(index) = guard.as_ref() else { return (decided, 0) };
            let sets: Vec<Option<crate::words::Bits>> =
                (0..query.len()).map(|i| query.words_of(i).map(|(fields, w)| index.matching(w, fields))).collect();
            let in_view = crate::list::in_view(self);
            for m in self.mailbox.messages.values() {
                let Some(slot) = index.slot_of(m.id) else { continue };
                if !in_view(m) {
                    continue;
                }
                let text_on_disk = crate::words::body_in_memory(m).is_none();
                let unread_phrase = Cell::new(false);
                let hit = query.matches_with(m, self.offset_at(m.date), |i| {
                    let set = sets.get(i)?.as_ref()?;
                    if !set.has(slot) {
                        return Some(false);
                    }
                    // A whole address (#301): its words only narrow the messages down, and the
                    // address itself is looked at on each.
                    if query.is_address_term(i) {
                        return None;
                    }
                    let (fields, w) = query.words_of(i)?;
                    if w.len() <= 1 {
                        return Some(true);
                    }
                    // A phrase: its words one after another, looked at on the message.
                    if fields & mailcraft_model::words::BODY != 0 && text_on_disk {
                        unread_phrase.set(true);
                        return Some(true);
                    }
                    None
                });
                // Said to be there before it was looked at: the whole query is looked at on
                // the text as it is (a phrase that is there may make a `-` term fail).
                if unread_phrase.get() {
                    verify.push(m.id);
                } else {
                    decided.insert(m.id, hit);
                }
            }
        }
        let deadline = std::time::Instant::now().checked_add(self.search_time);
        let mut unread = 0;
        for id in verify {
            let in_time = deadline.is_none_or(|d| std::time::Instant::now() < d);
            let hit = in_time && self.loaded_message_where(id, |_| false).is_ok_and(|m| query.matches(&m, self.offset_at(m.date)));
            unread += usize::from(!in_time);
            decided.insert(id, hit);
        }
        (decided, unread)
    }

    /// How far the word index a search answers from has got.
    pub fn word_index(&self) -> WordIndex {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(words) = &self.words {
            return match words.progress() {
                Some((done, total)) => WordIndex::Building { done, total },
                None if words.complete() => WordIndex::Complete,
                // Done reading, with what came while it read still to be looked at (when the
                // list is next worked out): nothing to count.
                None => WordIndex::Building { done: 0, total: 0 },
            };
        }
        WordIndex::Off
    }

    /// Wait until the word index holds the mailbox as it is now (started if it wasn't), for as
    /// long as that takes: for the bench and tests, never the window.
    pub fn wait_for_word_index(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        for _ in 0..8 {
            self.words_in_step(false);
            let Some(words) = &mut self.words else { return };
            words.wait();
            self.words_in_step(true);
            if self.words.as_ref().is_none_or(|w| !w.building()) {
                return;
            }
        }
    }

    /// Counts the times the word index's background finished: a list searched before it did is
    /// worked out again.
    fn words_finished(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(words) = &self.words {
            return words.finished();
        }
        0
    }

    /// The word index this session keeps, if any.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn words(&self) -> Option<&crate::words::Words> {
        self.words.as_ref()
    }

    /// The text of message `m` that is kept outside memory, read from where it is kept (also when
    /// reading it failed before, and the message says so in its place).
    #[cfg(not(target_arch = "wasm32"))]
    fn kept_text(&self, m: &Message) -> Option<String> {
        let r = m.kept.as_deref()?.text?;
        self.fetch(&r).ok().map(|bytes| text_of(&bytes))
    }

    // ---- content that isn't in memory ----

    /// Where content comes from: what the host set, else the content folder beside the mailbox
    /// file (`crate::store`), where this build has one.
    fn source(&self) -> Option<Arc<dyn crate::io::ContentSource>> {
        if let Some(source) = &self.content_source {
            return Some(source.clone());
        }
        #[cfg(not(target_arch = "wasm32"))]
        let beside: Option<Arc<dyn crate::io::ContentSource>> = Some(Arc::new(crate::store::Content));
        #[cfg(target_arch = "wasm32")]
        let beside = None;
        beside
    }

    /// Fetch what `r` names from the source.
    fn fetch(&self, r: &ContentRef) -> Result<Arc<Vec<u8>>, CmdError> {
        let kept_nowhere = || CmdError::Failed("the mailbox isn't kept in a file, so content outside memory can't be read".into());
        let path = self.path.as_deref().ok_or_else(kept_nowhere)?;
        let source = self.source().ok_or_else(kept_nowhere)?;
        source.fetch(path, r).map_err(CmdError::Failed)
    }

    /// The bytes of one part of a message: from memory when they are there, else from where the
    /// mailbox keeps them (the content folder beside its file on the desktop). The one way to
    /// the bytes of an attachment, wherever they are. An error names what is wrong: no such
    /// message or part, or a part whose file is missing or damaged.
    pub fn content(&self, message: Id, part: Part) -> Result<Arc<Vec<u8>>, CmdError> {
        let m = self.mailbox.message(message).ok_or_else(|| failed!("no message with id {id}", id = message))?;
        let kept = m.kept.as_deref().filter(|k| !k.failed);
        match part {
            Part::Text => match kept.and_then(|k| k.text).filter(|_| m.body.text.is_empty()) {
                Some(r) => self.fetch(&r),
                None => Ok(Arc::new(m.body.text.as_bytes().to_vec())),
            },
            Part::Html => match kept.and_then(|k| k.html).filter(|_| m.body.html.is_none()) {
                Some(r) => self.fetch(&r),
                None => Ok(Arc::new(m.body.html.as_deref().unwrap_or("").as_bytes().to_vec())),
            },
            Part::Attachment(i) => {
                let a = m.attachments.get(i).ok_or_else(|| failed!("the message has no attachment {index}", index = i))?;
                match a.kept.filter(|_| a.data.is_empty()) {
                    Some(r) => self.fetch(&r),
                    None => Ok(a.data.clone()),
                }
            }
        }
    }

    /// A copy of message `id` with its body and the attachments `pick` chooses in memory (the
    /// message itself when nothing had to be fetched), without changing the mailbox: for what
    /// writes a message out (MIME to send or export, a forward).
    pub fn loaded_message_where(&self, id: Id, pick: impl Fn(&Attachment) -> bool) -> Result<Arc<Message>, CmdError> {
        let m = self.mailbox.messages.get(&id).ok_or_else(|| failed!("no message with id {id}", id))?;
        if !m.body_is_kept() && m.attachments.iter().all(|a| a.is_loaded() || !pick(a)) {
            return Ok(m.clone());
        }
        let mut copy = Message::clone(m);
        loaded_into(&mut copy, |part| self.content(id, part), &pick)?;
        Ok(Arc::new(copy))
    }

    /// A copy of message `id` with what a reply quotes in memory: its body, and the pictures its
    /// HTML shows in place (`cid:`). Its other attachments stay where they are.
    pub fn loaded_for_reply(&self, id: Id) -> Result<Arc<Message>, CmdError> {
        let m = self.loaded_message_where(id, |_| false)?;
        let html = m.body.html.as_deref().unwrap_or("");
        if m.attachments.iter().all(|a| a.is_loaded() || !shown_in(html, a)) {
            return Ok(m);
        }
        let mut copy = Message::clone(&m);
        loaded_into(&mut copy, |part| self.content(id, part), &|a| shown_in(html, a))?;
        Ok(Arc::new(copy))
    }

    /// [`Session::loaded_message_where`] with every attachment.
    pub fn loaded_message(&self, id: Id) -> Result<Arc<Message>, CmdError> {
        self.loaded_message_where(id, |_| true)
    }

    /// Bring the body of message `id` and the attachments `pick` chooses into memory, in the
    /// mailbox itself, so that everything that reads the message finds them. Not a change to the
    /// mailbox (nothing to undo or save), though caches that read bodies see it. Returns whether
    /// anything was brought in. A body whose file couldn't be read says so in its text from then
    /// on (until the mailbox is opened again), and the error says why. What the message names
    /// for its body stays as it was: a read that failed (a file locked for a moment, memory that
    /// wasn't there) is never written back as the body, and the file stays named, to be read
    /// the next time.
    pub fn load_where(&mut self, id: Id, pick: impl Fn(&Attachment) -> bool) -> Result<bool, CmdError> {
        let Some(m) = self.mailbox.messages.get(&id) else { return Err(failed!("no message with id {id}", id)) };
        if !m.body_is_kept() && m.attachments.iter().all(|a| a.is_loaded() || !pick(a)) {
            return Ok(false);
        }
        let mut copy = Message::clone(m);
        let loaded = loaded_into(&mut copy, |part| self.content(id, part), &pick);
        if let Err(e) = &loaded
            && copy.body_is_kept()
            && let Some(kept) = copy.kept.as_deref_mut()
        {
            // Said once, in the message, instead of being asked for again every frame. Only the
            // text that is kept outside gives way to the notice (which a save takes for the
            // kept text, `store::prepare`); a short text kept in the mailbox file stays, and the
            // HTML stays named, going with the text as it now reads.
            log::warn!("message {id}: its text couldn't be read ({e})");
            kept.failed = true;
            if kept.text.is_some() && copy.body.text.is_empty() {
                copy.body.text = crate::store_placeholder().to_string();
            }
            kept.text_hash = mailcraft_model::text_hash(&copy.body.text);
        }
        // What came in before a part that failed is in memory all the same, and counted.
        let brought = in_memory(&copy).saturating_sub(in_memory(m));
        self.put_message(id, copy);
        self.loads += 1;
        self.used(id, brought);
        loaded.map(|_| true)
    }

    /// Put `m`, message `id` with more or less of its content in memory, in the place of the
    /// one the mailbox has. The one it replaces may still be held by an Undo or Redo step: the
    /// content it keeps on disk is kept from the sweep while it is (`store::keep_held`). In a command,
    /// the state the command started from gets it too ([`Session::run`]).
    fn put_message(&mut self, id: Id, m: Message) {
        let new = Arc::new(m);
        let Some(old) = self.mailbox.messages.insert(id, new.clone()) else { return };
        if Arc::strong_count(&old) > 1 {
            self.hold(std::iter::once(&old));
        }
        if let Some(swaps) = &mut self.swaps {
            swaps.push(Swap { id, old, new });
        }
    }

    /// Keep what `messages` keep on disk from the sweep while something holds them.
    fn hold<'a>(&self, messages: impl IntoIterator<Item = &'a Arc<Message>>) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = &self.path {
            crate::store::keep_held(path, messages);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = messages;
    }

    /// After a command changed the mailbox: the messages of the state before it (`before`, its
    /// Undo step) that the mailbox no longer has as they were, kept from the sweep while that
    /// step holds them. One walk of both, in order of id.
    fn hold_replaced(&self, before: &Mailbox) {
        let mut now = self.mailbox.messages.iter().peekable();
        let mut gone = Vec::new();
        for (id, m) in &before.messages {
            while now.peek().is_some_and(|(n, _)| *n < id) {
                now.next();
            }
            if !now.peek().is_some_and(|(n, x)| *n == id && Arc::ptr_eq(x, m)) {
                gone.push(m);
            }
        }
        self.hold(gone);
    }

    /// Message `id` was just used, with `brought` more bytes in memory: it goes to the back of
    /// the line, and what is past the budget at the front goes back to disk.
    fn used(&mut self, id: Id, brought: u64) {
        let mut bytes = brought;
        if let Some(at) = self.resident.iter().position(|(r, _)| *r == id)
            && let Some((_, before)) = self.resident.remove(at)
        {
            bytes = bytes.saturating_add(before);
        }
        if bytes == 0 {
            return;
        }
        self.resident.push_back((id, bytes));
        self.resident_bytes = self.resident_bytes.saturating_add(brought);
        self.evict(id);
    }

    /// Let go of content brought into messages, the longest unused first, until what is kept is
    /// within [`Session::loaded_budget`]. Only what the mailbox file names as it is, and so is
    /// on disk, goes (`store::evictable`); a message shown or picked (or `keep`) stays, and so
    /// does one that can't go yet (changed since the last save, or the file busy being written):
    /// it keeps its place in the line and is tried again at a later load. A few tries a load.
    fn evict(&mut self, keep: Id) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = self.path.clone() else { return };
            let mut tries = self.resident.len().min(EVICT_TRIES);
            while self.resident_bytes > self.loaded_budget && tries > 0 {
                tries -= 1;
                let Some((id, bytes)) = self.resident.pop_front() else { break };
                let Some(m) = self.mailbox.messages.get(&id) else {
                    // Gone from the mailbox: nothing of it is kept here any more.
                    self.resident_bytes = self.resident_bytes.saturating_sub(bytes);
                    continue;
                };
                let lean = if id == keep || self.sel.messages.contains(&id) { None } else { crate::store::evictable(&path, id, m) };
                // A search under way has what it is looking for matched first: its body is
                // about to be named only, and the list reads it after this.
                let hit = lean.as_ref().and(self.search_pass.as_ref()).map(|(q, _)| q.matches(m, self.offset_at(m.date)));
                if let (Some(hit), Some((_, hits))) = (hit, &mut self.search_pass) {
                    hits.insert(id, hit);
                }
                match lean {
                    Some(lean) => {
                        self.put_message(id, lean);
                        self.resident_bytes = self.resident_bytes.saturating_sub(bytes);
                        self.loads += 1;
                    }
                    None => self.resident.push_back((id, bytes)),
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = keep;
    }

    /// Messages `ids` have no content in memory any more (they went back to their headers and
    /// preview, #143): what was counted for them is let go of.
    pub(crate) fn let_go(&mut self, ids: &std::collections::HashSet<Id>) {
        let mut freed = 0u64;
        self.resident.retain(|(id, bytes)| {
            let goes = ids.contains(id);
            if goes {
                freed = freed.saturating_add(*bytes);
            }
            !goes
        });
        self.resident_bytes = self.resident_bytes.saturating_sub(freed);
    }

    /// How many bytes brought into messages this session keeps there now.
    pub fn resident_bytes(&self) -> u64 {
        self.resident_bytes
    }

    /// Bring everything of message `id` into memory ([`Session::load_where`]).
    pub fn load_message(&mut self, id: Id) -> Result<bool, CmdError> {
        self.load_where(id, |_| true)
    }

    /// Bring in what showing message `id` needs: its body, the pictures its HTML shows in place
    /// (`cid:`), and any calendar part (an invitation's bar reads it). Other attachments wait
    /// until they are opened or saved. What fails is said once: the message isn't asked for again
    /// ([`Session::needs_load_for_display`]) until the mailbox is opened anew.
    pub fn load_for_display(&mut self, id: Id) -> Result<bool, CmdError> {
        let loaded = self.load_for_display_once(id);
        if loaded.is_err() {
            self.display_failed.insert(id);
        }
        loaded
    }

    fn load_for_display_once(&mut self, id: Id) -> Result<bool, CmdError> {
        // The body first: which pictures are shown is in its HTML.
        let body = self.load_where(id, |_| false)?;
        let Some(m) = self.mailbox.message(id) else { return Ok(body) };
        let html = m.body.html.clone().unwrap_or_default();
        let parts = self.load_where(id, move |a| is_readable_calendar_part(a) || shown_in(&html, a))?;
        Ok(body || parts)
    }

    /// Whether showing message `id` would bring anything into memory ([`Session::load_for_display`]).
    pub fn needs_load_for_display(&self, id: Id) -> bool {
        let Some(m) = self.mailbox.message(id).filter(|_| !self.display_failed.contains(&id)) else { return false };
        if m.body_is_kept() {
            return true;
        }
        let html = m.body.html.as_deref().unwrap_or("");
        m.attachments.iter().any(|a| !a.is_loaded() && (is_readable_calendar_part(a) || shown_in(html, a)))
    }

    /// The whole mailbox with every part in memory, as a self-contained file needs it (File ›
    /// Save As, an export, saving to another place): a copy when anything had to be fetched. An
    /// error names the message whose content couldn't be read.
    pub fn loaded_mailbox(&self) -> Result<Mailbox, CmdError> {
        let mut mailbox = self.mailbox.clone();
        for (id, m) in &self.mailbox.messages {
            if !m.is_loaded() {
                mailbox.messages.insert(*id, self.loaded_message(*id)?);
            }
        }
        Ok(mailbox)
    }

    /// The messages of the Outbox `outbox` holds, whole, for a synchronisation to send: a
    /// message whose content can't be read is left out of this run, with a line in the log,
    /// rather than sent without its files.
    pub fn loaded_outbox(&self, outbox: Vec<Message>) -> Vec<Message> {
        outbox
            .into_iter()
            .filter_map(|m| {
                if m.is_loaded() {
                    return Some(m);
                }
                match self.loaded_message(m.id) {
                    Ok(loaded) => Some(Message::clone(&loaded)),
                    Err(e) => {
                        log::warn!("message {} stays in the Outbox this time: {e}", m.id);
                        None
                    }
                }
            })
            .collect()
    }

    /// Before the list is built for a search that reads bodies: the bodies it lacks of the
    /// messages the word index didn't answer for (`decided`), newest first, within [`Session::search_time`]; the rest are counted as not searched. Bringing them
    /// in keeps within [`Session::loaded_budget`], so a body may be let go of again before the
    /// list is built: each is matched before it goes, and what is returned says whether it did
    /// (for [`crate::list::rows_with`]).
    fn load_bodies_for_search(&mut self, decided: &std::collections::HashMap<Id, bool>) -> std::collections::HashMap<Id, bool> {
        self.unsearched = 0;
        self.search_pass = None;
        let query = mailcraft_model::search::Query::parse(&self.view.search);
        if query.is_empty() || !query.reads_bodies() {
            return Default::default();
        }
        // The messages the list looks at (`list::rows`) that the word index didn't answer for.
        let in_view = crate::list::in_view(self);
        let mut wanted: Vec<(Timestamp, Id)> = self
            .mailbox
            .messages
            .values()
            .filter(|m| in_view(m) && m.body_is_kept() && !decided.contains_key(&m.id))
            .map(|m| (m.date, m.id))
            .collect();
        drop(in_view);
        if wanted.is_empty() {
            return Default::default();
        }
        wanted.sort_unstable_by(|a, b| b.cmp(a));
        // The browser has no clock to time it by (and keeps every body in memory anyway); a
        // deadline past what the clock can say is none.
        #[cfg(not(target_arch = "wasm32"))]
        let deadline = std::time::Instant::now().checked_add(self.search_time);
        let out_of_time = || {
            #[cfg(not(target_arch = "wasm32"))]
            let out = deadline.is_some_and(|d| std::time::Instant::now() >= d);
            #[cfg(target_arch = "wasm32")]
            let out = false;
            out
        };
        let mut left = wanted.len();
        self.search_pass = Some((query, Default::default()));
        for (_, id) in wanted {
            if out_of_time() {
                break;
            }
            // A body that can't be read says so in its place, and is not asked for again.
            let _ = self.load_where(id, |_| false);
            left = left.saturating_sub(1);
        }
        self.unsearched = left;
        // Kept: what is let go of until the list changes is matched before it goes, too.
        self.search_pass.as_ref().map(|(_, hits)| hits.clone()).unwrap_or_default()
    }

    /// Keep the view and the selection pointing at things that exist.
    pub fn normalize(&mut self) {
        if self.mailbox.folder(self.view.folder).is_none() {
            self.view.folder = self.mailbox.inbox().unwrap_or(0);
        }
        let mb = &self.mailbox;
        self.sel.messages.retain(|id| mb.message(*id).is_some());
        self.sel.messages.dedup();
        if self.sel.contact.is_some_and(|c| mb.contact(c).is_none()) {
            self.sel.contact = None;
        }
        if self.sel.event.is_some_and(|e| mb.event(e).is_none()) {
            self.sel.event = None;
            self.sel.event_day = None;
        }
        if self.sel.task.is_some_and(|t| mb.task(t).is_none()) {
            self.sel.task = None;
        }
        if let Some(c) = &mut self.compose
            && mb.account(c.account).is_none()
        {
            c.account = mb.default_account().map_or(0, |a| a.id);
        }
        // And its times are by the clock of the zone the calendar is shown in.
        self.settle_zone();
    }

    /// The mailbox was changed outside a command (mail arriving from a server): everything worked
    /// out from it (the message list, what is saved) is due again.
    pub fn mailbox_changed(&mut self) {
        self.rev += 1;
        self.dirty = true;
        self.normalize();
    }

    /// The mailbox file holds the mailbox as it is now (it was just saved, or nothing changed
    /// since it was opened or saved): the messages being moved out of a Microsoft account that
    /// are whole are whole in the file, and the server's copies may go (R3 spec S19). A host
    /// that saves by other means than `file.save` calls this when a save of the mailbox as it is
    /// now succeeded; the engine also notes it whenever it sees nothing unsaved.
    pub fn note_saved(&mut self) {
        let whole = self.mailbox.messages.values().filter(|m| m.moving_out.is_some() && !m.unfetched).map(|m| m.id);
        self.saved_whole.extend(whole);
        self.saved_whole.retain(|id| self.mailbox.messages.get(id).is_some_and(|m| m.moving_out.is_some()));
    }

    /// Whether the mailbox file is known to hold the mailbox as it is now.
    pub(crate) fn file_is_current(&self) -> bool {
        self.path.is_some() && !self.dirty
    }

    /// Replace the mailbox (File › New, File › Open): history and selection start over.
    pub fn replace_mailbox(&mut self, mailbox: Mailbox) {
        self.mailbox = mailbox;
        self.resident.clear();
        self.resident_bytes = 0;
        self.display_failed.clear();
        self.search_pass = None;
        self.searched_for = None;
        self.saved_whole.clear();
        self.kept_for_now.clear();
        self.graph_swept.clear();
        self.graph_listed_short.clear();
        self.jmap_read.clear();
        self.openings = self.openings.wrapping_add(1);
        self.history.clear();
        self.redo.clear();
        self.sel = Selection::default();
        self.compose = None;
        self.view.folder = 0;
        self.view.search.clear();
        self.dirty = false;
        self.rev += 1;
        self.normalize();
    }

    // ---- undo ----

    fn snapshot(&self, label: &'static str) -> Undo {
        Undo { label, mailbox: self.mailbox.clone(), touched: Default::default(), sel: self.sel.clone(), folder: self.view.folder }
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What Undo would undo (`Delete`, `Move to Folder`).
    pub fn undo_label(&self) -> Option<&'static str> {
        self.history.last().map(|u| u.label)
    }

    pub fn redo_label(&self) -> Option<&'static str> {
        self.redo.last().map(|u| u.label)
    }

    /// Forget every undo and redo step: the mailbox as it was then is not what the mail server
    /// has any more (changes were pushed, mail was sent), and going back to it would undo here
    /// what can't be undone there.
    pub fn forget_history(&mut self) {
        self.history.clear();
        self.redo.clear();
    }

    /// What the last undo or redo couldn't do, in words (empty when it did everything).
    pub fn undo_said(&self) -> &[String] {
        &self.undo_said
    }

    /// Go to the mailbox of `u`. A Microsoft account's mail stays in step with its server: what
    /// the server last said is kept, so the next run sends the reverse ([`crate::undo`]).
    fn swap(&mut self, u: Undo) -> Undo {
        let now = std::mem::take(&mut self.mailbox);
        let (back, said) = crate::undo::keep_in_step(u.mailbox, &u.touched, &now);
        // Undone, the step is to be done again from here: what that does is the way back.
        let touched = crate::undo::touched(&now, &back);
        self.mailbox = back;
        self.undo_said = said;
        // Where each message is, is looked at afresh.
        self.graph_index = Default::default();
        let cur = Undo { label: u.label, mailbox: now, touched, sel: std::mem::replace(&mut self.sel, u.sel), folder: self.view.folder };
        self.view.folder = u.folder;
        self.rev += 1;
        self.restored = self.restored.wrapping_add(1);
        self.dirty = true;
        self.normalize();
        cur
    }

    /// What undoing (`redo`: redoing) would put back besides mail and the like: the accounts, or
    /// the rules and signatures, as they were. For the agent gate, which asks the owner first.
    pub fn undo_reverts(&self, redo: bool) -> Option<crate::agent::Class> {
        use crate::agent::Class;
        let step = if redo { self.redo.last() } else { self.history.last() }?;
        let (to, now) = (&step.mailbox, &self.mailbox);
        if to.accounts != now.accounts {
            return Some(Class::Accounts);
        }
        // A send-bearing change to an event that the mailbox hasn't got: the step brings it back.
        let same = |a: &mailcraft_model::remote::EventChange, b: &mailcraft_model::remote::EventChange| {
            b.send && a.account == b.account && a.kind == b.kind && a.event == b.event && a.token == b.token
        };
        if to.event_changes.iter().any(|c| c.send && !now.event_changes.iter().any(|n| same(c, n))) {
            return Some(Class::Send);
        }
        // Things that go for good (they don't go to Deleted Items) and that the step lacks.
        let gone = now.contacts.keys().any(|k| !to.contacts.contains_key(k))
            || now.events.keys().any(|k| !to.events.contains_key(k))
            || now.tasks.keys().any(|k| !to.tasks.contains_key(k))
            || now.calendars.iter().any(|c| !to.calendars.iter().any(|t| t.id == c.id))
            || now.categories.iter().any(|c| !to.categories.iter().any(|t| t.name.to_lowercase() == c.name.to_lowercase()));
        if gone {
            return Some(Class::Delete);
        }
        if to.rules != now.rules || to.signatures != now.signatures {
            return Some(Class::Settings);
        }
        None
    }

    pub fn undo(&mut self) -> bool {
        let Some(u) = self.history.pop() else { return false };
        let cur = self.swap(u);
        self.redo.push(cur);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(u) = self.redo.pop() else { return false };
        let cur = self.swap(u);
        self.history.push(cur);
        true
    }

    // ---- commands ----

    /// Whether a command exists and is enabled now.
    pub fn enabled(&self, id: &str) -> bool {
        self.registry.get(id).is_some_and(|s| (s.enabled)(self).is_none())
    }

    /// Run a command by id. A command that fails, or panics, leaves the mailbox as it was.
    ///
    /// A command that leaves a message being written with changes that aren't saved
    /// (`cmd::compose::saves_first`, #442) saves it to Drafts first, as a Save Draft step of its
    /// own; when the command then fails, that Save Draft step stays behind, and the draft with it.
    pub fn run(&mut self, id: &str, params: &Value) -> Result<Value, CmdError> {
        let reg = self.registry.clone();
        let Some(spec) = reg.get(id) else { return Err(CmdError::Unknown(id.to_string())) };
        if let Some(reason) = (spec.enabled)(self) {
            // A command that only lacks a selection still runs on what the caller names.
            let named = spec.selection && ["id", "ids", "message"].iter().any(|k| params.get(*k).is_some_and(|v| !v.is_null()));
            if !named {
                return Err(CmdError::Disabled(reason.to_string()));
            }
        }
        // A message being written isn't let go of with what isn't saved of it (#442): it goes to
        // Drafts first, in a step of its own, and a save that fails stops here.
        if crate::cmd::compose::saves_first(self, id, params) {
            self.run("compose.saveDraft", &Value::Null)?;
        }
        // Undo and redo manage the history themselves.
        let history = spec.mutates && !matches!(id, "edit.undo" | "edit.redo");
        // The computer may have been carried to another zone since the last command: the
        // mailbox is put by that zone's clock first, and that is not this command's to undo.
        self.settle_zone();
        let before = self.snapshot(spec.label);
        let compose = self.compose.clone();
        let signature_edit = self.signature_edit.clone();
        let view = self.view.clone();
        let run = spec.run;
        // Last-resort guard (never-crash): a panic in a command becomes an error and the state is restored.
        let outer = self.swaps.replace(Swaps::default());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)));
        // Content the command brought in or let go of is in the state it started from too: it
        // is no change to undo, and a command that fails keeps it (and what is counted of it).
        let swaps = std::mem::replace(&mut self.swaps, outer).unwrap_or_default();
        let mut before = before;
        for swap in &swaps.list {
            if before.mailbox.messages.get(&swap.id).is_some_and(|m| Arc::ptr_eq(m, &swap.old)) {
                before.mailbox.messages.insert(swap.id, swap.new.clone());
            }
        }
        if let Some(outer) = &mut self.swaps {
            for swap in swaps.list {
                outer.push(swap);
            }
        }
        let result = match result {
            Ok(r) => r,
            Err(_) => {
                log::error!("command `{id}` panicked; the mailbox was restored");
                Err(failed!("{command} failed unexpectedly; nothing was changed", command = mailcraft_i18n::Arg::words(spec.label)))
            }
        };
        // An agent's command that did more than it was allowed (mail into an Outbox, messages
        // gone for good) is undone, whatever path it took there.
        let result = match (result, &self.agent) {
            (Ok(_), Some(allow)) if let Some(c) = crate::agent::overstep(&before.mailbox, &self.mailbox, allow) => {
                Err(CmdError::Failed(crate::agent::needs_owner(id, c, "do more than an agent is allowed to; nothing was changed")))
            }
            (r, _) => r,
        };
        match &result {
            Ok(_) => {
                // The working hours of an account just removed go with it (#325): here, once
                // nothing can turn the removal back, as preferences aren't put back with the mailbox.
                if wipes_history(id) {
                    crate::cmd::calendar_settings::forget_gone_hours(self);
                }
                if self.mailbox != before.mailbox {
                    self.hold_replaced(&before.mailbox);
                    self.rev += 1;
                    if history && wipes_history(id) {
                        // The mail is gone from this computer (#177): no step brings it back,
                        // and none holds its content on disk.
                        self.dirty = true;
                        self.history.clear();
                        self.redo.clear();
                    } else if history {
                        self.dirty = true;
                        let mut before = before;
                        before.touched = crate::undo::touched(&before.mailbox, &self.mailbox);
                        self.history.push(before);
                        if self.history.len() > MAX_HISTORY {
                            self.history.remove(0);
                        }
                        self.redo.clear();
                    }
                }
                self.normalize();
            }
            Err(_) => {
                if self.mailbox != before.mailbox {
                    self.rev += 1;
                }
                self.mailbox = before.mailbox;
                self.sel = before.sel;
                self.compose = compose;
                self.signature_edit = signature_edit;
                self.view = view;
            }
        }
        result
    }
}

/// Commands that remove what is kept of an account on this computer (#177): they leave nothing
/// to undo, as the content they remove would have to stay on disk for it.
fn wipes_history(id: &str) -> bool {
    matches!(id, "account.remove" | "account.removeSharedMailbox")
}

/// Whether an attachment is a calendar part (an invitation, an answer to one).
pub(crate) fn is_calendar_part(a: &Attachment) -> bool {
    let kind = a.mime.trim().to_ascii_lowercase();
    kind.starts_with("text/calendar") || kind.starts_with("application/ics") || a.name.to_ascii_lowercase().ends_with(".ics")
}

/// A calendar part small enough to be read ([`MAX_CALENDAR_PART`]), by its size before it is
/// brought in.
pub(crate) fn is_readable_calendar_part(a: &Attachment) -> bool {
    is_calendar_part(a) && a.size() <= MAX_CALENDAR_PART
}

/// Whether `html` shows the picture `a` in place (`cid:`).
fn shown_in(html: &str, a: &Attachment) -> bool {
    !a.content_id.is_empty() && html.contains(&format!("cid:{}", a.content_id))
}

/// Bring the body of `m` and the attachments `pick` chooses into it, through `fetch`. Stops at
/// the first part that can't be read, with the message changed as far as it got.
fn loaded_into(m: &mut Message, fetch: impl Fn(Part) -> Result<Arc<Vec<u8>>, CmdError>, pick: &impl Fn(&Attachment) -> bool) -> Result<(), CmdError> {
    if m.body_is_kept()
        && let Some(kept) = m.kept.as_deref().copied()
    {
        let text = kept.text.filter(|_| m.body.text.is_empty()).map(|_| fetch(Part::Text)).transpose()?;
        let html = kept.html.filter(|_| m.body.html.is_none()).map(|_| fetch(Part::Html)).transpose()?;
        if let Some(text) = text {
            m.body.text = text_of(&text);
        }
        if let Some(html) = html {
            m.body.html = Some(text_of(&html));
        }
        m.kept = None;
    }
    for i in 0..m.attachments.len() {
        let Some(a) = m.attachments.get(i) else { break };
        if a.is_loaded() || !pick(a) {
            continue;
        }
        let bytes = fetch(Part::Attachment(i))?;
        if let Some(a) = m.attachments.get_mut(i) {
            a.data = bytes;
            a.kept = None;
        }
    }
    Ok(())
}

/// How many bytes of a message's body and attachments are in memory.
fn in_memory(m: &Message) -> u64 {
    let body = m.body.text.len().saturating_add(m.body.html.as_ref().map_or(0, String::len));
    m.attachments.iter().fold(body as u64, |n, a| n.saturating_add(a.data.len() as u64))
}

/// Bytes as the text they hold; what isn't UTF-8 is read as well as it can be.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
