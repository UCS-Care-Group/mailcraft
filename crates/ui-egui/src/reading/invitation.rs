// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A meeting that arrived as mail, above the message that carries it: an invitation with the
//! ways to answer it, an update, a cancellation with a way to take the meeting out of the
//! calendar, or somebody's answer to a meeting.
//!
//! What the bar says comes from `calendar.invitation` (the engine reads the message's calendar
//! part and looks the meeting up in the calendar); what it does, it does by running a command
//! through [`MailApp::run`]: `calendar.respond` for the answers, `calendar.removeCancelled`,
//! `calendar.showInvitation`. Nothing here writes to anybody: an answer is sent to the organizer
//! only when the owner picks "Send the Response Now", in the question a plain click on an answer
//! asks ([`question`]) or in the menu beside it.
//!
//! For an account whose server keeps its calendar (Microsoft 365), the invitation in the mail
//! and the item the server has put on the calendar already are one meeting, and answering the
//! message answers that item. Until the item has come down there is nothing to answer through:
//! the bar says so instead of offering buttons that could only fail, and asks the server for the
//! days round the meeting, a few times by itself and whenever the owner says.
//!
//! Everything in the message is data. It is cut to size by the engine, shown as text, and never
//! followed.

use std::sync::{Arc, Mutex};

use egui::{Align2, Color32, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use mailcraft_model::{Id, Message, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Place;
use crate::dialogs::Question;
use crate::theme::{Tokens, paint_clipped, regular, semibold};
use crate::{MailApp, icons};

/// How many messages' meetings are kept from one frame to the next.
const KEPT: usize = 24;
/// How much of a calendar part is looked at to tell what kind of meeting mail a message is.
const GLANCE: usize = 16 * 1024;
/// How often the calendar is asked for on behalf of one message without the owner asking, and
/// how long between two askings (milliseconds).
const TRIES: u32 = 4;
const BETWEEN_MS: f64 = 15_000.0;
/// How far back and ahead of today a meeting's day may be for a server to be asked about it: a
/// message can name any day at all, and a server is not bothered with the year 9999.
const FIND_BEHIND: i64 = 31;
const FIND_AHEAD: i64 = 5 * 366;
/// The narrowest bar that has room for a look at the day beside what it says, and the narrowest
/// in which that look is a little wider.
const WIDE: f32 = 600.0;
const ROOMY: f32 = 820.0;
/// In a pane too short for all of the bar: what is left to the body under it, and the least the
/// bar is given all the same (what it says first, and its buttons).
const BODY_STRIP: f32 = 110.0;
const BAR_LEAST: f32 = 56.0;
/// How wide the look at the day is, and how many hours of it are shown at most.
const STRIP_W: f32 = 200.0;
const STRIP_HOURS: i64 = 5;
/// How much of a list of people is laid out: more than two lines never show.
const PEOPLE_CHARS: usize = 600;

/// What somebody answered: `none`, `accepted`, `tentative`, `declined`.
type Said = mailcraft_model::Response;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Person {
    name: String,
    email: String,
    response: Said,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct People {
    organizer: Option<Person>,
    required: Vec<Person>,
    optional: Vec<Person>,
    required_count: usize,
    optional_count: usize,
}

/// Something that is on the calendar: an occurrence of an event.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Slot {
    title: String,
    start: String,
    end: String,
    all_day: bool,
    show_as: ShowAs,
    /// It is the meeting itself.
    #[serde(rename = "self")]
    itself: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Answer {
    response: Said,
    at: Option<i64>,
}

/// What `calendar.invitation` says of a message.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct Info {
    account: Id,
    /// Which attachment of the message the meeting was read from.
    part: usize,
    kind: String,
    update: bool,
    location: String,
    start: String,
    end: String,
    all_day: bool,
    /// Its times by the clock of the zone it was made in, when that shows another time.
    own_time: Option<String>,
    /// What was unclear about the time zone the message names.
    notes: Vec<String>,
    people: People,
    repeats: Option<String>,
    occurrence: Option<String>,
    answered: Option<Answer>,
    answerable: bool,
    blocked: Option<String>,
    can_send: bool,
    removable: bool,
    from_organizer: bool,
    reply: Option<Person>,
    conflicts: Vec<Slot>,
    adjacent: Vec<Slot>,
    day: Vec<Slot>,
    /// Another time may be proposed with an answer: the organizer takes proposals, and the
    /// answer can be sent.
    can_propose: bool,
    /// For a message that proposes another time: the time it proposes.
    proposed: Option<Proposed>,
    /// How many proposals of another time are known for the meeting, this message's among them.
    proposals: usize,
    /// The meeting on the calendar that the message is about, when it is there.
    event: Option<Id>,
    /// The owner organizes the meeting.
    mine: bool,
}

/// The time a message proposes, as the engine writes it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Proposed {
    start: String,
    end: String,
}

impl Info {
    /// Whether it is one of the kinds of meeting mail the bar is for.
    fn shown(&self) -> bool {
        matches!(self.kind.as_str(), "request" | "cancellation" | "reply" | "counter")
    }

    /// Whether the bar says what the owner answered.
    pub(super) fn says_answer(&self) -> bool {
        self.kind == "request" && self.answered.is_some()
    }

    /// When the meeting is, when the message says so in a way that can be read.
    fn when(&self) -> Option<When> {
        if self.all_day {
            let (first, last) = (Date::parse_iso(&self.start)?, Date::parse_iso(&self.end)?);
            let last = last.max(first);
            return Some(When { start: DateTime::at_midnight(first), end: DateTime::at_midnight(last.add_days(1)), all_day: true });
        }
        let (start, end) = (time_of(&self.start)?, time_of(&self.end)?);
        Some(When { start, end: end.max(start), all_day: false })
    }

    /// The other time the message proposes, when it proposes one that can be read.
    fn proposed_when(&self) -> Option<When> {
        let proposed = self.proposed.as_ref()?;
        let (start, end) = (time_of(&proposed.start)?, time_of(&proposed.end)?);
        (end > start).then_some(When { start, end, all_day: false })
    }
}

/// A wall-clock time as the engine writes it (`2026-10-15T10:00:00`).
fn time_of(text: &str) -> Option<DateTime> {
    mailcraft_time::parse_iso(text).map(|(at, _)| at)
}

/// When a meeting is: from `start` up to `end` (for whole days, the midnight after the last).
#[derive(Clone, Copy, Debug, PartialEq)]
struct When {
    start: DateTime,
    end: DateTime,
    all_day: bool,
}

impl When {
    /// `Thursday, October 15, 2026 10:00 AM – 11:00 AM`.
    fn text(&self, twelve_hour: bool) -> String {
        use crate::dates::{clock, long};
        let (first, last) = (self.start.date, self.end.date);
        if self.all_day {
            let last = self.end.add_minutes(-1).date.max(first);
            let days = if last == first { long(first) } else { format!("{} – {}", long(first), long(last)) };
            return crate::i18n::fmt(tl!("{days} (all day)"), &[("days", &days)]);
        }
        let (from, to) = (clock(self.start, twelve_hour), clock(self.end, twelve_hour));
        if first == last { format!("{} {from} – {to}", long(first)) } else { format!("{} {from} – {} {to}", long(first), long(last)) }
    }
}

/// How long the engine may take to read a meeting before the answer is kept until the mailbox
/// changes.
const SLOW: std::time::Duration = std::time::Duration::from_millis(100);

/// What is kept between frames.
#[derive(Default)]
struct Store {
    /// What the engine said of the messages shown lately: the message, the state of the mailbox
    /// and the minute it was said in, and the frame it was last asked for.
    infos: Vec<Kept>,
    /// The calendar was asked for on behalf of a message: how often, and when last (milliseconds).
    asked: Vec<(Id, u32, f64)>,
}

struct Kept {
    message: Id,
    rev: u64,
    minute: i64,
    /// It took long to work out: it is kept until the mailbox changes, not asked again by the
    /// minute (a message built to be slow must not keep the window busy).
    slow: bool,
    info: Option<Arc<Info>>,
    used: u64,
}

impl Store {
    fn of(ctx: &egui::Context) -> Arc<Mutex<Store>> {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Arc<Mutex<Store>>>(egui::Id::new("mailcraft-reading-invitations")).clone())
    }
}

