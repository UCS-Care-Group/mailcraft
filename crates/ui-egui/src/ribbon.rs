// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ribbon: the tab strip and every tab's commands. Each control runs a command by id.
//!
//! A tab is described once ([`describe`]): its groups, and in each the controls with their
//! command, icon, label and tooltip. Two layouts draw that description. The classic one sets it
//! out in three rows, in named groups. The simplified one is a single row, in which what doesn't
//! fit first loses its label and then goes behind `…` ([`plan`]).

use egui::{Align2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::{CalView, Filter, Module, Pane, SortBy, TaskFilter};
use mailcraft_model::{FolderKind, Id, Importance};
use serde_json::{Value, json};

use crate::theme::{Tokens, medium, paint_clipped, regular, semibold};
use crate::widgets::{CONTENT_H, LABEL_H, bg, big_frame, command_tooltip, group, mi, mi_check, shortcut_text, small_frame, stack};
use crate::{MailApp, icons};

/// The height of the tab strip, and the size of the tabs' names (10 pt).
const TAB_H: f32 = 30.0;
const TAB_SIZE: f32 = 13.333;
/// The gap between the ribbon and the panes under it.
const SHEET_GAP: f32 = 7.0;
/// The gap between the tabs and the ribbon.
const TOP_GAP: f32 = 5.0;

/// The simplified row: the band's height and a button's.
const BAND_H: f32 = 48.0;
const BUTTON_H: f32 = 32.0;
/// In a button: the space at its ends, its icon, the gap before its label and the label's size
/// (10 pt), the part that holds its `▾` and the gap before that part with a label and without.
const PAD: f32 = 8.0;
const ICON: f32 = 20.0;
const LABEL_GAP: f32 = 8.0;
const LABEL_SIZE: f32 = 13.333;
const DROP_W: f32 = 22.0;
const DROP_GAP_LABEL: f32 = 2.0;
const DROP_GAP_ICON: f32 = 5.0;
/// Between two buttons, and between two that a rule parts (the rule is in the middle).
const BUTTON_GAP: f32 = 2.0;
const RULE_GAP: f32 = 9.0;
/// The `…` button, and the button at the right end that chooses the layout.
const MORE_W: f32 = 36.0;
const MODE_W: f32 = 24.0;
/// A note after the buttons is drawn when this much room is left for it.
const NOTE_MIN: f32 = 180.0;

/// How the ribbon lays its commands out (the menu at its right end, View › Layout,
/// `ui.ribbonMode`): `Simplified`, one row of commands with what doesn't fit behind `…`, which
/// is what a new installation starts with; or `Classic`, three rows in groups with their names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Mode {
    #[default]
    Simplified,
    Classic,
}

impl Mode {
    /// In menu order.
    pub const ALL: [Mode; 2] = [Mode::Classic, Mode::Simplified];

    /// The value used in `ui.json` and by the `ui.ribbonMode` command.
    pub fn code(self) -> &'static str {
        match self {
            Mode::Simplified => "simplified",
            Mode::Classic => "classic",
        }
    }

    pub fn from_code(code: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.code().eq_ignore_ascii_case(code.trim()))
    }

    /// The English label (translated with `tl!` where shown).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Simplified => "Simplified Ribbon",
            Mode::Classic => "Classic Ribbon",
        }
    }
}

/// An unknown saved value reads as the default rather than discarding the whole `ui.json`.
impl From<String> for Mode {
    fn from(s: String) -> Self {
        Mode::from_code(&s).unwrap_or_default()
    }
}

impl From<Mode> for String {
    fn from(m: Mode) -> Self {
        m.code().to_string()
    }
}

/// The tabs in order. `File` opens Backstage; `Message` is there while a message is being written.
pub fn tabs(app: &MailApp) -> Vec<&'static str> {
    let mut tabs = vec!["File", "Home", "Send / Receive", "Folder", "View", "Help"];
    if app.session.compose.is_some() {
        tabs.insert(1, "Message");
    }
    tabs
}

/// The id a control of the ribbon goes by in the hit map, from its key (for most, the id of the
/// command it runs). The kinds: `ribbon` for a control, `ribbonMenu` for the `▾` half of one that
/// both runs a command and opens a menu, `ribbonMore` for `…` and `ribbonMode` for the layout
/// button (both with id 0), `ribbonBand` (id 0) for the band they are all in.
pub fn hit_id(key: &str) -> u64 {
    key.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3))
}

/// A menu a control of the ribbon opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Menu {
    NewItems,
    MoveMessages,
    Rules,
    CategorizeMail,
    FollowUp,
    FilterMail,
    ShowAs,
    Reminder,
    CategorizeEvent,
    CategorizeContact,
    CategorizeTask,
    FilterTasks,
    Signature,
    MoveFolder,
    ReadingPane,
    Preview,
    ArrangeBy,
    Theme,
    RibbonLayout,
    TimeScale,
    FontSize,
    Font,
    FontColor,
}

/// How a control is drawn in the classic layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Size {
    /// A large button: the icon over its label.
    Big,
    /// A small button: the icon beside its label, three to a column.
    Small,
    /// Not drawn there: it gathers commands into one menu for the simplified row.
    Hidden,
}

/// One control of the ribbon, as both layouts need it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Item {
    /// What the hit map and the tests call it, unique in its tab: the id of its command unless
    /// several controls run that command.
    pub key: &'static str,
    pub icon: &'static str,
    /// Its label (English; shown through `tl!`). A `\n` breaks it in two under a large button;
    /// everywhere else it reads as a space.
    pub label: &'static str,
    /// What its tooltip calls it when that is more than the label.
    pub tip: &'static str,
    /// The command it runs, with its parameters.
    pub command: Option<(&'static str, Value)>,
    /// The menu it opens.
    pub menu: Option<Menu>,
    pub size: Size,
    /// It is something that stays on, and it is on.
    pub checked: bool,
    /// Simplified row: which stretch of the row it is in. The stretches follow one another with a
    /// rule between them, and in each the controls keep the order they are described in.
    pub section: u8,
    /// Simplified row: how much it matters, 1 the most. What matters least gives way first.
    pub rank: u8,
    /// Simplified row: its icon says what it is, so its label is among the first to go.
    pub plain: bool,
    /// Simplified row: it is not a button of its own but an entry under the `▾` of the button
    /// with this key.
    pub under: Option<&'static str>,
}

impl Item {
    fn new(key: &'static str, icon: &'static str, label: &'static str, size: Size) -> Item {
        Item { key, icon, label, tip: "", command: None, menu: None, size, checked: false, section: 9, rank: 5, plain: false, under: None }
    }

    /// A large button that runs `id`.
    fn big(icon: &'static str, label: &'static str, id: &'static str, params: Value) -> Item {
        Item { command: Some((id, params)), ..Item::new(id, icon, label, Size::Big) }
    }

    /// A small button that runs `id`; `tip` is what its tooltip calls it.
    fn small(icon: &'static str, label: &'static str, tip: &'static str, id: &'static str, params: Value) -> Item {
        Item { command: Some((id, params)), tip, ..Item::new(id, icon, label, Size::Small) }
    }

    /// A large button that opens `menu`.
    fn big_menu(key: &'static str, icon: &'static str, label: &'static str, tip: &'static str, menu: Menu) -> Item {
        Item { menu: Some(menu), tip, ..Item::new(key, icon, label, Size::Big) }
    }

    /// A small button that opens `menu`.
    fn small_menu(key: &'static str, icon: &'static str, label: &'static str, tip: &'static str, menu: Menu) -> Item {
        Item { menu: Some(menu), tip, ..Item::new(key, icon, label, Size::Small) }
    }

    /// A button of the simplified row alone, whose `▾` lists the controls that are `under` it.
    fn gather(key: &'static str, icon: &'static str, label: &'static str) -> Item {
        Item { tip: label, ..Item::new(key, icon, label, Size::Hidden) }
    }

    fn key(self, key: &'static str) -> Item {
        Item { key, ..self }
    }

    fn checked(self, checked: bool) -> Item {
        Item { checked, ..self }
    }

    /// Its place in the simplified row, and how much it matters there.
    fn at(self, section: u8, rank: u8) -> Item {
        Item { section, rank, ..self }
    }

    fn plain(self) -> Item {
        Item { plain: true, ..self }
    }

    fn under(self, key: &'static str) -> Item {
        Item { under: Some(key), ..self }
    }

    /// The label on one line, in the interface language.
    fn name(&self) -> String {
        tl!(self.label).replace("-\n", "-").replace("/\n", "/").replace('\n', " ")
    }

    /// What the tooltip calls it.
    fn tip_text(&self) -> String {
        if self.tip.is_empty() { self.name() } else { tl!(self.tip).to_string() }
    }
}

/// A named group of the classic layout.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Group {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn g(title: &'static str, items: Vec<Item>) -> Group {
    Group { title, items }
}

/// A tab: its groups, and a line of explanation after them where there is something to explain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tab {
    pub groups: Vec<Group>,
    pub note: &'static str,
    /// How wide the note is set in the classic layout.
    pub note_width: f32,
}

impl Tab {
    fn of(groups: Vec<Group>) -> Tab {
        Tab { groups, note: "", note_width: 0.0 }
    }

    /// Every control, in the order it is described.
    pub(crate) fn items(&self) -> impl Iterator<Item = &Item> {
        self.groups.iter().flat_map(|g| g.items.iter())
    }
}

/// The tab in view, as the window stands.
pub(crate) fn describe(app: &MailApp) -> Tab {
    match app.ui.tab.as_str() {
        "Message" => message(app),
        "Send / Receive" => send_receive(app),
        "Folder" => folder(app),
        "View" => view(app),
        "Help" => help(),
        _ => match app.session.view.module {
            Module::Mail => home_mail(app),
            Module::Calendar => home_calendar(app),
            Module::People => home_people(app),
            Module::Tasks => home_tasks(app),
        },
    }
}

fn no() -> Value {
    json!({})
}

fn home_mail(app: &MailApp) -> Tab {
    let in_junk = app.session.mailbox.folder(app.session.view.folder).is_some_and(|f| f.kind == FolderKind::Junk);
    let junk = if in_junk {
        Item::small("notJunk", "Not Junk", "Not Junk", "mail.notJunk", no())
    } else {
        Item::small("junk", "Junk", "Move to Junk Email", "mail.junk", no())
    };
    let pinned = app.session.focused_message().is_some_and(|m| m.pinned);
    Tab::of(vec![
        g(
            "New",
            vec![
                Item::big("newMail", "New\nEmail", "mail.new", no()).at(0, 1),
                Item::big_menu("newItems", "plus", "New\nItems", "New Items", Menu::NewItems).under("mail.new"),
            ],
        ),
        g(
            "Delete",
            vec![
                Item::big("delete", "Delete", "mail.delete", no()).at(1, 1).plain(),
                Item::big("archive", "Archive", "mail.archive", no()).at(1, 2).plain(),
                junk.under("mail.delete"),
                Item::small("deletePermanently", "Delete for Good", "Permanently Delete", "mail.deletePermanently", no()).under("mail.delete"),
            ],
        ),
        g(
            "Respond",
            vec![
                Item::big("reply", "Reply", "mail.reply", no()).at(2, 1).plain(),
                Item::big("replyAll", "Reply\nAll", "mail.replyAll", no()).at(2, 2).plain(),
                Item::big("forward", "Forward", "mail.forward", no()).at(2, 2).plain(),
                Item::small("meeting", "Meeting", "Reply with Meeting", "calendar.fromMessage", no()).at(5, 6),
                Item::small("newContact", "Add Sender", "Add to Contacts", "people.fromMessage", no()).at(5, 7),
            ],
        ),
        g(
            "Move",
            vec![
                Item::big_menu("moveMessages", "move", "Move", "Move to Folder", Menu::MoveMessages).at(1, 2).plain(),
                Item::big_menu("rules", "rules", "Rules", "Rules", Menu::Rules).at(4, 4),
            ],
        ),
        g(
            "Tags",
            vec![
                Item::big("toggleRead", "Unread/\nRead", "mail.toggleRead", no()).at(3, 3),
                Item::big_menu("categorize", "categorize", "Categorize", "Categorize", Menu::CategorizeMail).at(3, 3).plain(),
                Item::big_menu("followUp", "flag", "Follow\nUp", "Follow Up", Menu::FollowUp).at(3, 3).plain(),
                Item::small("pin", "Pin", "Pin to the top of the list", "mail.pin", no()).checked(pinned).at(5, 5),
            ],
        ),
        g(
            "Find",
            vec![
                Item::big_menu("filter", "filter", "Filter\nEmail", "Filter Email", Menu::FilterMail).at(4, 4),
                Item::small("findRelated", "Related", "Find Related Messages", "mail.findRelated", no()).at(5, 7),
                Item::small("findFromSender", "From Sender", "Messages from Sender", "mail.findFromSender", no()).at(5, 7),
            ],
        ),
        g("Send/Receive", vec![Item::big("sync", "Send/Receive\nAll Folders", "ui.sync", no()).at(5, 5)]),
    ])
}

/// The calendar's four views, the one in use marked, in `section` of the simplified row: Home has
/// them, and View while the calendar is in front.
fn calendar_views(cur: CalView, section: u8) -> Vec<Item> {
    vec![
        Item::big("day", "Day", "view.day", no()).checked(cur == CalView::Day).at(section, 2),
        Item::big("workWeek", "Work\nWeek", "view.workWeek", no()).checked(cur == CalView::WorkWeek).at(section, 2),
        Item::big("week", "Week", "view.week", no()).checked(cur == CalView::Week).at(section, 2),
        Item::big("month", "Month", "view.month", no()).checked(cur == CalView::Month).at(section, 2),
    ]
}

