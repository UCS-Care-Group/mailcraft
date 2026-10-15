//! The session: the mailbox, what is selected, how it is viewed, the message being written, and
//! undo/redo.

use std::path::PathBuf;
use std::sync::Arc;

use mailcraft_model::rich::{CharStyle, Styles};
use mailcraft_model::{Attachment, ContentRef, Id, Importance, Mailbox, Message};
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
}

impl SortBy {
    pub const ALL: [SortBy; 5] = [SortBy::Date, SortBy::From, SortBy::Subject, SortBy::Size, SortBy::Importance];

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Date => "Date",
            SortBy::From => "From",
            SortBy::Subject => "Subject",
            SortBy::Size => "Size",
            SortBy::Importance => "Importance",
        }
    }

    /// Whether a list arranged this way starts at the top of the scale until it is turned over:
    /// the newest, the largest, the most important. Senders and subjects start at A.
    pub fn starts_high(self) -> bool {
        !matches!(self, SortBy::From | SortBy::Subject)
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
    pub task_filter: TaskFilter,
    /// The To-Do Bar beside the mail: the days ahead and the tasks to do (`view.todoBar`).
    #[serde(skip_serializing_if = "is_false")]
    pub todo_bar: bool,
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
            task_filter: TaskFilter::Active,
            todo_bar: false,
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
    /// The contact group that is open (`people.selectGroup`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<Id>,
}

impl Selection {
    /// The message in the reading pane.
    pub fn focus(&self) -> Option<Id> {
        self.messages.last().copied()
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

/// The message being written. Recipient lines are text, as typed (`Ada <ada@example.org>; bob@…`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Compose {
    pub mode: ComposeMode,
    /// The message replied to or forwarded.
    pub source: Option<Id>,
    /// The saved draft this message continues.
    pub draft: Option<Id>,
    /// The account it is sent from.
    pub account: Id,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub subject: String,
    /// The text of the message. In HTML form its lines are its paragraphs and a picture is one
    /// `PICTURE` character; `styles` says how it looks.
    pub body: String,
    /// Written in HTML (Format Text › HTML), with the formatting in `styles`; otherwise plain
    /// text.
    #[serde(skip_serializing_if = "is_false")]
    pub html: bool,
    /// How the body looks, in HTML form (always fitted to it again before it is used).
    #[serde(skip_serializing_if = "Styles::is_plain")]
    pub styles: Styles,
    /// The characters of the body that are selected (a caret where both are the same), as the
    /// editor last said: what formatting acts on when a command doesn't say.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<(usize, usize)>,
    /// The look chosen at the caret with nothing selected (Bold, then typing): the text typed
    /// next takes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing: Option<CharStyle>,
    pub attachments: Vec<Attachment>,
    pub importance: Importance,
    pub in_reply_to: String,
    pub references: Vec<String>,
    /// Changed since it was opened or last saved.
    pub dirty: bool,
    /// Do not deliver before this time (Options › Delay Delivery): the message waits in the
    /// Outbox until then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_after: Option<Timestamp>,
    /// Ask for a read receipt (`Disposition-Notification-To`).
    #[serde(skip_serializing_if = "is_false")]
    pub read_receipt: bool,
    /// Ask for a delivery receipt (`Return-Receipt-To`).
    #[serde(skip_serializing_if = "is_false")]
    pub delivery_receipt: bool,
    /// Sign it (Options › Encrypt › Sign): S/MIME or OpenPGP, with the owner's own key.
    #[serde(skip_serializing_if = "is_false")]
    pub sign: bool,
    /// Encrypt it (Options › Encrypt › Encrypt), to every recipient's key and the owner's own.
    #[serde(skip_serializing_if = "is_false")]
    pub encrypt: bool,
}

/// What to do when a message that is opened asks for a read receipt (File › Options › Mail).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReceiptPolicy {
    /// Ask each time: the reading pane says so in a bar with Send and Don't Send.
    #[default]
    Ask,
    /// Send one, without asking, where that is safe; ask where it isn't.
    Always,
    /// Never send one, and don't ask.
    Never,
}