/// Whether an attachment is a calendar part: what the engine looks for a meeting in. (Asked of
/// every row of the message list in every frame: nothing is copied to tell.)
fn is_calendar(part: &mailcraft_model::Attachment) -> bool {
    let kind = part.mime.trim().as_bytes();
    let starts = |with: &[u8]| kind.get(..with.len()).is_some_and(|head| head.eq_ignore_ascii_case(with));
    let name = part.name.as_bytes();
    let ends = name.len().checked_sub(4).and_then(|at| name.get(at..)).is_some_and(|tail| tail.eq_ignore_ascii_case(b".ics"));
    starts(b"text/calendar") || starts(b"application/ics") || ends
}

/// What the engine says of the meeting a message carries, asked now. `None` for a message that
/// carries none.
fn engine_says(app: &mut MailApp, message: Id) -> Option<Info> {
    let said = app.session.run("calendar.invitation", &json!({"message": message})).ok()?;
    serde_json::from_value::<Info>(said).ok()
}

/// The meeting a message carries, as the engine reads it: asked once for each state of the
/// mailbox and kept until that changes. `None` for a message that carries none.
pub(super) fn info(app: &mut MailApp, ctx: &egui::Context, m: &Message) -> Option<Arc<Info>> {
    if !m.attachments.iter().any(is_calendar) {
        return None;
    }
    let (rev, minute, frame) = (app.session.rev(), app.session.now().div_euclid(60), super::images::frame(ctx));
    let store = Store::of(ctx);
    {
        let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(kept) = store.infos.iter_mut().find(|kept| kept.message == m.id && kept.rev == rev && (kept.slow || kept.minute == minute)) {
            kept.used = frame;
            return kept.info.clone();
        }
    }
    // (Asked with the store let go of: the command is the engine's, and may take its time.)
    let began = std::time::Instant::now();
    let info = engine_says(app, m.id).map(Arc::new);
    let slow = began.elapsed() > SLOW;
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    store.infos.retain(|kept| kept.message != m.id);
    if store.infos.len() >= KEPT
        && let Some(oldest) = store.infos.iter().enumerate().min_by_key(|(_, kept)| kept.used).map(|(index, _)| index)
    {
        store.infos.swap_remove(oldest);
    }
    store.infos.push(Kept { message: m.id, rev, minute, slow, info: info.clone(), used: frame });
    info
}

/// What a calendar part says it is, from its first lines: the icon of a request, a cancellation
/// or an answer. A glance, not a reading: the engine reads the part when the message is opened.
fn glance(data: &[u8]) -> Option<&'static str> {
    /// What follows `name` on a line that starts with it, in letters of either case.
    fn value_of<'a>(line: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
        let line = line.trim_ascii();
        line.get(..name.len()).filter(|start| start.eq_ignore_ascii_case(name)).and_then(|_| line.get(name.len()..)).map(<[u8]>::trim_ascii)
    }
    let head = data.get(..data.len().min(GLANCE))?;
    let method = head.split(|byte| *byte == b'\n').take(400).find_map(|line| value_of(line, b"METHOD:"))?;
    let is = |word: &[u8]| method.eq_ignore_ascii_case(word);
    if is(b"REQUEST") || is(b"COUNTER") {
        return Some("meetingRequest");
    }
    if is(b"CANCEL") {
        return Some("meetingCancelled");
    }
    if !is(b"REPLY") {
        return None;
    }
    // What the one who answers said.
    const SAID: &[u8] = b"PARTSTAT=";
    let said = head.windows(SAID.len()).position(|window| window.eq_ignore_ascii_case(SAID)).and_then(|at| head.get(at + SAID.len()..));
    let said = said.map(|rest| rest.split(|byte| !byte.is_ascii_alphabetic() && *byte != b'-').next().unwrap_or(&[]));
    let answer = |word: &[u8]| said.is_some_and(|said| said.eq_ignore_ascii_case(word));
    Some(if answer(b"ACCEPTED") {
        "meetingAccepted"
    } else if answer(b"DECLINED") {
        "meetingDeclined"
    } else if answer(b"TENTATIVE") {
        "meetingTentative"
    } else {
        "meetingRequest"
    })
}