fn home_calendar(app: &MailApp) -> Tab {
    let cur = app.session.view.calendar;
    let selected = app.session.sel.event.and_then(|id| app.session.mailbox.event(id));
    let private = selected.is_some_and(|e| e.private);
    let respond = |icon, label, key, answer: &str| Item::small(icon, label, label, "calendar.respond", json!({"response": answer})).key(key).at(4, 4);
    // The simplified row reads: the new items | today and the next seven days | the four views,
    // with their names (which keep them longest: their icons alone don't tell them apart), the
    // one in use marked | and, while an event is selected, what is done with it and the answers
    // to somebody else's meeting | the rest.
    let mut groups = vec![
        g(
            "New",
            vec![
                Item::big("newAppointment", "New\nAppointment", "calendar.new", no()).at(0, 1),
                Item::big("newMeeting", "New\nMeeting", "calendar.newMeeting", no()).at(0, 3),
                Item::big_menu("newItems", "plus", "New\nItems", "New Items", Menu::NewItems).under("calendar.new"),
            ],
        ),
        g(
            "Go To",
            vec![
                Item::big("today", "Today", "view.today", no()).at(1, 3),
                Item::big("week", "Next 7\nDays", "view.next7Days", no()).at(1, 4),
                // (Back and Forward are in the calendar's own header: in the row they are under Today.)
                Item::small("previous", "Back", "Back", "view.previous", no()).under("view.today"),
                Item::small("next", "Forward", "Forward", "view.next", no()).under("view.today"),
                Item::small("calendar", "Go to Date", "Go to Date", "view.goToDate", no()).under("view.today"),
            ],
        ),
        g("Arrange", calendar_views(cur, 2)),
    ];
    // What is done with an appointment is there while one is selected (in Outlook, a tab of its
    // own comes up for it), and the answers to a meeting while it is somebody else's.
    if let Some(event) = selected {
        // High and Low Importance are each on or off: pressing the one that is on turns it off.
        let (high, low) = (event.importance == Importance::High, event.importance == Importance::Low);
        let matters = |on: bool, value: &str| json!({"value": if on { "normal" } else { value }});
        let mut items = vec![
            Item::big("edit", "Open", "calendar.edit", no()).at(3, 5),
            Item::big("delete", "Delete", "calendar.delete", no()).at(3, 5).plain(),
            Item::small("forward", "Forward", "Forward", "calendar.forward", no()).at(3, 7).plain(),
            Item::small_menu("showAs", "showAs", "Show As", "Show As", Menu::ShowAs).at(3, 6),
            Item::small_menu("reminder", "reminder", "Reminder", "Reminder", Menu::Reminder).at(3, 6),
            Item::small("private", "Private", "Private", "calendar.private", no()).checked(private).at(3, 6),
            Item::small("importanceHigh", "High Importance", "High Importance", "calendar.importance", matters(high, "high"))
                .key("calendar.importanceHigh")
                .checked(high)
                .at(3, 7)
                .plain(),
            Item::small("importanceLow", "Low Importance", "Low Importance", "calendar.importance", matters(low, "low"))
                .key("calendar.importanceLow")
                .checked(low)
                .at(3, 7)
                .plain(),
            Item::big_menu("categorize", "categorize", "Categorize", "Categorize", Menu::CategorizeEvent).at(3, 5).plain(),
        ];
        // An online meeting that has a link to join by.
        if event.join_link().is_some() {
            items.insert(2, Item::small("joinMeeting", "Join", "Join Online Meeting", "ui.joinMeeting", no()).at(3, 4));
        }
        groups.push(g("Appointment", items));
        if crate::calendar_view::invitation(app, event).0 {
            let mut answers = vec![
                respond("accept", "Accept", "calendar.accept", "accepted"),
                respond("tentative", "Tentative", "calendar.tentative", "tentative"),
                respond("decline", "Decline", "calendar.decline", "declined"),
            ];
            // Another time can be proposed for a meeting that an account's server keeps, when
            // its organizer takes proposals.
            let kept = mailcraft_engine::cmd::remote::keeper(&app.session.mailbox, &event.origin).is_some();
            if kept && event.origin.guest && event.response_options.allow_proposals {
                answers.push(Item::small("proposeTime", "Propose New Time", "Propose New Time", "calendar.propose", no()).at(4, 5));
            }
            groups.push(g("Respond", answers));
        }
    }
    // Somebody else's calendar, by their address (#70).
    groups.push(g("Manage Calendars", vec![Item::big("calendar", "Open\nCalendar", "calendar.openShared", no()).at(4, 9)]));
    groups.push(g("Share", vec![Item::big("export", "Save\nCalendar", "file.exportCalendar", no()).at(5, 6)]));
    Tab::of(groups)
}

fn home_people(app: &MailApp) -> Tab {
    let favorite = app.session.sel.contact.and_then(|id| app.session.mailbox.contact(id)).is_some_and(|c| c.favorite);
    Tab::of(vec![
        g(
            "New",
            vec![
                Item::big("newContact", "New\nContact", "people.new", no()).at(0, 1),
                Item::big_menu("newItems", "plus", "New\nItems", "New Items", Menu::NewItems).under("people.new"),
            ],
        ),
        g("Delete", vec![Item::big("delete", "Delete", "people.delete", no()).at(1, 2).plain()]),
        g("Communicate", vec![Item::big("mail", "Email", "people.email", no()).at(2, 2)]),
        g(
            "Actions",
            vec![
                Item::big("edit", "Edit\nContact", "people.edit", no()).at(3, 2),
                Item::big(if favorite { "starOn" } else { "star" }, "Favorite", "people.favorite", no()).checked(favorite).at(3, 3),
                Item::big_menu("categorize", "categorize", "Categorize", "Categorize", Menu::CategorizeContact).at(3, 3).plain(),
            ],
        ),
        g(
            "Share",
            vec![
                Item::big("export", "Export\nContacts", "file.exportContacts", no()).at(4, 5),
                Item::big("import", "Import", "ui.import", no()).at(4, 5),
            ],
        ),
    ])
}

fn home_tasks(app: &MailApp) -> Tab {
    let task = app.session.sel.task.and_then(|id| app.session.mailbox.task(id));
    let done = task.is_some_and(|x| x.is_complete());
    let importance = task.map(|x| x.importance);
    let today = app.session.today();
    let to_friday = (4 - i64::from(today.weekday().index())).max(0);
    let due = |icon, label, tip, key, date: Value| Item::small(icon, label, tip, "tasks.edit", json!({"due": date})).key(key).under("followUp");
    let weight = |icon, label, key, which: Importance, code: &str| {
        let on = importance == Some(which);
        Item::small(icon, label, label, "tasks.edit", json!({"importance": if on { "normal" } else { code }})).key(key).checked(on).at(3, 5)
    };
    Tab::of(vec![
        g(
            "New",
            vec![
                Item::big("newTask", "New\nTask", "tasks.new", no()).at(0, 1),
                Item::big_menu("newItems", "plus", "New\nItems", "New Items", Menu::NewItems).under("tasks.new"),
            ],
        ),
        g("Delete", vec![Item::big("delete", "Delete", "tasks.delete", no()).at(1, 2).plain()]),
        g(
            "Manage Task",
            vec![
                Item::big("complete", "Mark\nComplete", "tasks.complete", no()).checked(done).at(2, 2),
                Item::big("edit", "Edit\nTask", "tasks.edit", no()).at(2, 3),
            ],
        ),
        g(
            "Follow Up",
            vec![
                Item::gather("followUp", "flag", "Follow Up").at(3, 3).plain(),
                due("flag", "Today", "Due today", "tasks.due.today", json!(today.iso())),
                due("flag", "Tomorrow", "Due tomorrow", "tasks.due.tomorrow", json!(today.add_days(1).iso())),
                due("flag", "This Week", "Due this week", "tasks.due.thisWeek", json!(today.add_days(to_friday).iso())),
                due("flag", "Next Week", "Due next week", "tasks.due.nextWeek", json!(today.add_days(to_friday + 7).iso())),
                due("clearFlag", "No Date", "No due date", "tasks.due.none", Value::Null),
            ],
        ),
        g(
            "Tags",
            vec![
                Item::big_menu("categorize", "categorize", "Categorize", "Categorize", Menu::CategorizeTask).at(3, 4).plain(),
                weight("highImportance", "High Importance", "tasks.importance.high", Importance::High, "high"),
                weight("lowImportance", "Low Importance", "tasks.importance.low", Importance::Low, "low"),
            ],
        ),
        g("Current View", vec![Item::big_menu("filter", "filter", "Filter", "Filter Tasks", Menu::FilterTasks).at(4, 4)]),
    ])
}

/// How the text the selection covers looks (with nothing selected, how the next character typed
/// will), for the buttons that say so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// The size all of it has: `Some(None)` the message's own.
    pub size: Option<Option<u8>>,
    /// The font and the colour all of it has, the same way.
    pub font: Option<Option<mailcraft_model::rich::Font>>,
    pub color: Option<Option<String>>,
    pub bullets: bool,
    pub numbering: bool,
    pub link: bool,
    pub plain: bool,
}

/// How the selection of the text being edited looks ([`mailcraft_engine::Session::doc`]).
pub(crate) fn look_now(app: &MailApp) -> Look {
    app.session.doc().map(|(body, plain)| look(body, plain)).unwrap_or_default()
}

pub(crate) fn look(body: &mailcraft_model::RichBody, plain: bool) -> Look {
    use mailcraft_model::rich::ListKind;
    let sel = body.selection();
    let text = body.text();
    let (from, to) = (sel.start(), sel.end());
    let all = |f: &dyn Fn(&mailcraft_model::rich::Style) -> bool| {
        if sel.is_empty() { f(&body.typing_style()) } else { text.all(from, to, f).unwrap_or(false) }
    };
    let first = if sel.is_empty() { body.typing_style() } else { text.style_at(from + 1) };
    Look {
        bold: all(&|s| s.bold),
        italic: all(&|s| s.italic),
        underline: all(&|s| s.underline),
        size: all(&|s| s.size == first.size).then_some(first.size),
        font: all(&|s| s.font == first.font).then_some(first.font),
        color: all(&|s| s.color == first.color).then(|| first.color.clone()),
        bullets: text.all_lists(from, to, ListKind::Bullet).unwrap_or(false),
        numbering: text.all_lists(from, to, ListKind::Numbered).unwrap_or(false),
        link: if sel.is_empty() { text.link_span(sel.head).is_some() } else { text.all(from, to, |s| s.link.is_some()).unwrap_or(false) },
        plain,
    }
}

/// The font sizes offered, in points.
pub(crate) const FONT_SIZES: [u8; 12] = [8, 9, 10, 11, 12, 14, 16, 18, 20, 24, 28, 36];

fn message(app: &MailApp) -> Tab {
    let importance = app.session.compose.as_ref().map(|c| c.importance);
    let look = look_now(app);
    Tab::of(vec![
        g("Send", vec![Item::big("send", "Send", "compose.send", no()).at(0, 1)]),
        g(
            "Basic Text",
            vec![
                Item::small_menu("font", "font", "Font", "Font", Menu::Font).at(1, 4).plain(),
                Item::small_menu("fontSize", "fontSize", "Font Size", "Font Size", Menu::FontSize).at(1, 4).plain(),
                Item::small_menu("fontColor", "fontColor", "Font Color", "Font Color", Menu::FontColor).at(1, 3).plain(),
                Item::small("bold", "Bold", "Bold", "format.bold", no()).checked(look.bold && !look.plain).at(1, 2).plain(),
                Item::small("italic", "Italic", "Italic", "format.italic", no()).checked(look.italic && !look.plain).at(1, 2).plain(),
                Item::small("underline", "Underline", "Underline", "format.underline", no()).checked(look.underline && !look.plain).at(1, 2).plain(),
                Item::small("bullets", "Bullets", "Bullets", "format.bullets", no()).checked(look.bullets).at(1, 3).plain(),
                Item::small("numbering", "Numbering", "Numbering", "format.numbering", no()).checked(look.numbering).at(1, 3).plain(),
                Item::small("decreaseIndent", "Decrease Indent", "Decrease Indent", "format.outdent", no()).at(1, 4).plain(),
                Item::small("increaseIndent", "Increase Indent", "Increase Indent", "format.indent", no()).at(1, 4).plain(),
                Item::small("clearFormatting", "Clear Formatting", "Clear All Formatting", "format.clear", no()).at(1, 5).plain(),
            ],
        ),
        g("Names", vec![Item::big("checkNames", "Check\nNames", "compose.checkNames", no()).at(3, 4)]),
        g(
            "Include",
            vec![
                Item::big("attach", "Attach\nFile", "ui.attach", no()).at(2, 2),
                Item::big("insertLink", "Link", "compose.link", no()).checked(look.link && !look.plain).at(2, 3),
                Item::big_menu("signature", "signature", "Signature", "Signature", Menu::Signature).at(2, 3),
            ],
        ),
        g(
            "Tags",
            vec![
                Item::small("highImportance", "High Importance", "High Importance", "compose.highImportance", no())
                    .checked(importance == Some(Importance::High))
                    .at(4, 4),
                Item::small("lowImportance", "Low Importance", "Low Importance", "compose.lowImportance", no())
                    .checked(importance == Some(Importance::Low))
                    .at(4, 4),
            ],
        ),
        g(
            "Format",
            vec![
                Item::small("formatHtml", "HTML", "Format as HTML", "compose.formatHtml", no()).checked(!look.plain).at(5, 5),
                Item::small("formatPlain", "Plain Text", "Format as Plain Text", "compose.formatPlain", no()).checked(look.plain).at(5, 5),
            ],
        ),
        g(
            "Draft",
            vec![
                Item::big("saveDraft", "Save\nDraft", "compose.saveDraft", no()).at(6, 2),
                Item::big("discard", "Discard", "compose.discard", no()).at(6, 3).plain(),
                Item::big("close", "Close", "compose.close", no()).at(6, 3),
            ],
        ),
    ])
}

