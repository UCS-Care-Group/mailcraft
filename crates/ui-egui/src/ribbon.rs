// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The ribbon: the tab strip and every tab's commands. Each control runs a command by id.
//!
//! A tab is described once ([`describe`]): its groups, and in each the controls with their
//! command, icon, label and tooltip. Two layouts draw that description. The classic one sets it
//! out in three rows, in named groups. The simplified one is a single row, in which what doesn't
//! fit first loses its label and then goes behind `…` ([`plan`]).

use egui::{Align2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use mailcraft_engine::{CalView, Filter, Module, Pane, PeopleSort, SortBy, TaskFilter, TaskSort};
use mailcraft_model::{FolderKind, Id, Importance};
use serde_json::{Value, json};

use crate::theme::{Tokens, medium, paint_clipped, regular, semibold};
use crate::widgets::{self, CONTENT_H, LABEL_H, bg, big_frame, command_tooltip, group, mi, mi_check, shortcut_text, small_frame, stack};
use crate::{MailApp, icons};

mod fit;
use fit::{Cell, Stage};
mod view_menus;

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
pub(crate) const DROP_W: f32 = 22.0;
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

/// The tabs in order. `File` opens Backstage; `Message`, `Insert`, `Format Text` and `Review` are
/// there while a message is being written in the reading pane (a window of its own has them in
/// its ribbon, [`show_window`]).
pub fn tabs(app: &MailApp) -> Vec<&'static str> {
    let mut tabs = vec!["File", "Home", "Send / Receive", "Folder", "View", "Help"];
    if crate::windows::compose_window::pane_compose(app).is_some() {
        tabs.splice(1..1, ["Message", "Insert", "Format Text", "Review"]);
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
    GmailCategories,
    NavigationBar,
    Preview,
    ArrangeBy,
    Theme,
    RibbonLayout,
    TimeScale,
    ChangeView,
    Zoom,
    RecentSearches,
    FontSize,
    Font,
    FontColor,
    Highlight,
    Junk,
    RecentAttachments,
}

impl Menu {
    /// The command a menu stands for, when all it does is run it with a choice (the font menus):
    /// it is greyed when that command can't run, as a button would be.
    pub(crate) fn command(self) -> Option<&'static str> {
        match self {
            Menu::Font => Some("format.font"),
            Menu::FontSize => Some("format.size"),
            Menu::FontColor => Some("format.color"),
            Menu::Highlight => Some("format.highlight"),
            Menu::RecentAttachments => Some("ui.attach"),
            _ => None,
        }
    }
}

/// Whether a control can be used now: its command can run, or the command its menu stands for
/// ([`Menu::command`]); a menu that stands for none always can. Every layout draws by it.
pub(crate) fn usable(app: &MailApp, ctx: &egui::Context, item: &Item) -> bool {
    match (&item.command, item.menu) {
        (Some((command, _)), _) => crate::text_menu::compose_clipboard_enabled(app, ctx, command).unwrap_or_else(|| app.enabled(command)),
        (None, Some(menu)) => menu_usable(app, menu),
        (None, None) => true,
    }
}

/// Whether a menu can be opened now: the command it stands for can run, or it stands for none.
fn menu_usable(app: &MailApp, menu: Menu) -> bool {
    (menu != Menu::RecentAttachments || crate::recent_attachments::available(app)) && menu.command().is_none_or(|command| app.enabled(command))
}

/// A menu as an entry of another (a `▾`, a `…`): it opens while it can ([`menu_usable`]), and is
/// greyed otherwise, saying why when hovered.
pub(crate) fn submenu(ui: &mut Ui, app: &mut MailApp, name: String, menu: Menu) {
    if menu_usable(app, menu) {
        ui.menu_button(name, |ui| menu_contents(ui, app, menu));
        return;
    }
    let reason = menu.command().and_then(|c| app.session.registry.get(c)).and_then(|spec| (spec.enabled)(&app.session));
    let entry = ui.add_enabled(false, egui::Button::new(name).min_size(vec2(190.0, 0.0)));
    if let Some(reason) = reason {
        entry.on_disabled_hover_text(crate::i18n::fmt(tl!("Not available: {reason}"), &[("reason", tl!(reason))]));
    }
}

/// A menu button's tooltip: its name, and when its menu stands for a command that can't run now
/// ([`usable`]), why, as a greyed button's says.
fn menu_tooltip(app: &MailApp, resp: egui::Response, item: &Item, on: bool) -> egui::Response {
    match item.menu.and_then(Menu::command) {
        Some(command) if !on => command_tooltip(app, resp, &item.tip_text(), command),
        _ => resp.on_hover_text(item.tip_text()),
    }
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
    /// One of classic Outlook's controls with no feature behind it yet: shown greyed, with
    /// "Pending development", and runs nothing.
    pub pending: bool,
    /// Only the classic ribbon shows this control.
    classic_only: bool,
}

impl Item {
    fn new(key: &'static str, icon: &'static str, label: &'static str, size: Size) -> Item {
        Item {
            key,
            icon,
            label,
            tip: "",
            command: None,
            menu: None,
            size,
            checked: false,
            section: 9,
            rank: 5,
            plain: false,
            under: None,
            pending: false,
            classic_only: false,
        }
    }