/// What the message list marks a message with: the icon of the meeting mail it is (a request, a
/// cancellation, an answer), and whether it has files besides the calendar part that makes it so.
pub(crate) fn list_marks(m: &Message) -> (Option<&'static str>, bool) {
    let Some((index, part)) = m.attachments.iter().enumerate().find(|(_, part)| is_calendar(part)) else { return (None, m.has_attachments()) };
    match glance(&part.data) {
        Some(mark) => (Some(mark), m.listed_attachments().any(|(other, _)| other != index)),
        None => (None, m.has_attachments()),
    }
}

/// Whether an attachment of a message is the calendar part its bar stands for: that one is not
/// listed with the files.
pub(super) fn stands_for(info: Option<&Info>, index: usize) -> bool {
    info.is_some_and(|info| info.shown() && info.part == index)
}

// ---- asking the calendar for a meeting that hasn't come down ----

/// A meeting the account's server should have isn't on the calendar here: ask for its days,
/// when that hasn't been done too often already (`asked_for`: the owner asks, whenever).
fn look_for(app: &mut MailApp, ctx: &egui::Context, message: Id, info: &Info, asked_for: bool) -> Result<(), String> {
    let Some(when) = info.when() else { return Err(tl!("This invitation doesn't say when the meeting is.").to_string()) };
    if !(-FIND_BEHIND..=FIND_AHEAD).contains(&app.session.today().days_until(when.start.date)) {
        return Err(tl!("The day of this meeting is not one the account's calendar can be asked about.").to_string());
    }
    let now = crate::now_ms();
    let store = Store::of(ctx);
    if !asked_for {
        if !app.auto_sync {
            return Ok(());
        }
        let store = store.lock().unwrap_or_else(|e| e.into_inner());
        let (tries, last) = store.asked.iter().find(|(id, _, _)| *id == message).map_or((0, 0.0), |(_, tries, last)| (*tries, *last));
        if tries >= TRIES {
            return Ok(());
        }
        // Looked at again when the next asking is due.
        let wait = if tries == 0 { 0.0 } else { BETWEEN_MS - (now - last) };
        if wait > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(wait.clamp(100.0, BETWEEN_MS) as u64));
            return Ok(());
        }
    }
    // (A calendar job of its own, which only asks: [`MailApp::calendar_job_around`].)
    if let Err(why) = app.calendar_job_around(info.account, when.start.date) {
        // Busy with something else: in a moment, then.
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        return Err(why);
    }
    let mut store = store.lock().unwrap_or_else(|e| e.into_inner());
    match store.asked.iter_mut().find(|(id, _, _)| *id == message) {
        Some((_, tries, last)) => (*tries, *last) = (tries.saturating_add(1), now),
        None => {
            if store.asked.len() >= KEPT {
                store.asked.remove(0);
            }
            store.asked.push((message, 1, now));
        }
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    Ok(())
}

/// The window command of the bar (`UI_COMMANDS` lists it): look for the meeting of a message on
/// the account's calendar now.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id != "ui.findInvitation" {
        return None;
    }
    let message = match p.get("message") {
        None | Some(Value::Null) => app.session.sel.focus(),
        Some(id) => id.as_u64(),
    };
    let Some(message) = message else { return Some(Err(tl!("Select a message first").to_string())) };
    let Some(info) = engine_says(app, message) else { return Some(Err(tl!("This message carries no invitation.").to_string())) };
    if info.blocked.as_deref() != Some("calendar") {
        return Some(Ok(json!({"message": message, "asked": false})));
    }
    let Some(ctx) = app.ctx.clone() else { return Some(Err(tl!("The window isn't up yet.").to_string())) };
    Some(look_for(app, &ctx, message, &info, true).map(|()| json!({"message": message, "asked": true})))
}

// ---- the question a plain click on an answer asks ----

/// What a user-run `calendar.respond` for a message is asked first: whether the organizer is to
/// be told, with a note to go along. `None` for any other command (the calendar's own questions
/// are the dialogs'); `Some(None)` when there is nothing to ask: the command says what is to be
/// sent already, or nothing can be.
pub(crate) fn question(app: &mut MailApp, id: &str, params: &Value) -> Option<Option<Question>> {
    if id != "calendar.respond" {
        return None;
    }
    let message = params.get("message").filter(|message| !message.is_null())?;
    let said = |key: &str| params.get(key).is_some_and(|value| !value.is_null());
    if said("send") {
        return Some(None);
    }
    let Some(info) = message.as_u64().and_then(|message| engine_says(app, message)) else { return Some(None) };
    // What can't be answered is refused by the command, in words; what can't be sent isn't asked about.
    if !info.answerable || !info.can_send {
        return Some(None);
    }
    let organizer = info.people.organizer.as_ref().map(|who| who.name.trim().chars().take(80).collect::<String>()).filter(|who| !who.is_empty());
    let who = organizer.unwrap_or_else(|| tl!("Somebody else").to_string());
    let lead = crate::i18n::fmt(tl!("{name} organizes this meeting; you are invited to it."), &[("name", &who)]);
    let text = format!("{lead} {}", tl!("Send your answer to the organizer? You can write a note to go with it."));
    Some(Some(Question {
        title: tl!("Send a Response?").to_string(),
        text,
        action: tl!("Send the Response Now").to_string(),
        with: json!({"send": true}),
        more: vec![(tl!("Don't Send a Response").to_string(), json!({"send": false}))],
        note: Some(("comment", String::new(), tl!("A note to go with it (optional)").to_string())),
    }))
}

// ---- the bar ----

/// What in the bar can be pointed at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Answer(Said),
    Menu(Said),
    Calendar,
    Remove,
    Find,
    /// Propose New Time, on an invitation.
    Propose,
    /// Accept Proposal and View All Proposals, on a message that proposes another time.
    AcceptProposal,
    Proposals,
}