fn send_receive(app: &MailApp) -> Tab {
    let note = if app.services.sync.is_none() {
        "This build can't connect to mail servers. Use the desktop app, or bring mail in with File › Import."
    } else if !app.session.enabled("sync.request") {
        "No account is connected to a mail server yet."
    } else {
        ""
    };
    Tab {
        groups: vec![
            g(
                "Send & Receive",
                vec![
                    Item::big("sync", "Send/Receive\nAll Folders", "ui.sync", no()).at(0, 1),
                    Item::big("syncProgress", "Show\nProgress", "ui.dialog", json!({"name": "syncProgress"})).key("sync.progress").at(0, 3),
                ],
            ),
            g(
                "Accounts",
                vec![
                    Item::big("account", "Add\nAccount", "ui.dialog", json!({"name": "account"})).key("account.add").at(1, 2),
                    Item::big("info", "Account\nSettings", "ui.backstage", json!({"value": true, "page": "info"})).key("account.settings").at(1, 2),
                ],
            ),
        ],
        note,
        note_width: 360.0,
    }
}

fn folder(app: &MailApp) -> Tab {
    // (As the folder pane lists it: by Outlook's own list where an account has one.)
    let favorite = mailcraft_engine::cmd::folder::favorites(&app.session).contains(&app.session.view.folder);
    Tab::of(vec![
        g("New", vec![Item::big("folderNew", "New\nFolder", "folder.new", no()).at(0, 1)]),
        g(
            "Actions",
            vec![
                Item::big("rename", "Rename\nFolder", "folder.rename", no()).at(1, 2),
                Item::big_menu("moveFolder", "move", "Move\nFolder", "Move Folder", Menu::MoveFolder).at(1, 2),
                Item::big("delete", "Delete\nFolder", "folder.delete", no()).at(1, 2),
            ],
        ),
        g(
            "Clean Up",
            vec![
                Item::big("markAllRead", "Mark All\nas Read", "mail.markAllRead", no()).at(2, 3),
                Item::big("rules", "Run Rules\nNow", "rules.run", no()).at(2, 4),
                Item::big("empty", "Empty\nFolder", "folder.empty", no()).at(2, 4),
            ],
        ),
        g(
            "Favorites",
            vec![
                Item::big(
                    if favorite { "starOn" } else { "star" },
                    if favorite { "Remove from\nFavorites" } else { "Add to\nFavorites" },
                    "folder.favorite",
                    no(),
                )
                .checked(favorite)
                .at(3, 3),
            ],
        ),
        g(
            "Export",
            vec![
                Item::big("export", "Export\nFolder", "file.exportFolder", no()).at(4, 5),
                Item::big("import", "Import", "ui.import", no()).at(4, 5),
            ],
        ),
        g("Properties", vec![Item::big("info", "Folder\nProperties", "folder.properties", no()).at(5, 5)]),
    ])
}

fn view(app: &MailApp) -> Tab {
    let v = &app.session.view;
    let mut tab = view_of_any(app);
    if v.module == Module::Calendar {
        // The calendar's own, in place of the messages' preview and sorting, which mean nothing
        // here: its four views, as Home has them (Outlook's View tab has them too, and that is
        // where people look); what a row of its hours stands for; and whether the calendars that
        // are shown lie over one another or stand side by side.
        let scale = Item::big_menu("timeScale", "clock", "Time\nScale", "Time Scale", Menu::TimeScale).at(2, 3);
        let over = mailcraft_engine::cmd::calendar_settings::overlaid(&app.session);
        let overlay = Item::big("copy", "Overlay", "view.overlay", no()).checked(over).at(2, 3);
        let beside = Item::big("readingPane", "Side-by-\nSide", "view.sideBySide", no()).at(2, 3);
        tab.groups.retain(|group| group.title != "Messages" && group.title != "Arrangement");
        let after_layout = tab.groups.len().min(1);
        tab.groups.splice(after_layout..after_layout, [g("Arrangement", calendar_views(v.calendar, 1)), g("Calendar", vec![scale, overlay, beside])]);
    }
    tab
}

/// The View tab as every module has it: Layout, then the messages' two groups (which the calendar
/// replaces with its own, by their titles), then Window.
fn view_of_any(app: &MailApp) -> Tab {
    let v = &app.session.view;
    Tab::of(vec![
        g(
            "Layout",
            vec![
                Item::big("folderPane", "Folder\nPane", "view.folderPane", no()).checked(v.folder_pane).at(0, 1),
                Item::big_menu("readingPane", "readingPane", "Reading\nPane", "Reading Pane", Menu::ReadingPane).at(0, 2),
                Item::big_menu("ribbonLayout", "ribbon", "Ribbon\nLayout", "Ribbon Layout", Menu::RibbonLayout).at(3, 4),
            ],
        ),
        g(
            "Messages",
            vec![
                Item::big("conversations", "Show as\nConversations", "view.conversations", no()).checked(v.conversations).at(1, 2),
                Item::big_menu("preview", "previewLines", "Message\nPreview", "Message Preview", Menu::Preview).at(1, 3),
            ],
        ),
        g(
            "Arrangement",
            vec![
                Item::big_menu("arrangeBy", "sort", "Arrange\nBy", "Arrange By", Menu::ArrangeBy).at(2, 2),
                Item::big("reverseSort", "Reverse\nSort", "view.reverseSort", no()).at(2, 3),
            ],
        ),
        g(
            "Window",
            vec![
                Item::big_menu("theme", "theme", "Theme", "Interface Theme", Menu::Theme).at(3, 3),
                Item::big("bell", "Reminders\nWindow", "view.reminders", no()).at(3, 4),
                Item::big("reset", "Reset\nView", "view.reset", no()).at(3, 5),
            ],
        ),
    ])
}

fn help() -> Tab {
    Tab {
        groups: vec![g(
            "Help",
            vec![
                Item::big("search", "Search\nCommands", "ui.dialog", json!({"name": "commands"})).key("help.commands").at(0, 1),
                Item::big("about", "About\nMailCraft", "ui.dialog", json!({"name": "about"})).key("help.about").at(0, 2),
                Item::big("copy", "Copy\nDiagnostics", "ui.copyDiagnostics", no()).at(0, 3),
                Item::big("report", "Report a\nProblem", "ui.reportProblem", no()).at(0, 4),
            ],
        )],
        note: "MailCraft is open source (MIT or Apache-2.0). Every button here runs a command that scripts and agents can run too: see Search Commands.",
        note_width: 420.0,
    }
}

// ---- menus ----

/// Longest search read (the rest is ignored), so that a pasted wall of text costs nothing.
const FOLDER_SEARCH_MAX: usize = 200;

/// What the search box of a folder menu remembers while the menu is open (#74).
#[derive(Clone, Default)]
struct FolderSearch {
    text: String,
    /// The row the arrow keys are on, among the rows they can choose.
    at: usize,
    /// The frame the menu was last drawn in: a gap means it was closed, and it opens afresh.
    seen: Option<u64>,
}

/// A folder as the menu lists it: its id, name, place in the tree (`Inbox/Projects`) and depth.
type FolderRow = (Id, String, String, usize);

/// The folders whose name holds `query` (any part, ignoring case and accents), as indexes into
/// `list`. Nothing typed (or nothing but marks) is all of them.
fn folder_matches(list: &[FolderRow], query: &str) -> Vec<usize> {
    let wanted = mailcraft_model::words::fold(&query.chars().take(FOLDER_SEARCH_MAX).collect::<String>());
    list.iter().enumerate().filter(|(_, row)| wanted.is_empty() || mailcraft_model::words::fold(&row.1).contains(&wanted)).map(|(i, _)| i).collect()
}

/// A search box and then every folder of the account in view as menu items, indented; `pick` runs
/// with the chosen one. Typing narrows the list to the folders whose name matches, each shown
/// with its place in the tree; the arrow keys and Enter choose, and Escape clears the search
/// before it closes the menu (#74).
pub fn folder_menu(ui: &mut Ui, app: &mut MailApp, mut pick: impl FnMut(&mut MailApp, Id)) {
    let account = app.session.mailbox.folder(app.session.view.folder).map(|f| f.account).or(app.session.mailbox.default_account().map(|a| a.id));
    let Some(account) = account else { return };
    fn walk(app: &MailApp, account: Id, parent: Option<Id>, depth: usize, out: &mut Vec<FolderRow>) {
        if depth > 32 {
            return;
        }
        for f in app.session.mailbox.children(account, parent) {
            out.push((f.id, crate::nav::folder_name(f).to_string(), crate::nav::folder_path(&app.session.mailbox, f.id), depth));
            walk(app, account, Some(f.id), depth + 1, out);
        }
    }
    let mut list = Vec::new();
    walk(app, account, None, 0, &mut list);

    let key = ui.scope_id().with("folder-search");
    let frame = ui.ctx().cumulative_frame_nr();
    let mut st: FolderSearch = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    let fresh = st.seen.is_none_or(|seen| frame > seen.saturating_add(2));
    if fresh {
        st = FolderSearch::default();
    }
    st.seen = Some(frame);

    let edit = ui.add(egui::TextEdit::singleline(&mut st.text).hint_text(tl!("Search folders")).desired_width(200.0).char_limit(FOLDER_SEARCH_MAX));
    if fresh {
        edit.request_focus();
    }
    let searching = !mailcraft_model::words::fold(&st.text).is_empty();
    let shown = folder_matches(&list, &st.text);
    // The rows the keys can choose: not the folder in view, which can't be moved to.
    let current = app.session.view.folder;
    let choosable: Vec<usize> = shown.iter().copied().filter(|i| list.get(*i).is_some_and(|row| row.0 != current)).collect();
    st.at = st.at.min(choosable.len().saturating_sub(1));
    let (down, up, enter, escape) = ui.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            // Escape clears a search first; with none typed the menu takes it and closes.
            searching && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
        )
    });
    if escape {
        st.text.clear();
        st.at = 0;
    }
    if !choosable.is_empty() {
        if down {
            st.at = (st.at + 1) % choosable.len();
        }
        if up {
            st.at = if st.at == 0 { choosable.len() - 1 } else { st.at - 1 };
        }
    }
    let marked = if searching { choosable.get(st.at).copied() } else { None };
    let mut chosen = if enter { marked.and_then(|i| list.get(i)).map(|row| row.0) } else { None };

    ui.add_space(2.0);
    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
        if shown.is_empty() {
            ui.label(egui::RichText::new(tl!("No folders match")).color(Tokens::get(ui.ctx()).text_dim));
        }
        for i in shown {
            let Some((id, name, path, depth)) = list.get(i) else { continue };
            let label = if searching { path.clone() } else { format!("{}{name}", "    ".repeat(*depth)) };
            let button = egui::Button::new(label).selected(marked == Some(i)).min_size(vec2(200.0, 0.0));
            let response = ui.add_enabled(*id != current, button);
            if marked == Some(i) && (down || up) {
                response.scroll_to_me(None);
            }
            if response.clicked() {
                chosen = Some(*id);
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(key, st));
    if let Some(id) = chosen {
        pick(app, id);
        ui.data_mut(|d| d.insert_temp(key, FolderSearch::default()));
        ui.close();
    }
}

/// The categories as menu items that toggle one on the selection through `command`.
pub fn category_menu(ui: &mut Ui, app: &mut MailApp, command: &str, has: impl Fn(&MailApp, &str) -> bool, clear: Option<&str>) {
    let cats: Vec<(String, egui::Color32)> = app.session.mailbox.categories.iter().map(|c| (c.name.clone(), crate::theme::c32(c.color))).collect();
    for (name, color) in cats {
        let on = has(app, &name);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(r.shrink(1.0), 3.0, color);
            if on {
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "✓", medium(10.0), egui::Color32::WHITE);
            }
            if ui.add(egui::Button::new(name.as_str()).selected(on).min_size(vec2(170.0, 0.0))).clicked() {
                let _ = app.run(command, json!({"category": name}));
                ui.close();
            }
        });
    }
    if let Some(clear) = clear {
        ui.separator();
        mi(ui, app, "Clear All Categories", clear, json!({}));
    }
    ui.separator();
    if ui.add(egui::Button::new(tl!("All Categories…")).min_size(vec2(190.0, 0.0))).clicked() {
        let _ = app.run("ui.dialog", json!({"name": "categories"}));
        ui.close();
    }
}

fn new_items(ui: &mut Ui, app: &mut MailApp) {
    mi(ui, app, "Email Message", "mail.new", json!({}));
    mi(ui, app, "Appointment", "calendar.new", json!({}));
    mi(ui, app, "Meeting", "calendar.newMeeting", json!({}));
    mi(ui, app, "Contact", "people.new", json!({}));
    mi(ui, app, "Task", "tasks.new", json!({}));
    ui.separator();
    mi(ui, app, "Folder…", "folder.new", json!({}));
}

/// A menu item that isn't a command of its own: `true` when it is chosen.
fn plain_item(ui: &mut Ui, label: &str) -> bool {
    let chosen = ui.add(egui::Button::new(label).min_size(vec2(190.0, 0.0))).clicked();
    if chosen {
        ui.close();
    }
    chosen
}