    /// A small button for a control of classic Outlook's that is still to be built (`key`: its
    /// id in the catalog): greyed, "Pending development", and the first to give way in the row.
    fn pending(key: &'static str, label: &'static str, section: u8) -> Item {
        Item { pending: true, ..Item::new(key, "pending", label, Size::Small) }.at(section, 5)
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

    fn classic_only(self) -> Item {
        Item { classic_only: true, ..self }
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
    /// Classic layout: how long it keeps its form as the window narrows, the higher the longer
    /// (set per tab in [`fit::prioritise`]).
    pub priority: u8,
    /// An icon grid for the classic layout, in rows by item key.
    classic_rows: &'static [&'static [&'static str]],
}

fn g(title: &'static str, items: Vec<Item>) -> Group {
    Group { title, items, priority: 5, classic_rows: &[] }
}

impl Group {
    fn classic_rows(self, classic_rows: &'static [&'static [&'static str]]) -> Group {
        Group { classic_rows, ..self }
    }
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
    describe_tab(app, &app.ui.tab)
}

/// The tab `name`, as the window stands.
pub(crate) fn describe_tab(app: &MailApp, name: &str) -> Tab {
    let (mut tab, which) = match name {
        "Message" => (message(app), "Message"),
        "Insert" => (insert(), "Insert"),
        "Format Text" => (format_text(app), "Format Text"),
        "Review" => (review(), "Review"),
        "Send / Receive" => (send_receive(app), "Send / Receive"),
        "Folder" => (folder(app), "Folder"),
        "View" => (view(app), "View"),
        "Help" => (help(), "Help"),
        _ => match app.session.view.module {
            Module::Mail => (home_mail(app), "Home mail"),
            Module::Calendar => (home_calendar(app), "Home calendar"),
            Module::People => (home_people(app), "Home people"),
            Module::Tasks => (home_tasks(app), "Home tasks"),
        },
    };
    if app.services.recent_attachments.is_some() {
        for item in tab.groups.iter_mut().flat_map(|group| &mut group.items) {
            if item.command.as_ref().is_some_and(|(command, _)| *command == "ui.attach") {
                item.menu = Some(Menu::RecentAttachments);
            }
        }
    }
    fit::prioritise(&mut tab, which);
    tab
}

fn no() -> Value {
    json!({})
}

fn home_mail(app: &MailApp) -> Tab {
    // Junk ▾: Block Sender, Never Block…, Not Junk and Junk E-mail Options (#504).
    let junk = Item::small_menu("junk", "junk", "Junk", "Junk", Menu::Junk);
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
                Item::small("deletePermanently", "Delete Permanently", "Delete Permanently", "mail.deletePermanently", no()).under("mail.delete"),
            ],
        ),
        g(
            "Respond",
            vec![
                Item::big("reply", "Reply", "mail.reply", no()).at(2, 1).plain(),
                Item::big("replyAll", "Reply\nAll", "mail.replyAll", no()).at(2, 2).plain(),
                Item::big("forward", "Forward", "mail.forward", no()).at(2, 2).plain(),
                Item::small("forwardAsAttachment", "Forward as Attachment", "", "mail.forwardAsAttachment", no()).under("mail.forward"),
                Item::small("delete", "Reply and Delete", "", "mail.replyAndDelete", no()).under("mail.reply"),
                Item::small("open", "View Source", "", "mail.viewSource", no()).under("mail.forward"),
                Item::small("meeting", "Meeting", "Reply with Meeting", "calendar.fromMessage", no()).at(5, 6),
                Item::small("newContact", "Add Sender", "Add to Contacts", "people.fromMessage", no()).at(5, 7),
            ],
        ),
        g(
            "Move",
            vec![
                Item::big_menu("moveMessages", "move", "Move", "Move to Folder", Menu::MoveMessages).at(1, 2).plain(),
                Item::big_menu("rules", "rules", "Rules", "Rules", Menu::Rules).at(4, 4),
                Item::small("send", "Resend Message", "Open a message you sent as a new one", "mail.resend", no()).at(5, 7),
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
        // A meeting: a message to its people, and how they have answered.
        if !event.attendees.is_empty() {
            items.push(Item::small("people", "Contact Attendees", "Email the people of this meeting", "calendar.contactAttendees", no()).at(3, 7));
            items.push(Item::small("list", "Tracking", "Who has answered, and how", "calendar.tracking", no()).at(3, 7));
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
            answers.push(Item::small("reply", "Reply by Email", "Write to the organizer without answering", "calendar.respondEmail", no()).at(4, 6));
            groups.push(g("Respond", answers));
        }
    }
    // Somebody else's calendar, by their address (#70).
    groups.push(g("Manage Calendars", vec![Item::big("calendar", "Open\nCalendar", "calendar.openShared", no()).at(4, 9)]));
    groups.push(g(
        "Share",
        vec![
            Item::big("email", "Email\nCalendar", "calendar.email", no()).at(5, 5),
            Item::big("export", "Save\nCalendar", "file.exportCalendar", no()).at(5, 6),
        ],
    ));
    groups.push(g("Find", vec![Item::big("search", "Search\nCalendar", "calendar.search", no()).at(4, 6)]));
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
        g(
            "Communicate",
            vec![Item::big("mail", "Email", "people.email", no()).at(2, 2), Item::big("meeting", "Meeting", "people.meeting", no()).at(2, 3)],
        ),
        g(
            "Actions",
            vec![
                Item::big("edit", "Edit\nContact", "people.edit", no()).at(3, 2),
                Item::big(if favorite { "starOn" } else { "star" }, "Favorite", "people.favorite", no()).checked(favorite).at(3, 3),
                Item::big_menu("categorize", "categorize", "Categorize", "Categorize", Menu::CategorizeContact).at(3, 3).plain(),
                // The contacts picked together, else every group that shares an address or a
                // number (#506).
                Item::big("mergeContacts", "Merge\nDuplicates", "people.merge", merge_params(app)).at(3, 4),
            ],
        ),
        g(
            "Share",
            vec![
                Item::big("forward", "Forward\nContact", "people.forward", no()).at(4, 4),
                Item::big("export", "Export\nContacts", "file.exportContacts", no()).at(4, 5),
                Item::big("import", "Import", "ui.import", no()).at(4, 5),
            ],
        ),
    ])
}

/// What Merge Duplicates opens its dialog with: the contacts picked together in the list, when
/// there are two or more.
pub(crate) fn merge_params(app: &MailApp) -> Value {
    if app.people_picked.len() >= 2 { json!({"ids": app.people_picked}) } else { json!({}) }
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
                Item::big("mail", "Send Status\nReport", "tasks.statusReport", no()).at(2, 4),
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
    pub highlight: Option<Option<String>>,
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
        highlight: all(&|s| s.highlight == first.highlight).then(|| first.highlight.clone()),
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
    let read_receipt = app.session.compose.as_ref().is_some_and(|c| c.read_receipt);
    let look = look_now(app);
    let painting = app.session.doc().is_some_and(|(b, _)| b.painter().is_some());
    Tab::of(vec![
        g("Send", vec![Item::big("send", "Send", "compose.send", no()).at(0, 1)]),
        g(
            "Clipboard",
            vec![
                Item::big("paste", "Paste", "edit.paste", no()).classic_only(),
                Item::small("cut", "Cut", "Cut", "edit.cut", no()).classic_only(),
                Item::small("copy", "Copy", "Copy", "edit.copy", no()).classic_only(),
                Item::small("formatPainter", "Format Painter", "Format Painter", "edit.formatPainter", no()).checked(painting).at(1, 5).plain(),
            ],
        ),
        g(
            "Basic Text",
            vec![
                Item::small_menu("font", "font", "Font", "Font", Menu::Font).at(1, 4).plain(),
                Item::small_menu("fontSize", "fontSize", "Font Size", "Font Size", Menu::FontSize).at(1, 4).plain(),
                Item::small_menu("fontColor", "fontColor", "Font Color", "Font Color", Menu::FontColor).at(1, 3).plain(),
                Item::small_menu("highlight", "highlight", "Text Highlight Color", "Text Highlight Color", Menu::Highlight).at(1, 4).plain(),
                Item::small("bold", "Bold", "Bold", "format.bold", no()).checked(look.bold && !look.plain).at(1, 2).plain(),
                Item::small("italic", "Italic", "Italic", "format.italic", no()).checked(look.italic && !look.plain).at(1, 2).plain(),
                Item::small("underline", "Underline", "Underline", "format.underline", no()).checked(look.underline && !look.plain).at(1, 2).plain(),
                Item::small("bullets", "Bullets", "Bullets", "format.bullets", no()).checked(look.bullets).at(1, 3).plain(),
                Item::small("numbering", "Numbering", "Numbering", "format.numbering", no()).checked(look.numbering).at(1, 3).plain(),
                Item::small("decreaseIndent", "Decrease Indent", "Decrease Indent", "format.outdent", no()).at(1, 4).plain(),
                Item::small("increaseIndent", "Increase Indent", "Increase Indent", "format.indent", no()).at(1, 4).plain(),
                Item::small("clearFormatting", "Clear Formatting", "Clear All Formatting", "format.clear", no()).at(1, 5).plain(),
            ],
        )
        .classic_rows(&[
            &["font", "fontSize"],
            &["format.bold", "format.italic", "format.underline", "format.bullets", "format.numbering", "format.clear"],
            &["highlight", "fontColor", "format.outdent", "format.indent"],
        ]),
        g("Names", vec![Item::big("checkNames", "Check\nNames", "compose.checkNames", no()).at(3, 4)]),
        g(
            "Include",
            vec![
                Item::small("attach", "Attach\nFile", "", "ui.attach", no()).at(2, 2),
                Item::small("open", "Attach Item", "", "compose.attachItem", no()).classic_only(),
                Item::small_menu("signature", "signature", "Signature", "Signature", Menu::Signature).at(2, 3),
                Item::small("insertLink", "Link", "Link", "compose.link", no()).checked(look.link && !look.plain).at(2, 3),
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
                Item::small("reply", "Direct Replies To", "Have replies sent to other addresses", "compose.replyTo", no()).at(4, 6),
                Item::small("read", "Request a Read Receipt", "Ask to be told when the message is read", "compose.readReceipt", no())
                    .checked(read_receipt)
                    .at(4, 6),
            ],
        )
        .classic_rows(&[&["compose.highImportance", "compose.readReceipt"], &["compose.lowImportance", "compose.replyTo"]]),
        g(
            "Format",
            vec![
                Item::small("formatHtml", "HTML", "Format as HTML", "compose.formatHtml", no()).checked(!look.plain).at(5, 5),
                Item::small("formatPlain", "Plain Text", "Format as Plain Text", "compose.formatPlain", no()).checked(look.plain).at(5, 5),
            ],
        ),
        g(
            "Editing",
            vec![
                Item::small("find", "Find", "Find", "edit.find", no()).at(5, 4).plain(),
                Item::small("replace", "Replace", "Replace", "edit.replace", no()).at(5, 5).plain(),
            ],
        ),
        g(
            "Draft",
            vec![
                Item::small("saveDraft", "Save\nDraft", "", "compose.saveDraft", no()).at(6, 2),
                Item::small("discard", "Discard", "", "compose.discard", no()).at(6, 3).plain(),
                Item::small("close", "Close", "", "compose.close", no()).at(6, 3),
            ],
        ),
        // The reading pane's message into a window of its own (#286; a window's ribbon leaves it out).
        g("Window", vec![Item::big("popOut", "Pop\nOut", "compose.popOut", no()).at(6, 4)]),
    ])
}

/// Format Text, in a window of a message being written (#286): the message's format, and the
/// text's font and paragraphs, in classic Outlook's groups.
fn format_text(app: &MailApp) -> Tab {
    let look = look_now(app);
    let painting = app.session.doc().is_some_and(|(b, _)| b.painter().is_some());
    let rich = !look.plain;
    Tab::of(vec![
        g(
            "Clipboard",
            vec![
                Item::big("paste", "Paste", "edit.paste", no()).at(0, 1),
                Item::small("formatPainter", "Format Painter", "Format Painter", "edit.formatPainter", no()).checked(painting).at(0, 5).plain(),
            ],
        ),
        g(
            "Format",
            vec![
                Item::big("formatHtml", "HTML", "compose.formatHtml", no()).checked(rich).at(1, 2),
                Item::big("formatPlain", "Plain\nText", "compose.formatPlain", no()).checked(look.plain).at(1, 2),
                Item::pending("compose.formatRich", "Rich Text", 1),
            ],
        ),
        g(
            "Font",
            vec![
                Item::small_menu("font", "font", "Font", "Font", Menu::Font).at(2, 4).plain(),
                Item::small_menu("fontSize", "fontSize", "Font Size", "Font Size", Menu::FontSize).at(2, 4).plain(),
                Item::small_menu("fontColor", "fontColor", "Font Color", "Font Color", Menu::FontColor).at(2, 3).plain(),
                Item::small_menu("highlight", "highlight", "Text Highlight Color", "Text Highlight Color", Menu::Highlight).at(2, 4).plain(),
                Item::small("bold", "Bold", "Bold", "format.bold", no()).checked(look.bold && rich).at(2, 2).plain(),
                Item::small("italic", "Italic", "Italic", "format.italic", no()).checked(look.italic && rich).at(2, 2).plain(),
                Item::small("underline", "Underline", "Underline", "format.underline", no()).checked(look.underline && rich).at(2, 2).plain(),
                Item::small("clearFormatting", "Clear Formatting", "Clear All Formatting", "format.clear", no()).at(2, 5).plain(),
            ],
        ),
        g(
            "Paragraph",
            vec![
                Item::small("bullets", "Bullets", "Bullets", "format.bullets", no()).checked(look.bullets).at(3, 3).plain(),
                Item::small("numbering", "Numbering", "Numbering", "format.numbering", no()).checked(look.numbering).at(3, 3).plain(),
                Item::small("decreaseIndent", "Decrease Indent", "Decrease Indent", "format.outdent", no()).at(3, 4).plain(),
                Item::small("increaseIndent", "Increase Indent", "Increase Indent", "format.indent", no()).at(3, 4).plain(),
                Item::pending("format.lineSpacing", "Line Spacing", 3),
            ],
        ),
        g("Styles", vec![Item::pending("format.styles", "Styles", 4)]),
        g(
            "Editing",
            vec![
                Item::small("find", "Find", "Find", "edit.find", no()).at(5, 4).plain(),
                Item::small("replace", "Replace", "Replace", "edit.replace", no()).at(5, 5).plain(),
            ],
        ),
    ])
}

/// Insert, while a message is being written, in classic Outlook's groups: what Message › Include
/// has, the date or the time, and the rest still to be built.
fn insert() -> Tab {
    Tab::of(vec![
        g(
            "Include",
            vec![
                Item::big("attach", "Attach\nFile", "ui.attach", no()).at(0, 2),
                Item::big("open", "Item", "compose.attachItem", no()).at(0, 3),
                Item::big("people", "Contact\nCard", "insert.businessCard", no()).at(0, 4),
                Item::small("calendar", "Calendar", "Insert Calendar", "insert.calendar", no()).at(0, 4),
                Item::big_menu("signature", "signature", "Signature", "Signature", Menu::Signature).at(0, 3),
            ],
        ),
        g("Tables", vec![Item::pending("insert.table", "Table", 1)]),
        g(
            "Illustrations",
            vec![
                Item::small("picture", "Pictures", "Insert Picture", "insert.picture", no()).at(2, 5),
                Item::pending("insert.shape", "Shapes", 2),
                Item::pending("insert.icon", "Icons", 2),
                Item::pending("insert.models3d", "3D Models", 2),
                Item::pending("insert.diagram", "Diagram", 2),
                Item::pending("insert.chart", "Chart", 2),
                Item::pending("insert.screenshot", "Screenshot", 2),
            ],
        ),
        g("Links", vec![Item::big("insertLink", "Link", "compose.link", no()).at(3, 3), Item::pending("insert.bookmark", "Bookmark", 3)]),
        g(
            "Text",
            vec![
                Item::pending("insert.textBox", "Text Box", 4),
                Item::pending("insert.quickParts", "Saved Text", 4),
                Item::pending("insert.textEffects", "Text Effects", 4),
                Item::pending("insert.dropCap", "Drop Cap", 4),
                Item::big("dateTime", "Date &\nTime", "insert.dateTime", no()).at(4, 3),
                Item::pending("insert.object", "Object", 4),
            ],
        ),
        g(
            "Symbols",
            vec![
                Item::pending("insert.equation", "Equation", 5),
                Item::big("symbol", "Symbol", "insert.symbol", no()).at(5, 4),
                Item::pending("insert.rule", "Horizontal Line", 5),
                Item::pending("insert.emoji", "Emoji", 5),
            ],
        ),
    ])
}

/// Review, while a message is being written, in classic Outlook's groups: Word Count, and the
/// rest still to be built.
fn review() -> Tab {
    Tab::of(vec![
        g(
            "Proofing",
            vec![
                Item::pending("review.spelling", "Spelling & Grammar", 0),
                Item::pending("review.thesaurus", "Thesaurus", 0),
                Item::big("wordCount", "Word\nCount", "review.wordCount", no()).at(0, 3),
            ],
        ),
        g("Accessibility", vec![Item::pending("review.accessibility", "Check Accessibility", 1)]),
        g("Language", vec![Item::pending("review.translate", "Translate", 2), Item::pending("review.language", "Language", 2)]),
    ])
}

/// What a control still to be built says when the pointer is on it; a screen reader has it as a
/// button that can't be used, "<name>, pending development". (The other controls of the ribbon
/// have no name for it yet: tests that look for a menu's or a dialog's items by name count on that.)
fn pending_tip(resp: egui::Response, item: &Item) -> egui::Response {
    let name = crate::i18n::fmt(tl!("{name}, pending development"), &[("name", &item.name())]);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, false, name.clone()));
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(item.name()).font(semibold(12.0)));
        ui.label(egui::RichText::new(tl!("Pending development")).small().weak());
    })
}