/// The name `ui.inspect` and `ui.click` know a part of the bar by, in the reading pane and in a
/// message's own window (told apart as the window's toolbar is).
fn hit_kind(place: Place, part: Part) -> &'static str {
    let pane = place == Place::Pane;
    match part {
        Part::Answer(Said::Accepted) if pane => "invitationAccept",
        Part::Answer(Said::Accepted) => "windowInvitationAccept",
        Part::Answer(Said::Tentative) if pane => "invitationTentative",
        Part::Answer(Said::Tentative) => "windowInvitationTentative",
        Part::Answer(_) if pane => "invitationDecline",
        Part::Answer(_) => "windowInvitationDecline",
        Part::Menu(Said::Accepted) if pane => "invitationAcceptMenu",
        Part::Menu(Said::Accepted) => "windowInvitationAcceptMenu",
        Part::Menu(Said::Tentative) if pane => "invitationTentativeMenu",
        Part::Menu(Said::Tentative) => "windowInvitationTentativeMenu",
        Part::Menu(_) if pane => "invitationDeclineMenu",
        Part::Menu(_) => "windowInvitationDeclineMenu",
        Part::Calendar if pane => "invitationCalendar",
        Part::Calendar => "windowInvitationCalendar",
        Part::Remove if pane => "invitationRemove",
        Part::Remove => "windowInvitationRemove",
        Part::Find if pane => "invitationFind",
        Part::Find => "windowInvitationFind",
        Part::Propose if pane => "invitationPropose",
        Part::Propose => "windowInvitationPropose",
        Part::AcceptProposal if pane => "invitationAcceptProposal",
        Part::AcceptProposal => "windowInvitationAcceptProposal",
        Part::Proposals if pane => "invitationProposals",
        Part::Proposals => "windowInvitationProposals",
    }
}

fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.ctx().fonts_mut(|f| f.layout_no_wrap(text.to_string(), regular(size), Color32::WHITE).size().x)
}

/// A button of the bar: an icon and a label in a frame, and beside it, when `menu`, a smaller
/// one that opens its menu. Checked when it is the answer that was given.
fn button(ui: &mut Ui, t: &Tokens, icon: &str, label: &str, checked: bool, menu: bool) -> (Response, Option<Response>) {
    let main_w = 8.0 + 16.0 + 5.0 + text_width(ui, label, 12.0) + 9.0;
    let (rect, _) = ui.allocate_exact_size(vec2(main_w + if menu { 18.0 } else { 0.0 }, 26.0), Sense::hover());
    let main_rect = Rect::from_min_max(rect.min, pos2(rect.min.x + main_w, rect.max.y));
    let id = ui.id().with(("invitation", label));
    let main = ui.interact(main_rect, id.with("main"), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let more = menu.then(|| {
        let more = ui.interact(Rect::from_min_max(pos2(main_rect.max.x, rect.min.y), rect.max), id.with("menu"), Sense::click());
        more.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, crate::i18n::fmt(tl!("{answer}: more ways to answer"), &[("answer", label)]))
        });
        more
    });
    let p = ui.painter();
    let (fill, line) = if checked { (t.checked, t.accent) } else { (t.surface, t.border_strong) };
    p.rect(rect, 4.0, fill, Stroke::new(1.0, line), egui::StrokeKind::Inside);
    let lit = |resp: &Response| {
        if resp.is_pointer_button_down_on() {
            Some(t.pressed)
        } else if resp.hovered() {
            Some(t.hover)
        } else {
            None
        }
    };
    if let Some(fill) = lit(&main) {
        p.rect_filled(main_rect.shrink(1.0), 3.0, fill);
    }
    icons::paint(p, Rect::from_center_size(pos2(rect.min.x + 16.0, rect.center().y), vec2(16.0, 16.0)), icon, t.icon, t.accent);
    p.text(pos2(rect.min.x + 29.0, rect.center().y), Align2::LEFT_CENTER, label, regular(12.0), t.text);
    if let Some(more) = &more {
        if let Some(fill) = lit(more) {
            p.rect_filled(more.rect.shrink(1.0), 3.0, fill);
        }
        p.line_segment([pos2(more.rect.min.x, rect.min.y + 5.0), pos2(more.rect.min.x, rect.max.y - 5.0)], Stroke::new(1.0, line));
        icons::paint(p, Rect::from_center_size(more.rect.center(), vec2(8.0, 8.0)), "dropdown", t.icon, t.accent);
    }
    (main, more)
}

/// One line of the bar that says something: an icon and words.
fn says(ui: &mut Ui, t: &Tokens, icon: &str, text: &str, strong: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
        let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
        icons::paint(ui.painter(), r.shrink(1.0), icon, t.text_dim, t.accent);
        let words =
            egui::RichText::new(text).font(if strong { semibold(12.0) } else { regular(12.0) }).color(if strong { t.text } else { t.text_dim });
        ui.add(egui::Label::new(words).wrap());
    });
}