/// What `menu` lists.
fn menu_contents(ui: &mut Ui, app: &mut MailApp, menu: Menu) {
    match menu {
        Menu::NewItems => new_items(ui, app),
        Menu::MoveMessages => {
            if app.session.sel.messages.is_empty() {
                ui.label(tl!("Select a message first"));
                return;
            }
            folder_menu(ui, app, |app, folder| {
                let _ = app.run("mail.move", json!({"folder": folder}));
            });
            ui.separator();
            ui.menu_button(tl!("Copy to Folder"), |ui| {
                folder_menu(ui, app, |app, folder| {
                    let _ = app.run("mail.copy", json!({"folder": folder}));
                });
            });
        }
        Menu::Rules => {
            if !app.session.sel.messages.is_empty() {
                ui.menu_button(tl!("Always Move Messages From This Sender"), |ui| {
                    folder_menu(ui, app, |app, folder| {
                        let _ = app.run("rules.fromMessage", json!({"folder": folder}));
                    });
                });
                ui.separator();
            }
            mi(ui, app, "Run Rules Now", "rules.run", json!({}));
            if plain_item(ui, tl!("Manage Rules…")) {
                let _ = app.run("ui.dialog", json!({"name": "rules"}));
            }
        }
        Menu::CategorizeMail => {
            let ids = app.session.sel.messages.clone();
            category_menu(
                ui,
                app,
                "mail.categorize",
                |app, name| !ids.is_empty() && ids.iter().all(|id| app.session.mailbox.message(*id).is_some_and(|m| m.has_category(name))),
                Some("mail.clearCategories"),
            );
        }
        Menu::FollowUp => {
            let today = app.session.today();
            let weekday = i64::from(today.weekday().index());
            for (label, days) in [("Today", 0), ("Tomorrow", 1), ("This Week", 4 - weekday), ("Next Week", 11 - weekday)] {
                mi(ui, app, label, "mail.flag", json!({"due": today.add_days(days.max(0)).iso()}));
            }
            mi(ui, app, "No Date", "mail.flag", json!({}));
            ui.separator();
            mi(ui, app, "Mark Complete", "mail.complete", json!({}));
            mi(ui, app, "Clear Flag", "mail.clearFlag", json!({}));
            ui.separator();
            mi(ui, app, "Add to Tasks", "tasks.fromMessage", json!({}));
        }
        Menu::FilterMail => {
            let filter = app.session.view.filter;
            for f in Filter::ALL {
                mi_check(ui, app, f.label(), f == filter, "view.filter", json!({"value": f}));
            }
        }
        Menu::ShowAs => {
            let shown = app.session.sel.event.and_then(|id| app.session.mailbox.event(id)).map(|e| e.show_as);
            for s in mailcraft_model::ShowAs::ALL {
                mi_check(ui, app, s.label(), shown == Some(s), "calendar.showAs", json!({"value": s.code()}));
            }
        }
        Menu::Reminder => {
            let cur = app.session.sel.event.and_then(|id| app.session.mailbox.event(id)).and_then(|e| e.reminder_minutes);
            mi_check(ui, app, "None", cur.is_none(), "calendar.reminder", json!({"minutes": null}));
            for m in [0u32, 5, 10, 15, 30, 60, 120, 1440] {
                let label = if m == 0 { tl!("At start time").to_string() } else { crate::dates::duration(i64::from(m)) };
                mi_check(ui, app, &label, cur == Some(m), "calendar.reminder", json!({"minutes": m}));
            }
        }
        Menu::CategorizeEvent => {
            let id = app.session.sel.event;
            let has = |app: &MailApp, name: &str| {
                id.and_then(|id| app.session.mailbox.event(id)).is_some_and(|e| e.categories.iter().any(|c| c.eq_ignore_ascii_case(name)))
            };
            category_menu(ui, app, "calendar.categorize", has, None);
        }
        Menu::CategorizeContact => {
            let id = app.session.sel.contact;
            let has = |app: &MailApp, name: &str| {
                id.and_then(|id| app.session.mailbox.contact(id)).is_some_and(|c| c.categories.iter().any(|x| x.eq_ignore_ascii_case(name)))
            };
            category_menu(ui, app, "people.categorize", has, None);
        }
        Menu::CategorizeTask => {
            let id = app.session.sel.task;
            let has = |app: &MailApp, name: &str| {
                id.and_then(|id| app.session.mailbox.task(id)).is_some_and(|x| x.categories.iter().any(|c| c.eq_ignore_ascii_case(name)))
            };
            category_menu(ui, app, "tasks.categorize", has, None);
        }
        Menu::FilterTasks => {
            let cur = app.session.view.task_filter;
            for f in TaskFilter::ALL {
                mi_check(ui, app, f.label(), f == cur, "view.taskFilter", json!({"value": f}));
            }
        }
        Menu::Signature => {
            let names: Vec<String> = app.session.mailbox.signatures.iter().map(|s| s.name.clone()).collect();
            if names.is_empty() {
                ui.label(tl!("No signatures yet"));
            }
            for name in &names {
                if plain_item(ui, name) {
                    let _ = app.run("compose.signature", json!({"name": name}));
                }
            }
            ui.separator();
            if plain_item(ui, tl!("Signatures…")) {
                let _ = app.run("ui.backstage", json!({"value": true, "page": "signatures"}));
            }
        }
        Menu::MoveFolder => {
            if ui.add(egui::Button::new(tl!("Top Level")).min_size(vec2(200.0, 0.0))).clicked() {
                let _ = app.run("folder.move", json!({"parent": null}));
                ui.close();
            }
            ui.separator();
            folder_menu(ui, app, |app, parent| {
                let _ = app.run("folder.move", json!({"parent": parent}));
            });
        }
        Menu::ReadingPane => {
            let cur = app.session.view.reading_pane;
            for (label, code, pane) in [("Right", "right", Pane::Right), ("Bottom", "bottom", Pane::Bottom), ("Off", "off", Pane::Off)] {
                mi_check(ui, app, label, cur == pane, "view.readingPane", json!({"value": code}));
            }
        }
        Menu::Preview => {
            let lines = app.session.prefs.preview_lines;
            for (label, n) in [("Off", 0u8), ("1 Line", 1), ("2 Lines", 2), ("3 Lines", 3)] {
                mi_check(ui, app, label, lines == n, "view.previewLines", json!({"value": n}));
            }
            ui.separator();
            let one = app.session.prefs.one_line;
            mi_check(ui, app, "Single Line", one, "view.oneLine", json!({"value": !one}));
        }
        Menu::ArrangeBy => {
            let sort = app.session.view.sort;
            for s in SortBy::ALL {
                mi_check(ui, app, s.label(), sort == s, "view.sort", json!({"by": s}));
            }
            ui.separator();
            let groups = app.session.prefs.show_groups;
            mi_check(ui, app, "Show in Groups", groups, "view.groups", json!({"value": !groups}));
        }
        Menu::Theme => {
            let theme = app.ui.theme;
            for a in crate::theme::Appearance::ALL {
                mi_check(ui, app, a.label(), a == theme, "ui.theme", json!({"value": a.code()}));
            }
        }
        Menu::RibbonLayout => {
            let mode = app.ui.ribbon_mode;
            for m in Mode::ALL {
                mi_check(ui, app, m.label(), m == mode, "ui.ribbonMode", json!({"value": m.code()}));
            }
        }
        Menu::TimeScale => crate::calendar_view::time_scale_menu(ui, app),
        Menu::Font => font_menu(ui, app),
        Menu::FontColor => color_menu(ui, app),
        Menu::FontSize => size_menu(ui, app),
    }
}

/// The font sizes (`format.size`), each in a menu entry.
pub(crate) fn size_menu(ui: &mut Ui, app: &mut MailApp) {
    let now = look_now(app).size;
    mi_check(ui, app, "Default", now == Some(None), "format.size", json!({"size": null}));
    let enabled = app.enabled("format.size");
    for size in FONT_SIZES {
        let checked = now == Some(Some(size));
        // A number reads the same in every language.
        let mark = egui::RichText::new("✓");
        let mark = if checked { mark } else { mark.color(egui::Color32::TRANSPARENT) };
        if ui.add_enabled(enabled, egui::Button::new((mark, size.to_string())).selected(checked).min_size(vec2(190.0, 0.0))).clicked() {
            let _ = app.run("format.size", json!({"size": size}));
            ui.close();
        }
    }
}

/// The fonts a message may ask for (`format.font`), each in a menu entry.
pub(crate) fn font_menu(ui: &mut Ui, app: &mut MailApp) {
    use mailcraft_model::rich::Font;
    let now = look_now(app).font;
    mi_check(ui, app, "Default", now == Some(None), "format.font", json!({"value": null}));
    for f in Font::ALL {
        mi_check(ui, app, f.label(), now == Some(Some(f)), "format.font", json!({"value": f.label()}));
    }
}

/// Text colours offered: an original palette, readable on white.
pub(crate) const COLORS: [(&str, &str); 10] = [
    ("Black", "#000000"),
    ("Grey", "#595959"),
    ("Dark Red", "#c00000"),
    ("Orange", "#c55a11"),
    ("Gold", "#bf8f00"),
    ("Green", "#2e7d32"),
    ("Teal", "#00796b"),
    ("Navy", "#1f4e79"),
    ("Blue", "#0563c1"),
    ("Purple", "#7030a0"),
];

/// The text colours (`format.color`): Automatic, the palette as swatches, and a value of the
/// owner's own (`#rrggbb`).
pub(crate) fn color_menu(ui: &mut Ui, app: &mut MailApp) {
    let now = look_now(app).color;
    let enabled = app.enabled("format.color");
    mi_check(ui, app, "Automatic", now == Some(None), "format.color", json!({"value": null}));
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(190.0);
        for (name, hex) in COLORS {
            let Ok(c) = egui::Color32::from_hex(hex) else { continue };
            let chosen = now.as_ref().is_some_and(|n| n.as_deref() == Some(hex));
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(r, 2.0, c);
            if chosen || resp.hovered() {
                ui.painter().rect_stroke(r.expand(1.5), 2.0, egui::Stroke::new(1.5, ui.visuals().text_color()), egui::StrokeKind::Outside);
            }
            if resp.on_hover_text(tl!(name)).clicked() && enabled {
                let _ = app.run("format.color", json!({"value": hex}));
                ui.close();
            }
        }
    });
    ui.horizontal(|ui| {
        let key = egui::Id::unique("mailcraft-custom-color");
        let mut typed: String = ui.data(|d| d.get_temp(key)).unwrap_or_else(|| now.clone().flatten().unwrap_or_default());
        ui.add(egui::TextEdit::singleline(&mut typed).hint_text("#rrggbb").desired_width(90.0).char_limit(7));
        let valid = mailcraft_model::rich::safe_color(&typed);
        if ui.add_enabled(enabled && valid.is_some(), egui::Button::new(tl!("Apply"))).clicked()
            && let Some(hex) = valid
        {
            let _ = app.run("format.color", json!({"value": hex}));
            ui.close();
        }
        ui.data_mut(|d| d.insert_temp(key, typed));
    });
}

/// A control that runs a command, as an entry of a menu: its name on one line, its key, a mark
/// when it is on.
fn entry(ui: &mut Ui, app: &mut MailApp, item: &Item) {
    let Some((id, params)) = &item.command else { return };
    let (key, enabled) = (shortcut_text(app, id), app.enabled(id));
    let mut button = egui::Button::new(item.name()).selected(item.checked).min_size(vec2(190.0, 0.0));
    if !key.is_empty() {
        button = button.shortcut_text(key);
    }
    if ui.add_enabled(enabled, button).clicked() {
        let _ = app.run(id, params.clone());
        ui.close();
    }
}

// ---- the simplified row ----

/// A button of the simplified row: a control, and the controls listed under its `▾`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Button<'a> {
    pub item: &'a Item,
    pub drops: Vec<&'a Item>,
}

impl Button<'_> {
    /// It has a `▾`: it opens a menu of its own, or has controls under it.
    fn has_drop(&self) -> bool {
        self.item.menu.is_some() || !self.drops.is_empty()
    }

    /// Its main part runs a command and its `▾` opens a menu.
    fn is_split(&self) -> bool {
        self.item.command.is_some() && self.has_drop()
    }
}

/// The buttons of a tab's simplified row, in the row's order: the stretches one after another,
/// and in each the controls as they are described.
pub(crate) fn row(tab: &Tab) -> Vec<Button<'_>> {
    let mut buttons: Vec<Button<'_>> = tab
        .items()
        .filter(|item| item.under.is_none())
        .map(|item| Button { item, drops: tab.items().filter(|under| under.under == Some(item.key)).collect() })
        .collect();
    buttons.sort_by_key(|b| b.item.section);
    buttons
}

/// What the simplified row shows at a width.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The buttons in the row, in order, by their place in [`row`]: each with whether its label shows.
    pub shown: Vec<(usize, bool)>,
    /// The buttons behind `…`, in the row's order.
    pub more: Vec<usize>,
}