/// Preferences the engine acts on (File › Options); the front end saves them between runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Prefs {
    /// `3:05 PM` rather than `15:05`.
    pub twelve_hour: bool,
    pub week_start: Weekday,
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
    /// Write new messages, replies and forwards as plain text rather than HTML (File › Options ›
    /// Mail; each message can still be switched with Format Text › HTML or Plain Text).
    #[serde(skip_serializing_if = "is_false")]
    pub compose_plain: bool,
    /// Lines of body text under each message in the list (0–3).
    pub preview_lines: u8,
    /// Most messages one synchronisation downloads per folder (0: all).
    pub sync_limit: u32,
    /// Minutes between automatic Send/Receive runs (0: only when asked).
    pub sync_minutes: u32,
    /// Seconds between the runs over the Inbox and the folders under it alone, while no run is
    /// under way (`SyncScope::InboxTree`; 0: only with the rounds every `sync_minutes`). Set
    /// with `ui.inboxSeconds` in the app.
    pub inbox_seconds: u32,
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
    /// A third time zone the time bar shows, left of the second (`view.timeZone` with
    /// `"third"`); empty for none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub third_zone: String,
    /// What the time bar calls the two zones, when the owner gave them names; else the zones'
    /// own.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub zone_label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub second_zone_label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub third_zone_label: String,
    /// The zones the owner chose lately, the latest first: a zone picker lists them after the
    /// computer's own.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recent_zones: Vec<String>,
    /// What to do about a request for a read receipt in a message that is opened.
    #[serde(skip_serializing_if = "is_default")]
    pub read_receipts: ReceiptPolicy,
    /// Keep the certificates and keys that signed messages bring, so that mail to their senders
    /// can be encrypted (File › Options › Trust Center). On unless the owner turns it off; only
    /// public keys are kept.
    pub collect_keys: bool,
    /// New messages start with Sign on.
    #[serde(skip_serializing_if = "is_false")]
    pub sign_by_default: bool,
    /// New messages start with Encrypt on.
    #[serde(skip_serializing_if = "is_false")]
    pub encrypt_by_default: bool,
    /// Which kind to sign and encrypt with when both could be: `smime`, `openpgp`, or empty for
    /// whichever the owner and the recipients have (S/MIME first).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub security_format: String,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            twelve_hour: true,
            week_start: Weekday::Sun,
            work_days: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri],
            work_start: 8,
            work_end: 17,
            mark_read_on_open: true,
            default_reminder: Some(15),
            quote_on_reply: true,
            compose_plain: false,
            preview_lines: 1,
            sync_limit: 500,
            sync_minutes: 5,
            inbox_seconds: 60,
            time_scale: 30,
            week_numbers: false,
            delete_answered_requests: false,
            own_work_hours: false,
            mailbox_hours: None,
            own_favorites: false,
            time_zone: String::new(),
            second_zone: String::new(),
            third_zone: String::new(),
            zone_label: String::new(),
            second_zone_label: String::new(),
            third_zone_label: String::new(),
            recent_zones: Vec::new(),
            read_receipts: ReceiptPolicy::Ask,
            collect_keys: true,
            sign_by_default: false,
            encrypt_by_default: false,
            security_format: String::new(),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
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

struct Undo {
    label: &'static str,
    mailbox: Mailbox,
    sel: Selection,
    folder: Id,
}

pub struct Session {
    pub mailbox: Mailbox,
    pub view: ViewState,
    pub sel: Selection,
    /// The message being written, if one is open.
    pub compose: Option<Compose>,
    pub prefs: Prefs,
    pub registry: Arc<Registry>,
    /// The mailbox file, once opened from or saved to one.
    pub path: Option<PathBuf>,
    /// The mailbox changed since it was opened or saved.
    pub dirty: bool,
    pub clock: Clock,
    /// What the last synchronisation reported, for the status bar.
    pub sync_status: String,
    history: Vec<Undo>,
    redo: Vec<Undo>,
    /// Why there is nothing to undo, when the history was let go of for a change that can't
    /// be undone on the mail server ([`Session::seal_history`]); `None` once there is again.
    sealed: Option<&'static str>,
    rev: u64,
    list_cache: Option<(String, Arc<Vec<Row>>)>,
    /// Private keys and opened messages, in memory for this session only ([`crate::security`]).
    pub(crate) security: crate::security::SecurityState,
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
            prefs: Prefs::default(),
            registry: Arc::new(crate::cmd::registry()),
            path: None,
            dirty: false,
            clock,
            sync_status: String::new(),
            history: Vec::new(),
            redo: Vec::new(),
            sealed: None,
            rev: 0,
            list_cache: None,
            security: Default::default(),
            content_source: None,
            loads: 0,
            unsearched: 0,
            search_time: SEARCH_TIME,
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