/// A row of what the meeting is: a name, and text of at most `rows` lines beside it (what is cut
/// short shows whole when the pointer rests on it).
fn row(ui: &mut Ui, t: &Tokens, name: &str, text: &str, rows: usize, width: f32) {
    const NAME_W: f32 = 78.0;
    let shown: String = text.chars().take(PEOPLE_CHARS).collect();
    let mut job =
        egui::text::LayoutJob::single_section(shown, egui::text::TextFormat { font_id: regular(12.0), color: t.text, ..Default::default() });
    job.wrap = egui::text::TextWrapping {
        max_width: (width - NAME_W).max(40.0),
        max_rows: rows.max(1),
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let (r, resp) = ui.allocate_exact_size(vec2(width, galley.size().y.max(16.0)), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{name} {text}")));
    ui.painter().text(r.min, Align2::LEFT_TOP, name, regular(12.0), t.text_dim);
    let cut = galley.elided || text.chars().count() > PEOPLE_CHARS;
    ui.painter().galley(pos2(r.min.x + NAME_W, r.min.y), galley, t.text);
    if cut {
        resp.on_hover_text(text.chars().take(4_000).collect::<String>());
    }
}

/// The names of some of the people invited, and how many more there are.
fn names(people: &[Person], count: usize) -> String {
    let mut out = String::new();
    for who in people {
        if out.chars().count() > PEOPLE_CHARS * 6 {
            break;
        }
        if !out.is_empty() {
            out.push_str("; ");
        }
        out.push_str(if who.name.trim().is_empty() { who.email.trim() } else { who.name.trim() });
    }
    match count.saturating_sub(people.len()) {
        0 => out,
        more => crate::i18n::fmt(tl!("{names} and {n} more"), &[("names", &out), ("n", &more.to_string())]),
    }
}

/// `Dentist (10:30 AM – 11:00 AM)`, for the first few of what is on the calendar.
fn listed(slots: &[&Slot], twelve_hour: bool) -> String {
    let one = |slot: &&Slot| {
        let title: String = slot.title.chars().filter(|c| !c.is_control()).take(60).collect();
        let title = if title.trim().is_empty() { tl!("(no title)").to_string() } else { title };
        match (slot.all_day, time_of(&slot.start), time_of(&slot.end)) {
            (false, Some(start), Some(end)) => {
                format!("{title} ({} – {})", crate::dates::clock(start, twelve_hour), crate::dates::clock(end, twelve_hour))
            }
            _ => crate::i18n::fmt(tl!("{title} (all day)"), &[("title", &title)]),
        }
    };
    let shown: Vec<String> = slots.iter().take(3).map(one).collect();
    match slots.len().saturating_sub(shown.len()) {
        0 => shown.join("; "),
        more => crate::i18n::fmt(tl!("{names} and {n} more"), &[("names", &shown.join("; ")), ("n", &more.to_string())]),
    }
}

/// What the bar says first: what the message is, what was answered, why it can't be.
fn headlines(app: &MailApp, info: &Info) -> Vec<(&'static str, String, bool)> {
    let mut lines: Vec<(&'static str, String, bool)> = Vec::new();
    let who = |person: &Option<Person>| {
        let name = person.as_ref().map(|who| if who.name.trim().is_empty() { who.email.trim() } else { who.name.trim() }).unwrap_or("");
        let name: String = name.chars().filter(|c| !c.is_control()).take(80).collect();
        if name.is_empty() { tl!("Somebody").to_string() } else { name }
    };
    match info.kind.as_str() {
        "request" => {
            if let Some(answer) = &info.answered {
                let at = answer.at.map(|at| crate::dates::header_label(at, app.session.offset_at(at), app.session.prefs.twelve_hour));
                let (icon, on, plain) = match answer.response {
                    Said::Accepted => ("accept", tl!("Accepted on {when}"), tl!("You accepted this meeting.")),
                    Said::Tentative => ("tentative", tl!("Tentatively accepted on {when}"), tl!("You tentatively accepted this meeting.")),
                    _ => ("decline", tl!("Declined on {when}"), tl!("You declined this meeting.")),
                };
                lines.push((icon, at.map_or(plain.to_string(), |at| crate::i18n::fmt(on, &[("when", &at)])), true));
            }
            match info.blocked.as_deref() {
                Some("organizer") => lines.push(("meeting", tl!("You organize this meeting: there is nothing to answer.").to_string(), false)),
                Some("past") => lines.push(("clock", tl!("This meeting is over.").to_string(), false)),
                Some("readOnly") => lines.push(("private", tl!("This meeting is in a calendar that can't be changed from here.").to_string(), false)),
                Some("incomplete") => {
                    lines.push(("info", tl!("This invitation doesn't say which meeting it is about, so it can't be answered.").to_string(), false))
                }
                // (What is said while the meeting hasn't come down is a line of its own, with a way to ask.)
                Some(_) => {}
                None if info.update => lines.push(("sync", tl!("The organizer has changed this meeting.").to_string(), info.answered.is_none())),
                None if info.answered.is_none() => lines.push(("meetingRequest", tl!("You haven't answered this invitation yet.").to_string(), true)),
                None => {}
            }
            if info.occurrence.is_some() {
                lines.push(("recurrence", tl!("This is about one occurrence of a series.").to_string(), false));
            }
        }
        "cancellation" => {
            lines.push(("meetingCancelled", tl!("This meeting was called off by its organizer.").to_string(), true));
            if !info.from_organizer {
                lines.push(("info", tl!("This message didn't come from the address of the meeting's organizer.").to_string(), false));
            }
            if !info.removable {
                lines.push(("calendar", tl!("It is not on your calendar.").to_string(), false));
            }
        }
        "reply" => {
            let name = who(&info.reply);
            let (icon, words) = match info.reply.as_ref().map_or(Said::None, |reply| reply.response) {
                Said::Accepted => ("meetingAccepted", tl!("{name} accepted this meeting.")),
                Said::Tentative => ("meetingTentative", tl!("{name} tentatively accepted this meeting.")),
                Said::Declined => ("meetingDeclined", tl!("{name} declined this meeting.")),
                Said::None => ("meetingRequest", tl!("{name} answered this invitation.")),
            };
            lines.push((icon, crate::i18n::fmt(words, &[("name", &name)]), true));
        }
        _ => {
            let words = crate::i18n::fmt(tl!("{name} proposes another time for this meeting."), &[("name", &who(&info.reply))]);
            lines.push(("meetingTentative", words, true));
        }
    }
    lines
}

/// Run a command of the bar. What it starts in the main window (a question, the calendar) is
/// looked at there, when the bar is in a window of the system.
fn run(app: &mut MailApp, ui: &Ui, command: &str, params: Value) {
    let done = app.run(command, params);
    if done.is_ok() && ui.ctx().viewport_id() != egui::ViewportId::ROOT {
        ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
}

/// The ways to answer: Accept, Tentative, Decline, each with its menu.
fn answers(app: &mut MailApp, ui: &mut Ui, t: &Tokens, message: Id, info: &Info, place: Place, native: bool) {
    let given = info.answered.as_ref().map(|answer| answer.response);
    for (said, icon, label, code) in [
        (Said::Accepted, "accept", "Accept", "accept"),
        (Said::Tentative, "tentative", "Tentative", "tentative"),
        (Said::Declined, "decline", "Decline", "decline"),
    ] {
        let (main, more) = button(ui, t, icon, tl!(label), given == Some(said), true);
        let tip = if info.can_send {
            tl!("Answer the invitation. You are asked whether to tell the organizer.")
        } else {
            tl!("Answer the invitation in your calendar. The organizer can't be written to from this account.")
        };
        let main = main.on_hover_text(tip);
        if !native {
            app.hit.add(hit_kind(place, Part::Answer(said)), message, main.rect);
        }
        if main.clicked() {
            run(app, ui, "calendar.respond", json!({"message": message, "response": code}));
        }
        let Some(more) = more else { continue };
        if !native {
            app.hit.add(hit_kind(place, Part::Menu(said)), message, more.rect);
        }
        let mut chosen: Option<Value> = None;
        egui::Popup::menu(&more).show(|ui| {
            ui.set_min_width(230.0);
            let mut item = |ui: &mut Ui, label: &str, enabled: bool, with: Value| {
                if ui.add_enabled(enabled, egui::Button::new(tl!(label)).min_size(vec2(220.0, 0.0))).clicked() {
                    chosen = Some(with);
                    ui.close();
                }
            };
            item(ui, "Write a Note with the Response…", info.can_send, json!({}));
            item(ui, "Send the Response Now", info.can_send, json!({"send": true}));
            item(ui, "Don't Send a Response", true, json!({"send": false}));
        });
        if let Some(with) = chosen {
            let mut params = json!({"message": message, "response": code});
            if let (Some(params), Some(with)) = (params.as_object_mut(), with.as_object()) {
                params.extend(with.iter().map(|(key, value)| (key.clone(), value.clone())));
            }
            run(app, ui, "calendar.respond", params);
        }
    }
    // Another time, proposed with a tentative answer or a decline: asked for in a dialog of its
    // own, which says who is written to and sends from its own buttons.
    if info.can_propose {
        let (propose, _) = button(ui, t, "proposeTime", tl!("Propose New Time"), false, false);
        let propose = propose.on_hover_text(tl!(
            "Propose another time to the organizer, with a tentative answer or a decline. Nothing is sent until you say so there."
        ));
        if !native {
            app.hit.add(hit_kind(place, Part::Propose), message, propose.rect);
        }
        if propose.clicked() {
            run(app, ui, "ui.dialog", json!({"name": "proposeTime", "message": message}));
        }
    }
}

/// What can be done about a message that proposes another time for a meeting the owner
/// organizes: take the proposal up (the meeting opens with that time in its fields, to be sent
/// as an update or not), and look at all the proposals there are when there are several.
fn proposal_buttons(app: &mut MailApp, ui: &mut Ui, t: &Tokens, message: Id, info: &Info, place: Place, native: bool) {
    let (Some(event), Some(proposed), true) = (info.event, info.proposed_when(), info.mine) else { return };
    let (accept, _) = button(ui, t, "accept", tl!("Accept Proposal"), false, false);
    let accept = accept.on_hover_text(tl!("Open the meeting with the proposed time filled in. Nothing is sent until you send the update there."));
    if !native {
        app.hit.add(hit_kind(place, Part::AcceptProposal), message, accept.rect);
    }
    if accept.clicked() {
        crate::meeting_dialogs::accept_proposal(app, event, info.occurrence.as_deref(), proposed.start, proposed.end);
        if native {
            ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }
    if info.proposals > 1 {
        let (all, _) = button(ui, t, "proposeTime", tl!("View All Proposals"), false, false);
        let all = all.on_hover_text(tl!("Every other time that was proposed for this meeting."));
        if !native {
            app.hit.add(hit_kind(place, Part::Proposals), message, all.rect);
        }
        if all.clicked() {
            run(app, ui, "ui.dialog", json!({"name": "proposals", "message": message}));
        }
    }
}

/// A look at the stretch of the day the meeting falls in: the hours, what is on, and the
/// meeting itself.
fn day_strip(ui: &Ui, rect: Rect, info: &Info, when: &When, t: &Tokens, twelve_hour: bool) {
    let p = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    p.rect(rect, 4.0, t.surface, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    // The day, and under it the hours from the one before the meeting.
    let head = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + 20.0));
    let day = crate::dates::weekday_month_day_long(when.start.date);
    paint_clipped(&p, pos2(head.min.x + 8.0, head.center().y), head.width() - 16.0, &day, semibold(11.0), t.text_dim);
    p.line_segment([pos2(rect.min.x, head.max.y), pos2(rect.max.x, head.max.y)], Stroke::new(1.0, t.border));
    let grid = Rect::from_min_max(pos2(rect.min.x, head.max.y), rect.max);
    let minutes = |at: DateTime| when.start.date.days_until(at.date).clamp(-1, 2) * 24 * 60 + i64::from(at.minutes_of_day());
    let (starts, ends) = (minutes(when.start), minutes(when.end).max(minutes(when.start)));
    let first = (starts.div_euclid(60) - 1).clamp(0, 24 - STRIP_HOURS);
    let last = ((ends + 59).div_euclid(60) + 1).clamp(first + 3, first + STRIP_HOURS).min(24);
    let per_minute = grid.height() / ((last - first) * 60) as f32;
    let y_of = |minute: i64| grid.min.y + (minute - first * 60).clamp(0, (last - first) * 60) as f32 * per_minute;
    const HOURS_W: f32 = 44.0;
    for hour in first..last {
        let y = y_of(hour * 60);
        if hour > first {
            p.line_segment([pos2(grid.min.x, y), pos2(grid.max.x, y)], Stroke::new(1.0, t.border));
        }
        let label = match (twelve_hour, hour) {
            (false, hour) => format!("{hour:02}:00"),
            (true, 0) => "12 AM".to_string(),
            (true, 12) => "12 PM".to_string(),
            (true, hour) => format!("{} {}", hour % 12, if hour < 12 { "AM" } else { "PM" }),
        };
        p.text(pos2(grid.min.x + HOURS_W - 6.0, y + 2.0), Align2::RIGHT_TOP, label, regular(10.0), t.text_dim);
    }
    p.line_segment([pos2(grid.min.x + HOURS_W, grid.min.y), pos2(grid.min.x + HOURS_W, grid.max.y)], Stroke::new(1.0, t.border));
    // What else is on in those hours, beside the meeting when they are at the same time.
    let lane = Rect::from_min_max(pos2(grid.min.x + HOURS_W + 2.0, grid.min.y), pos2(grid.max.x - 3.0, grid.max.y));
    let others: Vec<(i64, i64, &Slot)> = info
        .day
        .iter()
        .filter(|slot| !slot.itself && !slot.all_day)
        .filter_map(|slot| Some((minutes(time_of(&slot.start)?), minutes(time_of(&slot.end)?), slot)))
        .filter(|(from, to, _)| *to > first * 60 && *from < last * 60 && to > from)
        .take(12)
        .collect();
    let clash = others.iter().any(|(from, to, _)| *from < ends && *to > starts);
    let half = lane.center().x;
    let block = |from: i64, to: i64, left: f32, right: f32| {
        let (top, bottom) = (y_of(from), y_of(to));
        Rect::from_min_max(pos2(left, top + 1.0), pos2(right, bottom.max(top + 12.0) - 1.0))
    };
    // Side by side, those of them that are at the same time as each other (three abreast at most).
    let mut free_at: Vec<i64> = Vec::new();
    let placed: Vec<usize> = others
        .iter()
        .map(|(from, to, _)| {
            let column = free_at.iter().position(|free| *free <= *from).unwrap_or(free_at.len().min(2));
            match free_at.get_mut(column) {
                Some(free) => *free = (*free).max(*to),
                None => free_at.push(*to),
            }
            column
        })
        .collect();
    let left = if clash { half + 1.0 } else { lane.min.x };
    let column_w = (lane.max.x - left) / free_at.len().max(1) as f32;
    for ((from, to, slot), column) in others.iter().zip(&placed) {
        let x = left + *column as f32 * column_w;
        let r = block(*from, *to, x, x + column_w - 1.0);
        let overlaps = *from < ends && *to > starts && slot.show_as != ShowAs::Free;
        let fill = if slot.show_as == ShowAs::Free { t.surface } else { t.hover };
        p.rect(r, 2.0, fill, Stroke::new(1.0, t.border_strong), egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_min_max(r.min, pos2(r.min.x + 3.0, r.max.y)), 1.0, if overlaps { t.orange } else { t.border_strong });
        if r.height() >= 13.0 {
            paint_clipped(&p, pos2(r.min.x + 7.0, r.min.y + 7.0), r.width() - 10.0, &slot.title, regular(10.5), t.text);
        }
    }
    // The meeting itself.
    let r = block(starts, ends.max(starts + 1), lane.min.x, if clash { half - 1.0 } else { lane.max.x });
    p.rect(r, 2.0, t.accent.linear_multiply(if t.dark { 0.35 } else { 0.16 }), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    if r.height() >= 13.0 {
        paint_clipped(&p, pos2(r.min.x + 6.0, r.min.y + 7.0), r.width() - 9.0, tl!("This meeting"), semibold(10.5), t.text);
    }
}

/// The bar above the body of a message that carries a meeting. Nothing is laid out for any
/// other message.
pub(super) fn bar(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, place: Place) {
    let ctx = ui.ctx().clone();
    let Some(info) = info(app, &ctx, m).filter(|info| info.shown()) else { return };
    let t = Tokens::get(&ctx);
    let twelve_hour = app.session.prefs.twelve_hour;
    // A window of the system has its own coordinates: nothing in it is pointed at from the main one.
    let native = ctx.viewport_id() != egui::ViewportId::ROOT;
    let when = info.when();
    // The account's server keeps the calendar, and the meeting isn't on it here: there is nothing
    // to answer through. One that was declined is gone for good; any other is on its way, and is
    // asked for (when its day is one a server can be asked about).
    let waiting = info.blocked.as_deref() == Some("calendar");
    let declined = info.answered.as_ref().is_some_and(|answer| answer.response == Said::Declined);
    let today = app.session.today();
    let findable = when.is_some_and(|when| (-FIND_BEHIND..=FIND_AHEAD).contains(&today.days_until(when.start.date)));
    if waiting && findable && !declined {
        let _ = look_for(app, &ctx, m.id, &info, false);
    }
    let top = ui.available_rect_before_wrap();
    // In a pane with little room the body comes first: what the meeting is, in a line or two.
    let (compact, tight) = (top.height() < 380.0, top.height() < 230.0);
    // In a pane too short even for that, the bar scrolls by itself (its buttons are at its top)
    // and leaves a strip to the body.
    let most = (top.height() - BODY_STRIP).max(BAR_LEAST);
    let frame = egui::Frame::NONE.fill(t.panel).inner_margin(egui::Margin { left: 20, right: 12, top: 8, bottom: 9 });
    let scroll = egui::ScrollArea::vertical().id_salt(("invitation", m.id)).max_height(most).auto_shrink([false, true]);
    let around = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        scroll.show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 5.0);
            // (With next to no room at all, the buttons come before any words.)
            let lines = if top.height() < 64.0 && info.answerable {
                0
            } else if tight {
                1
            } else {
                usize::MAX
            };
            for (icon, text, strong) in headlines(app, &info).into_iter().take(lines) {
                says(ui, &t, icon, &text, strong);
            }
            if waiting {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    icons::paint(ui.painter(), r.shrink(1.0), "sync", t.text_dim, t.accent);
                    let looking = app.calendar_syncing();
                    let words = if looking {
                        tl!("Looking for this meeting on your calendar…")
                    } else if declined {
                        tl!("It is no longer on your calendar, so the answer can't be changed from here.")
                    } else {
                        tl!("This meeting isn't on your calendar here yet. It can be answered as soon as it shows there.")
                    };
                    ui.add(egui::Label::new(egui::RichText::new(words).font(regular(12.0)).color(t.text_dim)).wrap());
                    if !looking && findable {
                        // The link stays whole: it goes to the next line when the words before it
                        // leave no room, instead of being split over two (and being pressed between).
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                        let find = ui
                            .add(egui::Link::new(egui::RichText::new(tl!("Look Again")).font(regular(12.0))))
                            .on_hover_text(tl!("Ask the account's server for the calendar round this meeting now."));
                        if !native {
                            app.hit.add(hit_kind(place, Part::Find), m.id, find.rect);
                        }
                        if find.clicked() {
                            let _ = app.run("ui.findInvitation", json!({"message": m.id}));
                        }
                    }
                });
            }
            // What can be done.
            let request = info.kind == "request";
            if request || info.kind == "cancellation" || info.kind == "counter" {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    if request && info.answerable {
                        answers(app, ui, &t, m.id, &info, place, native);
                    }
                    if info.kind == "counter" {
                        proposal_buttons(app, ui, &t, m.id, &info, place, native);
                    }
                    if info.removable {
                        let (remove, _) = button(ui, &t, "remove", tl!("Remove from Calendar"), false, false);
                        let remove = remove.on_hover_text(tl!("Take the meeting that was called off out of your calendar. Nobody is written to."));
                        if !native {
                            app.hit.add(hit_kind(place, Part::Remove), m.id, remove.rect);
                        }
                        if remove.clicked() {
                            run(app, ui, "calendar.removeCancelled", json!({"message": m.id}));
                        }
                    }
                    if when.is_some() {
                        let (calendar, _) = button(ui, &t, "calendar", tl!("Calendar…"), false, false);
                        let calendar = calendar.on_hover_text(tl!("Show the day of this meeting in the calendar."));
                        if !native {
                            app.hit.add(hit_kind(place, Part::Calendar), m.id, calendar.rect);
                        }
                        if calendar.clicked() {
                            run(app, ui, "calendar.showInvitation", json!({"message": m.id}));
                        }
                    }
                });
            }
            // What the meeting is, and beside it a look at its day when there is room.
            let width = ui.available_width();
            let strip = when.filter(|when| !when.all_day && !compact && width >= WIDE && info.kind != "reply");
            let strip_w = if width >= ROOMY { STRIP_W + 50.0 } else { STRIP_W };
            let details_w = if strip.is_some() { width - strip_w - 16.0 } else { width };
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing = vec2(16.0, 0.0);
                let details = ui.allocate_ui_with_layout(vec2(details_w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 3.0);
                    ui.set_width(details_w);
                    details(app, ui, &t, &info, when.as_ref(), details_w, (compact, tight));
                });
                if let Some(when) = &strip {
                    let (r, _) = ui.allocate_exact_size(vec2(strip_w, details.response.rect.height().clamp(112.0, 190.0)), Sense::hover());
                    day_strip(ui, r, &info, when, &t, twelve_hour);
                }
            });
        });
    });
    let r = around.response.rect;
    for y in [r.min.y, r.max.y] {
        ui.painter().line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, t.border));
    }
}