/// The width of a row of buttons: `state` says of each whether it is in the row, and with its
/// label; `width` how wide the button at a place in the row is, with its label and without.
fn row_width(buttons: &[Button<'_>], state: &[(bool, bool)], width: &dyn Fn(usize, bool) -> f32) -> f32 {
    let mut total = 0.0;
    let mut before: Option<u8> = None;
    for (i, (b, (shown, labelled))) in buttons.iter().zip(state).enumerate() {
        if !shown {
            continue;
        }
        if let Some(section) = before {
            total += if section == b.item.section { BUTTON_GAP } else { RULE_GAP };
        }
        total += width(i, *labelled);
        before = Some(b.item.section);
    }
    if state.iter().any(|(shown, _)| !shown) {
        total += BUTTON_GAP + MORE_W;
    }
    total
}

/// Fit `buttons` into `available`: labels go first and then buttons, what matters least before
/// what matters more, until the row fits.
///
/// The order: the labels of the buttons whose icon says what they are; the other labels, all but
/// the first-rank buttons'; then buttons go behind `…`, all but the first rank; then the
/// first-rank labels and last those buttons themselves. In each step the higher rank number goes
/// first, and of two the one further along the row.
pub(crate) fn plan(buttons: &[Button<'_>], width: &dyn Fn(usize, bool) -> f32, available: f32) -> Plan {
    let mut state = vec![(true, true); buttons.len()];
    // (step, rank, place in the row, whether the step takes the button out of the row)
    let mut steps: Vec<(u8, u8, usize, bool)> = Vec::with_capacity(buttons.len() * 2);
    for (i, b) in buttons.iter().enumerate() {
        let (rank, first) = (b.item.rank, b.item.rank <= 1);
        steps.push((
            if b.item.plain {
                0
            } else if first {
                3
            } else {
                1
            },
            rank,
            i,
            false,
        ));
        steps.push((if first { 4 } else { 2 }, rank, i, true));
    }
    steps.sort_by_key(|(step, rank, place, _)| (*step, std::cmp::Reverse(*rank), std::cmp::Reverse(*place)));
    for (_, _, i, out) in steps {
        if row_width(buttons, &state, width) <= available {
            break;
        }
        if let Some(s) = state.get_mut(i) {
            *s = if out { (false, false) } else { (s.0, false) };
        }
    }
    Plan {
        shown: state.iter().enumerate().filter(|(_, (shown, _))| *shown).map(|(i, (_, labelled))| (i, *labelled)).collect(),
        more: state.iter().enumerate().filter(|(_, (shown, _))| !shown).map(|(i, _)| i).collect(),
    }
}

/// The width of a button of the simplified row whose name is `text` wide.
fn button_width(b: &Button<'_>, labelled: bool, text: f32) -> f32 {
    let label = if labelled { LABEL_GAP + text } else { 0.0 };
    let end = match (b.has_drop(), labelled) {
        (true, true) => DROP_GAP_LABEL + DROP_W,
        (true, false) => DROP_GAP_ICON + DROP_W,
        (false, _) => PAD,
    };
    PAD + ICON + label + end
}

/// What is under a button's `▾`: its own menu, then the controls under it.
fn drop_menu(ui: &mut Ui, app: &mut MailApp, b: &Button<'_>) {
    ui.set_min_width(200.0);
    if let Some(menu) = b.item.menu {
        menu_contents(ui, app, menu);
    }
    // A single menu under a button that has none of its own is that button's menu.
    let alone = b.item.menu.is_none() && b.drops.len() == 1;
    for under in &b.drops {
        match under.menu {
            Some(menu) if alone => menu_contents(ui, app, menu),
            Some(menu) => {
                ui.menu_button(under.name(), |ui| menu_contents(ui, app, menu));
            }
            None => entry(ui, app, under),
        }
    }
}

/// A button that is behind `…`, as entries of that menu.
fn more_entries(ui: &mut Ui, app: &mut MailApp, b: &Button<'_>) {
    if b.item.command.is_none() {
        ui.menu_button(b.item.name(), |ui| drop_menu(ui, app, b));
        return;
    }
    entry(ui, app, b.item);
    if let Some(menu) = b.item.menu {
        ui.menu_button(b.item.tip_text(), |ui| menu_contents(ui, app, menu));
    }
    for under in &b.drops {
        match under.menu {
            Some(menu) => {
                ui.menu_button(under.name(), |ui| menu_contents(ui, app, menu));
            }
            None => entry(ui, app, under),
        }
    }
}

/// A small `▾`, centred on `c`.
fn drop_mark(ui: &Ui, c: egui::Pos2, color: egui::Color32) {
    ui.painter().add(Shape::line(vec![pos2(c.x - 2.75, c.y - 1.0), pos2(c.x, c.y + 1.25), pos2(c.x + 2.75, c.y - 1.0)], Stroke::new(1.1, color)));
}

/// One button of the simplified row, in `rect`.
fn simple_button(app: &mut MailApp, ui: &mut Ui, t: &Tokens, b: &Button<'_>, labelled: bool, rect: Rect) {
    let item = b.item;
    let id = ui.scope_id().with(("ribbon", item.key));
    let on = item.command.as_ref().is_none_or(|(command, _)| app.enabled(command));
    let drop_rect = Rect::from_min_max(pos2(rect.max.x - DROP_W, rect.min.y), rect.max);
    let main_rect = if b.is_split() { Rect::from_min_max(rect.min, pos2(drop_rect.min.x, rect.max.y)) } else { rect };
    let main = ui.interact(main_rect, id, Sense::click());
    app.hit.add("ribbon", hit_id(item.key), main_rect);
    if on {
        bg(ui, main_rect, &main, item.checked, t);
    }
    let (ink, accent, text) = if on { (t.icon, t.accent, t.text) } else { (t.text_disabled, t.text_disabled, t.text_disabled) };
    let icon = Rect::from_center_size(pos2(rect.min.x + PAD + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
    icons::paint(ui.painter(), icon, item.icon, ink, accent);
    if labelled {
        ui.painter().text(pos2(icon.max.x + LABEL_GAP, rect.center().y), Align2::LEFT_CENTER, item.name(), regular(LABEL_SIZE), text);
    }
    let Some((command, params)) = &item.command else {
        // The whole button opens its menu.
        drop_mark(ui, drop_rect.center(), ink);
        let main = main.on_hover_text(item.tip_text());
        egui::Popup::menu(&main).show(|ui| drop_menu(ui, app, b));
        return;
    };
    let main = command_tooltip(app, main, &item.tip_text(), command);
    if main.clicked() && on {
        let _ = app.run(command, params.clone());
    }
    if !b.has_drop() {
        return;
    }
    // Its other half: the `▾` opens the menu, whether or not the command can run now.
    let drop = ui.interact(drop_rect, id.with("menu"), Sense::click());
    app.hit.add("ribbonMenu", hit_id(item.key), drop_rect);
    bg(ui, drop_rect, &drop, false, t);
    drop_mark(ui, drop_rect.center(), t.icon);
    let tip = b.drops.first().map_or_else(|| item.tip_text(), |under| under.tip_text());
    let drop = drop.on_hover_text(tip);
    egui::Popup::menu(&drop).show(|ui| drop_menu(ui, app, b));
}

/// The keys of the buttons that were behind `…` in the frame just laid out.
#[cfg(test)]
pub(crate) fn behind_more(ctx: &egui::Context) -> Vec<&'static str> {
    ctx.data(|d| d.get_temp::<Vec<&'static str>>(egui::Id::unique("ribbon_more"))).unwrap_or_default()
}

/// The simplified layout: one row of `tab` in `area`, with `…` at its end for what doesn't fit.
fn simplified(app: &mut MailApp, ui: &mut Ui, t: &Tokens, tab: &Tab, area: Rect) {
    let buttons = row(tab);
    let names: Vec<f32> =
        buttons.iter().map(|b| ui.ctx().fonts_mut(|f| f.layout_no_wrap(b.item.name(), regular(LABEL_SIZE), t.text).size().x)).collect();
    let width = |i: usize, labelled: bool| match (buttons.get(i), names.get(i)) {
        (Some(b), Some(text)) => button_width(b, labelled, *text),
        _ => 0.0,
    };
    let plan = plan(&buttons, &width, area.width());
    let mut x = area.min.x;
    let mut before: Option<u8> = None;
    for (i, labelled) in &plan.shown {
        let Some(b) = buttons.get(*i) else { continue };
        if let Some(section) = before {
            if section == b.item.section {
                x += BUTTON_GAP;
            } else {
                // A rule between two stretches of the row.
                let rule = (x + RULE_GAP / 2.0).floor() + 0.5;
                ui.painter().line_segment([pos2(rule, area.min.y), pos2(rule, area.max.y)], Stroke::new(1.0, t.border_strong));
                x += RULE_GAP;
            }
        }
        let w = width(*i, *labelled);
        simple_button(app, ui, t, b, *labelled, Rect::from_min_size(pos2(x, area.min.y), vec2(w, area.height())));
        x += w;
        before = Some(b.item.section);
    }
    let more: Vec<&Button<'_>> = plan.more.iter().filter_map(|i| buttons.get(*i)).collect();
    ui.data_mut(|d| d.insert_temp(egui::Id::unique("ribbon_more"), more.iter().map(|b| b.item.key).collect::<Vec<&'static str>>()));
    if !more.is_empty() {
        // What didn't fit, behind `…` at the end of the row.
        let rect = Rect::from_min_size(pos2(area.max.x - MORE_W, area.min.y), vec2(MORE_W, area.height()));
        let resp = ui.interact(rect, ui.scope_id().with("ribbon_more"), Sense::click());
        app.hit.add("ribbonMore", 0, rect);
        bg(ui, rect, &resp, false, t);
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(ICON, ICON)), "more", t.icon, t.accent);
        let resp = resp.on_hover_text(tl!("More commands"));
        egui::Popup::menu(&resp).show(|ui| {
            ui.set_min_width(200.0);
            let mut before: Option<u8> = None;
            for b in &more {
                if before.is_some_and(|section| section != b.item.section) {
                    ui.separator();
                }
                more_entries(ui, app, b);
                before = Some(b.item.section);
            }
        });
    } else if !tab.note.is_empty() && area.max.x - x - 16.0 >= NOTE_MIN {
        // Room is left: the tab's line of explanation, as much of it as fits (all of it in the tooltip).
        let rect = Rect::from_min_max(pos2(x + 16.0, area.min.y), area.max);
        paint_clipped(ui.painter(), pos2(rect.min.x, rect.center().y), rect.width(), tl!(tab.note), regular(12.0), t.text_dim);
        ui.interact(rect, ui.scope_id().with("ribbon_note"), Sense::hover()).on_hover_text(tl!(tab.note));
    }
}

// ---- the classic layout ----

/// A large button of the classic layout.
fn classic_big(app: &mut MailApp, ui: &mut Ui, item: &Item) {
    let label = tl!(item.label);
    match (&item.command, item.menu) {
        (Some((command, params)), _) => {
            let on = app.enabled(command);
            let (resp, text) = big_frame(ui, item.icon, label, on, item.checked, false);
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            app.hit.add("ribbonLabel", hit_id(item.key), text);
            let resp = command_tooltip(app, resp, &item.tip_text(), command);
            if resp.clicked() && on {
                let _ = app.run(command, params.clone());
            }
        }
        (None, Some(menu)) => {
            let (resp, text) = big_frame(ui, item.icon, label, true, false, true);
            let resp = resp.on_hover_text(item.tip_text());
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            app.hit.add("ribbonLabel", hit_id(item.key), text);
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(200.0);
                menu_contents(ui, app, menu);
            });
        }
        (None, None) => {}
    }
}

/// A column of small buttons of the classic layout (up to three).
fn classic_column(app: &mut MailApp, ui: &mut Ui, items: &[&Item]) {
    if items.is_empty() {
        return;
    }
    stack(ui, |ui| {
        for item in items {
            let label = Some(tl!(item.label));
            match (&item.command, item.menu) {
                (Some((command, params)), _) => {
                    let on = app.enabled(command);
                    let resp = small_frame(ui, item.icon, label, on, item.checked, false);
                    app.hit.add("ribbon", hit_id(item.key), resp.rect);
                    let resp = command_tooltip(app, resp, &item.tip_text(), command);
                    if resp.clicked() && on {
                        let _ = app.run(command, params.clone());
                    }
                }
                (None, Some(menu)) => {
                    let resp = small_frame(ui, item.icon, label, true, false, true).on_hover_text(item.tip_text());
                    app.hit.add("ribbon", hit_id(item.key), resp.rect);
                    egui::Popup::menu(&resp).show(|ui| {
                        ui.set_min_width(200.0);
                        menu_contents(ui, app, menu);
                    });
                }
                (None, None) => {}
            }
        }
    });
}

/// The classic layout: `tab` in its groups, large buttons side by side and small ones three to a column.
fn classic(app: &mut MailApp, ui: &mut Ui, t: &Tokens, tab: &Tab) {
    for group_of in &tab.groups {
        let items: Vec<&Item> = group_of.items.iter().filter(|item| item.size != Size::Hidden).collect();
        if items.is_empty() {
            continue;
        }
        group(ui, group_of.title, app, |ui, app| {
            let mut column: Vec<&Item> = Vec::new();
            for item in items {
                if item.size == Size::Small {
                    column.push(item);
                    if column.len() == 3 {
                        classic_column(app, ui, &std::mem::take(&mut column));
                    }
                    continue;
                }
                classic_column(app, ui, &std::mem::take(&mut column));
                classic_big(app, ui, item);
            }
            classic_column(app, ui, &column);
        });
    }
    if !tab.note.is_empty() {
        ui.add_space(8.0);
        ui.allocate_ui(vec2(tab.note_width, CONTENT_H), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(tl!(tab.note)).color(t.text_dim).size(11.5)).wrap());
        });
    }
}