fn send_receive(app: &MailApp) -> Tab {
    let note = if !app.services.can_sync() {
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
                    Item::big("close", "Cancel\nAll", "sync.cancel", no()).at(0, 4),
                ],
            ),
            g(
                "Accounts",
                vec![
                    Item::big("account", "Add\nAccount", "ui.dialog", json!({"name": "account"})).key("account.add").at(1, 2),
                    Item::big("info", "Account\nSettings", "ui.backstage", json!({"value": true, "page": "info"})).key("account.settings").at(1, 2),
                ],
            ),
            g("Preferences", vec![Item::big("device", "Work\nOffline", "sync.offline", no()).checked(app.offline()).at(2, 2)]),
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
        tab.groups.retain(|group| !["Current View", "Messages", "Arrangement", "Search"].contains(&group.title));
        let after_layout = tab.groups.len().min(1);
        tab.groups.splice(after_layout..after_layout, [g("Arrangement", calendar_views(v.calendar, 1)), g("Calendar", vec![scale, overlay, beside])]);
    }
    tab
}

/// The View tab as every module has it: the list's ready-made views, Layout, then the messages'
/// two groups (which the calendar replaces with its own, by their titles), the recent searches,
/// then Window. The calendar drops the ready-made views and the recent searches, which are the
/// mail's.
fn view_of_any(app: &MailApp) -> Tab {
    let v = &app.session.view;
    let tight = app.session.prefs.tight_spacing;
    let mut layout = vec![
        Item::big("folderPane", "Folder\nPane", "view.folderPane", no()).checked(v.folder_pane).at(0, 1),
        Item::big_menu("readingPane", "readingPane", "Reading\nPane", "Reading Pane", Menu::ReadingPane).at(0, 2),
        Item::big_menu("navigationBar", "navigationBar", "Navigation\nBar", "Navigation Bar", Menu::NavigationBar).at(0, 3),
    ];
    // Gmail's categories as folders or as tabs (#911): only with a Gmail account that shows them.
    if mailcraft_engine::gmail::offered(&app.session).is_none() {
        layout.push(Item::big_menu("gmailCategories", "inbox", "Gmail\nCategories", "Gmail Categories", Menu::GmailCategories).at(0, 3));
    }
    layout.push(Item::big_menu("ribbonLayout", "ribbon", "Ribbon\nLayout", "Ribbon Layout", Menu::RibbonLayout).at(3, 4));
    Tab::of(vec![
        g("Current View", vec![Item::big_menu("changeView", "changeView", "Change\nView", "Change View", Menu::ChangeView).at(0, 1)]),
        g("Layout", layout),
        g(
            "Messages",
            vec![
                Item::big("conversations", "Show as\nConversations", "view.conversations", no()).checked(v.conversations).at(1, 2),
                Item::big_menu("preview", "previewLines", "Message\nPreview", "Message Preview", Menu::Preview).at(1, 3),
                Item::big("density", "Tighter\nSpacing", "view.density", no()).checked(tight).at(1, 4),
            ],
        ),
        g(
            "Arrangement",
            vec![
                Item::big_menu("arrangeBy", "sort", "Arrange\nBy", "Arrange By", Menu::ArrangeBy).at(2, 2),
                Item::big("reverseSort", "Reverse\nSort", "view.reverseSort", no()).at(2, 3),
            ],
        ),
        g("Search", vec![Item::big_menu("recentSearches", "recentSearches", "Recent\nSearches", "Recent Searches", Menu::RecentSearches).at(2, 4)]),
        g(
            "Window",
            vec![
                Item::big_menu("theme", "theme", "Theme", "Interface Theme", Menu::Theme).at(3, 3),
                Item::big_menu("zoom", "zoom", "Zoom", "Zoom the Message", Menu::Zoom).at(3, 4),
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
                Item::big("help", "Help", "help.help", no()).at(0, 1),
                Item::big("info", "Show\nTraining", "help.training", no()).at(0, 3),
                Item::big("web", "Get\nSupport", "help.support", no()).at(0, 3),
                Item::big("notes", "Feedback", "help.feedback", no()).at(0, 4),
                Item::big("favorite", "What's\nNew", "help.whatsNew", no()).at(0, 3),
                Item::big("search", "Search\nCommands", "ui.dialog", json!({"name": "commands"})).key("help.commands").at(0, 1),
                Item::big("about", "About\nMailCraft", "ui.dialog", json!({"name": "about"})).key("help.about").at(0, 2),
                Item::big("copy", "Copy\nDiagnostics", "ui.copyDiagnostics", no()).at(0, 3),
                Item::big("report", "Report a\nProblem", "ui.reportProblem", no()).at(0, 4),
            ],
        )],
        note: tl!(
            "MailCraft is open source (MIT or Apache-2.0). Every button here runs a command that scripts and agents can run too: see Search Commands."
        ),
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
    let edit = crate::text_menu::field(edit);
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

/// The accounts of the selected messages, each once.
pub(crate) fn selected_accounts(app: &MailApp) -> Vec<Id> {
    let mut accounts: Vec<Id> = app.session.sel.messages.iter().filter_map(|id| app.session.mailbox.message(*id)).map(|m| m.account).collect();
    accounts.sort_unstable();
    accounts.dedup();
    accounts
}

/// The categories the selected messages carry, each once.
pub(crate) fn selected_categories(app: &MailApp) -> Vec<String> {
    let messages = app.session.sel.messages.iter().filter_map(|id| app.session.mailbox.message(*id));
    let mut names: Vec<String> = messages.flat_map(|m| m.categories.iter().cloned()).collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Most names off the list that the Categorize menu shows.
const MAX_OFF_LIST: usize = 32;

/// The categories of `accounts`, those of the selection's items (#633), and the names of `carried`
/// (what the items have) that aren't on those lists, as menu items that toggle one on the
/// selection through `command`: an item moved from another account, or a name its server stopped
/// listing, can still have it taken off.
pub fn category_menu(
    ui: &mut Ui,
    app: &mut MailApp,
    accounts: &[Id],
    carried: &[String],
    command: &str,
    has: impl Fn(&MailApp, &str) -> bool,
    clear: Option<&str>,
) {
    let mailbox = &app.session.mailbox;
    let mut cats: Vec<(String, egui::Color32)> = mailbox.categories_for(accounts).into_iter().map(|c| (c.name, crate::theme::c32(c.color))).collect();
    let listed = cats.len();
    for name in carried {
        if cats.len() < listed + MAX_OFF_LIST && !cats.iter().any(|(c, _)| c.eq_ignore_ascii_case(name)) {
            let color = accounts.iter().find_map(|a| mailbox.category_of(*a, name)).map_or(mailcraft_model::GREY_CATEGORY, |c| c.color);
            cats.push((name.clone(), crate::theme::c32(color)));
        }
    }
    for (name, color) in cats {
        let on = has(app, &name);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().rect_filled(r.shrink(1.0), 3.0, color);
            if on {
                ui.painter().text(r.center(), Align2::CENTER_CENTER, "✓", medium(10.0), Tokens::get(ui.ctx()).on_swatch);
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
        let _ = app.run("ui.dialog", json!({"name": "categories", "account": accounts.first()}));
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

/// Home › Delete › Junk ▾ (and the message's context menu): the lists and what is in Junk E-mail.
pub(crate) fn junk_menu(ui: &mut Ui, app: &mut MailApp) {
    mi(ui, app, "Block Sender", "junk.blockSender", json!({}));
    mi(ui, app, "Never Block Sender", "junk.neverBlockSender", json!({}));
    mi(ui, app, "Never Block Sender's Domain", "junk.neverBlockDomain", json!({}));
    ui.separator();
    // Where the message is (a search's result may be elsewhere than the folder in view).
    let folder = app.session.focused_message().map_or(app.session.view.folder, |m| m.folder);
    let in_junk = app.session.mailbox.folder(folder).is_some_and(|f| f.kind == FolderKind::Junk);
    if in_junk {
        mi(ui, app, "Not Junk", "mail.notJunk", json!({}));
    } else {
        mi(ui, app, "Move to Junk", "mail.junk", json!({}));
    }
    ui.separator();
    if plain_item(ui, tl!("Junk E-mail Options…")) {
        let _ = app.run("ui.dialog", json!({"name": "junkOptions"}));
    }
}

/// What `menu` lists.
fn menu_contents(ui: &mut Ui, app: &mut MailApp, menu: Menu) {
    match menu {
        Menu::RecentAttachments => crate::recent_attachments::contents(ui, app),
        Menu::NewItems => new_items(ui, app),
        Menu::MoveMessages => {
            if app.session.sel.messages.is_empty() {
                ui.label(tl!("Select a message first"));
                return;
            }
            // (Not mail a JMAP account's server has, which stays where it is for now: why, instead.)
            let refused = app.session.registry.get("mail.move").and_then(|c| (c.enabled)(&app.session));
            match refused {
                Some(why) => {
                    ui.label(tl!(why));
                }
                None => folder_menu(ui, app, |app, folder| {
                    let _ = app.run("mail.move", json!({"folder": folder}));
                }),
            }
            ui.separator();
            ui.menu_button(tl!("Copy to Folder"), |ui| {
                folder_menu(ui, app, |app, folder| {
                    let _ = app.run("mail.copy", json!({"folder": folder}));
                });
            });
        }
        Menu::Rules => {
            if !app.session.sel.messages.is_empty() {
                // A shared mailbox's rules are its own, which this version doesn't make: one for
                // the owner's account would move the owner's own mail into the shared mailbox.
                let mb = &app.session.mailbox;
                let shared = app.session.sel.focus().and_then(|m| mb.message(m)).and_then(|m| mb.account(m.account)).is_some_and(|a| a.is_shared());
                if shared {
                    ui.add_enabled(false, egui::Button::new(tl!("Always Move Messages From This Sender")).min_size(vec2(190.0, 0.0)))
                        .on_disabled_hover_text(tl!("A shared mailbox's rules are its own: this version doesn't make them"));
                } else {
                    ui.menu_button(tl!("Always Move Messages From This Sender"), |ui| {
                        folder_menu(ui, app, |app, folder| {
                            // The New rule filled in, for the owner to see and add, where it is kept too.
                            let _ = app.run("ui.dialog", json!({"name": "rules", "folder": folder}));
                        });
                    });
                }
                ui.separator();
            }
            mi(ui, app, "Run Rules Now", "rules.run", json!({}));
            if plain_item(ui, tl!("Manage Rules…")) {
                let _ = app.run("ui.dialog", json!({"name": "rules"}));
            }
        }
        Menu::Junk => junk_menu(ui, app),
        Menu::CategorizeMail => {
            let ids = app.session.sel.messages.clone();
            let accounts = selected_accounts(app);
            let carried = selected_categories(app);
            category_menu(
                ui,
                app,
                &accounts,
                &carried,
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
            let event = id.and_then(|id| app.session.mailbox.event(id));
            let account = event.map(|e| app.session.mailbox.event_account(e));
            let carried = event.map(|e| e.categories.clone()).unwrap_or_default();
            category_menu(ui, app, account.as_slice(), &carried, "calendar.categorize", has, None);
        }
        Menu::CategorizeContact => {
            let id = app.session.sel.contact;
            let has = |app: &MailApp, name: &str| {
                id.and_then(|id| app.session.mailbox.contact(id)).is_some_and(|c| c.categories.iter().any(|x| x.eq_ignore_ascii_case(name)))
            };
            let contact = id.and_then(|id| app.session.mailbox.contact(id));
            let (account, carried) = (contact.map(|c| c.origin.account), contact.map(|c| c.categories.clone()).unwrap_or_default());
            category_menu(ui, app, account.as_slice(), &carried, "people.categorize", has, None);
        }
        Menu::CategorizeTask => {
            let id = app.session.sel.task;
            let has = |app: &MailApp, name: &str| {
                id.and_then(|id| app.session.mailbox.task(id)).is_some_and(|x| x.categories.iter().any(|c| c.eq_ignore_ascii_case(name)))
            };
            let task = id.and_then(|id| app.session.mailbox.task(id));
            let (account, carried) = (task.map(|t| t.origin.account), task.map(|t| t.categories.clone()).unwrap_or_default());
            category_menu(ui, app, account.as_slice(), &carried, "tasks.categorize", has, None);
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
        Menu::NavigationBar => {
            let foot = app.session.view.nav_foot;
            mi_check(ui, app, "Bar on the Left", !foot, "view.navigationBar", json!({"value": "bar"}));
            mi_check(ui, app, "At the Foot of the Pane", foot, "view.navigationBar", json!({"value": "foot"}));
        }
        Menu::GmailCategories => {
            let tabs = app.session.view.gmail_tabs;
            mi_check(ui, app, "As Folders", !tabs, "view.gmailCategories", json!({"value": "folders"}));
            mi_check(ui, app, "As Tabs above the Inbox", tabs, "view.gmailCategories", json!({"value": "tabs"}));
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
        // Tasks and People have their own arrangements.
        Menu::ArrangeBy if app.session.view.module == Module::Tasks => {
            let sort = app.session.view.task_sort;
            for s in TaskSort::ALL {
                mi_check(ui, app, s.label(), sort == s, "tasks.arrange", json!({"by": s}));
            }
        }
        Menu::ArrangeBy if app.session.view.module == Module::People => {
            let sort = app.session.view.people_sort;
            for s in PeopleSort::ALL {
                mi_check(ui, app, s.label(), sort == s, "people.arrange", json!({"by": s}));
            }
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
        Menu::ChangeView | Menu::Zoom | Menu::RecentSearches => view_menus::show(ui, app, menu),
        Menu::Font => font_menu(ui, app),
        Menu::FontColor => color_menu(ui, app),
        Menu::Highlight => highlight_menu(ui, app),
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
        crate::text_menu::field(ui.add(egui::TextEdit::singleline(&mut typed).hint_text("#rrggbb").desired_width(90.0).char_limit(7)));
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

/// Highlights offered behind text: an original palette of light colours, which dark text reads on.
pub(crate) const HIGHLIGHTS: [(&str, &str); 8] = [
    ("Yellow", "#fff176"),
    ("Lime", "#c5e1a5"),
    ("Aqua", "#80deea"),
    ("Sky", "#90caf9"),
    ("Pink", "#f8bbd0"),
    ("Lavender", "#d1c4e9"),
    ("Peach", "#ffcc80"),
    ("Grey", "#d9d9d9"),
];

/// The highlights (`format.highlight`): No Color, and the palette as swatches.
pub(crate) fn highlight_menu(ui: &mut Ui, app: &mut MailApp) {
    let now = look_now(app).highlight;
    let enabled = app.enabled("format.highlight");
    mi_check(ui, app, "No Color", now == Some(None), "format.highlight", json!({"value": null}));
    ui.horizontal_wrapped(|ui| {
        ui.set_max_width(190.0);
        for (name, hex) in HIGHLIGHTS {
            let Ok(c) = egui::Color32::from_hex(hex) else { continue };
            let chosen = now.as_ref().is_some_and(|n| n.as_deref() == Some(hex));
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(r, 2.0, c);
            if chosen || resp.hovered() {
                ui.painter().rect_stroke(r.expand(1.5), 2.0, egui::Stroke::new(1.5, ui.visuals().text_color()), egui::StrokeKind::Outside);
            }
            app.hit.add("highlightSwatch", hit_id(hex), r);
            if resp.on_hover_text(tl!(name)).clicked() && enabled {
                let _ = app.run("format.highlight", json!({"value": hex}));
                ui.close();
            }
        }
    });
}

/// A control that runs a command, as an entry of a menu: its name on one line, its key, a mark
/// when it is on.
fn entry(ui: &mut Ui, app: &mut MailApp, item: &Item) {
    let Some((id, params)) = &item.command else { return };
    let (key, enabled) = (shortcut_text(app, id), usable(app, ui.ctx(), item));
    let mut button = egui::Button::new(item.name()).selected(item.checked).min_size(vec2(190.0, 0.0));
    if !key.is_empty() {
        button = button.shortcut_text(key);
    }
    if ui.add_enabled(enabled, button).clicked() {
        run_ribbon_command(app, ui, id, params);
        ui.close();
    }
}

fn run_ribbon_command(app: &mut MailApp, ui: &Ui, command: &str, params: &Value) {
    if !crate::text_menu::compose_clipboard(app, ui.ctx(), command) {
        let _ = app.run(command, params.clone());
    }
}

/// Record the menu's bounds so a click in it leaves a composer header's selection in place.
fn ribbon_popup(app: &mut MailApp, response: &egui::Response, contents: impl FnOnce(&mut Ui, &mut MailApp)) {
    if let Some(popup) = egui::Popup::menu(response).show(|ui| contents(ui, app)) {
        app.hit.add("ribbonPopup", 0, popup.response.rect);
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
        .filter(|item| !item.classic_only && item.under.is_none())
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
        // What is still to be built goes behind `…` before anything else gives way.
        if b.item.pending {
            steps.push((0, rank, i, true));
            continue;
        }
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
            Some(menu) => submenu(ui, app, under.name(), menu),
            None => entry(ui, app, under),
        }
    }
}

/// A button that is behind `…`, as entries of that menu.
fn more_entries(ui: &mut Ui, app: &mut MailApp, b: &Button<'_>) {
    if b.item.pending {
        let entry = ui.add_enabled(false, egui::Button::new(b.item.name()).min_size(vec2(190.0, 0.0)));
        entry.on_disabled_hover_text(tl!("Pending development"));
        return;
    }
    if b.item.command.is_none() {
        match b.item.menu {
            Some(menu) if b.drops.is_empty() => submenu(ui, app, b.item.name(), menu),
            _ => {
                ui.menu_button(b.item.name(), |ui| drop_menu(ui, app, b));
            }
        }
        return;
    }
    entry(ui, app, b.item);
    if let Some(menu) = b.item.menu {
        submenu(ui, app, b.item.tip_text(), menu);
    }
    for under in &b.drops {
        match under.menu {
            Some(menu) => submenu(ui, app, under.name(), menu),
            None => entry(ui, app, under),
        }
    }
}

/// A small `▾`, centred on `c` (also the one at an attachment chip's right end, #785).
pub(crate) fn drop_mark(ui: &Ui, c: egui::Pos2, color: egui::Color32) {
    ui.painter().add(Shape::line(vec![pos2(c.x - 2.75, c.y - 1.0), pos2(c.x, c.y + 1.25), pos2(c.x + 2.75, c.y - 1.0)], Stroke::new(1.1, color)));
}

/// One button of the simplified row, in `rect`.
fn simple_button(app: &mut MailApp, ui: &mut Ui, t: &Tokens, b: &Button<'_>, labelled: bool, rect: Rect) {
    let item = b.item;
    let id = ui.scope_id().with(("ribbon", item.key));
    let on = usable(app, ui.ctx(), item);
    let drop_rect = Rect::from_min_max(pos2(rect.max.x - DROP_W, rect.min.y), rect.max);
    let main_rect = if b.is_split() { Rect::from_min_max(rect.min, pos2(drop_rect.min.x, rect.max.y)) } else { rect };
    // (A control still to be built takes no clicks and no keyboard.)
    let main = ui.interact(main_rect, id, if item.pending { Sense::hover() } else { Sense::click() });
    app.hit.add("ribbon", hit_id(item.key), main_rect);
    if item.pending {
        let icon = Rect::from_center_size(pos2(rect.min.x + PAD + ICON / 2.0, rect.center().y), vec2(ICON, ICON));
        icons::paint(ui.painter(), icon, item.icon, t.text_disabled, t.text_disabled);
        if labelled {
            ui.painter().text(pos2(icon.max.x + LABEL_GAP, rect.center().y), Align2::LEFT_CENTER, item.name(), regular(LABEL_SIZE), t.text_disabled);
        }
        pending_tip(main, item);
        return;
    }
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
        let main = menu_tooltip(app, main, item, on);
        if on {
            ribbon_popup(app, &main, |ui, app| drop_menu(ui, app, b));
        }
        return;
    };
    let main = command_tooltip(app, main, &item.tip_text(), command);
    if main.clicked() && on {
        run_ribbon_command(app, ui, command, params);
    }
    if !b.has_drop() {
        return;
    }
    // Its other half: the `▾` opens the menu, whether or not the command can run now.
    let drop = ui.interact(drop_rect, id.with("menu"), Sense::click());
    if item.menu != Some(Menu::RecentAttachments) {
        app.hit.add("ribbonMenu", hit_id(item.key), drop_rect);
    }
    bg(ui, drop_rect, &drop, false, t);
    drop_mark(ui, drop_rect.center(), t.icon);
    let tip = b.drops.first().map_or_else(|| item.tip_text(), |under| under.tip_text());
    let drop = drop.on_hover_text(tip);
    if item.menu != Some(Menu::RecentAttachments) || menu_usable(app, Menu::RecentAttachments) {
        ribbon_popup(app, &drop, |ui, app| drop_menu(ui, app, b));
    }
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
        ribbon_popup(app, &resp, |ui, app| {
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

fn classic_recent_attachments(app: &mut MailApp, response: &egui::Response, enabled: bool) {
    if !enabled {
        return;
    }
    if crate::recent_attachments::available(app) {
        ribbon_popup(app, response, crate::recent_attachments::contents);
    } else if response.clicked() {
        let _ = app.run("ui.attach", json!({}));
    }
}

/// A large button of the classic layout.
fn classic_big(app: &mut MailApp, ui: &mut Ui, item: &Item) {
    let label = tl!(item.label);
    match (&item.command, item.menu) {
        (_, Some(Menu::RecentAttachments)) => {
            let on = usable(app, ui.ctx(), item);
            let (resp, text) = big_frame(ui, item.icon, label, on, false, true);
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            app.hit.add("ribbonLabel", hit_id(item.key), text);
            classic_recent_attachments(app, &resp, on);
        }
        (Some((command, params)), _) => {
            let on = usable(app, ui.ctx(), item);
            let (resp, text) = big_frame(ui, item.icon, label, on, item.checked, false);
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            app.hit.add("ribbonLabel", hit_id(item.key), text);
            let resp = command_tooltip(app, resp, &item.tip_text(), command);
            if resp.clicked() && on {
                run_ribbon_command(app, ui, command, params);
            }
        }
        (None, Some(menu)) => {
            let on = usable(app, ui.ctx(), item);
            let (resp, text) = big_frame(ui, item.icon, label, on, false, true);
            let resp = menu_tooltip(app, resp, item, on);
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            app.hit.add("ribbonLabel", hit_id(item.key), text);
            show_classic_menu(app, resp, menu, on);
        }
        (None, None) if item.pending => {
            let (resp, _) = big_frame(ui, item.icon, label, false, false, false);
            app.hit.add("ribbon", hit_id(item.key), resp.rect);
            pending_tip(resp, item);
        }
        (None, None) => {}
    }
}

/// A small button of the classic layout. The icon keeps its tooltip even without a visible label.
fn classic_small(app: &mut MailApp, ui: &mut Ui, item: &Item, labelled: bool, width: Option<f32>) {
    let name = item.name();
    let label = labelled.then_some(name.as_str());
    let on = usable(app, ui.ctx(), item) && !item.pending;
    let menu = fit::drops(item);
    let resp = match width {
        Some(width) => widgets::small_frame_with_width(ui, item.icon, label, on, item.checked, menu, width),
        None => small_frame(ui, item.icon, label, on, item.checked, menu),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, on, &name));
    app.hit.add("ribbon", hit_id(item.key), resp.rect);
    match (&item.command, item.menu) {
        (_, Some(Menu::RecentAttachments)) => {
            let resp = menu_tooltip(app, resp, item, on);
            classic_recent_attachments(app, &resp, on);
        }
        (Some((command, params)), _) => {
            let resp = command_tooltip(app, resp, &item.tip_text(), command);
            if resp.clicked() && on {
                run_ribbon_command(app, ui, command, params);
            }
        }
        (None, Some(menu)) => {
            let resp = menu_tooltip(app, resp, item, on);
            show_classic_menu(app, resp, menu, on);
        }
        (None, None) if item.pending => {
            pending_tip(resp, item);
        }
        (None, None) => {}
    }
}

fn show_classic_menu(app: &mut MailApp, resp: egui::Response, menu: Menu, on: bool) {
    if on {
        ribbon_popup(app, &resp, |ui, app| {
            ui.set_min_width(200.0);
            menu_contents(ui, app, menu);
        });
    }
}

/// The values of an Outlook-style font or size picker, using the selection or the typing style.
fn classic_picker(app: &mut MailApp, ui: &mut Ui, item: &Item, menu: Menu, width: f32) {
    let look = look_now(app);
    let value = match menu {
        Menu::Font => match look.font {
            Some(Some(font)) => tl!(font.label()).to_owned(),
            Some(None) => tl!("Default").to_owned(),
            None => String::new(),
        },
        _ => match look.size {
            Some(Some(size)) => size.to_string(),
            Some(None) => format!("{:.0}", crate::compose::editor::BASE_SIZE * 3.0 / 4.0),
            None => "—".to_owned(),
        },
    };
    let on = usable(app, ui.ctx(), item);
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), if on { Sense::click() } else { Sense::hover() });
    let field = rect.shrink2(vec2(0.0, 1.0));
    ui.painter().rect(field, 3.0, t.input, Stroke::new(1.0, t.input_border), egui::StrokeKind::Inside);
    if on {
        bg(ui, field, &resp, false, &t);
    }
    let color = if on { t.text } else { t.text_disabled };
    paint_clipped(ui.painter(), pos2(field.min.x + 6.0, field.center().y), width - 20.0, &value, regular(11.5), color);
    drop_mark(ui, pos2(field.max.x - 9.0, field.center().y), color);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, on, item.name()));
    app.hit.add("ribbon", hit_id(item.key), rect);
    let resp = menu_tooltip(app, resp, item, on);
    show_classic_menu(app, resp, menu, on);
}

/// A group's controls at this stage. The formatting grid keeps its arrangement in a narrow
/// window until the group folds, so its font and size fields stay together.
fn classic_cells(app: &mut MailApp, ui: &mut Ui, group: &Group, items: &[&Item], stage: Stage) {
    if !group.classic_rows.is_empty() {
        stack(ui, |ui| {
            for row in fit::packed_rows(group) {
                ui.horizontal(|ui| {
                    for item in row {
                        if let Some((menu, width)) = fit::picker(item, stage) {
                            classic_picker(app, ui, item, menu, width);
                        } else {
                            classic_small(app, ui, item, false, Some(fit::packed_icon_width(item, stage)));
                        }
                    }
                });
            }
        });
        return;
    }
    for cell in fit::cells(items, stage) {
        match cell {
            Cell::Big(item) => classic_big(app, ui, item),
            Cell::Column(column) => stack(ui, |ui| {
                for item in column {
                    let labelled = fit::show_label(stage);
                    classic_small(app, ui, item, labelled, None);
                }
            }),
        }
    }
}

/// A group that is behind `…`, as entries of a menu.
fn group_entries(ui: &mut Ui, app: &mut MailApp, group: &Group) {
    ui.set_min_width(200.0);
    for item in fit::shown(group) {
        if item.pending {
            let entry = ui.add_enabled(false, egui::Button::new(item.name()).min_size(vec2(190.0, 0.0)));
            entry.on_disabled_hover_text(tl!("Pending development"));
        } else if item.menu == Some(Menu::RecentAttachments) && crate::recent_attachments::available(app) {
            submenu(ui, app, item.name(), Menu::RecentAttachments);
        } else if item.command.is_some() {
            entry(ui, app, item);
        } else if let Some(menu) = item.menu {
            submenu(ui, app, item.name(), menu);
        }
    }
}

/// Each group of the classic layout in the frame just laid out: its title, its stage, and whether it is behind `…`.
#[cfg(test)]
pub(crate) fn last_fit(ctx: &egui::Context) -> Vec<(&'static str, Stage, bool)> {
    ctx.data(|d| d.get_temp::<Vec<(&'static str, Stage, bool)>>(egui::Id::NULL.with("ribbon_fit"))).unwrap_or_default()
}

/// The classic layout: `tab` in its groups, large buttons side by side and small ones three to a
/// column. In less than `available` wide, groups shrink, fold and go behind `…` ([`fit::fit`]).
fn classic(app: &mut MailApp, ui: &mut Ui, t: &Tokens, tab: &Tab, available: f32) {
    let groups: Vec<&Group> = tab.groups.iter().filter(|group| !fit::shown(group).is_empty()).collect();
    let widths: Vec<[f32; 4]> = groups.iter().map(|group| fit::group_widths(group, &|text, size| widgets::label_width(ui, text, size))).collect();
    let priority: Vec<u8> = groups.iter().map(|group| group.priority).collect();
    let note = if tab.note.is_empty() { 0.0 } else { tab.note_width };
    let plan = fit::fit(&widths, &priority, note, available);
    #[cfg(test)]
    {
        let shown: Vec<(&'static str, Stage, bool)> =
            groups.iter().zip(&plan.stages).enumerate().map(|(i, (group, stage))| (group.title, *stage, plan.behind.contains(&i))).collect();
        ui.data_mut(|d| d.insert_temp(egui::Id::NULL.with("ribbon_fit"), shown));
    }
    for (i, group_of) in groups.iter().enumerate() {
        let items = fit::shown(group_of);
        let stage = plan.stages.get(i).copied().unwrap_or(Stage::Full);
        if plan.behind.contains(&i) {
            continue;
        }
        if stage != Stage::Folded {
            group(ui, group_of.title, app, |ui, app| classic_cells(app, ui, group_of, &items, stage));
            continue;
        }
        let top = ui.cursor().min.y;
        let icon = items.first().map_or("more", |item| item.icon);
        let resp = widgets::fold_frame(ui, icon, tl!(group_of.title));
        app.hit.add("ribbonFold", hit_id(group_of.title), resp.rect);
        // The group as it normally looks; as small buttons when even that is wider than the window.
        let room = ui.ctx().content_rect().width() - 24.0;
        let open = if widths.get(i).is_some_and(|w| w[0] - fit::GROUP_TAIL <= room) { Stage::Full } else { Stage::Small };
        ribbon_popup(app, &resp, |ui, app| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(1.0, 1.0);
                classic_cells(app, ui, group_of, &items, open);
            });
        });
        widgets::group_end(ui, top);
    }
    if !plan.behind.is_empty() {
        ui.add_space(2.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(MORE_W, CONTENT_H + LABEL_H), Sense::click());
        app.hit.add("ribbonMore", 0, rect);
        bg(ui, rect, &resp, false, t);
        icons::paint(ui.painter(), Rect::from_center_size(rect.center(), vec2(ICON, ICON)), "more", t.icon, t.accent);
        let resp = resp.on_hover_text(tl!("More commands"));
        ribbon_popup(app, &resp, |ui, app| {
            ui.set_min_width(200.0);
            for i in &plan.behind {
                if let Some(group_of) = groups.get(*i) {
                    ui.menu_button(tl!(group_of.title), |ui| {
                        group_entries(ui, app, group_of);
                        app.hit.add("ribbonPopup", 0, ui.min_rect());
                    });
                }
            }
        });
    }
    if plan.note {
        ui.add_space(fit::NOTE_GAP);
        ui.allocate_ui(vec2(tab.note_width, CONTENT_H), |ui| {
            ui.add(egui::Label::new(egui::RichText::new(tl!(tab.note)).color(t.text_dim).size(11.5)).wrap());
        });
    }
}

// ---- the ribbon ----

pub fn show(app: &mut MailApp, ui: &mut Ui) {
    let tabs = tabs(app);
    if !tabs.contains(&app.ui.tab.as_str()) {
        app.ui.tab = "Home".into();
    }
    let current = if app.ui.backstage { String::new() } else { app.ui.tab.clone() };
    // The tabs and the ribbon start beside the module bar, in line with the panes under them.
    if let Some((tab, double)) = strip(app, ui, &tabs, &current, egui::Id::unique("tabs")) {
        let _ = app.run("ui.tab", json!({"tab": tab}));
        if double && tab != "File" {
            let _ = app.run("ui.collapseRibbon", json!({}));
        }
    }
    if app.ui.ribbon_collapsed {
        return;
    }
    let tab = describe(app);
    band(app, ui, &tab, egui::Id::unique("ribbon"), 0);
}

/// The ribbon of a window of its own with a message being written (#286), numbered `window`:
/// its tabs Message, Insert, Format Text and Review, of which `tab` is in view, and Search
/// commands, in the main ribbon's layout (classic or simplified). What it runs acts on the
/// window's message, which is the current one while the window is laid out.
pub(crate) fn show_window(app: &mut MailApp, ui: &mut Ui, window: u64, tab: &mut &'static str) {
    let tabs = crate::windows::compose_window::TABS;
    if !tabs.contains(tab) {
        *tab = "Message";
    }
    if let Some((chosen, _)) = strip(app, ui, &tabs, tab, egui::Id::unique(("tabs", window)))
        && let Some(chosen) = tabs.iter().find(|t| **t == chosen)
    {
        *tab = chosen;
    }
    let mut described = describe_tab(app, tab);
    // (It is in a window of its own already; and Send is beside its lines, as a message's window
    // has it in classic Outlook, not in its ribbon too.)
    for group in &mut described.groups {
        group.items.retain(|item| item.command.as_ref().is_none_or(|(id, _)| !matches!(*id, "compose.popOut" | "compose.send")));
    }
    described.groups.retain(|group| !group.items.is_empty());
    // Leave the same gutter at both edges of the composer window.
    band(app, ui, &described, egui::Id::unique(("ribbon", window)), 8);
}

/// The row of tabs, `current` the one in view, with Search commands at its right end; `id` names
/// its panel. The tab clicked, and whether it was a double click.
fn strip(app: &mut MailApp, ui: &mut Ui, tabs: &[&'static str], current: &str, id: egui::Id) -> Option<(&'static str, bool)> {
    let t = Tokens::get(ui.ctx());
    let mut clicked = None;
    egui::Panel::top(id)
        .exact_size(TAB_H)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.tab_strip).inner_margin(egui::Margin { left: 3, right: 10, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for tab in tabs {
                    let shown = tl!(tab);
                    let active = current == *tab;
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
                        clicked = Some((*tab, false));
                    }
                    if resp.double_clicked() {
                        clicked = Some((*tab, true));
                    }
                }
                // Search every command by name ("Tell me"), in what the tabs leave of the row: it
                // gives way before it would cover them, to the magnifier alone and then to nothing
                // (the Help tab has it too, #712).
                let label = tl!("Search commands");
                let shown = fit::search(ui.available_width(), widgets::icon_text_width(ui, label));
                if shown != fit::Search::Hidden {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let resp = match shown {
                            fit::Search::Named => widgets::icon_text_button(ui, "search", label, true),
                            _ => widgets::icon_button(ui, "search", label, 16.0, true),
                        };
                        app.hit.add("ribbonSearch", 0, resp.rect);
                        if resp.clicked() {
                            let _ = app.run("ui.dialog", json!({"name": "commands"}));
                        }
                    });
                }
            });
        });
    clicked
}

/// The commands of `tab` under the row of tabs, in the layout chosen; `id` names its panel.
fn band(app: &mut MailApp, ui: &mut Ui, tab: &Tab, id: egui::Id, left_inset: i8) {
    let t = Tokens::get(ui.ctx());
    let mode = app.ui.ribbon_mode;
    // The ribbon is a sheet on the window's own colour, with a gap under it before the panes. The
    // commands have the sheet less its edges and the layout button at its right end.
    let (height, edge) = match mode {
        Mode::Classic => (CONTENT_H + LABEL_H + 10.0, 4.0),
        Mode::Simplified => (BAND_H, (BAND_H - BUTTON_H) / 2.0),
    };
    let margin = egui::Margin { left: 8 + left_inset, right: (16.0 + MODE_W) as i8, top: (edge + TOP_GAP) as i8, bottom: (edge + SHEET_GAP) as i8 };
    egui::Panel::top(id)
        .exact_size(height + TOP_GAP + SHEET_GAP)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(t.tab_strip).inner_margin(margin))
        .show(ui, |ui| {
            let area = ui.max_rect();
            let sheet = Rect::from_min_max(pos2(area.min.x - 8.0, area.min.y - edge), pos2(area.max.x + 8.0 + MODE_W, area.max.y + edge));
            // In the dark the sheet stands out by itself; on a light window it needs its edge.
            let line = t.ribbon_edge;
            ui.painter().rect(sheet, 8.0, t.ribbon, line, egui::StrokeKind::Inside);
            app.hit.add("ribbonBand", 0, sheet);
            match mode {
                Mode::Classic => {
                    egui::ScrollArea::horizontal().scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                            classic(app, ui, &t, tab, area.width());
                        });
                    });
                }
                Mode::Simplified => {
                    simplified(app, ui, &t, tab, area);

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
mod fit_tests;

#[cfg(test)]
mod plain_tests;

#[cfg(test)]
mod tests;