    /// The message the reading pane shows.
    pub fn focused_message(&self) -> Option<&Message> {
        self.sel.focus().and_then(|id| self.mailbox.message(id))
    }

    /// The rows of the message list for the folder, filter, search and sort in view. A search
    /// that reads bodies brings the bodies it lacks into memory first, newest first, for as long
    /// as [`Session::search_time`] allows; what it didn't get to is counted in
    /// [`Session::unsearched`].
    pub fn rows(&mut self) -> Arc<Vec<Row>> {
        let key = format!("{}|{}|{}|{}", self.rev, self.loads, self.today().to_days(), serde_json::to_string(&self.view).unwrap_or_default());
        if let Some((k, rows)) = &self.list_cache
            && *k == key
        {
            return rows.clone();
        }
        self.load_bodies_for_search();
        let key = format!("{}|{}|{}|{}", self.rev, self.loads, self.today().to_days(), serde_json::to_string(&self.view).unwrap_or_default());
        let rows = Arc::new(crate::list::rows(self));
        self.list_cache = Some((key, rows.clone()));
        rows
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
        let m = self.mailbox.message(message).ok_or_else(|| CmdError::Failed(format!("no message with id {message}")))?;
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
                let a = m.attachments.get(i).ok_or_else(|| CmdError::Failed(format!("the message has no attachment {i}")))?;
                match a.kept.filter(|_| a.data.is_empty()) {
                    Some(r) => self.fetch(&r),
                    None => Ok(a.data.clone()),
                }
            }
        }
    }

    /// A copy of message `id` with its body and the attachments `pick` chooses in memory (the
    /// message itself when nothing had to be fetched), without changing the mailbox: for what
    /// writes a message out (MIME to send or export, a print, a forward).
    pub fn loaded_message_where(&self, id: Id, pick: impl Fn(&Attachment) -> bool) -> Result<Arc<Message>, CmdError> {
        let m = self.mailbox.messages.get(&id).ok_or_else(|| CmdError::Failed(format!("no message with id {id}")))?;
        if !m.body_is_kept() && m.attachments.iter().all(|a| a.is_loaded() || !pick(a)) {
            return Ok(m.clone());
        }
        let mut copy = Message::clone(m);
        loaded_into(&mut copy, |part| self.content(id, part), &pick)?;
        Ok(Arc::new(copy))
    }

    /// [`Session::loaded_message_where`] with every attachment.
    pub fn loaded_message(&self, id: Id) -> Result<Arc<Message>, CmdError> {
        self.loaded_message_where(id, |_| true)
    }

    /// Bring the body of message `id` and the attachments `pick` chooses into memory, in the
    /// mailbox itself, so that everything that reads the message finds them. Not a change to the
    /// mailbox (nothing to undo or save), though caches that read bodies see it. Returns whether
    /// anything was brought in. A body whose file is missing or damaged says so in its text from
    /// then on, and the error says why.
    pub fn load_where(&mut self, id: Id, pick: impl Fn(&Attachment) -> bool) -> Result<bool, CmdError> {
        let Some(m) = self.mailbox.messages.get(&id) else { return Err(CmdError::Failed(format!("no message with id {id}"))) };
        if !m.body_is_kept() && m.attachments.iter().all(|a| a.is_loaded() || !pick(a)) {
            return Ok(false);
        }
        let mut copy = Message::clone(m);
        let loaded = loaded_into(&mut copy, |part| self.content(id, part), &pick);
        if let Err(e) = &loaded
            && copy.body_is_kept()
            && let Some(kept) = copy.kept.as_deref_mut()
        {
            // Said once, in the message, instead of being asked for again every frame.
            log::warn!("message {id}: its text couldn't be read ({e})");
            kept.failed = true;
            copy.body.text = crate::store_placeholder().to_string();
            copy.body.html = None;
        }
        self.mailbox.messages.insert(id, Arc::new(copy));
        self.loads += 1;
        loaded.map(|_| true)
    }

    /// Bring everything of message `id` into memory ([`Session::load_where`]).
    pub fn load_message(&mut self, id: Id) -> Result<bool, CmdError> {
        self.load_where(id, |_| true)
    }

    /// Bring in what showing message `id` needs: its body, the pictures its HTML shows in place
    /// (`cid:`), and any calendar part (an invitation's bar reads it). Other attachments wait
    /// until they are opened or saved.
    pub fn load_for_display(&mut self, id: Id) -> Result<bool, CmdError> {
        // The body first: which pictures are shown is in its HTML.
        let body = self.load_where(id, |_| false)?;
        let Some(m) = self.mailbox.message(id) else { return Ok(body) };
        let html = m.body.html.clone().unwrap_or_default();
        let parts =
            self.load_where(id, move |a| is_calendar_part(a) || (!a.content_id.is_empty() && html.contains(&format!("cid:{}", a.content_id))))?;
        Ok(body || parts)
    }

    /// Whether showing message `id` would bring anything into memory ([`Session::load_for_display`]).
    pub fn needs_load_for_display(&self, id: Id) -> bool {
        let Some(m) = self.mailbox.message(id) else { return false };
        if m.body_is_kept() {
            return true;
        }
        let html = m.body.html.as_deref().unwrap_or("");
        m.attachments
            .iter()
            .any(|a| !a.is_loaded() && (is_calendar_part(a) || (!a.content_id.is_empty() && html.contains(&format!("cid:{}", a.content_id)))))
    }

    /// The whole mailbox with every part in memory, as a self-contained file needs it (File ›
    /// Save As, an export, the browser's storage): a copy when anything had to be fetched. An
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

    /// A synchronisation's request with the messages it carries to the server (the Outbox, the
    /// uploads) whole: a message whose content can't be read is left out of this run, with a
    /// line in the log, rather than sent without its files.
    pub fn loaded_request(&self, mut request: mailcraft_model::sync::SyncRequest) -> mailcraft_model::sync::SyncRequest {
        let whole = |m: &Message| -> Option<Message> {
            if m.is_loaded() {
                return None;
            }
            match self.loaded_message(m.id) {
                Ok(loaded) => Some(Message::clone(&loaded)),
                Err(e) => {
                    log::warn!("message {} stays here this time: {e}", m.id);
                    Some(Message { id: 0, ..Message::default() })
                }
            }
        };
        for m in &mut request.outbox {
            if let Some(loaded) = whole(m) {
                *m = loaded;
            }
        }
        request.outbox.retain(|m| m.id != 0);
        for up in &mut request.uploads {
            if let Some(loaded) = whole(&up.message) {
                up.message = loaded;
            }
        }
        request.uploads.retain(|up| up.message.id != 0);
        request
    }

    /// Before the list is built for a search that reads bodies: the bodies it lacks, newest
    /// first, within [`Session::search_time`]; the rest are counted as not searched.
    fn load_bodies_for_search(&mut self) {
        self.unsearched = 0;
        let saved = crate::cmd::search_folders::in_view(self);
        let query = match saved {
            Some(f) => mailcraft_model::search::Query::parse(&format!("{} {}", f.query, self.view.search)),
            None => mailcraft_model::search::Query::parse(&self.view.search),
        };
        if query.is_empty() || !query.reads_bodies() {
            return;
        }
        let everywhere = saved.is_some() || self.view.scope == Scope::Everywhere;
        let folder = self.view.folder;
        let mut wanted: Vec<(Timestamp, Id)> =
            self.mailbox.messages.values().filter(|m| (everywhere || m.folder == folder) && m.body_is_kept()).map(|m| (m.date, m.id)).collect();
        if wanted.is_empty() {
            return;
        }
        wanted.sort_unstable_by(|a, b| b.cmp(a));
        let deadline = std::time::Instant::now() + self.search_time;
        let mut left = wanted.len();
        for (_, id) in wanted {
            if std::time::Instant::now() >= deadline {
                break;
            }
            // A body that can't be read says so in its place, and is not asked for again.
            let _ = self.load_where(id, |_| false);
            left -= 1;
        }
        self.unsearched = left;
    }

    /// Keep the view and the selection pointing at things that exist.
    pub fn normalize(&mut self) {
        if self.mailbox.folder(self.view.folder).is_none() && crate::cmd::search_folders::in_view(self).is_none() {
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
        if self.sel.group.is_some_and(|g| !mb.contact_groups.iter().any(|x| x.id == g)) {
            self.sel.group = None;
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

    /// Replace the mailbox (File › New, File › Open): history and selection start over.
    pub fn replace_mailbox(&mut self, mailbox: Mailbox) {
        self.mailbox = mailbox;
        self.history.clear();
        self.redo.clear();
        self.sealed = None;
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
        Undo { label, mailbox: self.mailbox.clone(), sel: self.sel.clone(), folder: self.view.folder }
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

    /// [`forget_history`](Self::forget_history) for a change that can't be undone on the mail
    /// server (a message deleted for good there): Undo then says `why` instead of only that
    /// there is nothing to undo, until something is done that can be.
    pub fn seal_history(&mut self, why: &'static str) {
        self.forget_history();
        self.sealed = Some(why);
    }

    /// Why Undo can't go back past a change the mail server took on, when that is so.
    pub fn sealed_history(&self) -> Option<&'static str> {
        if self.history.is_empty() { self.sealed } else { None }
    }

    /// The mailboxes Undo and Redo would go back and forth to.
    pub(crate) fn snapshots(&self) -> impl Iterator<Item = &Mailbox> {
        self.history.iter().chain(&self.redo).map(|u| &u.mailbox)
    }

    /// Bring every mailbox Undo and Redo would go back and forth to up to what the mail server
    /// took on since it was kept (a move it made, a message it now has), so that going back to
    /// one of them asks the server for the opposite instead of losing track of what it has.
    pub(crate) fn rebase_history(&mut self, mut rebase: impl FnMut(&mut Mailbox)) {
        for u in self.history.iter_mut().chain(self.redo.iter_mut()) {
            rebase(&mut u.mailbox);
        }
    }

    fn swap(&mut self, u: Undo) -> Undo {
        let cur = Undo {
            label: u.label,
            mailbox: std::mem::replace(&mut self.mailbox, u.mailbox),
            sel: std::mem::replace(&mut self.sel, u.sel),
            folder: self.view.folder,
        };
        self.view.folder = u.folder;
        self.rev += 1;
        self.dirty = true;
        self.normalize();
        cur
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
        // Undo and redo manage the history themselves.
        let history = spec.mutates && !matches!(id, "edit.undo" | "edit.redo");
        // The computer may have been carried to another zone since the last command: the
        // mailbox is put by that zone's clock first, and that is not this command's to undo.
        self.settle_zone();
        let before = self.snapshot(spec.label);
        let compose = self.compose.clone();
        let view = self.view.clone();
        let run = spec.run;
        // Last-resort guard (never-crash): a panic in a command becomes an error and the state is restored.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, params)));
        let result = match result {
            Ok(r) => r,
            Err(_) => {
                log::error!("command `{id}` panicked; the mailbox was restored");
                Err(CmdError::Failed(format!("{} failed unexpectedly; nothing was changed", spec.label)))
            }
        };
        match &result {
            Ok(_) => {
                if self.mailbox != before.mailbox {
                    self.rev += 1;
                    if history {
                        self.dirty = true;
                        self.sealed = None;
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
                self.view = view;
            }
        }
        result
    }
}

/// Whether an attachment is a calendar part (an invitation, an answer to one).
fn is_calendar_part(a: &Attachment) -> bool {
    let kind = a.mime.trim().to_ascii_lowercase();
    kind.starts_with("text/calendar") || kind.starts_with("application/ics") || a.name.to_ascii_lowercase().ends_with(".ics")
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

/// Bytes as the text they hold; what isn't UTF-8 is read as well as it can be.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}