/// The rows that say what the meeting is: when, where, who, and what the owner's calendar has
/// then. In a pane with little room (`compact`) only when and where, with less still (`tight`)
/// only when.
fn details(app: &MailApp, ui: &mut Ui, t: &Tokens, info: &Info, when: Option<&When>, width: f32, (compact, tight): (bool, bool)) {
    let twelve_hour = app.session.prefs.twelve_hour;
    // (A message that proposes another time says the proposal first, then when the meeting is.)
    if let Some(proposed) = info.proposed_when() {
        row(ui, t, tl!("Proposed"), &proposed.text(twelve_hour), if tight { 1 } else { 2 }, width);
    }
    if let Some(when) = when {
        row(ui, t, tl!("When"), &when.text(twelve_hour), if tight { 1 } else { 2 }, width);
        // When that is where the meeting was made, for one from another time zone; and what
        // was unclear about its zone (its times are then as the message wrote them).
        let clean = |text: &str| text.chars().filter(|c| !c.is_control()).take(200).collect::<String>();
        if let Some(own) = info.own_time.as_deref().filter(|own| !tight && !own.trim().is_empty()) {
            row(ui, t, "", &clean(own), 1, width);
        }
        for note in info.notes.iter().take(2).filter(|_| !tight) {
            row(ui, t, "", &clean(note), 2, width);
        }
        if let Some(repeats) = info.repeats.as_deref().filter(|repeats| !tight && !repeats.trim().is_empty()) {
            row(ui, t, "", &repeats.chars().filter(|c| !c.is_control()).take(200).collect::<String>(), 1, width);
        }
    }
    if tight {
        return;
    }
    if !info.location.trim().is_empty() {
        row(ui, t, tl!("Where"), info.location.trim(), if compact { 1 } else { 2 }, width);
    }
    // (An answer names the one who answers, which the bar has said already.)
    if compact || info.kind == "reply" || info.kind == "counter" {
        return;
    }
    if let Some(organizer) = &info.people.organizer {
        row(ui, t, tl!("Organizer"), if organizer.name.trim().is_empty() { organizer.email.trim() } else { organizer.name.trim() }, 1, width);
    }
    if info.people.required_count > 0 {
        row(ui, t, tl!("Required"), &names(&info.people.required, info.people.required_count), 2, width);
    }
    if info.people.optional_count > 0 {
        row(ui, t, tl!("Optional"), &names(&info.people.optional, info.people.optional_count), 2, width);
    }
    // The owner's own calendar at that time (not for what is over, called off or somebody's answer).
    if info.kind != "request" || info.blocked.as_deref() == Some("past") || when.is_none() {
        return;
    }
    let taking: Vec<&Slot> = info.conflicts.iter().filter(|slot| slot.show_as != ShowAs::Free).collect();
    let beside: Vec<&Slot> = info.adjacent.iter().filter(|slot| slot.show_as != ShowAs::Free).collect();
    ui.add_space(2.0);
    if taking.is_empty() && beside.is_empty() {
        row(ui, t, tl!("Calendar"), tl!("Nothing else is on your calendar at that time."), 1, width);
    }
    if !taking.is_empty() {
        let said = crate::i18n::fmt(tl!("At the same time: {items}"), &[("items", &listed(&taking, twelve_hour))]);
        row(ui, t, tl!("Calendar"), &said, 2, width);
    }
    if !beside.is_empty() {
        let name = if taking.is_empty() { tl!("Calendar") } else { "" };
        let said = crate::i18n::fmt(tl!("Right before or after: {items}"), &[("items", &listed(&beside, twelve_hour))]);
        row(ui, t, name, &said, 2, width);
    }
}

/// The row of File › Options about meeting requests that were answered.
pub(crate) fn options(app: &mut MailApp, ui: &mut Ui) {
    let mut on = app.session.prefs.delete_answered_requests;
    let tip = tl!(
        "After you accept, tentatively accept or decline a meeting request in the mail, or take a cancelled meeting out of your calendar, the message goes to Deleted Items. Left off, it stays where it is and says what you answered."
    );
    if ui.checkbox(&mut on, tl!("Delete meeting requests from the Inbox after responding")).on_hover_text(tip).changed() {
        let _ = app.run("calendar.deleteAnsweredRequests", json!({"value": on}));
    }
}