// ---- the ribbon ----

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let tabs = tabs(app);
    if !tabs.contains(&app.ui.tab.as_str()) {
        app.ui.tab = "Home".into();
    }
    // The tabs and the ribbon start beside the module bar, in line with the panes under them.
    egui::Panel::top("tabs")
        .exact_size(TAB_H)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.tab_strip).inner_margin(egui::Margin { left: 3, right: 10, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for tab in &tabs {
                    let shown = tl!(tab);
                    let active = app.ui.tab == *tab && !app.ui.backstage;
                    // As wide as its name in the heavier weight, so a tab keeps its place when chosen.
                    let text_w = ui.ctx().fonts_mut(|f| f.layout_no_wrap(shown.to_string(), semibold(TAB_SIZE), t.text).size().x);
                    let w = text_w + 24.0;
                    let (r, resp) = ui.allocate_exact_size(vec2(w, TAB_H), Sense::click());
                    app.hit.add("ribbonTab", hit_id(tab), r);
                    if resp.hovered() && !active {
                        ui.painter().rect_filled(r.shrink2(vec2(2.0, 4.0)), 4.0, t.hover);
                    }
                    // The tab that came up for the message being written stands out.
                    let color = if *tab == "Message" && !active { t.accent_text } else { t.text };
                    ui.painter().text(r.center(), Align2::CENTER_CENTER, shown, if active { semibold(TAB_SIZE) } else { regular(TAB_SIZE) }, color);
                    if active {
                        // The tab in view: its name in the heavier weight, underlined in the accent.
                        let u = Rect::from_min_size(pos2(r.center().x - text_w / 2.0, r.min.y + 25.0), vec2(text_w, 2.0));
                        ui.painter().rect_filled(u, 1.0, t.accent_text);
                    }
                    if resp.clicked() {
                        let _ = app.run("ui.tab", json!({"tab": tab}));
                    }
                    if resp.double_clicked() && *tab != "File" {
                        let _ = app.run("ui.collapseRibbon", json!({}));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Search every command by name ("Tell me").
                    if crate::widgets::icon_text_button(ui, "search", tl!("Search commands"), true).clicked() {
                        let _ = app.run("ui.dialog", json!({"name": "commands"}));
                    }
                });
            });
        });
    if app.ui.ribbon_collapsed {
        return;
    }
    let tab = describe(app);
    let mode = app.ui.ribbon_mode;
    // The ribbon is a sheet on the window's own colour, with a gap under it before the panes. The
    // commands have the sheet less its edges and the layout button at its right end.
    let (height, edge) = match mode {
        Mode::Classic => (CONTENT_H + LABEL_H + 10.0, 4.0),
        Mode::Simplified => (BAND_H, (BAND_H - BUTTON_H) / 2.0),
    };
    let margin = egui::Margin { left: 8, right: (16.0 + MODE_W) as i8, top: (edge + TOP_GAP) as i8, bottom: (edge + SHEET_GAP) as i8 };
    egui::Panel::top("ribbon")
        .exact_size(height + TOP_GAP + SHEET_GAP)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.tab_strip).inner_margin(margin))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let sheet = Rect::from_min_max(pos2(area.min.x - 8.0, area.min.y - edge), pos2(area.max.x + 8.0 + MODE_W, area.max.y + edge));
            // In the dark the sheet stands out by itself; on a light window it needs its edge.
            let line = if t.dark { Stroke::NONE } else { Stroke::new(1.0, t.border) };
            ui.painter().rect(sheet, 8.0, t.ribbon, line, egui::StrokeKind::Inside);
            app.hit.add("ribbonBand", 0, sheet);
            match mode {
                Mode::Classic => {
                    egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                            classic(app, ui, &t, &tab);
                        });
                    });
                }
                Mode::Simplified => {
                    simplified(app, ui, &t, &tab, area);
                    ui.allocate_rect(area, Sense::hover());
                }
            }
            // The layout button, at the right end of the sheet (at its foot in the classic layout).
            let side = MODE_W - 4.0;
            let centre = match mode {
                Mode::Classic => pos2(sheet.max.x - 4.0 - side / 2.0, sheet.max.y - 4.0 - side / 2.0),
                Mode::Simplified => pos2(sheet.max.x - 4.0 - side / 2.0, sheet.center().y),
            };
            let rect = Rect::from_center_size(centre, vec2(side, side));
            let resp = ui.interact(rect, ui.scope_id().with("ribbon_mode"), Sense::click());
            app.hit.add("ribbonMode", 0, rect);
            bg(ui, rect, &resp, false, &t);
            ui.painter().add(Shape::line(
                vec![pos2(centre.x - 4.0, centre.y - 2.0), pos2(centre.x, centre.y + 2.0), pos2(centre.x + 4.0, centre.y - 2.0)],
                Stroke::new(1.5, t.icon),
            ));
            let resp = resp.on_hover_text(tl!("Ribbon Layout"));
            egui::Popup::menu(&resp).show(|ui| {
                ui.set_min_width(200.0);
                menu_contents(ui, app, Menu::RibbonLayout);
            });
        });
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;

    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    type Window = egui_kittest::Harness<'static, MailApp>;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        app
    }

    /// A run starts on Home, and File opens on Info every time, as in Outlook: where the owner
    /// last was is neither kept in the preferences nor while File is closed (#119).
    #[test]
    fn the_ribbon_opens_on_home_and_file_on_info() {
        let mut a = app();
        a.execute("ui.tab", json!({"tab": "View"})).unwrap();
        a.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        let saved: crate::UiState = serde_json::from_value(serde_json::to_value(a.prefs()).unwrap()).unwrap();
        let mut b = app();
        b.apply_prefs(saved);
        assert_eq!((b.ui.tab.as_str(), b.ui.backstage_page.as_str(), b.ui.backstage), ("Home", "info", false));
        // Within a run: a tab stays chosen, File's page doesn't once File is closed.
        b.execute("ui.tab", json!({"tab": "View"})).unwrap();
        assert_eq!(b.ui.tab, "View");
        b.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        assert_eq!(b.ui.backstage_page, "options");
        // Another page while it is open is kept.
        b.execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
        assert_eq!(b.ui.backstage_page, "signatures");
        for open in [json!({"value": true}), json!({})] {
            b.execute("ui.backstage", json!({"value": false})).unwrap();
            assert_eq!(b.execute("ui.backstage", open).unwrap()["page"], "info");
        }
        b.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        b.execute("ui.tab", json!({"tab": "Home"})).unwrap();
        b.execute("ui.tab", json!({"tab": "File"})).unwrap();
        assert_eq!((b.ui.backstage, b.ui.backstage_page.as_str()), (true, "info"));
    }

    fn window(app: MailApp, width: f32) -> Window {
        let mut harness =
            egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(width, 700.0)).build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(4);
        // The sample has reminders due, and their window would lie over the ribbon.
        let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        harness.run_steps(3);
        harness
    }

    fn click(harness: &mut Window, pos: egui::Pos2) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.step();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
            harness.step();
        }
        harness.run_steps(3);
    }

    /// What brings up every tab of every module, and the tab a message being written has.
    fn every_tab() -> Vec<(String, Vec<(&'static str, Value)>)> {
        let mut out = Vec::new();
        for module in ["view.mail", "view.calendar", "view.people", "view.tasks"] {
            for tab in ["Home", "Send / Receive", "Folder", "View", "Help"] {
                out.push((format!("{module} › {tab}"), vec![(module, json!({})), ("ui.tab", json!({"tab": tab}))]));
            }
        }
        out.push(("Message".to_string(), vec![("view.mail", json!({})), ("mail.new", json!({}))]));
        out
    }

    /// Every label, tooltip name and group title of every tab is in the template the catalogues
    /// are made from (`cargo xtask i18n`), as written, a large button's line break and all: a
    /// control the extraction missed would stay English in every language.
    #[test]
    fn every_ribbon_text_is_in_the_template() {
        let template = mailcraft_i18n::po::parse(include_str!("../../i18n/po/messages.pot"));
        let known: std::collections::BTreeSet<String> = template.entries.iter().filter(|e| e.context.is_none()).map(|e| e.id.clone()).collect();
        let mut a = app();
        let mut missing = std::collections::BTreeSet::new();
        for (_, commands) in every_tab() {
            for (command, params) in commands {
                a.run(command, params).unwrap();
            }
            let tab = describe(&a);
            for group in &tab.groups {
                let texts = std::iter::once(group.title).chain(group.items.iter().flat_map(|i| [i.label, i.tip]));
                missing.extend(texts.filter(|s| !s.is_empty() && !known.contains(*s)));
            }
        }
        assert!(missing.is_empty(), "not in messages.pot (run `cargo xtask i18n`): {missing:?}");
    }

    /// One description feeds both layouts: every control has a key of its own, an icon that is
    /// drawn and a command the program knows, and a place in the classic groups and in the row.
    #[test]
    fn every_control_has_its_place_in_both_layouts() {
        let mut a = app();
        let mut seen = 0;
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                a.run(command, params).unwrap();
            }
            let tab = describe(&a);
            let items: Vec<&Item> = tab.items().collect();
            assert!(!items.is_empty(), "{name}");
            for item in &items {
                let key = item.key;
                assert_eq!(items.iter().filter(|other| other.key == key).count(), 1, "{name}: two controls are called {key}");
                assert!(icons::NAMES.contains(&item.icon), "{name}: {key} has no drawing for \"{}\"", item.icon);
                assert!(!item.label.is_empty() && item.rank >= 1, "{name}: {key}");
                if let Some((command, _)) = &item.command {
                    let known = a.session.registry.get(command).is_some() || crate::UI_COMMANDS.iter().any(|c| c.0 == *command);
                    assert!(known, "{name}: {key} runs {command}, which is no command");
                }
                match item.size {
                    // Only there to gather others in the simplified row: nothing of its own to do.
                    Size::Hidden => {
                        assert!(item.command.is_none() && item.menu.is_none(), "{name}: {key}");
                        assert!(items.iter().any(|other| other.under == Some(key)), "{name}: nothing is under {key}");
                    }
                    Size::Big | Size::Small => assert!(item.command.is_some() != item.menu.is_some(), "{name}: {key} runs a command or opens a menu"),
                }
                if let Some(host) = item.under {
                    let host =
                        items.iter().find(|other| other.key == host).unwrap_or_else(|| panic!("{name}: {key} is under {host}, which isn't there"));
                    assert!(host.under.is_none(), "{name}: {key} is under {}, which is under something itself", host.key);
                }
            }
            // The simplified row has every control: as a button, or under one.
            let buttons = row(&tab);
            assert_eq!(buttons.len() + buttons.iter().map(|b| b.drops.len()).sum::<usize>(), items.len(), "{name}");
            assert!(buttons.windows(2).all(|pair| pair[0].item.section <= pair[1].item.section), "{name}: the row is in its stretches' order");
            assert!(buttons.iter().all(|b| b.item.command.is_some() || b.has_drop()), "{name}: a button does something");
            // The classic layout has all but what is only there for the row.
            assert!(tab.groups.iter().all(|group| !group.title.is_empty() && !group.items.is_empty()), "{name}");
            seen += 1;
        }
        assert_eq!(seen, 21);
    }

    /// Mail › Home in the order the simplified row has it, with its split buttons.
    #[test]
    fn the_mail_row_is_in_order() {
        let a = app();
        let tab = describe(&a);
        let buttons = row(&tab);
        let keys: Vec<&str> = buttons.iter().map(|b| b.item.key).collect();
        assert_eq!(
            keys.iter().take(12).copied().collect::<Vec<_>>(),
            [
                "mail.new",
                "mail.delete",
                "mail.archive",
                "moveMessages",
                "mail.reply",
                "mail.replyAll",
                "mail.forward",
                "mail.toggleRead",
                "categorize",
                "followUp",
                "rules",
                "filter"
            ]
        );
        let sections: Vec<u8> = buttons.iter().take(12).map(|b| b.item.section).collect();
        assert_eq!(sections, [0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4]);
        let button = |key: &str| buttons.iter().find(|b| b.item.key == key).unwrap();
        // New Email and Delete run their command and open a menu; Move only opens one; Archive only runs.
        assert!(button("mail.new").is_split() && button("mail.new").drops.iter().map(|d| d.key).eq(["newItems"]));
        assert!(button("mail.delete").is_split() && button("mail.delete").drops.iter().map(|d| d.key).eq(["mail.junk", "mail.deletePermanently"]));
        assert!(!button("moveMessages").is_split() && button("moveMessages").has_drop());
        assert!(!button("mail.archive").has_drop());
        assert_eq!(button("mail.replyAll").item.name(), "Reply All");
        // Calendar › Home: the new items, then today and the views.
        let mut a = app();
        a.execute("view.calendar", json!({})).unwrap();
        let tab = describe(&a);
        let buttons = row(&tab);
        let keys: Vec<&str> = buttons.iter().map(|b| b.item.key).take(8).collect();
        assert_eq!(
            keys,
            ["calendar.new", "calendar.newMeeting", "view.today", "view.next7Days", "view.day", "view.workWeek", "view.week", "view.month"]
        );
        assert_eq!(buttons.iter().take(8).map(|b| b.item.section).collect::<Vec<_>>(), [0, 0, 1, 1, 2, 2, 2, 2]);
        // New Appointment has the other new items under its ▾; the four views keep their
        // names as long as there is room (they aren't told from one another by their icons
        // alone), and the one in use is marked.
        let button = |key: &str| buttons.iter().find(|b| b.item.key == key).unwrap();
        assert!(button("calendar.new").is_split() && !button("calendar.newMeeting").has_drop());
        for view in ["view.day", "view.workWeek", "view.week", "view.month"] {
            assert!(!button(view).item.plain && button(view).item.checked == (view == "view.week"), "{view}");
        }
        assert_eq!((button("view.workWeek").item.name(), button("view.next7Days").item.name()), ("Work Week".to_string(), "Next 7 Days".to_string()));
        assert!(button("view.day").item.rank < button("view.today").item.rank && button("view.today").item.rank < button("view.next7Days").item.rank);
        // Back, Forward and Go to Date are under Today's ▾ (the calendar's own header has them too).
        assert!(
            button("view.today").is_split() && button("view.today").drops.iter().map(|d| d.key).eq(["view.previous", "view.next", "view.goToDate"])
        );
        // What is done with an appointment is in the row while one is selected, and the
        // answers to a meeting while it is somebody else's.
        let has = |a: &MailApp, key: &str| describe(a).items().any(|item| item.key == key);
        assert!(!has(&a, "calendar.edit") && !has(&a, "calendar.delete") && !has(&a, "calendar.accept") && has(&a, "file.exportCalendar"));
        let event = |a: &MailApp, title: &str| a.session.mailbox.events.values().find(|e| e.title.starts_with(title)).map(|e| e.id).unwrap();
        a.execute("calendar.select", json!({"id": event(&a, "Lunch with Dev")})).unwrap();
        assert!(has(&a, "calendar.edit") && has(&a, "calendar.delete") && has(&a, "showAs") && has(&a, "categorize") && !has(&a, "calendar.decline"));
        a.execute("calendar.select", json!({"id": event(&a, "Q4 roadmap")})).unwrap();
        assert!(has(&a, "calendar.edit") && has(&a, "calendar.accept") && has(&a, "calendar.tentative") && has(&a, "calendar.decline"));
        let tab = describe(&a);
        let sections: Vec<(&str, u8)> = row(&tab).iter().map(|b| (b.item.key, b.item.section)).filter(|(_, section)| *section >= 3).collect();
        assert_eq!(sections.first().copied(), Some(("calendar.edit", 3)));
        assert_eq!(sections.last().copied(), Some(("file.exportCalendar", 5)));
        a.execute("calendar.select", json!({"id": null})).unwrap();
        // The View tab has the calendar's time scale while the calendar is in front.
        a.execute("ui.tab", json!({"tab": "View"})).unwrap();
        assert!(describe(&a).items().any(|item| item.key == "timeScale" && item.menu == Some(Menu::TimeScale)));
        a.execute("view.mail", json!({})).unwrap();
        assert!(!describe(&a).items().any(|item| item.key == "timeScale"));
        a.execute("ui.tab", json!({"tab": "Home"})).unwrap();
        // Tasks › Home gathers the five due dates under one button.
        a.execute("view.tasks", json!({})).unwrap();
        let tab = describe(&a);
        let buttons = row(&tab);
        let follow_up = buttons.iter().find(|b| b.item.key == "followUp").unwrap();
        assert_eq!(follow_up.drops.len(), 5);
        assert!(follow_up.item.command.is_none() && !follow_up.is_split());
    }

    /// Calendar › View has the four views, the one in use marked, in place of the messages'
    /// sorting and preview, which mean nothing there; Mail › View keeps those and has no views.
    #[test]
    fn the_calendar_view_tab_has_its_views() {
        let mail_only = ["view.conversations", "preview", "arrangeBy", "view.reverseSort"];
        let views = ["view.day", "view.workWeek", "view.week", "view.month"];
        let checked_view = |a: &MailApp| -> Vec<&str> {
            describe(a).items().filter(|item| views.contains(&item.key) && item.checked).map(|item| item.key).collect()
        };
        let mut a = app();
        a.execute("view.calendar", json!({})).unwrap();
        a.execute("ui.tab", json!({"tab": "View"})).unwrap();
        let tab = describe(&a);
        assert_eq!(tab.groups.iter().map(|g| g.title).collect::<Vec<_>>(), ["Layout", "Arrangement", "Calendar", "Window"]);
        let arrangement: Vec<(&str, bool, u8, u8)> =
            tab.groups[1].items.iter().map(|item| (item.key, item.checked, item.section, item.rank)).collect();
        assert_eq!(arrangement, [("view.day", false, 1, 2), ("view.workWeek", false, 1, 2), ("view.week", true, 1, 2), ("view.month", false, 1, 2)]);
        // The calendar's own group is as it was.
        let calendar: Vec<&str> = tab.groups[2].items.iter().map(|item| item.key).collect();
        assert_eq!(calendar, ["timeScale", "view.overlay", "view.sideBySide"]);
        assert!(!tab.items().any(|item| mail_only.contains(&item.key)));
        // Each view, pressed, is the one marked.
        for view in views {
            a.execute(view, json!({})).unwrap();
            assert_eq!(checked_view(&a), [view]);
        }
        a.execute("view.mail", json!({})).unwrap();
        let tab = describe(&a);
        assert!(!tab.items().any(|item| views.contains(&item.key)));
        assert!(mail_only.iter().all(|key| tab.items().any(|item| item.key == *key)));
    }

    /// Labels go before buttons, the plain before the others, the least important first; the
    /// first-rank button keeps its label until all the rest is behind `…`.
    #[test]
    fn the_row_gives_way_in_order() {
        let item = |key: &'static str, section: u8, rank: u8, plain: bool| {
            let item = Item::big("mail", key, key, json!({})).at(section, rank);
            if plain { item.plain() } else { item }
        };
        let tab = Tab::of(vec![g("All", vec![item("a", 0, 1, false), item("b", 1, 1, true), item("c", 1, 3, false), item("d", 2, 5, false)])]);
        let buttons = row(&tab);
        let width = |_: usize, labelled: bool| if labelled { 100.0 } else { 40.0 };
        let at = |available: f32| plan(&buttons, &width, available);
        let all = |labels: [bool; 4]| labels.iter().copied().enumerate().collect::<Vec<_>>();
        // 100 + rule + 100 + gap + 100 + rule + 100.
        let full = 400.0 + 2.0 * RULE_GAP + BUTTON_GAP;
        assert_eq!(at(full), Plan { shown: all([true, true, true, true]), more: vec![] });
        assert_eq!(at(10_000.0).shown, at(full).shown);
        // The plain one's label first, then the others' from the least important.
        assert_eq!(at(full - 1.0).shown, all([true, false, true, true]));
        assert_eq!(at(full - 61.0).shown, all([true, false, true, false]));
        assert_eq!(at(full - 121.0).shown, all([true, false, false, false]));
        // Then buttons go behind `…`, the least important first; the first one keeps its label.
        assert_eq!(at(full - 181.0), Plan { shown: vec![(0, true), (1, false), (2, false)], more: vec![3] });
        let three = 100.0 + RULE_GAP + 40.0 + BUTTON_GAP + 40.0 + BUTTON_GAP + MORE_W;
        assert_eq!(at(three - 1.0), Plan { shown: vec![(0, true), (1, false)], more: vec![2, 3] });
        // Then the first-rank label, and last the first-rank buttons, from the end of the row.
        let two = 100.0 + RULE_GAP + 40.0 + BUTTON_GAP + MORE_W;
        assert_eq!(at(two - 1.0), Plan { shown: vec![(0, false), (1, false)], more: vec![2, 3] });
        assert_eq!(at(two - 61.0), Plan { shown: vec![(0, false)], more: vec![1, 2, 3] });
        assert_eq!(at(40.0), Plan { shown: vec![], more: vec![0, 1, 2, 3] });
        assert_eq!(at(-5.0).more.len(), 4);
        assert_eq!(plan(&[], &width, 100.0), Plan::default());

        // At any width: every button is in the row or behind `…`, the row fits, and a narrower
        // window never shows a button or a label that a wider one doesn't.
        let mut wider: Option<Plan> = None;
        for available in (0..=450).rev().map(|w| w as f32) {
            let p = at(available);
            let mut places: Vec<usize> = p.shown.iter().map(|(i, _)| *i).chain(p.more.iter().copied()).collect();
            places.sort_unstable();
            assert_eq!(places, [0, 1, 2, 3], "{available}");
            let state: Vec<(bool, bool)> =
                (0..4).map(|i| p.shown.iter().find(|(place, _)| *place == i).map_or((false, false), |(_, labelled)| (true, *labelled))).collect();
            assert!(p.shown.is_empty() || row_width(&buttons, &state, &width) <= available, "{available}: {p:?}");
            if let Some(wider) = &wider {
                for (i, labelled) in &p.shown {
                    let before = wider.shown.iter().find(|(place, _)| place == i);
                    assert!(before.is_some_and(|(_, was)| *was || !*labelled), "{available}: {p:?} after {wider:?}");
                }
            }
            wider = Some(p);
        }
    }

    /// What is on screen of a tab's row: checks that it is all inside the band, in order and not
    /// over one another, and that what isn't on screen is behind `…`. Returns the keys behind `…`.
    fn check_row(harness: &Window, name: &str) -> Vec<&'static str> {
        let app = harness.state();
        let tab = describe(app);
        let buttons = row(&tab);
        let band = app.hit.rect("ribbonBand", 0).unwrap_or_else(|| panic!("{name}: no band"));
        let more = behind_more(&harness.ctx);
        let inside =
            |r: Rect| r.min.x >= band.min.x - 0.5 && r.max.x <= band.max.x + 0.5 && r.min.y >= band.min.y - 0.5 && r.max.y <= band.max.y + 0.5;
        let mut end = band.min.x;
        for b in &buttons {
            let key = b.item.key;
            let Some(r) = app.hit.rect("ribbon", hit_id(key)) else {
                assert!(more.contains(&key), "{name}: {key} is neither in the row nor behind …");
                continue;
            };
            assert!(!more.contains(&key), "{name}: {key} is in the row and behind …");
            assert!(inside(r), "{name}: {key} at {r:?} is outside the band {band:?}");
            assert!(r.min.x >= end - 0.5 && r.width() >= ICON, "{name}: {key} at {r:?} is over the button before it (which ends at {end})");
            end = r.max.x;
            // Its `▾` half is beside it when it has one of its own.
            match app.hit.rect("ribbonMenu", hit_id(key)) {
                Some(drop) => {
                    assert!(b.is_split() && inside(drop) && (drop.min.x - end).abs() < 0.5, "{name}: {key} ▾ at {drop:?}");
                    end = drop.max.x;
                }
                None => assert!(!b.is_split(), "{name}: {key} has no ▾"),
            }
        }
        let mode = app.hit.rect("ribbonMode", 0).unwrap_or_else(|| panic!("{name}: no layout button"));
        assert!(inside(mode) && mode.min.x >= end - 0.5, "{name}: the layout button at {mode:?}");
        match app.hit.rect("ribbonMore", 0) {
            Some(r) => assert!(!more.is_empty() && inside(r) && r.min.x >= end - 0.5 && r.max.x <= mode.min.x + 0.5, "{name}: … at {r:?}"),
            None => assert!(more.is_empty(), "{name}: {more:?} are behind a … that isn't there"),
        }
        more
    }

    /// Every tab of every module in one row at 1200, 1000 and 800 wide: nothing is cut off and
    /// nothing scrolls; what doesn't fit is behind `…`.
    #[test]
    fn the_simplified_row_fits_at_every_width() {
        assert_eq!(crate::UiState::default().ribbon_mode, Mode::Simplified);
        for width in [1200.0, 1000.0, 800.0] {
            let mut harness = window(app(), width);
            let mut most = 0;
            for (name, commands) in every_tab() {
                for (command, params) in commands {
                    harness.state_mut().run(command, params).unwrap();
                }
                harness.run_steps(3);
                let name = format!("{name} at {width}");
                let more = check_row(&harness, &name);
                most = most.max(more.len());
                // What a tab is for stays in the row, with its label.
                let first = row(&describe(harness.state())).first().map(|b| b.item.key).unwrap();
                assert!(harness.state().hit.rect("ribbon", hit_id(first)).is_some_and(|r| r.width() > 60.0), "{name}: {first}");
                // The band is one row high, and the panes start right under it.
                let band = harness.state().hit.rect("ribbonBand", 0).unwrap();
                assert!((band.height() - BAND_H).abs() < 0.5, "{name}: {band:?}");
            }
            // Mail › Home has the most to show: all of it at 1200, some of it behind … at 800.
            harness.state_mut().run("compose.discard", json!({})).ok();
            harness.state_mut().run("ui.tab", json!({"tab": "Home"})).unwrap();
            harness.run_steps(3);
            let more = check_row(&harness, "mail home");
            match width as u32 {
                1200 => assert!(more.is_empty(), "{more:?}"),
                800 => assert!(
                    more.contains(&"mail.findRelated")
                        && more.contains(&"mail.findFromSender")
                        && !more.contains(&"mail.delete")
                        && most >= more.len(),
                    "{more:?}"
                ),
                _ => {}
            }
        }
    }

    /// The buttons of the row do what they say: the main part runs the command, a `▾` opens its
    /// menu, and what is behind `…` runs from there.
    #[test]
    fn the_row_runs_its_commands_and_so_does_what_is_behind_more() {
        let mut harness = window(app(), 800.0);
        harness.state_mut().run("mail.next", json!({})).unwrap();
        harness.run_steps(3);
        let focus = harness.state().session.sel.focus().unwrap();
        let message = |h: &Window| h.state().session.mailbox.message(focus).cloned().unwrap();
        let at = |h: &Window, kind: &str, key: &str| h.state().hit.rect(kind, hit_id(key)).unwrap_or_else(|| panic!("no {kind} {key}")).center();

        // The main part: Unread/Read on the message in view.
        let read = message(&harness).read;
        let pos = at(&harness, "ribbon", "mail.toggleRead");
        click(&mut harness, pos);
        assert_eq!(message(&harness).read, !read);

        // Behind `…`: Pin, Related and From Sender are there, and run.
        let more = check_row(&harness, "mail home at 800");
        assert!(more.contains(&"mail.pin") && more.contains(&"mail.findRelated") && more.contains(&"mail.findFromSender"), "{more:?}");
        assert!(harness.query_by_label("Pin").is_none(), "the menu is shut");
        let dots = harness.state().hit.rect("ribbonMore", 0).unwrap().center();
        click(&mut harness, dots);
        for key in &more {
            let item = describe(harness.state()).items().find(|item| item.key == *key).cloned().unwrap();
            assert!(harness.query_all_by_label_contains(&item.name()).next().is_some(), "{key} isn't in the menu behind …");
        }
        let pinned = message(&harness).pinned;
        harness.get_by_label("Pin").click();
        harness.run_steps(3);
        assert_eq!(message(&harness).pinned, !pinned);
        assert!(harness.query_by_label("Pin").is_none(), "the menu shuts when something is chosen");
        click(&mut harness, dots);
        harness.get_by_label("Related").click();
        harness.run_steps(3);
        assert!(harness.state().session.view.search.starts_with("subject:"), "{:?}", harness.state().session.view.search);
        harness.state_mut().execute("view.clearSearch", json!({})).unwrap();
        harness.state_mut().execute("mail.select", json!({"id": focus})).unwrap();
        harness.run_steps(3);
        let sender = mailcraft_model::search::keyword_value(message(&harness).from.email.trim());
        click(&mut harness, dots);
        harness.get_by_label("From Sender").click();
        harness.run_steps(3);
        assert_eq!(harness.state().session.view.search, format!("from:{sender}"));
        harness.state_mut().execute("view.clearSearch", json!({})).unwrap();
        harness.state_mut().execute("mail.select", json!({"id": focus})).unwrap();
        harness.run_steps(3);

        // A `▾`: New Email's lists the other new things; Delete's, what else gets rid of a message.
        let pos = at(&harness, "ribbonMenu", "mail.new");
        click(&mut harness, pos);
        assert!(harness.state().session.compose.is_none(), "the ▾ doesn't start a message");
        harness.get_by_label_contains("Folder…").click();
        harness.run_steps(3);
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("folder"));
        harness.state_mut().execute("ui.closeDialog", json!({})).unwrap();
        harness.run_steps(2);
        let junk = harness.state().session.mailbox.folders.iter().find(|f| f.kind == FolderKind::Junk).map(|f| f.id).unwrap();
        assert_ne!(message(&harness).folder, junk);
        let pos = at(&harness, "ribbonMenu", "mail.delete");
        click(&mut harness, pos);
        harness.get_by_label("Junk").click();
        harness.run_steps(3);
        assert_eq!(message(&harness).folder, junk);

        // And the main part of that button: New Email starts a message.
        let pos = at(&harness, "ribbon", "mail.new");
        click(&mut harness, pos);
        assert!(harness.state().session.compose.is_some());
        assert_eq!(harness.state().ui.tab, "Message");
    }

    /// The layout is chosen at the right end of the band, in View and by command; it is kept with
    /// the settings, and settings that say nothing of it (or nonsense) mean the simplified one.
    #[test]
    fn the_layout_is_chosen_and_kept() {
        let mut harness = window(app(), 1200.0);
        assert_eq!(harness.state().ui.ribbon_mode, Mode::Simplified);
        let simple = harness.state().hit.rect("ribbonBand", 0).unwrap();
        let button = harness.state().hit.rect("ribbonMode", 0).unwrap().center();
        click(&mut harness, button);
        assert!(harness.query_all_by_label_contains("Simplified Ribbon").next().is_some());
        harness.get_by_label_contains("Classic Ribbon").click();
        harness.run_steps(3);
        assert_eq!(harness.state().ui.ribbon_mode, Mode::Classic);
        let classic = harness.state().hit.rect("ribbonBand", 0).unwrap();
        assert!(classic.height() > simple.height() + 30.0, "{simple:?} -> {classic:?}");

        // Kept with the settings.
        let saved = serde_json::to_value(harness.state().prefs()).unwrap();
        assert_eq!(saved["ribbonMode"], "classic");
        let restored: crate::UiState = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.ribbon_mode, Mode::Classic);
        let old: crate::UiState = serde_json::from_value(json!({"tab": "Home"})).unwrap();
        assert_eq!(old.ribbon_mode, Mode::Simplified);
        let odd: crate::UiState = serde_json::from_value(json!({"ribbonMode": "sideways", "tab": "View"})).unwrap();
        assert_eq!((odd.ribbon_mode, odd.tab.as_str()), (Mode::Simplified, "View"));

        // By command, and from the button again.
        let a = harness.state_mut();
        assert!(a.enabled("ui.ribbonMode"));
        assert!(a.execute("ui.ribbonMode", json!({"value": "sideways"})).is_err() && a.execute("ui.ribbonMode", json!({})).is_err());
        assert_eq!(a.ui.ribbon_mode, Mode::Classic);
        assert_eq!(a.execute("ui.ribbonMode", json!({"value": "Simplified"})).unwrap()["value"], "simplified");
        assert_eq!(a.execute("ui.ribbonMode", json!({"value": "classic"})).unwrap()["value"], "classic");
        harness.run_steps(3);
        let button = harness.state().hit.rect("ribbonMode", 0).unwrap().center();
        click(&mut harness, button);
        harness.get_by_label_contains("Simplified Ribbon").click();
        harness.run_steps(3);
        assert_eq!(harness.state().ui.ribbon_mode, Mode::Simplified);
        // View lists it too.
        harness.state_mut().run("ui.tab", json!({"tab": "View"})).unwrap();
        assert!(describe(harness.state()).items().any(|item| item.menu == Some(Menu::RibbonLayout)));
    }

    /// #123: there is room between the tabs and the ribbon in both layouts, and in the classic one
    /// between the lowest line of a large button's label and the group's name under it.
    #[test]
    fn the_ribbon_has_room_under_the_tabs_and_over_the_group_names() {
        let mut harness = window(app(), 1600.0);
        for mode in ["classic", "simplified"] {
            harness.state_mut().execute("ui.ribbonMode", json!({"value": mode})).unwrap();
            for (name, commands) in every_tab() {
                for (command, params) in commands {
                    harness.state_mut().run(command, params).unwrap();
                }
                harness.run_steps(3);
                let name = format!("{name} ({mode})");
                let app = harness.state();
                let band = app.hit.rect("ribbonBand", 0).unwrap();
                let strip = app.hit.rect("ribbonTab", hit_id("Home")).unwrap();
                assert!(band.min.y - strip.max.y >= 4.0, "{name}: tabs end at {}, the ribbon starts at {}", strip.max.y, band.min.y);
                if mode != "classic" {
                    continue;
                }
                let mut checked = 0;
                for group in &describe(app).groups {
                    let Some(title) = app.hit.rect("ribbonGroup", hit_id(group.title)) else { continue };
                    assert!(title.max.y <= band.max.y - 2.0, "{name}: {} at {title:?} in {band:?}", group.title);
                    for item in group.items.iter().filter(|item| item.size == Size::Big) {
                        let label = app.hit.rect("ribbonLabel", hit_id(item.key)).unwrap_or_else(|| panic!("{name}: {} has no label", item.key));
                        let gap = title.min.y - label.max.y;
                        // 2 pt, not 4: Windows draws the label half a point taller than Linux does.
                        assert!(gap >= 2.0, "{name}: {}'s label ends {gap} over {}", item.key, group.title);
                        checked += 1;
                    }
                }
                assert!(checked > 0 || name.contains("Help"), "{name}: no large buttons were measured");
            }
        }
    }

    /// The classic layout is what it was: every control in its group, large buttons the height of
    /// the band's content side by side, small ones three to a column, in the order described.
    #[test]
    fn the_classic_layout_sets_every_control_in_its_group() {
        let mut harness = window(app(), 1600.0);
        harness.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
        for (name, commands) in every_tab() {
            for (command, params) in commands {
                harness.state_mut().run(command, params).unwrap();
            }
            harness.run_steps(3);
            let app = harness.state();
            let tab = describe(app);
            let band = app.hit.rect("ribbonBand", 0).unwrap();
            assert!((band.height() - (CONTENT_H + LABEL_H + 10.0)).abs() < 0.5, "{name}: {band:?}");
            assert!(app.hit.rect("ribbonMore", 0).is_none() && app.hit.rect("ribbonMode", 0).is_some(), "{name}");
            let mut end = band.min.x;
            for group in &tab.groups {
                let mut column: Vec<Rect> = Vec::new();
                for item in &group.items {
                    let at = app.hit.rect("ribbon", hit_id(item.key));
                    match item.size {
                        Size::Hidden => assert!(at.is_none(), "{name}: {} is drawn", item.key),
                        Size::Big => {
                            let r = at.unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                            assert!((r.height() - CONTENT_H).abs() < 0.5 && r.width() >= 46.0, "{name}: {} at {r:?}", item.key);
                            assert!(r.min.x >= end - 0.5 && r.max.x <= band.max.x, "{name}: {} at {r:?} after {end}", item.key);
                            end = r.max.x;
                            column.clear();
                        }
                        Size::Small => {
                            let r = at.unwrap_or_else(|| panic!("{name}: {} isn't drawn", item.key));
                            assert!((r.height() - 22.0).abs() < 0.5, "{name}: {} at {r:?}", item.key);
                            if column.len() == 3 {
                                column.clear();
                            }
                            match column.last() {
                                // Under the one before it, in the same column.
                                Some(above) => {
                                    assert!((r.min.x - above.min.x).abs() < 0.5 && r.min.y >= above.max.y - 0.5, "{name}: {} at {r:?}", item.key)
                                }
                                // A new column, after what came before.
                                None => {
                                    assert!(r.min.x >= end - 0.5, "{name}: {} at {r:?} after {end}", item.key);
                                }
                            }
                            column.push(r);
                            end = end.max(r.max.x);
                        }
                    }
                }
            }
        }
        // Mail › Home, group by group, as the classic ribbon has always had it.
        let a = app();
        let tab = describe(&a);
        let groups: Vec<(&str, Vec<&str>)> =
            tab.groups.iter().map(|g| (g.title, g.items.iter().filter(|i| i.size != Size::Hidden).map(|i| i.label).collect())).collect();
        assert_eq!(
            groups,
            [
                ("New", vec!["New\nEmail", "New\nItems"]),
                ("Delete", vec!["Delete", "Archive", "Junk", "Delete for Good"]),
                ("Respond", vec!["Reply", "Reply\nAll", "Forward", "Meeting", "Add Sender"]),
                ("Move", vec!["Move", "Rules"]),
                ("Tags", vec!["Unread/\nRead", "Categorize", "Follow\nUp", "Pin"]),
                ("Find", vec!["Filter\nEmail", "Related", "From Sender"]),
                ("Send/Receive", vec!["Send/Receive\nAll Folders"]),
            ]
        );
    }

    /// The folder menu with a search box, drawn on its own: the app, and the folder chosen.
    type Picker = egui_kittest::Harness<'static, (MailApp, Option<Id>)>;

    fn picker(app: MailApp) -> Picker {
        let mut h =
            egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(vec2(400.0, 500.0)).build_ui_state(
                |ui, (app, picked): &mut (MailApp, Option<Id>)| {
                    folder_menu(ui, app, |_, id| *picked = Some(id));
                },
                (app, None),
            );
        h.run_steps(3);
        h
    }

    fn type_into(h: &mut Picker, text: &str) {
        h.event(egui::Event::Text(text.to_string()));
        h.run_steps(3);
    }

    fn press(h: &mut Picker, key: egui::Key) {
        h.key_press(key);
        h.run_steps(3);
    }

    fn folder_row(name: &str) -> FolderRow {
        (1, name.to_string(), name.to_string(), 0)
    }

    /// Typing narrows the list to the folders whose name holds it, any part, whatever the case
    /// and accents; nothing typed shows all (#74).
    #[test]
    fn folder_search_matches_any_part_ignoring_case_and_accents() {
        let list = [folder_row("Inbox"), folder_row("Café Receipts"), folder_row("Projects"), folder_row("Archive 2026")];
        assert_eq!(folder_matches(&list, ""), [0, 1, 2, 3]);
        assert_eq!(folder_matches(&list, "   "), [0, 1, 2, 3]);
        assert_eq!(folder_matches(&list, "BOX"), [0]);
        assert_eq!(folder_matches(&list, "cafe"), [1]);
        assert_eq!(folder_matches(&list, "é rec"), [1]);
        assert_eq!(folder_matches(&list, "202"), [3]);
        assert!(folder_matches(&list, "zzz").is_empty());
    }

    /// Hostile or enormous text asks for nothing and never panics.
    #[test]
    fn folder_search_survives_hostile_text() {
        let list = [folder_row("Inbox"), folder_row("Ünï")];
        let long = "é".repeat(100_000);
        for q in [long.as_str(), "\u{0}\u{7f}\u{202e}", "\u{1F600}\u{200d}\u{1F600}", "%s{}[]\\", "\n\r\t", "ß", "\u{301}"] {
            let _ = folder_matches(&list, q);
        }
        assert!(folder_matches(&list, &long).is_empty());
        assert!(folder_matches(&[], "x").is_empty());
        // Through the window as well: typed, then keys, with nothing to choose.
        let mut h = picker(app());
        type_into(&mut h, &"\u{1F600}x".repeat(500));
        for key in [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::Enter, egui::Key::Escape] {
            press(&mut h, key);
        }
        assert_eq!(h.state().1, None);
    }

    /// The box has the keyboard when the menu opens: typing filters, Enter chooses.
    #[test]
    fn folder_search_is_typed_into_and_chosen_by_keyboard() {
        let a = app();
        let sent = {
            let mb = &a.session.mailbox;
            let account = mb.default_account().map(|x| x.id).unwrap();
            mb.folder_of_kind(account, mailcraft_model::FolderKind::Sent).unwrap()
        };
        let mut h = picker(a);
        assert!(h.query_by_label_contains("Drafts").is_some());
        type_into(&mut h, "SENT");
        assert!(h.query_by_label_contains("Drafts").is_none());
        press(&mut h, egui::Key::Enter);
        assert_eq!(h.state().1, Some(sent));
    }

    /// Nothing matching says so, and Enter chooses nothing.
    #[test]
    fn folder_search_with_no_match_says_so() {
        let mut h = picker(app());
        type_into(&mut h, "qqqqzz");
        assert!(h.query_by_label_contains("No folders match").is_some());
        press(&mut h, egui::Key::Enter);
        assert_eq!(h.state().1, None);
    }

    /// Down moves to the next match, and Escape clears the search (so the full list is back).
    #[test]
    fn folder_search_arrows_move_and_escape_clears() {
        let mut probe = picker(app());
        type_into(&mut probe, "e");
        press(&mut probe, egui::Key::Enter);
        let first = probe.state().1;
        let mut h = picker(app());
        type_into(&mut h, "e");
        press(&mut h, egui::Key::ArrowDown);
        press(&mut h, egui::Key::Enter);
        assert!(h.state().1.is_some());
        assert_ne!(h.state().1, first);
        let mut h = picker(app());
        type_into(&mut h, "sent");
        assert!(h.query_by_label_contains("Drafts").is_none());
        press(&mut h, egui::Key::Escape);
        assert!(h.query_by_label_contains("Drafts").is_some());
    }
}
