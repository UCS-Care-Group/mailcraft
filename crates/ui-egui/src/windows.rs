// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Windows of their own: a message or an appointment opened out of the main window, as a double
//! click on it does.
//!
//! Each is a window of the operating system where there can be more than one (egui's immediate
//! viewports, so what is in it reads and changes the same state as the main window, in the same
//! frame), and a movable window inside the main one where there can't (the browser, and the
//! headless window the tests and the screenshot example use). Either way it shows what the main
//! window would show, with the same code: a message window is the reading pane's message
//! ([`crate::reading::show_message`]) under a row of actions, an appointment window is the form
//! the dialog hosts ([`crate::forms::Form`]) under a bar of commands of its own. Everything they
//! do, they do by running a command.
//!
//! A window follows what it shows: a message that is moved stays open (it is the same message),
//! one that is gone for good, or a mailbox that is replaced, closes it.
//!
//! Any number of appointments can be open at a time (up to [`MAX_WINDOWS`] windows in all),
//! each in a window of its own: the fields of a form are told apart by the window it is in
//! ([`crate::forms::kit::hosted`]). A new appointment or meeting gets such a window too, before
//! it exists: saving it makes it.
//!
//! An appointment's window behaves as Outlook's does. Its bar saves and closes (or sends, for a
//! meeting); Ctrl+S saves and stays, Ctrl+Enter and Alt+S send; and closing it (Escape, its
//! close button, Ctrl+W) asks first when something in it was changed and not saved
//! ([`crate::dialogs::unsaved_changes`]). A script that closes it (`ui.closeWindow`) is asked
//! nothing: it said what it meant.

use std::sync::Arc;

use egui::{Key, Modifiers, Ui, vec2};
use mailcraft_model::{FlagStatus, Id, Message};
use serde_json::{Value, json};

use crate::dialogs::Unsaved;
use crate::forms::kit::Host;
use crate::forms::{Ask, Form, Verdict};
use crate::reading::Place;
use crate::theme::{Tokens, semibold};
use crate::{MailApp, keys};

/// Most windows open at a time.
pub const MAX_WINDOWS: usize = 12;

/// The size a message window opens at, and the least it can be made, in points.
const MESSAGE_SIZE: [f32; 2] = [820.0, 640.0];
const MIN_SIZE: [f32; 2] = [420.0, 300.0];
/// How tall a window of the system opens that shows a form (the form fills it: its last field
/// takes what the others leave, `forms::kit::body_sized`).
const FORM_HEIGHT: f32 = 780.0;

/// What a window shows.
enum What {
    /// A message, as it was when it was last looked at: that is how a different message that
    /// came to have its number (another mailbox was opened) is told from it.
    Message { message: Id, seen: Arc<Message> },
    /// An appointment or a meeting that is being looked at, changed or made. `event` is the one
    /// it is about, with the name the calendar world knows it by; what is being made has none
    /// until it is saved.
    Event { event: Option<(Id, String)>, form: Box<Form> },
}

impl What {
    /// What it is about: a message or an appointment, and which. `None` for what doesn't exist
    /// yet, which is about nothing another window could be about too.
    fn subject(&self) -> Option<(bool, Id)> {
        match self {
            What::Message { message, .. } => Some((true, *message)),
            What::Event { event, .. } => event.as_ref().map(|(id, _)| (false, *id)),
        }
    }

    /// The form it shows, when it shows one.
    fn form(&self) -> Option<&Form> {
        match self {
            What::Message { .. } => None,
            What::Event { form, .. } => Some(form),
        }
    }

    fn form_mut(&mut self) -> Option<&mut Form> {
        match self {
            What::Message { .. } => None,
            What::Event { form, .. } => Some(form),
        }
    }

    /// Whether something in it was changed and is not saved.
    fn dirty(&self) -> bool {
        self.form().is_some_and(Form::dirty)
    }
}

/// A window that is open.
struct Open {
    id: u64,
    what: What,
    /// Bring it to the front when it is next shown.
    raise: bool,
    /// It was last shown inside the main window rather than as a window of the system.
    embedded: bool,
    title: String,
    /// It was to close with unsaved changes, and asks about them.
    asking: bool,
    /// What its form is asked to do when it is next laid out (the answer to that question).
    ask: Option<Ask>,
}

/// The windows that are open, in the order they were opened or brought forward.
#[derive(Default)]
pub struct Windows {
    open: Vec<Open>,
    /// The number the last window got; they count up from 1.
    last: u64,
    /// The main window is to close once the windows with unsaved changes have been asked about.
    quitting: bool,
}

impl Windows {
    /// How many windows are open.
    pub(crate) fn count(&self) -> usize {
        self.open.len()
    }

    /// The form of the window numbered `id`, when it shows one.
    #[cfg(test)]
    pub(crate) fn form_mut(&mut self, id: u64) -> Option<&mut Form> {
        self.open.iter_mut().find(|w| w.id == id)?.what.form_mut()
    }

    /// How many windows are open.
    pub fn len(&self) -> usize {
        self.open.len()
    }

    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// The window that shows `message`, by its number.
    pub fn of_message(&self, message: Id) -> Option<u64> {
        self.open.iter().find(|w| matches!(w.what, What::Message { message: m, .. } if m == message)).map(|w| w.id)
    }

    /// The window that shows `event`, by its number.
    pub fn of_event(&self, event: Id) -> Option<u64> {
        self.open.iter().find(|w| w.what.subject() == Some((false, event))).map(|w| w.id)
    }

    fn add(&mut self, what: What) -> u64 {
        self.last = self.last.saturating_add(1);
        self.open.push(Open { id: self.last, what, raise: true, embedded: true, title: String::new(), asking: false, ask: None });
        self.last
    }

    /// Bring a window forward: it is the one in front from now on.
    fn raise(&mut self, id: u64) {
        if let Some(at) = self.open.iter().position(|w| w.id == id) {
            let mut window = self.open.remove(at);
            window.raise = true;
            self.open.push(window);
        }
    }

    /// How many appointments and meetings that are being made have a window.
    pub fn new_events(&self) -> usize {
        self.open.iter().filter(|w| matches!(w.what, What::Event { event: None, .. })).count()
    }

    /// How many windows have something in them that was changed and is not saved.
    pub fn unsaved(&self) -> usize {
        self.open.iter().filter(|w| w.what.dirty()).count()
    }
}

/// The main window is to close: an appointment window with unsaved changes asks about them
/// first, and when there are several, one after the other. `true` when the main window has to
/// wait for an answer; it closes by itself once the last one is given, unless one was Cancel.
pub(crate) fn hold_quit(app: &mut MailApp) -> bool {
    let Some(first) = app.windows.open.iter().find(|w| w.what.dirty()).map(|w| w.id) else { return false };
    app.windows.quitting = true;
    if !app.windows.open.iter().any(|w| w.asking) {
        if let Some(window) = app.windows.open.iter_mut().find(|w| w.id == first) {
            window.asking = true;
        }
        app.windows.raise(first);
    }
    true
}

/// The zone the calendar is shown in is another one than `was` (the computer was carried to
/// another zone, or the owner chose one): the appointments that stand open, in windows of their
/// own or in the dialog, read by the new clock as the calendar does, so that saving one of them
/// moves nothing.
pub(crate) fn rezone(app: &mut MailApp, was: mailcraft_time::Zone) {
    let (now, twelve) = (app.session.zone(), app.session.prefs.twelve_hour);
    let in_windows = app.windows.open.iter_mut().filter_map(|window| window.what.form_mut());
    let in_dialog = match &mut app.dialog {
        Some(crate::dialogs::Dialog::Form(form)) => Some(form),
        _ => None,
    };
    for form in in_windows.chain(in_dialog).filter_map(Form::event_mut) {
        form.rezone(was, now, twelve);
    }
}

fn open_message(app: &mut MailApp, id: Id) -> Result<Value, String> {
    let Some(m) = app.session.mailbox.messages.get(&id).cloned() else { return Err(format!("no message with id {id}")) };
    // A draft opens where it is written.
    if m.draft {
        return app.execute("mail.editDraft", json!({"id": id}));
    }
    if let Some(window) = app.windows.of_message(id) {
        app.windows.raise(window);
        return Ok(json!({"window": window, "message": id, "already": true}));
    }
    if app.windows.len() >= MAX_WINDOWS {
        return Err(tl!("Too many windows are open: close one first").to_string());
    }
    let window = app.windows.add(What::Message { message: id, seen: m });
    Ok(json!({"window": window, "message": id}))
}

/// A window for the appointment or meeting `id`. When `p` says which of a series is meant
/// (`scope`, `occurrence`: the owner was asked on opening it), its form is about that.
fn open_event(app: &mut MailApp, id: Id, p: &Value) -> Result<Value, String> {
    let Some(uid) = app.session.mailbox.event(id).map(|e| e.uid.clone()) else { return Err(format!("no event with id {id}")) };
    // Opened to take up another time that somebody proposed (`"proposed": {"start", "end"}`):
    // that time goes into its fields, for the owner to send as an update or not.
    let proposed = p.get("proposed").filter(|proposed| proposed.is_object());
    if let Some(window) = app.windows.of_event(id) {
        app.windows.raise(window);
        let time = |key: &str| proposed.and_then(|proposed| proposed.get(key)).and_then(Value::as_str).and_then(mailcraft_time::parse_iso);
        if let (Some((start, _)), Some((end, _))) = (time("start"), time("end")) {
            let twelve = app.session.prefs.twelve_hour;
            let open = app.windows.open.iter_mut().find(|w| w.id == window);
            if let Some(form) = open.and_then(|w| w.what.form_mut()).and_then(Form::event_mut) {
                form.take_proposal(start, end, twelve);
            }
        }
        return Ok(json!({"window": window, "event": id, "already": true}));
    }
    if app.windows.len() >= MAX_WINDOWS {
        return Err(tl!("Too many windows are open: close one first").to_string());
    }
    let mut which = json!({"id": id});
    for key in ["scope", "occurrence"] {
        if let (Some(said), Some(fields)) = (p.get(key).filter(|said| said.is_string()), which.as_object_mut()) {
            fields.insert(key.to_string(), said.clone());
        }
    }
    if let (Some(proposed), Some(fields)) = (proposed, which.as_object_mut()) {
        fields.insert("proposed".into(), proposed.clone());
    }
    let Some(form) = Form::open("editEvent", app, &which) else { return Err(format!("no event with id {id}")) };
    let window = app.windows.add(What::Event { event: Some((id, uid)), form: Box::new(form) });
    Ok(json!({"window": window, "event": id}))
}

/// A window for an appointment (or, with `meeting`, a meeting) that is to be made, with what
/// `p` says of it filled in: `title`, `start`, `end`, `allDay`, `calendar` and the other fields
/// of `calendar.new`. Nothing is made until it is saved there.
fn open_new(app: &mut MailApp, meeting: bool, p: &Value) -> Result<Value, String> {
    if app.windows.len() >= MAX_WINDOWS {
        return Err(tl!("Too many windows are open: close one first").to_string());
    }
    let name = if meeting { "meeting" } else { "event" };
    let Some(form) = Form::open(name, app, p) else { return Err(tl!("That can't be opened in a window of its own").to_string()) };
    let window = app.windows.add(What::Event { event: None, form: Box::new(form) });
    Ok(json!({"window": window, "new": if meeting { "meeting" } else { "appointment" }}))
}

/// A command that the owner ran lacks what it needs, and the form that asks for it is the
/// appointment's (`event`, `meeting`, `editEvent`): that form opens in a window of its own, as
/// appointments do, rather than in the dialog. `None` for any other form.
pub(crate) fn open_form(app: &mut MailApp, name: &str, p: &Value) -> Option<Result<Value, String>> {
    let opened = match name {
        "event" => open_new(app, false, p),
        "meeting" => open_new(app, true, p),
        "editEvent" => {
            let event = p.get("id").and_then(Value::as_u64).filter(|id| *id != 0).or(app.session.sel.event)?;
            // An item that repeats is asked about before it opens (just this one, or the
            // series?): that is the dialog's question, and its answer comes back here.
            let mut which = json!({"event": event});
            for key in ["scope", "occurrence"] {
                if let (Some(said), Some(fields)) = (p.get(key).filter(|said| !said.is_null()), which.as_object_mut()) {
                    fields.insert(key.to_string(), said.clone());
                }
            }
            if crate::dialogs::confirmation(app, "ui.openWindow", &which).is_some() {
                return None;
            }
            open_event(app, event, p)
        }
        _ => return None,
    };
    Some(match opened {
        Ok(mut answer) => {
            if let Some(fields) = answer.as_object_mut() {
                fields.insert("pending".into(), json!(name));
            }
            Ok(answer)
        }
        Err(e) => {
            app.status(e.clone());
            Err(e)
        }
    })
}

fn close(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let number = |key: &str| p.get(key).filter(|v| !v.is_null()).map(|v| v.as_u64().ok_or_else(|| format!("`{key}` is a number")));
    let open = &mut app.windows.open;
    let before = open.len();
    if p.get("all").and_then(Value::as_bool) == Some(true) {
        open.clear();
    } else if let Some(window) = number("window") {
        let window = window?;
        open.retain(|w| w.id != window);
    } else if let Some(message) = number("message") {
        let message = message?;
        open.retain(|w| w.what.subject() != Some((true, message)));
    } else if let Some(event) = number("event") {
        let event = event?;
        open.retain(|w| w.what.subject() != Some((false, event)));
    } else {
        // The one in front.
        open.pop();
    }
    Ok(json!({"closed": before.saturating_sub(open.len()), "open": open.len()}))
}

/// What `ui.eventWindow` and `ui.inspect` say of an appointment's window.
fn event_state(window: &Open) -> Value {
    let (event, form) = match &window.what {
        What::Event { event, form } => (event.as_ref().map(|(id, _)| *id), form.event()),
        What::Message { .. } => (None, None),
    };
    let mut state = json!({
        "window": window.id,
        "event": event,
        "page": form.map(|f| f.page().code()),
        "meeting": form.is_some_and(|f| f.is_meeting()),
        "dirty": window.what.dirty(),
        "recurrence": form.is_some_and(|f| f.modal()),
        "asking": window.asking,
    });
    // How much it matters, whether it is (to be) a Teams meeting, and the link to join it by.
    if let Some((importance, online, link)) = form.map(|f| f.meeting_state()) {
        state["importance"] = json!(importance);
        state["online"] = json!(online);
        state["joinUrl"] = json!(link);
    }
    // What Find a Time last drew, while its page is the one in view.
    if let Some(form) = form.filter(|f| f.page() == crate::forms::Page::Scheduling) {
        let (drawn, meeting) = (form.scheduling(), form.meeting());
        let iso = |at: mailcraft_time::DateTime| at.iso();
        state["scheduling"] = json!({
            "day": meeting.day.iso(),
            "start": meeting.span.map(|(start, _)| iso(start)),
            "end": meeting.span.map(|(_, end)| iso(end)),
            "rows": drawn.rows,
            "known": drawn.known,
            "unknown": drawn.unknown,
            "bars": drawn.bars,
            "pending": drawn.pending,
            "note": drawn.note,
        });
    }
    state
}

/// `ui.eventWindow`: the window of an appointment or a meeting, by its number or the one in
/// front. It shows a page (`page`), opens or closes its Appointment Recurrence dialog
/// (`recurrence`), and does what its keys and its close button do (`do`): whatever asks the
/// owner first there, asks first here. `freeBusy` hands Find a Time an answer
/// about the people invited in a server's place (a stand-in: nothing is asked of anybody).
/// The people the appointment window `p` names (default: the one in front) is to invite, as the
/// form has them now: for the question an agent's save or send is asked first.
pub(crate) fn event_window_people(app: &MailApp, p: &Value) -> Vec<String> {
    let wanted = p.get("window").and_then(Value::as_u64);
    let form = app.windows.open.iter().rev().filter(|w| wanted.is_none_or(|id| w.id == id)).find_map(|w| w.what.form().and_then(Form::event));
    let Some(meeting) = form.map(|f| f.meeting()) else { return Vec::new() };
    meeting.required.iter().chain(meeting.optional.iter()).map(|a| a.email.clone()).collect()
}

fn event_window(app: &mut MailApp, p: &Value) -> Result<Value, String> {
    let wanted = match p.get("window").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().ok_or_else(|| "`window` is a number".to_string())?),
        None => None,
    };
    let is_event = |w: &Open| w.what.form().is_some_and(|f| f.event().is_some()) && wanted.is_none_or(|id| w.id == id);
    let Some(at) = app.windows.open.iter().rposition(is_event) else {
        return Err(tl!("No appointment is open in a window of its own").to_string());
    };
    let text = |key: &str| match p.get(key).filter(|v| !v.is_null()) {
        Some(v) => v.as_str().map(Some).ok_or_else(|| format!("`{key}` is text")),
        None => Ok(None),
    };
    let (page, action) = (text("page")?, text("do")?);
    let recurrence = match p.get("recurrence").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_bool().ok_or_else(|| "`recurrence` is true or false".to_string())?),
        None => None,
    };
    if let Some(schedule) = p.get("freeBusy").filter(|v| !v.is_null()) {
        let meeting = app.windows.open.get(at).and_then(|w| w.what.form()).and_then(Form::event).map(|form| form.meeting());
        let Some(meeting) = meeting else { return Err(tl!("No appointment is open in a window of its own").to_string()) };
        crate::scheduling::stand_in(app, &meeting, schedule)?;
    }
    let Some(window) = app.windows.open.get_mut(at) else { return Err(tl!("No appointment is open in a window of its own").to_string()) };
    let mut closed = false;
    if let Some(form) = window.what.form_mut().and_then(Form::event_mut) {
        match page {
            None => {}
            Some("appointment") => {
                form.set_page(crate::forms::Page::Appointment);
            }
            Some("scheduling") => {
                if !form.set_page(crate::forms::Page::Scheduling) {
                    return Err(tl!("Only a meeting you organize has Find a Time").to_string());
                }
            }
            Some(_) => return Err("`page` is appointment or scheduling".to_string()),
        }
        match recurrence {
            Some(true) => form.open_recurrence(),
            Some(false) => form.close_recurrence(),
            None => {}
        }
    }
    match action {
        None => {}
        Some("save") => window.ask = Some(Ask::Save),
        Some("saveAndClose") => window.ask = Some(Ask::SaveAndClose),
        Some("send") => window.ask = Some(Ask::Send),
        // As its close button does: it asks first when something isn't saved.
        Some("close") if window.what.dirty() => window.asking = true,
        Some("close") => closed = true,
        Some(_) => return Err("`do` is save, saveAndClose, send or close".to_string()),
    }
    let mut state = event_state(window);
    if closed {
        let id = window.id;
        app.windows.open.retain(|w| w.id != id);
        state["closed"] = json!(true);
    }
    Ok(state)
}

/// The window commands that are about these windows (`UI_COMMANDS` lists them).
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let number = |key: &str| p.get(key).filter(|v| !v.is_null()).map(Value::as_u64);
    let new = p.get("new").filter(|v| !v.is_null()).map(Value::as_str);
    Some(match id {
        "ui.openWindow" => match (number("message"), number("event"), new) {
            (Some(Some(message)), None, None) => open_message(app, message),
            (None, Some(Some(event)), None) => open_event(app, event, p),
            (None, None, Some(Some("appointment"))) => open_new(app, false, p),
            (None, None, Some(Some("meeting"))) => open_new(app, true, p),
            _ => Err(tl!("Say what to open: `message` or `event` by its id, or `new` (\"appointment\" or \"meeting\")").to_string()),
        },
        "ui.closeWindow" => close(app, p),
        "ui.eventWindow" => event_window(app, p),
        _ => return None,
    })
}

/// The open windows, for `ui.inspect`.
pub fn inspect(app: &MailApp) -> Value {
    Value::Array(
        app.windows
            .open
            .iter()
            .map(|w| match &w.what {
                What::Message { message, .. } => json!({"id": w.id, "kind": "message", "message": message, "title": w.title, "embedded": w.embedded}),
                What::Event { event, form } => {
                    let state = event_state(w);
                    let mut entry = match event {
                        Some((event, _)) => json!({"id": w.id, "kind": "event", "event": event, "title": w.title, "embedded": w.embedded}),
                        None => json!({"id": w.id, "kind": "newEvent", "form": form.name(), "title": w.title, "embedded": w.embedded}),
                    };
                    for key in ["page", "dirty", "asking", "meeting", "importance", "online", "joinUrl"] {
                        entry[key] = state[key].clone();
                    }
                    entry
                }
            })
            .collect(),
    )
}

/// The egui name of the window numbered `id`, where it is shown inside the main window.
fn area(id: u64) -> egui::Id {
    egui::Id::unique(("mailcraft-window", id))
}

/// The layer of the window numbered `id`, where it is shown inside the main window.
pub(crate) fn embedded_layer(id: u64) -> egui::LayerId {
    egui::LayerId::new(egui::Order::Middle, area(id))
}

/// Whether a message is the one a window has been showing.
fn same_message(a: &Message, b: &Message) -> bool {
    std::ptr::eq(a, b) || (a.message_id == b.message_id && a.date == b.date && a.from == b.from && a.subject == b.subject)
}

/// What the keys ask of the window that has the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Keys {
    close: bool,
    ask: Option<Ask>,
}

/// The keys of a window, taken out of what was pressed so that they are nobody else's: Escape
/// (when it is the window's, [`crate::keys`]) and Ctrl+W close it; in an appointment's, Ctrl+S
/// saves, and Ctrl+Enter and Alt+S send (a meeting) or save and close (an appointment). While a
/// window asks about its unsaved changes, or its form has a dialog of its own open, Escape and
/// Enter are that question's or that dialog's, and the others are nobody's. A held key is one
/// press: its repeats are taken, and do nothing (#406). `layer` is the window's.
fn keys_of(ctx: &egui::Context, window: &Open, layer: egui::LayerId) -> Keys {
    let engaged = window.asking || window.what.form().is_some_and(Form::engaged);
    let form = window.what.form().is_some();
    let escape = crate::keys::take_escape(ctx, layer);
    ctx.input_mut(|i| {
        let mut keys = Keys { close: take_key(i, Modifiers::COMMAND, Key::W) | escape, ask: None };
        if form {
            if take_key(i, Modifiers::COMMAND, Key::S) {
                keys.ask = Some(Ask::Save);
            }
            // (Taken before a text field could see them.)
            if take_key(i, Modifiers::COMMAND, Key::Enter) | take_key(i, Modifiers::ALT, Key::S) {
                keys.ask = Some(Ask::Send);
            }
        }
        if engaged { Keys::default() } else { keys }
    })
}

/// Take `key`, pressed with exactly `modifiers`, out of what was pressed, its repeats too. Whether
/// it was pressed anew: a held key is one press. (Exactly: a letter typed with AltGr, which a
/// keyboard reports as Ctrl+Alt, is a letter and nobody's shortcut.)
fn take_key(input: &mut egui::InputState, modifiers: Modifiers, key: Key) -> bool {
    let this = |event: &egui::Event| matches!(event, egui::Event::Key { key: pressed, pressed: true, modifiers: held, .. } if *pressed == key && held.matches_exact(modifiers));
    let new = input.events.iter().any(|event| this(event) && matches!(event, egui::Event::Key { repeat: false, .. }));
    input.events.retain(|event| !this(event));
    new
}

/// The viewport of the window numbered `id`, where it is a window of the system.
fn viewport_of(id: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("mailcraft-window", id))
}

/// The viewports of the windows that are open.
pub(crate) fn viewports(app: &MailApp) -> Vec<egui::ViewportId> {
    app.windows.open.iter().map(|w| viewport_of(w.id)).collect()
}

/// Show the windows that are open, and close the ones whose subject is gone. Once a frame,
/// before the main window's shortcuts: where the windows are inside the main one, the keys of
/// the active one (Escape closes it) are not also the main window's.
pub fn show(app: &mut MailApp, ctx: &egui::Context) {
    crate::reading::tick(app, ctx);
    if app.windows.open.is_empty() {
        // Nothing is left to ask about: a main window that waited for the answers closes.
        if std::mem::take(&mut app.windows.quitting) {
            app.quit_requested = true;
        }
        return;
    }
    let embedded = ctx.embed_viewports();
    let mut open = std::mem::take(&mut app.windows.open);
    // What a window shows may have gone since the last frame, or become something else.
    open.retain_mut(|w| match &mut w.what {
        What::Message { message, seen } => match app.session.mailbox.messages.get(message) {
            Some(now) if same_message(seen, now) => {
                *seen = now.clone();
                true
            }
            _ => false,
        },
        What::Event { event: Some((event, uid)), .. } => app.session.mailbox.event(*event).is_some_and(|e| e.uid == *uid),
        // What is being made is there until it is saved or given up.
        What::Event { event: None, .. } => true,
    });
    // Inside the main window the active one is the one of them that has the keyboard: the one
    // last clicked or typed in (#406), not the one in front.
    let active = crate::keys::active(ctx);
    let mut kept = Vec::with_capacity(open.len());
    for mut window in open {
        window.embedded = embedded;
        window.title = title(app, &window.what);
        // A dialog of the main window is answered first: it has the keyboard.
        let behind = embedded && (active != embedded_layer(window.id) || app.dialog.is_some());
        let closed = if embedded { show_embedded(app, ctx, &mut window, behind) } else { show_native(app, ctx, &mut window) };
        if !closed {
            kept.push(window);
        }
    }
    // What ran inside may have opened more: nothing twice, and no more than there may be.
    for new in std::mem::take(&mut app.windows.open) {
        let twice = |window: &Open| new.what.subject().is_some() && window.what.subject() == new.what.subject();
        if kept.len() < MAX_WINDOWS && !kept.iter().any(twice) {
            kept.push(new);
        }
    }
    // The main window waits to close: the next window with unsaved changes asks, and once none
    // is left, it closes.
    if app.windows.quitting {
        let busy = |w: &Open| w.asking || w.ask.is_some() || w.what.form().is_some_and(Form::engaged);
        if !kept.iter().any(busy) {
            match kept.iter_mut().find(|w| w.what.dirty()) {
                Some(next) => (next.asking, next.raise) = (true, true),
                None => {
                    app.windows.quitting = false;
                    app.quit_requested = true;
                }
            }
        }
        ctx.request_repaint();
    }
    app.windows.open = kept;
}

/// What a window is called: the message's subject, or the appointment's title and what it is.
fn title(app: &MailApp, what: &What) -> String {
    let named = |text: &str, fallback: &str| -> String {
        let text: String = text.chars().filter(|c| !c.is_control()).take(120).collect();
        if text.trim().is_empty() { fallback.to_string() } else { text }
    };
    match what {
        What::Message { seen, .. } => named(&seen.subject, tl!("(no subject)")),
        What::Event { event: Some((event, _)), form } => {
            let kind = form.title();
            match app.session.mailbox.event(*event).map(|e| named(&e.title, "")).filter(|title| !title.is_empty()) {
                Some(title) => format!("{title} - {kind}"),
                None => kind,
            }
        }
        // `New Appointment`, `New Meeting`.
        What::Event { event: None, form } => form.title(),
    }
}

/// One frame of a window: what is in it, and then what it was asked beyond that (to close, by
/// its keys or its close button; the question about its unsaved changes). `wish` is what its
/// keys asked, `closing` that its close button was pressed. `true` when it is to close.
fn frame(app: &mut MailApp, ui: &mut Ui, window: &mut Open, wish: Keys, closing: bool, own_keys: bool) -> bool {
    let ask = window.ask.take().or(wish.ask);
    let waited = window.what.form().is_some_and(Form::waiting);
    let verdict = contents(app, ui, window.id, &mut window.what, own_keys, ask);
    // A question came up about what the form was to do (mail to the people invited, which of a
    // series): it is asked in the main window, which comes forward for it.
    if own_keys && !waited && window.what.form().is_some_and(Form::waiting) {
        ui.ctx().send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
    }
    match verdict {
        Verdict::Close => return true,
        // It was saved and stays open: from now on it is about what was saved.
        Verdict::Now(id) => {
            if let (What::Event { event, .. }, Some(uid)) = (&mut window.what, app.session.mailbox.event(id).map(|e| e.uid.clone())) {
                *event = Some((id, uid));
            }
        }
        Verdict::Stay => {}
    }
    if wish.close || closing {
        // As Outlook has it: a window with nothing to save just closes, one with changes asks.
        if !window.what.dirty() {
            return true;
        }
        window.asking = true;
    }
    if window.asking {
        // By what the item is called, or else by what its window is.
        let named = window.what.form().and_then(Form::event).map(|form| form.subject().to_string()).filter(|name| !name.is_empty());
        let name = named.unwrap_or_else(|| window.title.clone());
        match crate::dialogs::unsaved_changes(ui.ctx(), &mut app.hit, window.id, &name) {
            None => {}
            Some(Unsaved::Save) => {
                window.asking = false;
                window.ask = Some(Ask::SaveAndClose);
                ui.ctx().request_repaint();
            }
            Some(Unsaved::Discard) => return true,
            Some(Unsaved::Cancel) => {
                window.asking = false;
                // The main window stays too, if it was waiting to close.
                app.windows.quitting = false;
            }
        }
    }
    false
}

/// A window of the system with the main window's icon, and its application id, by which a
/// Wayland desktop finds the icon of a window's title bar.
pub(crate) fn like_main(ctx: &egui::Context, mut builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
    let (icon, app_id) = ctx.viewport_for(egui::ViewportId::ROOT, |main| (main.builder.icon.clone(), main.builder.app_id.clone()));
    if let Some(icon) = icon {
        builder = builder.with_icon(icon);
    }
    if let Some(app_id) = app_id {
        builder = builder.with_app_id(app_id);
    }
    builder
}

/// A window of the system. `true` when it is to close.
fn show_native(app: &mut MailApp, ctx: &egui::Context, window: &mut Open) -> bool {
    let viewport = viewport_of(window.id);
    let size = match window.what.form() {
        None => MESSAGE_SIZE,
        Some(form) => [form.window_width(), FORM_HEIGHT],
    };
    let builder =
        like_main(ctx, egui::ViewportBuilder::default().with_title(window.title.clone()).with_inner_size(size).with_min_inner_size(MIN_SIZE));
    if std::mem::take(&mut window.raise) {
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
    }
    let mut close = false;
    ctx.show_viewport_immediate(viewport, builder, |ui, class| {
        let ctx = ui.ctx().clone();
        let t = Tokens::get(&ctx);
        // A backend that has no windows after all puts this inside the main one itself.
        let inside = class == egui::ViewportClass::EmbeddedWindow;
        let closing = !inside && ctx.input(|i| i.viewport().close_requested());
        // The window has the keyboard to itself: what is on top in it gets Escape and Enter. While
        // the main window asks something (an agent's question, or a question for this window's
        // form), they are nobody's here.
        if !inside {
            let waits = !app.asking.is_empty() || window.what.form().is_some_and(Form::waiting);
            crate::keys::gate(&ctx, waits);
        }
        let wish = keys_of(&ctx, window, ui.layer_id());
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| {
            close |= frame(app, ui, window, wish, closing, !inside);
        });
    });
    close
}

/// A movable window inside the main one; `behind`: another of them is the active one (or a
/// dialog is open), so the keyboard isn't its own. `true` when it is to close.
fn show_embedded(app: &mut MailApp, ctx: &egui::Context, window: &mut Open, behind: bool) -> bool {
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    let step = (window.id % 8) as f32 * 26.0;
    let mut shown = true;
    let frame_of = egui::Frame::window(&ctx.global_style()).inner_margin(0.0).fill(t.surface);
    let mut inner = egui::Window::new(egui::RichText::new(window.title.as_str()).font(semibold(13.0)))
        .id(area(window.id))
        .open(&mut shown)
        .collapsible(false)
        .frame(frame_of)
        .default_pos(screen.min + vec2(90.0 + step, 70.0 + step));
    inner = match window.what.form() {
        None => {
            let size =
                vec2(MESSAGE_SIZE[0].min(screen.width() - 40.0), MESSAGE_SIZE[1].min(screen.height() - 60.0)).max(vec2(MIN_SIZE[0], MIN_SIZE[1]));
            inner.resizable(true).default_size(size).min_size(MIN_SIZE)
        }
        Some(form) => {
            let width = form.window_width().min((screen.width() - 24.0).max(MIN_SIZE[0]));
            inner.resizable(false).default_width(width).min_width(width).max_width(width)
        }
    };
    if std::mem::take(&mut window.raise) {
        ctx.move_to_top(embedded_layer(window.id));
    }
    // The keys are the window's when it is the active one.
    let wish = if behind { Keys::default() } else { keys_of(ctx, window, embedded_layer(window.id)) };
    let id = window.id;
    let mut close = false;
    let response = inner.show(ctx, |ui| {
        close = frame(app, ui, window, wish, false, false);
    });
    if let Some(response) = response {
        app.hit.add("window", id, response.response.rect);
    }
    if !shown && !close {
        // Its close button.
        if window.what.dirty() {
            window.asking = true;
            ctx.request_repaint();
        } else {
            close = true;
        }
    }
    close
}

/// What is in a window. `own_keys`: the window has the keyboard to itself (it is a window of the
/// system), so the keys of its actions are its to act on. `ask`: what its form is asked to do.
fn contents(app: &mut MailApp, ui: &mut Ui, id: u64, what: &mut What, own_keys: bool, ask: Option<Ask>) -> Verdict {
    let t = Tokens::get(ui.ctx());
    match what {
        What::Message { seen, .. } => {
            let m = seen.clone();
            let mut close = false;
            egui::Panel::top(egui::Id::unique(("mailcraft-window-actions", id)))
                .resizable(false)
                .frame(egui::Frame::NONE.fill(t.ribbon).inner_margin(egui::Margin::symmetric(10, 5)))
                .show(ui, |ui| close = actions(app, ui, &m, own_keys));
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.surface)).show(ui, |ui| {
                crate::reading::show_message(app, ui, &m, Place::Window(id));
            });
            if close { Verdict::Close } else { Verdict::Stay }
        }
        What::Event { form, .. } => {
            // The form's fields are this window's own; in a window of the system it has the
            // whole of it to fill.
            let host = Host { id, fill: own_keys };
            crate::forms::kit::hosted(host, || form.show_in_window(app, ui, ask))
        }
    }
}

/// What a message window's toolbar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Run(&'static str),
    Flag,
    Read,
}

/// The row of actions on a message in a window of its own. `true` when the window is to close:
/// the message was deleted from it.
fn actions(app: &mut MailApp, ui: &mut Ui, m: &Arc<Message>, own_keys: bool) -> bool {
    let flagged = m.flag == FlagStatus::Flagged;
    let mut row: Vec<(&str, &str, &'static str, Action, &'static str)> =
        crate::reading::ACTIONS.iter().map(|(icon, label, command)| (*icon, *label, *command, Action::Run(command), hit_kind(command))).collect();
    row.push(("flag", if flagged { "Clear Flag" } else { "Flag" }, "mail.toggleFlag", Action::Flag, "windowFlag"));
    let (icon, label, command) = if m.read { ("markUnread", "Mark Unread", "mail.markUnread") } else { ("markRead", "Mark Read", "mail.markRead") };
    row.push((icon, label, command, Action::Read, "windowUnread"));
    let mut asked = None;
    // The keys the main window has for these, when this window has the keyboard to itself.
    if own_keys && !ui.ctx().egui_wants_keyboard_input() {
        let pressed: Vec<String> = ui.input(|i| {
            let keys = i.events.iter().filter_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => keys::key_name(*key).map(|name| keys::combo(*modifiers, name)),
                _ => None,
            });
            keys.take(8).collect()
        });
        for (.., command, action, _) in &row {
            let bound = app.session.registry.get(command).map(|spec| spec.shortcut).unwrap_or_default();
            if !bound.is_empty() && bound.split(" / ").any(|key| pressed.iter().any(|p| p.eq_ignore_ascii_case(key))) {
                asked = Some(*action);
            }
        }
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 0.0);
        for (n, (icon, label, command, action, kind)) in row.iter().enumerate() {
            if n == 3 || n == 4 {
                ui.add_space(8.0);
            }
            let key = crate::widgets::shortcut_text(app, command);
            let tip = if key.is_empty() { tl!(label).to_string() } else { format!("{} ({key})", tl!(label)) };
            let button = crate::widgets::icon_text_button(ui, icon, tl!(label), true).on_hover_text(tip);
            if !own_keys {
                app.hit.add(kind, m.id, button.rect);
            }
            if button.clicked() {
                asked = Some(*action);
            }
        }
    });
    let Some(action) = asked else { return false };
    let ctx = ui.ctx().clone();
    // What a window of the system starts in the main window is looked at there.
    let to_main = |ctx: &egui::Context| {
        if own_keys {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    };
    match action {
        Action::Flag => {
            let _ = app.run("mail.toggleFlag", json!({"ids": [m.id]}));
            false
        }
        Action::Read => {
            let _ = app.run(if m.read { "mail.markUnread" } else { "mail.markRead" }, json!({"ids": [m.id]}));
            false
        }
        Action::Run("mail.delete") => match app.run("mail.delete", json!({"ids": [m.id]})) {
            // Asked about first (it would be for good): the question is in the main window.
            Ok(answer) if answer.get("pending").is_some() => {
                to_main(&ctx);
                false
            }
            Ok(_) => true,
            Err(_) => false,
        },
        Action::Run(command) => {
            // Replying and forwarding are written in the main window.
            if app.run(command, json!({"id": m.id})).is_ok() {
                if app.session.view.module != mailcraft_engine::Module::Mail {
                    let _ = app.execute("view.mail", json!({}));
                }
                to_main(&ctx);
            }
            false
        }
    }
}

/// The name `ui.inspect` and `ui.click` know a toolbar button by.
fn hit_kind(command: &str) -> &'static str {
    match command {
        "mail.reply" => "windowReply",
        "mail.replyAll" => "windowReplyAll",
        "mail.forward" => "windowForward",
        _ => "windowDelete",
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;
    use mailcraft_engine::Session;
    use mailcraft_model::FolderKind;

    use super::*;
    use crate::Services;

    /// Wednesday 2026-10-14 16:00 UTC.
    const NOW: i64 = 1_791_993_600;

    type Harness = egui_kittest::Harness<'static, MailApp>;

    fn app() -> MailApp {
        let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
        app.autosave = false;
        app.auto_sync = false;
        app
    }

    /// The whole window, its frames a sixtieth of a second apart, as on a screen: two clicks one
    /// after the other are a double click. (These windows are inside the main one: the headless
    /// window has no others, like a browser.)
    fn window(app: MailApp) -> Harness {
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(3);
        harness
    }

    fn click(harness: &mut Harness, pos: egui::Pos2, times: usize) {
        harness.event(egui::Event::PointerMoved(pos));
        harness.step();
        for _ in 0..times {
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
                harness.step();
            }
        }
        harness.run_steps(3);
    }

    fn press(harness: &mut Harness, key: Key) {
        harness.key_press(key);
        harness.run_steps(3);
    }

    fn run(harness: &mut Harness, command: &str, params: Value) -> Value {
        let answer = harness.state_mut().execute(command, params).unwrap_or_else(|e| panic!("{command}: {e}"));
        harness.run_steps(3);
        answer
    }

    /// Open a window by command; its number.
    fn open(harness: &mut Harness, params: Value) -> u64 {
        run(harness, "ui.openWindow", params)["window"].as_u64().unwrap()
    }

    /// The messages of the list in view.
    fn listed(harness: &mut Harness) -> Vec<Id> {
        harness.state_mut().session.rows().iter().map(|row| row.id).collect()
    }

    fn says(harness: &Harness, text: &str) -> bool {
        harness.query_all_by_label_contains(text).next().is_some()
    }

    fn windows(harness: &mut Harness) -> Value {
        let ctx = harness.ctx.clone();
        crate::control::inspect(harness.state_mut(), &ctx)["windows"].clone()
    }

    #[test]
    fn a_double_click_opens_a_message_in_a_window_of_its_own() {
        let mut harness = window(app());
        let rows = listed(&mut harness);
        let (first, second) = (rows[1], rows[2]);
        let subject = harness.state().session.mailbox.message(first).unwrap().subject.clone();
        let at = |harness: &Harness, id: Id| harness.state().hit.rect("message", id).unwrap().center();
        // One click selects, as ever, and opens nothing.
        let pos = at(&harness, first);
        click(&mut harness, pos, 1);
        assert_eq!(harness.state().session.sel.focus(), Some(first));
        assert!(harness.state().windows.is_empty() && windows(&mut harness) == json!([]));
        // Two in a row open it, in a window titled with its subject; the message stays selected,
        // and nothing is being written.
        harness.run_steps(40);
        click(&mut harness, pos, 2);
        assert_eq!(harness.state().windows.of_message(first), Some(1));
        assert_eq!(windows(&mut harness), json!([{"id": 1, "kind": "message", "message": first, "title": subject, "embedded": true}]));
        assert_eq!((harness.state().session.sel.focus(), harness.state().session.compose.is_none()), (Some(first), true));
        // It is on screen, with the message in it (as the reading pane has it) and what can be done with it.
        assert!(harness.state().hit.rect("window", 1).is_some());
        for kind in ["windowReply", "windowReplyAll", "windowForward", "windowDelete", "windowFlag", "windowUnread"] {
            assert!(harness.state().hit.rect(kind, first).is_some(), "{kind}");
        }
        assert!(harness.query_all_by_label(subject.as_str()).count() >= 2, "the subject is in the pane and in the window");
        // The same message again is the same window; another message is another.
        run(&mut harness, "ui.closeWindow", json!({}));
        assert!(harness.state().windows.is_empty());
        harness.run_steps(40);
        let pos = at(&harness, second);
        click(&mut harness, pos, 2);
        assert_eq!((harness.state().windows.of_message(second), harness.state().windows.len()), (Some(2), 1));
        assert_eq!(run(&mut harness, "ui.openWindow", json!({"message": second}))["already"], true);
        assert_eq!(open(&mut harness, json!({"message": first})), 3);
        assert_eq!(harness.state().windows.len(), 2);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));

        // A draft opens where it is written, as it did before; it gets no window.
        run(&mut harness, "mail.new", json!({"to": "maya@lumen.example", "subject": "Unfinished business"}));
        run(&mut harness, "compose.saveDraft", json!({}));
        run(&mut harness, "compose.close", json!({}));
        let draft = harness
            .state()
            .session
            .mailbox
            .messages
            .values()
            .find(|m| m.draft && m.subject == "Unfinished business")
            .map(|m| (m.id, m.folder))
            .unwrap();
        run(&mut harness, "folder.select", json!({"id": draft.1}));
        harness.run_steps(40);
        let pos = at(&harness, draft.0);
        click(&mut harness, pos, 2);
        assert_eq!(harness.state().session.compose.as_ref().and_then(|c| c.draft), Some(draft.0));
        assert!(harness.state().windows.is_empty());
        run(&mut harness, "compose.close", json!({}));
        // By command too.
        assert_eq!(run(&mut harness, "ui.openWindow", json!({"message": draft.0}))["composing"], true);
        assert!(harness.state().windows.is_empty());
    }

    #[test]
    fn a_message_window_acts_on_its_message_with_the_commands_of_the_main_window() {
        let mut harness = window(app());
        let rows = listed(&mut harness);
        let (selected, id) = (rows[0], rows[3]);
        run(&mut harness, "mail.select", json!({"id": selected}));
        open(&mut harness, json!({"message": id}));
        let button = |harness: &Harness, kind: &str| harness.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind}")).center();
        let message = |harness: &Harness| harness.state().session.mailbox.message(id).unwrap().clone();
        // Flag and Mark Unread turn it over, and back.
        let (flag, read) = (message(&harness).flag, message(&harness).read);
        let pos = button(&harness, "windowFlag");
        click(&mut harness, pos, 1);
        assert_ne!(message(&harness).flag, flag);
        harness.run_steps(30);
        click(&mut harness, pos, 1);
        assert_eq!(message(&harness).flag, flag);
        let pos = button(&harness, "windowUnread");
        click(&mut harness, pos, 1);
        assert_ne!(message(&harness).read, read);
        harness.run_steps(30);
        click(&mut harness, pos, 1);
        assert_eq!(message(&harness).read, read);
        // Reply, Reply All and Forward are written in the main window, about this message.
        for (kind, prefix) in [("windowReply", "RE:"), ("windowReplyAll", "RE:"), ("windowForward", "FW:")] {
            let pos = button(&harness, kind);
            click(&mut harness, pos, 1);
            let compose = harness.state().session.compose.clone().unwrap_or_else(|| panic!("{kind} wrote nothing"));
            assert_eq!(compose.source, Some(id), "{kind}");
            assert!(compose.subject.starts_with(prefix), "{kind}: {}", compose.subject);
            assert_eq!(harness.state().ui.tab, "Message");
            run(&mut harness, "compose.close", json!({}));
            harness.run_steps(30);
        }
        // None of it touched what is selected in the main window, and the window is still there.
        assert_eq!(harness.state().session.sel.messages, [selected]);
        assert!(harness.state().windows.of_message(id).is_some());
        // Delete moves it to Deleted Items and closes the window.
        let pos = button(&harness, "windowDelete");
        click(&mut harness, pos, 1);
        let deleted = harness.state().session.mailbox.folder(message(&harness).folder).map(|f| f.kind);
        assert_eq!(deleted, Some(FolderKind::Deleted));
        assert!(harness.state().windows.is_empty());
        assert_eq!(harness.state().session.sel.messages, [selected]);
        // From there, deleting is for good and is asked about first, in the main window; the
        // window stays until the answer.
        open(&mut harness, json!({"message": id}));
        let pos = button(&harness, "windowDelete");
        click(&mut harness, pos, 1);
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));
        assert!(harness.state().windows.of_message(id).is_some() && harness.state().session.mailbox.message(id).is_some());
        harness.state_mut().dialog = None;
        run(&mut harness, "mail.delete", json!({"ids": [id]}));
        assert!(harness.state().session.mailbox.message(id).is_none() && harness.state().windows.is_empty());
    }

    #[test]
    fn escape_and_the_close_button_close_the_window_in_front() {
        let mut harness = window(app());
        let rows = listed(&mut harness);
        let (a, b) = (rows[0], rows[1]);
        open(&mut harness, json!({"message": a}));
        open(&mut harness, json!({"message": b}));
        // Escape closes the one in front, then the next.
        press(&mut harness, Key::Escape);
        assert_eq!((harness.state().windows.of_message(a).is_some(), harness.state().windows.of_message(b)), (true, None));
        // A dialog of the main window is answered first.
        run(&mut harness, "ui.dialog", json!({"name": "about"}));
        press(&mut harness, Key::Escape);
        assert!(harness.state().dialog.is_none() && harness.state().windows.len() == 1);
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty());
        // With a window open, Escape is the window's and not also the main window's: a message
        // that is being written stays open until Escape is pressed again.
        run(&mut harness, "mail.new", json!({}));
        open(&mut harness, json!({"message": a}));
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty() && harness.state().session.compose.is_some());
        press(&mut harness, Key::Escape);
        assert!(harness.state().session.compose.is_none());

        // The window that was clicked on last is the one in front.
        let first = open(&mut harness, json!({"message": a}));
        let second = open(&mut harness, json!({"message": b}));
        // (Its left edge shows beside the other, which was opened after it and lies over the rest.)
        let edge = harness.state().hit.rect("window", first).unwrap().min + vec2(8.0, 12.0);
        click(&mut harness, edge, 1);
        press(&mut harness, Key::Escape);
        assert_eq!((harness.state().windows.of_message(a), harness.state().windows.of_message(b)), (None, Some(second)));
        // The window's own close button.
        assert_eq!(harness.query_all_by_label("Close window").count(), 1);
        harness.get_by_label("Close window").click();
        harness.run_steps(3);
        assert!(harness.state().windows.is_empty());

        // And by command: one by its number, one by what it shows, the one in front, all of them.
        let first = open(&mut harness, json!({"message": a}));
        open(&mut harness, json!({"message": b}));
        open(&mut harness, json!({"message": rows[2]}));
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"window": first}))["closed"], 1);
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"message": b}))["closed"], 1);
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"message": b}))["closed"], 0);
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({}))["closed"], 1);
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({}))["closed"], 0);
        open(&mut harness, json!({"message": a}));
        open(&mut harness, json!({"message": b}));
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"all": true})), json!({"closed": 2, "open": 0}));
    }

    #[test]
    fn a_window_follows_its_message_and_closes_when_it_is_gone() {
        let mut harness = window(app());
        let rows = listed(&mut harness);
        let (a, b, c) = (rows[0], rows[1], rows[2]);
        open(&mut harness, json!({"message": a}));
        // Filed elsewhere, flagged, read: it is the same message, and the window keeps showing it.
        run(&mut harness, "mail.archive", json!({"ids": [a]}));
        run(&mut harness, "mail.flag", json!({"ids": [a]}));
        run(&mut harness, "mail.markUnread", json!({"ids": [a]}));
        assert!(harness.state().windows.of_message(a).is_some());
        // Gone for good: the window goes with it, and does not come back when that is undone.
        run(&mut harness, "mail.deletePermanently", json!({"ids": [a]}));
        assert!(harness.state().windows.is_empty());
        run(&mut harness, "edit.undo", json!({}));
        assert!(harness.state().session.mailbox.message(a).is_some() && harness.state().windows.is_empty());

        // A different message that comes to have the same number (another mailbox was opened) is
        // not the one the window was showing.
        open(&mut harness, json!({"message": b}));
        open(&mut harness, json!({"message": c}));
        let mut other = harness.state().session.mailbox.clone();
        let stranger = other.message_mut(b).unwrap();
        (stranger.subject, stranger.message_id) = ("Something else entirely".into(), "other@elsewhere.example".into());
        harness.state_mut().session.mailbox = other;
        harness.state_mut().session.mailbox_changed();
        harness.run_steps(3);
        assert_eq!((harness.state().windows.of_message(b), harness.state().windows.of_message(c).is_some()), (None, true));
        // An empty mailbox: nothing is left to show.
        run(&mut harness, "file.new", json!({}));
        assert!(harness.state().windows.is_empty());
    }

    #[test]
    fn only_so_many_windows_and_nothing_hostile_opens_one() {
        let mut harness = window(app());
        let messages: Vec<Id> = harness.state().session.mailbox.messages.values().filter(|m| !m.draft).map(|m| m.id).take(MAX_WINDOWS + 1).collect();
        assert_eq!(messages.len(), MAX_WINDOWS + 1, "the sample mailbox has that many messages");
        for id in messages.iter().take(MAX_WINDOWS) {
            open(&mut harness, json!({"message": id}));
        }
        let refused = harness.state_mut().execute("ui.openWindow", json!({"message": messages[MAX_WINDOWS]})).unwrap_err();
        assert!(refused.contains("Too many windows"), "{refused}");
        // An appointment is a window like any other.
        let event = harness.state().session.mailbox.events.keys().next().copied().unwrap();
        assert!(harness.state_mut().execute("ui.openWindow", json!({"event": event})).unwrap_err().contains("Too many windows"));
        harness.run_steps(4);
        assert_eq!((harness.state().windows.len(), windows(&mut harness).as_array().map(Vec::len)), (MAX_WINDOWS, Some(MAX_WINDOWS)));
        // One that is open already is still found.
        assert_eq!(run(&mut harness, "ui.openWindow", json!({"message": messages[0]}))["already"], true);
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"all": true}))["closed"], MAX_WINDOWS);
        // Nonsense opens nothing and closes nothing, by command or as the interface runs it.
        for params in [
            json!(null),
            json!({}),
            json!([1, 2]),
            json!({"message": "1"}),
            json!({"message": 0}),
            json!({"message": -4}),
            json!({"message": 1.5}),
            json!({"message": u64::MAX}),
            json!({"event": u64::MAX}),
            json!({"event": {"id": 3}}),
            json!({"message": messages[0], "event": event}),
            json!({"contact": 1}),
        ] {
            assert!(harness.state_mut().execute("ui.openWindow", params.clone()).is_err(), "{params}");
            assert!(harness.state_mut().run("ui.openWindow", params.clone()).is_err(), "{params}");
            let _ = harness.state_mut().execute("ui.closeWindow", params);
        }
        for params in [json!({"window": "x"}), json!({"message": []}), json!({"event": -1})] {
            assert!(harness.state_mut().execute("ui.closeWindow", params.clone()).is_err(), "{params}");
        }
        harness.run_steps(3);
        assert!(harness.state().windows.is_empty());
    }

    /// Ctrl (⌘ on a Mac), as the keyboard of this machine has it.
    fn ctrl() -> egui::Modifiers {
        egui::Modifiers { ctrl: !cfg!(target_os = "macos"), mac_cmd: cfg!(target_os = "macos"), command: true, ..Default::default() }
    }

    /// Press `key` with `mods` held.
    fn chord(harness: &mut Harness, mods: egui::Modifiers, key: Key) {
        harness.key_press_modifiers(mods, key);
        harness.run_steps(4);
    }

    fn type_text(harness: &mut Harness, text: &str) {
        harness.event(egui::Event::Text(text.into()));
        harness.run_steps(3);
    }

    /// Where a button of the bar of the window numbered `window` is.
    fn bar_rect(harness: &Harness, window: u64, key: &str) -> Option<egui::Rect> {
        harness.state().hit.rect("windowBar", crate::forms::kit::bar::hit_id(window, key))
    }

    /// Click a button of the bar of the window numbered `window`.
    fn bar(harness: &mut Harness, window: u64, key: &str) {
        let rect = bar_rect(harness, window, key).unwrap_or_else(|| panic!("no {key} in the bar of window {window}"));
        click(harness, rect.center(), 1);
    }

    /// Click the last thing that says `label` (a menu's entry is laid out after what is under it).
    fn click_label(harness: &mut Harness, label: &str) {
        let rect = harness.query_all_by_label_contains(label).last().unwrap_or_else(|| panic!("nothing says {label}")).rect();
        click(harness, rect.center(), 1);
    }

    /// Answer the question a window asks about its unsaved changes.
    fn answer(harness: &mut Harness, window: u64, kind: &str) {
        let rect = harness.state().hit.rect(kind, window).unwrap_or_else(|| panic!("window {window} doesn't ask ({kind})"));
        click(harness, rect.center(), 1);
    }

    fn asking(harness: &mut Harness) -> bool {
        windows(harness).as_array().is_some_and(|all| all.iter().any(|w| w["asking"] == true))
    }

    fn event_named(harness: &Harness, title: &str) -> Option<mailcraft_model::Event> {
        harness.state().session.mailbox.events.values().find(|e| e.title == title).map(|e| (**e).clone())
    }

    fn id_of(harness: &Harness, title: &str) -> Id {
        event_named(harness, title).unwrap_or_else(|| panic!("no event called {title}")).id
    }

    #[test]
    fn an_appointment_opens_in_a_window_and_is_saved_from_it() {
        let mut harness = window(app());
        let events: Vec<Id> = harness.state().session.mailbox.events.keys().copied().take(2).collect();
        let (a, b) = (events[0], events[1]);
        let title = |harness: &Harness, id: Id| harness.state().session.mailbox.event(id).unwrap().title.clone();
        // As the calendar does it on a double click.
        let opened = harness.state_mut().run("ui.openWindow", json!({"event": a})).unwrap();
        let window = opened["window"].as_u64().unwrap();
        harness.run_steps(4);
        assert!(harness.state().dialog.is_none(), "it is a window, not the dialog");
        let shown = windows(&mut harness);
        assert_eq!((&shown[0]["kind"], &shown[0]["event"], &shown[0]["dirty"]), (&json!("event"), &json!(a), &json!(false)));
        assert!(shown[0]["title"].as_str().unwrap().starts_with(&format!("{} - ", title(&harness, a))), "{shown}");
        assert!(harness.state().hit.rect("window", window).is_some() && says(&harness, "All day"));
        // Again: the same window. Another appointment: a window of its own, beside the first.
        assert_eq!(run(&mut harness, "ui.openWindow", json!({"event": a}))["already"], true);
        let other = open(&mut harness, json!({"event": b}));
        assert_ne!(other, window);
        assert_eq!(
            (harness.state().windows.len(), harness.state().windows.of_event(a), harness.state().windows.of_event(b)),
            (2, Some(window), Some(other))
        );
        assert!(harness.state().hit.rect("window", window).is_some() && harness.state().hit.rect("window", other).is_some());
        // What is typed goes into the one in front (its title has the keyboard), and into that
        // one alone: the two forms don't share their fields.
        let (before_a, before_b) = (title(&harness, a), title(&harness, b));
        type_text(&mut harness, " (moved)");
        assert_eq!(harness.state().windows.unsaved(), 1);
        // Enter saves the one in front, with the command the dialog runs, and its window closes;
        // the other is still open, and its appointment is as it was.
        press(&mut harness, Key::Enter);
        let after = title(&harness, b);
        assert!(after != before_b && after.contains("(moved)"), "{before_b:?} became {after:?}");
        assert_eq!(title(&harness, a), before_a);
        assert_eq!((harness.state().windows.len(), harness.state().windows.of_event(a)), (1, Some(window)));
        // Nothing was typed into the one that is left: Enter saves it as it is.
        press(&mut harness, Key::Enter);
        assert_eq!(title(&harness, a), before_a);
        assert!(harness.state().windows.is_empty());

        // A dialog of the main window is answered first: the window waits behind it, and Enter
        // is the dialog's, not also the window's.
        let window = open(&mut harness, json!({"event": a}));
        run(&mut harness, "ui.dialog", json!({"name": "goToDate"}));
        assert!(harness.state().hit.rect("window", window).is_some());
        press(&mut harness, Key::Enter);
        assert!(harness.state().dialog.is_none() && harness.state().windows.of_event(a) == Some(window));
        // Escape closes it (nothing in it was changed); an appointment deleted elsewhere takes
        // its window along.
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty());
        open(&mut harness, json!({"event": a}));
        run(&mut harness, "calendar.delete", json!({"id": a}));
        assert!(harness.state().windows.is_empty());
        assert!(harness.state_mut().execute("ui.openWindow", json!({"event": a})).is_err());
    }

    #[test]
    fn a_new_appointment_is_made_in_a_window_of_its_own() {
        let mut harness = window(app());
        let events = |harness: &Harness| harness.state().session.mailbox.events.len();
        let count = events(&harness);
        // By command, for a time: nothing exists until it is saved.
        let opened = run(&mut harness, "ui.openWindow", json!({"new": "appointment", "start": "2026-10-16T15:00", "end": "2026-10-16T16:30"}));
        let window = opened["window"].as_u64().unwrap();
        assert_eq!(opened["new"], "appointment");
        assert_eq!(
            windows(&mut harness),
            json!([{"id": window, "kind": "newEvent", "form": "event", "title": "New Appointment", "embedded": true,
                "page": "appointment", "dirty": false, "asking": false,
                "meeting": false, "importance": "normal", "online": false, "joinUrl": ""}])
        );
        assert_eq!((harness.state().windows.new_events(), harness.state().dialog.is_none(), events(&harness)), (1, true, count));
        assert!(harness.state().hit.rect("window", window).is_some());
        // Its title has the keyboard; Enter saves, and the window closes.
        type_text(&mut harness, "Walk the route");
        press(&mut harness, Key::Enter);
        assert!(harness.state().windows.is_empty());
        let s = &harness.state().session;
        let made = s.mailbox.events.values().find(|e| e.title == "Walk the route").expect("it was made");
        assert_eq!(
            (made.start.iso().as_str(), made.duration_minutes(), s.sel.event, events(&harness)),
            ("2026-10-16T15:00:00", 90, Some(made.id), count + 1)
        );
        // Whole days, and the calendar it is to go into.
        let calendar = harness.state().session.mailbox.calendars.first().map(|c| c.id).unwrap();
        open(
            &mut harness,
            json!({"new": "appointment", "start": "2026-10-19", "end": "2026-10-20", "allDay": true, "calendar": calendar, "title": "Offsite"}),
        );
        press(&mut harness, Key::Enter);
        let made = harness.state().session.mailbox.events.values().find(|e| e.title == "Offsite").cloned().expect("it was made");
        assert_eq!(
            (made.all_day, made.start.iso().as_str(), made.end.iso().as_str(), made.calendar),
            (true, "2026-10-19T00:00:00", "2026-10-21T00:00:00", calendar)
        );

        // New Appointment and New Meeting, as the ribbon and their keys run them, open such a
        // window rather than the dialog; so does Open on the appointment that is selected.
        let pending = harness.state_mut().run("calendar.new", json!({})).unwrap();
        assert_eq!((&pending["pending"], pending["window"].is_u64()), (&json!("event"), true));
        let pending = harness.state_mut().run("calendar.newMeeting", json!({})).unwrap();
        assert_eq!(pending["pending"], "meeting");
        harness.run_steps(3);
        let shown = windows(&mut harness);
        assert_eq!(
            (&shown[0]["title"], &shown[1]["title"], harness.state().dialog.is_none()),
            (&json!("New Appointment"), &json!("New Meeting"), true)
        );
        // Two of them at once, and neither is the other's: what is typed goes into the one in
        // front, which is the meeting.
        type_text(&mut harness, "Kick-off");
        // A meeting needs somebody to invite: Enter doesn't save it (it says why), nor the
        // appointment behind it.
        press(&mut harness, Key::Enter);
        assert_eq!((harness.state().windows.new_events(), events(&harness)), (2, count + 2));
        assert!(says(&harness, "Name somebody to invite first"));
        // Escape gives the one in front up, once it is said that what was typed isn't wanted;
        // the appointment behind it, in which nothing was typed, just closes. Nothing was made.
        press(&mut harness, Key::Escape);
        assert!(asking(&mut harness) && harness.state().windows.len() == 2);
        let meeting = windows(&mut harness)[1]["id"].as_u64().unwrap();
        answer(&mut harness, meeting, "unsavedDiscard");
        assert_eq!(windows(&mut harness)[0]["title"], "New Appointment");
        assert_eq!(harness.state().windows.len(), 1);
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty() && events(&harness) == count + 2);
        run(&mut harness, "calendar.select", json!({"id": made.id}));
        let pending = harness.state_mut().run("calendar.edit", json!({})).unwrap();
        assert_eq!(
            (&pending["pending"], harness.state().windows.of_event(made.id), harness.state().dialog.is_none()),
            (&json!("editEvent"), pending["window"].as_u64(), true)
        );
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        // A script that asks for the dialog still gets the dialog.
        run(&mut harness, "ui.dialog", json!({"name": "event"}));
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("event"));
        run(&mut harness, "ui.closeDialog", json!({}));

        // Only so many windows, and nothing hostile opens or breaks one.
        for _ in 0..MAX_WINDOWS {
            open(&mut harness, json!({"new": "meeting"}));
        }
        assert!(harness.state_mut().execute("ui.openWindow", json!({"new": "appointment"})).unwrap_err().contains("Too many windows"));
        assert!(harness.state_mut().run("calendar.new", json!({})).unwrap_err().contains("Too many windows"));
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"all": true}))["closed"], MAX_WINDOWS);
        let junk = "\u{0}\u{202e}é\n<b>".repeat(20_000);
        for params in [
            json!({"new": "nothing"}),
            json!({"new": 7}),
            json!({"new": ["appointment"]}),
            json!({"new": "appointment", "event": made.id}),
            json!({"new": "meeting", "message": 1}),
        ] {
            assert!(harness.state_mut().execute("ui.openWindow", params.clone()).is_err(), "{params}");
        }
        open(
            &mut harness,
            json!({"new": "appointment", "start": junk, "end": 7, "allDay": "yes", "title": junk, "calendar": u64::MAX, "reminder": 1e300}),
        );
        open(&mut harness, json!({"new": "meeting", "attendees": junk, "optional": [1, 2], "start": "9999-12-31T23:59", "end": "0001-01-01"}));
        type_text(&mut harness, &junk);
        press(&mut harness, Key::Tab);
        press(&mut harness, Key::Enter);
        chord(&mut harness, ctrl(), Key::S);
        chord(&mut harness, ctrl(), Key::Enter);
        chord(&mut harness, ctrl(), Key::W);
        press(&mut harness, Key::Escape);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        assert!(harness.state().windows.is_empty());
    }

    /// In a window of the system the form has the whole window: its bar at the head of it, and
    /// its last field takes what the others leave, down to the window's foot. (In a dialog, and
    /// inside the main window, a form is as tall as its fields.)
    #[test]
    fn a_form_fills_a_window_of_its_own() {
        use egui::accesskit::Role;
        struct Shown {
            app: MailApp,
            what: What,
            fill: bool,
            frames: u32,
            /// Where the window's panel ends.
            foot: f32,
        }
        // The bar's first button, the description, and where the window's panel ends.
        let lay_out = |fill: bool, size: egui::Vec2| {
            let app = app();
            let form = Form::open("event", &app, &json!({"title": "Plan"})).unwrap();
            let state = Shown { app, what: What::Event { event: None, form: Box::new(form) }, fill, frames: 0, foot: 0.0 };
            let mut harness = egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(size).build_ui_state(
                |ui, shown: &mut Shown| {
                    let ctx = ui.ctx().clone();
                    shown.app.logic(&ctx);
                    // (The fonts `logic` installs are there from the frame after.)
                    shown.frames += 1;
                    if shown.frames <= 2 {
                        return;
                    }
                    // As `show_native` has it: the form in the window's central panel.
                    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                        shown.foot = ui.max_rect().max.y;
                        shown.app.hit.clear();
                        contents(&mut shown.app, ui, 7, &mut shown.what, shown.fill, None);
                    });
                },
                state,
            );
            harness.run_steps(6);
            let save = harness.state().app.hit.rect("windowBar", crate::forms::kit::bar::hit_id(7, "save")).expect("the bar has Save & Close");
            let notes = harness.get_by_role(Role::MultilineTextInput).rect();
            (save, notes, harness.state().foot)
        };
        for height in [780.0, 1000.0, 620.0] {
            let (save, notes, foot) = lay_out(true, egui::vec2(792.0, height));
            assert!(foot > height - 40.0, "at {height}: the panel ends at {foot}");
            // The bar at the window's head, the description down to its foot: no buttons under
            // it, and no empty band.
            assert!(save.min.y < 24.0 && save.height() == 32.0, "at {height}: the bar's button at {save:?}");
            assert!(notes.min.y > save.max.y && (foot - 16.0 - notes.max.y).abs() < 12.0, "at {height}: {notes:?}, the foot at {foot}");
        }
        // The taller the window, the more of the description; never less than its lines.
        let (tall, short) = (lay_out(true, egui::vec2(792.0, 1000.0)).1, lay_out(true, egui::vec2(792.0, 330.0)).1);
        assert!(tall.height() > 500.0 && short.height() > 80.0 && short.height() < 200.0, "{tall:?} {short:?}");
        // Not filling (a window inside the main one): the description is as tall as its lines.
        let (_, notes, foot) = lay_out(false, egui::vec2(792.0, 1000.0));
        assert!(notes.max.y < foot - 300.0 && notes.height() < 200.0, "{notes:?} {foot}");
    }

    /// The bar of an appointment's window: each of its buttons acts on the appointment of that
    /// window (whatever is selected in the main one), and what they change is saved with it.
    #[test]
    fn the_bar_acts_on_the_appointment_of_its_window() {
        let mut harness = window(app());
        let (dentist, review) = (id_of(&harness, "Dentist"), id_of(&harness, "Design review: homepage"));
        run(&mut harness, "calendar.select", json!({"id": review}));
        let other = event_named(&harness, "Design review: homepage").unwrap();
        let was = event_named(&harness, "Dentist").unwrap();
        let w = open(&mut harness, json!({"event": dentist}));
        // An appointment is saved and closed; a meeting would be sent.
        for key in ["save", "delete", "showAs", "reminder", "recurrence", "categorize", "private", "invite"] {
            assert!(bar_rect(&harness, w, key).is_some(), "{key}");
        }
        for key in ["send", "scheduling", "appointment", "checkNames", "more"] {
            assert!(bar_rect(&harness, w, key).is_none(), "{key}");
        }
        // The buttons are those of the main window's row: 32 points tall, on one line.
        let (save, private) = (bar_rect(&harness, w, "save").unwrap(), bar_rect(&harness, w, "private").unwrap());
        assert_eq!((save.height(), save.min.y, save.max.y), (32.0, private.min.y, private.max.y));
        // Private is a switch of the window's item: nothing is saved until the window is.
        bar(&mut harness, w, "private");
        assert_eq!(windows(&mut harness)[0]["dirty"], true);
        assert_eq!(event_named(&harness, "Dentist").unwrap(), was);
        // Show As, Reminder and Categorize each open a list.
        bar(&mut harness, w, "showAs");
        click_label(&mut harness, "Tentative");
        bar(&mut harness, w, "reminder");
        click_label(&mut harness, "30 minutes");
        bar(&mut harness, w, "categorize");
        click_label(&mut harness, "Blue category");
        assert_eq!(event_named(&harness, "Dentist").unwrap(), was);
        // Save & Close saves all of it, with one command, and closes the window.
        bar(&mut harness, w, "save");
        let now = event_named(&harness, "Dentist").unwrap();
        assert_eq!(
            (now.private, now.show_as, now.reminder_minutes, now.categories.as_slice()),
            (!was.private, mailcraft_model::ShowAs::Tentative, Some(30), ["Blue category".to_string()].as_slice())
        );
        assert_eq!((now.title.as_str(), now.start, now.end), (was.title.as_str(), was.start, was.end));
        assert!(harness.state().windows.is_empty());
        assert_eq!(harness.state().session.undo_label(), Some("Edit Event"));
        // What is selected in the main window was never touched.
        assert_eq!((harness.state().session.sel.event, event_named(&harness, "Design review: homepage").unwrap()), (Some(review), other));
        // Taking a category off again.
        let w = open(&mut harness, json!({"event": dentist}));
        bar(&mut harness, w, "categorize");
        click_label(&mut harness, "Blue category");
        bar(&mut harness, w, "save");
        assert!(event_named(&harness, "Dentist").unwrap().categories.is_empty());
        // Delete deletes the window's appointment, and the window goes with it.
        let w = open(&mut harness, json!({"event": dentist}));
        bar(&mut harness, w, "delete");
        assert!(event_named(&harness, "Dentist").is_none() && harness.state().windows.is_empty());
        assert!(event_named(&harness, "Design review: homepage").is_some());
        // On what was never saved, Delete gives it up.
        let w = open(&mut harness, json!({"new": "appointment", "title": "Never mind"}));
        bar(&mut harness, w, "delete");
        assert!(event_named(&harness, "Never mind").is_none() && harness.state().windows.is_empty());

        // A narrow window: labels give way, and then buttons go behind `…`.
        let mut narrow = window(app());
        narrow.set_size(egui::vec2(470.0, 700.0));
        narrow.run_steps(3);
        let lunch = id_of(&narrow, "Lunch with Dev");
        let w = open(&mut narrow, json!({"event": lunch}));
        let save = bar_rect(&narrow, w, "save").expect("the first button stays");
        assert!(bar_rect(&narrow, w, "more").is_some() && save.width() > 80.0, "{save:?}");
        // What is behind `…` still works from there.
        bar(&mut narrow, w, "more");
        click_label(&mut narrow, "Private");
        assert_eq!(windows(&mut narrow)[0]["dirty"], true);
    }

    #[test]
    fn inviting_people_makes_it_a_meeting_and_send_makes_the_meeting() {
        let mut harness = window(app());
        let count = harness.state().session.mailbox.events.len();
        let w = open(&mut harness, json!({"new": "appointment", "start": "2026-10-16T10:00"}));
        type_text(&mut harness, "Roadmap");
        assert_eq!(windows(&mut harness)[0]["title"], "New Appointment");
        let lines = |harness: &Harness| harness.query_all_by_role(egui::accesskit::Role::TextInput).count();
        let plain = lines(&harness);
        // Invite Attendees: the lines for them come up over the times, and the bar sends.
        bar(&mut harness, w, "invite");
        assert_eq!(windows(&mut harness)[0]["title"], "New Meeting");
        assert_eq!(lines(&harness), plain + 2, "a line for those required and one for those optional");
        for key in ["send", "delete", "appointment", "scheduling", "showAs", "reminder", "recurrence", "checkNames", "invite"] {
            assert!(bar_rect(&harness, w, key).is_some(), "{key}");
        }
        assert!(bar_rect(&harness, w, "save").is_none());
        // Send is the first button, and it stands out; it needs somebody to send to.
        let (send, delete) = (bar_rect(&harness, w, "send").unwrap(), bar_rect(&harness, w, "delete").unwrap());
        assert!(send.max.x <= delete.min.x);
        bar(&mut harness, w, "send");
        assert!(says(&harness, "Name somebody to invite first"));
        assert_eq!((harness.state().windows.len(), harness.state().session.mailbox.events.len()), (1, count));
        // Cancel Invitation: an appointment again; and back.
        bar(&mut harness, w, "invite");
        assert_eq!(windows(&mut harness)[0]["title"], "New Appointment");
        assert!(bar_rect(&harness, w, "save").is_some() && lines(&harness) == plain);
        bar(&mut harness, w, "invite");
        // The title, then who: the people it could be are offered as the composer offers them.
        let title = harness.get_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.value().as_deref() == Some("Roadmap")).unwrap().rect();
        click(&mut harness, title.center(), 1);
        press(&mut harness, Key::Tab);
        type_text(&mut harness, "may");
        let typed =
            |harness: &Harness, text: &str| harness.query_all_by_role(egui::accesskit::Role::TextInput).any(|n| n.value().as_deref() == Some(text));
        assert!(typed(&harness, "may"), "it went into the line for those required");
        // (Maya is offered, and Enter takes her.)
        press(&mut harness, Key::Enter);
        assert!(typed(&harness, "Maya Okafor <maya@lumen.example>; "));
        assert_eq!((harness.state().windows.len(), harness.state().session.mailbox.events.len()), (1, count), "Enter took the name");
        // Send makes the meeting and closes the window. (This calendar is on this computer:
        // nobody is written to.)
        bar(&mut harness, w, "send");
        let made = event_named(&harness, "Roadmap").expect("the meeting was made");
        assert_eq!((made.attendees.len(), made.start.iso().as_str()), (1, "2026-10-16T10:00:00"));
        assert_eq!(made.attendees[0].address, mailcraft_model::Address::new("Maya Okafor", "maya@lumen.example"));
        assert!(harness.state().windows.is_empty() && harness.state().dialog.is_none());
        // Opened again it is a meeting: Send Update, and Cancel Meeting in place of Delete.
        let w = open(&mut harness, json!({"event": made.id}));
        assert!(bar_rect(&harness, w, "send").is_some() && typed(&harness, "Maya Okafor <maya@lumen.example>"));
        // Passing through its fields with Tab changes nothing in them: nothing is offered for a
        // line nobody typed in, and there is nothing to save.
        for _ in 0..6 {
            press(&mut harness, Key::Tab);
        }
        assert!(typed(&harness, "Maya Okafor <maya@lumen.example>") && windows(&mut harness)[0]["dirty"] == false);
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty(), "it closes without a question");
        // The same for a line that holds only an address somebody in the contacts has.
        open(&mut harness, json!({"new": "meeting", "title": "Addresses", "attendees": "priya@lumen.example", "optional": "tomas@lumen.example"}));
        for _ in 0..6 {
            press(&mut harness, Key::Tab);
        }
        assert!(typed(&harness, "priya@lumen.example") && typed(&harness, "tomas@lumen.example") && windows(&mut harness)[0]["dirty"] == false);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        // Check Names: a name that is one contact's becomes that contact, and what is nobody's
        // (or several people's) is said and left.
        let w = open(&mut harness, json!({"new": "meeting", "title": "Names", "attendees": "maya; zzzz", "optional": "tomas@lumen.example"}));
        bar(&mut harness, w, "checkNames");
        assert!(typed(&harness, "Maya Okafor <maya@lumen.example>; zzzz") && typed(&harness, "tomas@lumen.example"));
        assert!(says(&harness, "matches: zzzz"));
        assert_eq!(windows(&mut harness)[0]["dirty"], true);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
    }

    #[test]
    fn closing_with_unsaved_changes_asks_first() {
        let mut harness = window(app());
        let lunch = id_of(&harness, "Lunch with Dev");
        let title = |harness: &Harness| harness.state().session.mailbox.event(lunch).unwrap().title.clone();
        #[derive(Clone, Copy, Debug)]
        enum Way {
            Escape,
            CtrlW,
            CloseButton,
        }
        let close = |harness: &mut Harness, way: Way| match way {
            Way::Escape => press(harness, Key::Escape),
            Way::CtrlW => chord(harness, ctrl(), Key::W),
            Way::CloseButton => {
                harness.get_by_label("Close window").click();
                harness.run_steps(4);
            }
        };
        for way in [Way::Escape, Way::CtrlW, Way::CloseButton] {
            // Nothing was changed: it just closes.
            open(&mut harness, json!({"event": lunch}));
            close(&mut harness, way);
            assert!(harness.state().windows.is_empty(), "{way:?}");
            // Something was: it asks, and until it is answered nothing closes and nothing is saved.
            let w = open(&mut harness, json!({"event": lunch}));
            type_text(&mut harness, " (changed)");
            close(&mut harness, way);
            assert!(asking(&mut harness) && harness.state().windows.len() == 1, "{way:?}");
            for kind in ["unsavedSave", "unsavedDiscard", "unsavedCancel"] {
                assert!(harness.state().hit.rect(kind, w).is_some(), "{way:?}: {kind}");
            }
            assert!(says(&harness, "has changes that aren't saved"));
            // Cancel: back in the window, as it was.
            answer(&mut harness, w, "unsavedCancel");
            assert!(!asking(&mut harness) && harness.state().windows.len() == 1 && windows(&mut harness)[0]["dirty"] == true, "{way:?}");
            // Don't Save: it closes, and the appointment is as it was.
            close(&mut harness, way);
            answer(&mut harness, w, "unsavedDiscard");
            assert!(harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev", "{way:?}");
            // Save: it is saved, and then it closes.
            let w = open(&mut harness, json!({"event": lunch}));
            type_text(&mut harness, " (kept)");
            close(&mut harness, way);
            answer(&mut harness, w, "unsavedSave");
            harness.run_steps(3);
            assert!(harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev (kept)", "{way:?}: {}", title(&harness));
            run(&mut harness, "calendar.edit", json!({"id": lunch, "title": "Lunch with Dev"}));
        }
        // The question's own keys: Escape stays, Enter saves.
        open(&mut harness, json!({"event": lunch}));
        type_text(&mut harness, "!");
        press(&mut harness, Key::Escape);
        assert!(asking(&mut harness));
        press(&mut harness, Key::Escape);
        assert!(!asking(&mut harness) && harness.state().windows.len() == 1 && title(&harness) == "Lunch with Dev");
        press(&mut harness, Key::Escape);
        press(&mut harness, Key::Enter);
        harness.run_steps(3);
        assert!(harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev!");

        // A new appointment that something was typed into asks too. Saving one that can't be
        // saved yet says why, and the window stays.
        let count = harness.state().session.mailbox.events.len();
        let w = open(&mut harness, json!({"new": "appointment"}));
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty(), "nothing was typed");
        let w2 = open(&mut harness, json!({"new": "appointment"}));
        assert_ne!(w, w2);
        bar(&mut harness, w2, "private");
        press(&mut harness, Key::Escape);
        answer(&mut harness, w2, "unsavedSave");
        harness.run_steps(3);
        assert!(!asking(&mut harness) && harness.state().windows.len() == 1 && says(&harness, "Give it a title first"));
        assert_eq!(harness.state().session.mailbox.events.len(), count);
        // A script that closes a window is asked nothing: it said what it meant.
        assert_eq!(run(&mut harness, "ui.closeWindow", json!({"window": w2}))["closed"], 1);
        assert!(harness.state().windows.is_empty() && harness.state().session.mailbox.events.len() == count);
        // As its close button does, by command: that asks.
        open(&mut harness, json!({"event": lunch}));
        type_text(&mut harness, "?");
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"do": "close"}))["asking"], true);
        press(&mut harness, Key::Escape);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        open(&mut harness, json!({"event": lunch}));
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"do": "close"}))["closed"], true);
        assert!(harness.state().windows.is_empty());

        // The main window closing: a window with unsaved changes asks first, and the main
        // window closes once that is answered (and stays when the answer is Cancel).
        let w = open(&mut harness, json!({"event": lunch}));
        assert!(harness.state_mut().close_requested(), "nothing to ask about");
        type_text(&mut harness, " again");
        assert!(!harness.state_mut().close_requested());
        harness.run_steps(3);
        assert!(asking(&mut harness) && !harness.state().quit_requested);
        answer(&mut harness, w, "unsavedCancel");
        harness.run_steps(3);
        assert!(!harness.state().quit_requested && harness.state().windows.len() == 1);
        assert!(!harness.state_mut().close_requested());
        harness.run_steps(3);
        answer(&mut harness, w, "unsavedDiscard");
        harness.run_steps(3);
        assert!(harness.state().quit_requested && harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev!");
    }

    #[test]
    fn ctrl_s_saves_and_stays_and_the_send_keys_send() {
        let mut harness = window(app());
        let count = harness.state().session.mailbox.events.len();
        let w = open(&mut harness, json!({"new": "appointment", "start": "2026-10-16T09:00"}));
        type_text(&mut harness, "Stand-in");
        // Ctrl+S: it is made, and the window stays, on what was made.
        chord(&mut harness, ctrl(), Key::S);
        let made = event_named(&harness, "Stand-in").expect("it was saved");
        let shown = windows(&mut harness);
        assert_eq!(
            (&shown[0]["id"], &shown[0]["kind"], &shown[0]["event"], &shown[0]["dirty"], &shown[0]["title"]),
            (&json!(w), &json!("event"), &json!(made.id), &json!(false), &json!("Stand-in - Appointment"))
        );
        assert_eq!(harness.state().session.mailbox.events.len(), count + 1);
        // More is typed and saved: the same appointment, changed.
        type_text(&mut harness, " again");
        assert_eq!(windows(&mut harness)[0]["dirty"], true);
        chord(&mut harness, ctrl(), Key::S);
        assert_eq!(harness.state().session.mailbox.event(made.id).unwrap().title, "Stand-in again");
        assert_eq!((harness.state().session.mailbox.events.len(), harness.state().windows.len()), (count + 1, 1));
        // Nothing is left to ask about.
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty());
        // By command.
        open(&mut harness, json!({"event": made.id}));
        type_text(&mut harness, "!");
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"do": "save"}))["dirty"], true);
        harness.run_steps(3);
        assert_eq!((harness.state().session.mailbox.event(made.id).unwrap().title.as_str(), harness.state().windows.len()), ("Stand-in again!", 1));
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"do": "saveAndClose"}))["dirty"], false);
        harness.run_steps(3);
        assert!(harness.state().windows.is_empty());

        // In an appointment's window, Alt+S and Ctrl+Enter save and close.
        for (n, (mods, key)) in [(egui::Modifiers::ALT, Key::S), (ctrl(), Key::Enter)].into_iter().enumerate() {
            open(&mut harness, json!({"new": "appointment"}));
            type_text(&mut harness, &format!("Quick {n}"));
            chord(&mut harness, mods, key);
            assert!(event_named(&harness, &format!("Quick {n}")).is_some() && harness.state().windows.is_empty(), "{n}");
        }
        // In a meeting's they send it. (Its calendar is on this computer: nobody is written to,
        // so nothing is asked.)
        for (n, (mods, key)) in [(egui::Modifiers::ALT, Key::S), (ctrl(), Key::Enter)].into_iter().enumerate() {
            open(&mut harness, json!({"new": "meeting", "attendees": "maya@lumen.example"}));
            type_text(&mut harness, &format!("Sync {n}"));
            chord(&mut harness, mods, key);
            let made = event_named(&harness, &format!("Sync {n}")).expect("the meeting was made");
            assert!(made.attendees.len() == 1 && harness.state().windows.is_empty() && harness.state().dialog.is_none(), "{n}");
        }
        // A letter typed with AltGr (Ctrl+Alt, to the keyboard) is a letter, not Ctrl+S; and
        // Shift+Escape is not Escape.
        open(&mut harness, json!({"new": "appointment"}));
        type_text(&mut harness, "Notes");
        let alt_gr = egui::Modifiers { alt: true, ..ctrl() };
        chord(&mut harness, alt_gr, Key::S);
        chord(&mut harness, alt_gr, Key::W);
        chord(&mut harness, egui::Modifiers::SHIFT, Key::Escape);
        assert!(event_named(&harness, "Notes").is_none() && harness.state().windows.new_events() == 1 && !asking(&mut harness));
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        // Ctrl+S on a meeting nobody is named for yet keeps it as the appointment it still is.
        open(&mut harness, json!({"new": "meeting"}));
        type_text(&mut harness, "Draft");
        chord(&mut harness, ctrl(), Key::S);
        assert!(event_named(&harness, "Draft").is_some_and(|e| e.attendees.is_empty()) && harness.state().windows.len() == 1);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
    }

    /// A calendar that an account's server keeps: what would make the server write to other
    /// people. Send on a new meeting sends (the button was pressed for that); a key, saving, and
    /// an update each ask first, the window waits for the answer, and nothing is lost when the
    /// answer is no.
    #[test]
    fn what_mails_other_people_is_sent_by_send_and_asked_about_otherwise() {
        use crate::calendar_sync::testing::{event, microsoft_account, serve};
        let mut a = app();
        let server = serve(&mut a);
        microsoft_account(&mut a);
        server.lock().unwrap().events = vec![event("e-1", "Budget review", NOW - 3 * 3600)];
        // The calendar keeps itself current: what is made here goes to the stand-in at once.
        a.auto_sync = true;
        let mut harness = window(a);
        harness.run_steps(6);
        assert!(event_named(&harness, "Budget review").is_some(), "the server's calendar came down");
        let mail = |harness: &Harness| -> Vec<String> {
            let _ = harness;
            server.lock().unwrap().mail.clone()
        };
        let meeting = |harness: &mut Harness, title: &str| {
            let w = open(harness, json!({"new": "meeting", "start": "2026-10-16T10:00", "attendees": "Maya Okafor <maya@lumen.example>"}));
            type_text(harness, title);
            w
        };

        // Send, pressed: the meeting is made in the server's calendar and the invitation goes,
        // without another question.
        let w = meeting(&mut harness, "Launch");
        bar(&mut harness, w, "send");
        assert!(harness.state().dialog.is_none() && harness.state().windows.is_empty());
        harness.run_steps(8);
        assert_eq!(mail(&harness), ["invitation:Launch:1"]);
        let launch = event_named(&harness, "Launch").unwrap();
        assert!(!launch.origin.is_local(), "it is in the calendar the server keeps");

        // The key that sends asks first; while it asks, the window waits and nothing is made.
        let w = meeting(&mut harness, "Retro");
        chord(&mut harness, ctrl(), Key::Enter);
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));
        assert!(says(&harness, "Send This Invitation?"));
        assert!(harness.state().windows.len() == 1 && event_named(&harness, "Retro").is_none());
        // No: the window is still there with what was typed, and nothing went anywhere.
        press(&mut harness, Key::Escape);
        harness.run_steps(8);
        assert!(harness.state().dialog.is_none() && harness.state().windows.len() == 1 && windows(&mut harness)[0]["dirty"] == true);
        assert!(event_named(&harness, "Retro").is_none() && mail(&harness).len() == 1);
        // Yes: it is made, the invitation goes, and the window closes.
        chord(&mut harness, ctrl(), Key::Enter);
        click_label(&mut harness, "Send");
        harness.run_steps(8);
        assert!(harness.state().windows.is_empty() && event_named(&harness, "Retro").is_some());
        assert_eq!(mail(&harness), ["invitation:Launch:1", "invitation:Retro:1"]);
        let _ = w;

        // Saving a meeting (Ctrl+S) asks too, and can keep it on this computer, unsent: the
        // window stays, on what was saved.
        meeting(&mut harness, "Offsite");
        chord(&mut harness, ctrl(), Key::S);
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));
        click_label(&mut harness, "Don't Send, Keep on This Computer");
        harness.run_steps(8);
        let offsite = event_named(&harness, "Offsite").expect("it was kept");
        assert!(offsite.origin.is_local() && harness.state().windows.of_event(offsite.id).is_some());
        assert_eq!((mail(&harness).len(), windows(&mut harness)[0]["dirty"].clone()), (2, json!(false)));
        run(&mut harness, "ui.closeWindow", json!({"all": true}));

        // A meeting that was sent: its window says Send Update, and the update is asked about
        // as it is everywhere else.
        let launch = event_named(&harness, "Launch").unwrap();
        assert!(!launch.origin.id.is_empty(), "the server has it by now");
        let w = open(&mut harness, json!({"event": launch.id}));
        type_text(&mut harness, " day");
        bar(&mut harness, w, "send");
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));
        assert!(says(&harness, "Change This Meeting?") && harness.state().windows.len() == 1);
        assert_eq!(event_named(&harness, "Launch").map(|e| e.id), Some(launch.id), "nothing is changed yet");
        press(&mut harness, Key::Escape);
        harness.run_steps(4);
        assert!(harness.state().windows.len() == 1 && mail(&harness).len() == 2);
        bar(&mut harness, w, "send");
        click_label(&mut harness, "Send Update");
        harness.run_steps(8);
        assert!(harness.state().windows.is_empty() && event_named(&harness, "Launch day").is_some());
        assert_eq!(mail(&harness).last().map(String::as_str), Some("update:Launch day:1"));

        // Closing with unsaved changes and answering Save goes the same way: asked first.
        let w = open(&mut harness, json!({"event": launch.id}));
        type_text(&mut harness, " two");
        press(&mut harness, Key::Escape);
        answer(&mut harness, w, "unsavedSave");
        harness.run_steps(3);
        assert_eq!(harness.state().dialog.as_ref().map(|d| d.name()), Some("confirm"));
        press(&mut harness, Key::Escape);
        harness.run_steps(4);
        assert!(harness.state().windows.len() == 1 && mail(&harness).len() == 3 && event_named(&harness, "Launch day").is_some());
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
    }

    #[test]
    fn several_appointment_windows_keep_their_own_state_and_nothing_hostile_breaks_one() {
        let mut harness = window(app());
        let (lunch, dentist) = (id_of(&harness, "Lunch with Dev"), id_of(&harness, "Dentist"));
        let first = open(&mut harness, json!({"event": lunch}));
        let second = open(&mut harness, json!({"event": dentist}));
        // The one in front is changed; the other is as it was, and each says so of itself.
        bar(&mut harness, second, "private");
        let shown = windows(&mut harness);
        assert_eq!(
            (&shown[0]["id"], &shown[0]["dirty"], &shown[1]["id"], &shown[1]["dirty"]),
            (&json!(first), &json!(false), &json!(second), &json!(true))
        );
        assert_eq!(harness.state().windows.unsaved(), 1);
        // By command, each by its number; without one, the one in front.
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"window": first}))["event"], lunch);
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({}))["event"], dentist);
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"window": first, "recurrence": true}))["recurrence"], true);
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"window": second}))["recurrence"], false);
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"window": first, "recurrence": false}))["recurrence"], false);
        // An appointment has no second page.
        assert!(harness.state_mut().execute("ui.eventWindow", json!({"window": first, "page": "scheduling"})).is_err());
        // Nonsense changes nothing and breaks nothing.
        let junk = "\u{0}\u{202e}é\n<b>".repeat(5_000);
        for params in [
            json!(null),
            json!([1]),
            json!({"window": "x"}),
            json!({"window": -1}),
            json!({"window": u64::MAX}),
            json!({"page": 7}),
            json!({"page": junk}),
            json!({"do": junk}),
            json!({"do": ["save"]}),
            json!({"recurrence": "yes"}),
            json!({"window": 1e300, "do": "close"}),
        ] {
            let _ = harness.state_mut().execute("ui.eventWindow", params.clone());
            let _ = harness.state_mut().run("ui.eventWindow", params);
            harness.run_steps(2);
        }
        assert_eq!(harness.state().windows.len(), 2);
        // A message window is not an appointment's.
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        let message = listed(&mut harness)[0];
        open(&mut harness, json!({"message": message}));
        assert!(harness.state_mut().execute("ui.eventWindow", json!({})).is_err());
        // Its keys: Ctrl+W closes it, and Ctrl+S is nothing of its own.
        chord(&mut harness, ctrl(), Key::W);
        assert!(harness.state().windows.is_empty());
    }

    /// Press the button at `from`, move to `to` in a few steps, and let go.
    fn drag(harness: &mut Harness, from: egui::Pos2, to: egui::Pos2) {
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        harness.event(egui::Event::PointerMoved(from));
        harness.step();
        harness.event(button(from, true));
        harness.step();
        for k in 1..=6 {
            harness.event(egui::Event::PointerMoved(from + (to - from) * (k as f32 / 6.0)));
            harness.step();
        }
        harness.event(button(to, false));
        harness.step();
        harness.run_steps(3);
    }

    /// What `ui.eventWindow` says of the Find a Time page of the window in front.
    fn scheduling(harness: &mut Harness) -> Value {
        run(harness, "ui.eventWindow", json!({}))["scheduling"].clone()
    }

    #[test]
    fn the_recurrence_dialog_opens_from_the_bar_and_its_rule_is_saved_with_the_appointment() {
        use mailcraft_model::{Freq, Recurrence};
        let mut harness = window(app());
        // Thursday the 15th, half past twelve; it happens once.
        let lunch = id_of(&harness, "Lunch with Dev");
        let rule = |harness: &Harness| harness.state().session.mailbox.event(lunch).unwrap().recurrence.clone();
        let open_dialog = |harness: &mut Harness| run(harness, "ui.eventWindow", json!({}))["recurrence"] == true;
        assert_eq!(rule(&harness), None);
        let w = open(&mut harness, json!({"event": lunch}));
        bar(&mut harness, w, "recurrence");
        assert!(says(&harness, "Appointment Recurrence") && open_dialog(&mut harness));
        for part in ["Time", "Pattern", "Range", "No end date", "Every week on Thursday"] {
            assert!(says(&harness, part), "{part}");
        }
        // Escape leaves the dialog and not the window, and nothing is changed.
        press(&mut harness, Key::Escape);
        assert!(!open_dialog(&mut harness) && harness.state().windows.len() == 1 && !asking(&mut harness));
        assert_eq!(windows(&mut harness)[0]["dirty"], false);
        // While it is open, the window's keys are the dialog's: Ctrl+S saves nothing, Ctrl+W
        // closes nothing.
        bar(&mut harness, w, "recurrence");
        chord(&mut harness, ctrl(), Key::W);
        chord(&mut harness, ctrl(), Key::S);
        assert!(open_dialog(&mut harness) && harness.state().windows.len() == 1 && rule(&harness).is_none());
        // Enter is OK: what it offered (every week, on its own day) is the item's rule now, said
        // in a line of the form; nothing is saved yet.
        press(&mut harness, Key::Enter);
        assert!(!open_dialog(&mut harness) && harness.state().windows.len() == 1);
        assert!(says(&harness, "Every week on Thursday, from Oct 15, 2026, 12:30 PM – 1:30 PM"));
        assert_eq!((windows(&mut harness)[0]["dirty"].clone(), rule(&harness)), (json!(true), None));
        // Again, from the line this time: it ends after a number of times.
        click_label(&mut harness, "Change…");
        assert!(open_dialog(&mut harness));
        click_label(&mut harness, "After");
        press(&mut harness, Key::Enter);
        assert!(!open_dialog(&mut harness) && says(&harness, "Every week on Thursday, 10 times, from Oct 15, 2026"));
        // Save & Close: one command, with the rule in it.
        bar(&mut harness, w, "save");
        let weekly = Recurrence { by_day: vec![mailcraft_time::Weekday::Thu], count: Some(10), ..Recurrence::every(Freq::Weekly) };
        assert_eq!(rule(&harness), Some(weekly));
        assert!(harness.state().windows.is_empty());
        // Remove Recurrence is there for an item that repeats.
        let w = open(&mut harness, json!({"event": lunch}));
        assert!(says(&harness, "Every week on Thursday, 10 times"));
        bar(&mut harness, w, "recurrence");
        click_label(&mut harness, "Remove Recurrence");
        assert!(!open_dialog(&mut harness) && !says(&harness, "Every week on Thursday"));
        bar(&mut harness, w, "save");
        assert_eq!(rule(&harness), None);
        // Two windows, each with its own dialog: the fields of one are not the other's.
        let dentist = id_of(&harness, "Dentist");
        let first = open(&mut harness, json!({"event": lunch}));
        let second = open(&mut harness, json!({"event": dentist}));
        run(&mut harness, "ui.eventWindow", json!({"window": first, "recurrence": true}));
        run(&mut harness, "ui.eventWindow", json!({"window": second, "recurrence": true}));
        harness.run_steps(3);
        assert!(says(&harness, "Every week on Thursday") && says(&harness, "Every week on Saturday"));
        run(&mut harness, "ui.eventWindow", json!({"window": first, "recurrence": false}));
        run(&mut harness, "ui.eventWindow", json!({"window": second, "recurrence": false}));
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
    }

    #[test]
    fn the_scheduling_assistant_shows_everybodys_day_and_moves_the_meeting() {
        use std::sync::{Arc, Mutex, mpsc};

        use mailcraft_model::ShowAs;
        use mailcraft_model::remote::{BusyTime, FreeBusy, FreeBusyAsk, PersonSchedule};

        use crate::calendar_sync::testing::{event, microsoft_account, serve};
        let mut a = app();
        let server = serve(&mut a);
        microsoft_account(&mut a);
        server.lock().unwrap().events = vec![event("e-1", "Budget review", NOW - 3 * 3600)];
        a.auto_sync = true;
        // The stand-in for the server: Maya is busy from one to two and away from four on;
        // Tomás can't be looked up; of Priya it says nothing.
        let asks: Arc<Mutex<Vec<FreeBusyAsk>>> = Default::default();
        let seen = asks.clone();
        a.services.free_busy = Some(Box::new(move |request: crate::FreeBusyRequest| {
            seen.lock().unwrap().push(request.ask.clone());
            let at = |hour: i64| request.ask.from + hour * 3600;
            let maya = PersonSchedule {
                email: "maya@lumen.example".into(),
                busy: vec![
                    BusyTime { start: at(16), end: at(40), show_as: ShowAs::OutOfOffice },
                    BusyTime { start: at(13), end: at(14), show_as: ShowAs::Busy },
                ],
                ..Default::default()
            };
            let tomas =
                PersonSchedule { email: "tomas@lumen.example".into(), error: "their time couldn't be looked up".into(), ..Default::default() };
            let said = FreeBusy { from: request.ask.from, to: request.ask.to, people: vec![maya, tomas] };
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(crate::FreeBusyResult { account: request.account.id, schedule: Some(said), ..Default::default() });
            rx
        }));
        let mut harness = window(a);
        harness.run_steps(6);
        let w = open(
            &mut harness,
            json!({"new": "meeting", "title": "Sync", "start": "2026-10-15T10:00", "end": "2026-10-15T11:00",
                "attendees": "Maya Okafor <maya@lumen.example>; tomas@lumen.example", "optional": "priya@lumen.example; nobody"}),
        );
        // While the Appointment page is in view nobody is asked anything.
        harness.run_steps(10);
        assert!(asks.lock().unwrap().is_empty() && scheduling(&mut harness).is_null());
        // Find a Time: one question, for the day of the meeting and the people
        // invited to it.
        bar(&mut harness, w, "scheduling");
        harness.run_steps(6);
        let midnight = NOW - 16 * 3600 + 86_400;
        let people: Vec<String> = ["maya@lumen.example", "tomas@lumen.example", "priya@lumen.example"].map(String::from).to_vec();
        assert_eq!(*asks.lock().unwrap(), [FreeBusyAsk { people, from: midnight, to: midnight + 86_400 }]);
        let state = scheduling(&mut harness);
        assert_eq!(
            (&state["day"], &state["start"], &state["end"], &state["pending"], &state["note"]),
            (&json!("2026-10-15"), &json!("2026-10-15T10:00:00"), &json!("2026-10-15T11:00:00"), &json!(false), &json!(""))
        );
        // A row for everybody together, the organizer, and each of the four; Maya's day is
        // known, the others' are not.
        assert_eq!((&state["rows"], &state["known"], &state["unknown"]), (&json!(6), &json!(1), &json!(3)));
        // Maya's two stretches, and what the owner's own calendar has that day.
        assert!(state["bars"].as_u64().unwrap() >= 3, "{state}");
        // However long the page stays in view: the one question.
        harness.run_steps(60);
        assert_eq!(asks.lock().unwrap().len(), 1);

        // The meeting's own time is a band across the rows. Dragged, it moves; by its edges it
        // starts or ends at another time; the half hours hold it.
        let rect = |harness: &Harness, kind: &str| harness.state().hit.rect(kind, w).unwrap_or_else(|| panic!("no {kind}"));
        let band = rect(&harness, "schedulingMove");
        assert!((band.width() - 60.0).abs() < 1.0, "an hour is sixty points: {band:?}");
        drag(&mut harness, band.center(), band.center() + vec2(64.0, 3.0));
        let state = scheduling(&mut harness);
        assert_eq!((&state["start"], &state["end"]), (&json!("2026-10-15T11:00:00"), &json!("2026-10-15T12:00:00")));
        let end = rect(&harness, "schedulingEnd");
        drag(&mut harness, end.center(), end.center() + vec2(33.0, 0.0));
        assert_eq!(scheduling(&mut harness)["end"], "2026-10-15T12:30:00");
        let start = rect(&harness, "schedulingStart");
        drag(&mut harness, start.center(), start.center() - vec2(58.0, 0.0));
        let state = scheduling(&mut harness);
        assert_eq!((&state["start"], &state["end"]), (&json!("2026-10-15T10:00:00"), &json!("2026-10-15T12:30:00")));
        // It can't be made to end before it starts.
        let end = rect(&harness, "schedulingEnd");
        drag(&mut harness, end.center(), end.center() - vec2(400.0, 0.0));
        assert_eq!(scheduling(&mut harness)["end"], "2026-10-15T10:30:00");
        // A click on the grid puts the meeting there, as long as it is.
        let (band, grid) = (rect(&harness, "schedulingMove"), rect(&harness, "schedulingGrid"));
        click(&mut harness, egui::pos2(band.min.x - 95.0, grid.min.y + 20.0), 1);
        let state = scheduling(&mut harness);
        assert_eq!((&state["start"], &state["end"]), (&json!("2026-10-15T08:00:00"), &json!("2026-10-15T08:30:00")));
        // The same day and the same people: nothing more was asked for any of it.
        assert_eq!(asks.lock().unwrap().len(), 1);
        // On the Appointment page the fields say what the grid was told, and nobody is asked.
        bar(&mut harness, w, "appointment");
        let typed =
            |harness: &Harness, text: &str| harness.query_all_by_role(egui::accesskit::Role::TextInput).any(|n| n.value().as_deref() == Some(text));
        assert!(typed(&harness, "8:00 AM") && typed(&harness, "8:30 AM"));
        assert!(windows(&mut harness)[0]["dirty"] == true && scheduling(&mut harness).is_null());
        harness.run_steps(40);
        assert_eq!(asks.lock().unwrap().len(), 1);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));

        // A calendar on this computer has no server to ask: the owner's own day, and why.
        let own = harness.state().session.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
        let crowd: String = (0..3_000).map(|i| format!("p{i}@example.org; ")).collect();
        open(&mut harness, json!({"new": "meeting", "title": "Everybody", "calendar": own, "start": "2026-10-15T10:00", "attendees": crowd}));
        assert_eq!(run(&mut harness, "ui.eventWindow", json!({"page": "scheduling"}))["page"], "scheduling");
        harness.run_steps(4);
        let state = scheduling(&mut harness);
        assert!(state["note"].as_str().unwrap().contains("kept on this computer"), "{state}");
        // (Five hundred of them have a row; the window stays quick.)
        assert_eq!((&state["rows"], &state["known"], &state["unknown"]), (&json!(502), &json!(0), &json!(500)));
        assert_eq!(asks.lock().unwrap().len(), 1);
        // A script stands in for the server, with whatever it likes: thousands of people,
        // stretches out of order and far outside the day.
        let stretches: Vec<Value> = (0..3_000i64)
            .map(|i| json!({"start": midnight + (i * 7_919) % 400_000 - 150_000, "end": midnight + (i * 104_729) % 300_000, "showAs": "busy"}))
            .collect();
        let mut said: Vec<Value> = (0..5).map(|i| json!({"email": format!("p{i}@example.org"), "busy": stretches})).collect();
        said.extend(
            (0..4_000)
                .map(|i| json!({"email": format!("stranger{i}@example.org"), "busy": [{"start": 0, "end": i64::MAX, "showAs": "outOfOffice"}]})),
        );
        let state = run(&mut harness, "ui.eventWindow", json!({"freeBusy": {"people": said}}));
        harness.run_steps(4);
        let state = if state.is_object() { scheduling(&mut harness) } else { state };
        assert_eq!((&state["known"], &state["unknown"], &state["note"]), (&json!(5), &json!(495), &json!("")), "{state}");
        for junk in [json!(7), json!({"people": "x"}), json!([[]])] {
            assert!(harness.state_mut().execute("ui.eventWindow", json!({"freeBusy": junk})).is_err());
        }
        // In a small window, and with the meeting far outside the day shown, it still lays out.
        harness.set_size(vec2(360.0, 240.0));
        harness.run_steps(4);
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
        open(&mut harness, json!({"new": "meeting", "start": "9999-12-31T23:30", "end": "9999-12-31T23:59", "attendees": "maya@lumen.example"}));
        run(&mut harness, "ui.eventWindow", json!({"page": "scheduling"}));
        harness.run_steps(4);
        open(&mut harness, json!({"new": "meeting", "start": "2026-10-15", "end": "2026-10-17", "allDay": true, "attendees": "maya@lumen.example"}));
        run(&mut harness, "ui.eventWindow", json!({"page": "scheduling"}));
        harness.run_steps(4);
        let whole_days = windows(&mut harness)[1]["id"].as_u64().unwrap();
        assert!(harness.state().hit.rect("schedulingMove", whole_days).is_none(), "whole days aren't dragged");
        run(&mut harness, "ui.closeWindow", json!({"all": true}));
    }

    /// Where the system gives each window a place of its own, a window is laid out by another
    /// path than inside the main window (`show_native`). The headless window has no windows of
    /// the system, and with embedding switched off egui lays the window out inside after all,
    /// through that same path: the bar, the keys, the question about unsaved changes and the
    /// recurrence dialog work there as they do inside the main window.
    #[test]
    fn a_window_of_the_system_asks_and_saves_the_same_way() {
        let mut harness = window(app());
        harness.ctx.set_embed_viewports(false);
        harness.run_steps(2);
        let lunch = id_of(&harness, "Lunch with Dev");
        let title = |harness: &Harness| harness.state().session.mailbox.event(lunch).unwrap().title.clone();
        let w = open(&mut harness, json!({"event": lunch}));
        assert_eq!(windows(&mut harness)[0]["embedded"], false);
        for key in ["save", "delete", "showAs", "reminder", "recurrence", "categorize", "private", "invite"] {
            assert!(bar_rect(&harness, w, key).is_some(), "{key}");
        }
        // Nothing changed: Escape just closes.
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.is_empty());
        // Something changed: Escape and Ctrl+W ask; Cancel stays, Don't Save closes unchanged.
        let w = open(&mut harness, json!({"event": lunch}));
        type_text(&mut harness, " (changed)");
        assert_eq!(windows(&mut harness)[0]["dirty"], true);
        press(&mut harness, Key::Escape);
        assert!(asking(&mut harness) && harness.state().windows.len() == 1 && says(&harness, "has changes that aren't saved"));
        answer(&mut harness, w, "unsavedCancel");
        assert!(!asking(&mut harness) && harness.state().windows.len() == 1);
        chord(&mut harness, ctrl(), Key::W);
        assert!(asking(&mut harness));
        answer(&mut harness, w, "unsavedDiscard");
        assert!(harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev");
        // The question's keys: Escape stays, Enter saves and closes.
        open(&mut harness, json!({"event": lunch}));
        type_text(&mut harness, "!");
        press(&mut harness, Key::Escape);
        press(&mut harness, Key::Escape);
        assert!(!asking(&mut harness) && harness.state().windows.len() == 1 && title(&harness) == "Lunch with Dev");
        press(&mut harness, Key::Escape);
        press(&mut harness, Key::Enter);
        harness.run_steps(3);
        assert!(harness.state().windows.is_empty() && title(&harness) == "Lunch with Dev!");
        // Ctrl+S saves and stays; the recurrence dialog opens over the window and Escape leaves
        // it alone; Save & Close closes.
        let w = open(&mut harness, json!({"event": lunch}));
        type_text(&mut harness, "!");
        chord(&mut harness, ctrl(), Key::S);
        assert!(title(&harness) == "Lunch with Dev!!" && harness.state().windows.len() == 1 && windows(&mut harness)[0]["dirty"] == false);
        bar(&mut harness, w, "recurrence");
        assert!(says(&harness, "Appointment Recurrence"));
        press(&mut harness, Key::Escape);
        assert!(harness.state().windows.len() == 1 && !says(&harness, "Appointment Recurrence") && !asking(&mut harness));
        bar(&mut harness, w, "private");
        bar(&mut harness, w, "save");
        assert!(harness.state().windows.is_empty());
        // A new meeting: Send needs somebody, and the main window closing waits for the answer.
        let w = open(&mut harness, json!({"new": "meeting"}));
        type_text(&mut harness, "Kick-off");
        bar(&mut harness, w, "send");
        assert!(says(&harness, "Name somebody to invite first") && harness.state().windows.len() == 1);
        assert!(!harness.state_mut().close_requested());
        harness.run_steps(3);
        answer(&mut harness, w, "unsavedDiscard");
        harness.run_steps(3);
        assert!(harness.state().quit_requested && harness.state().windows.is_empty());
    }

    /// A recurring item is asked about when it is opened (just this one, or the entire series?),
    /// and what its window saves is saved for what was answered, without asking again, whatever
    /// was changed in it and whatever is selected by then.
    #[test]
    fn what_a_recurring_item_was_opened_as_is_what_its_window_saves() {
        let mut harness = window(app());
        run(&mut harness, "view.calendar", json!({}));
        let standup = id_of(&harness, "Team stand-up");
        let series = event_named(&harness, "Team stand-up").unwrap();
        let events = |harness: &Harness| harness.state().session.mailbox.events.len();
        let count = events(&harness);
        // Friday's, opened as the calendar opens it: asked first.
        run(&mut harness, "calendar.select", json!({"id": standup, "day": "2026-10-16"}));
        assert_eq!(harness.state_mut().run("ui.openWindow", json!({"event": standup})).unwrap()["pending"], "confirm");
        harness.run_steps(3);
        assert!(says(&harness, "Open Recurring Item") && harness.state().windows.is_empty());
        click_label(&mut harness, "Just This One");
        let w = harness.state().windows.of_event(standup).expect("its window opened");
        // The window shows that day, and has no Recurrence of its own.
        let typed =
            |harness: &Harness, text: &str| harness.query_all_by_role(egui::accesskit::Role::TextInput).any(|n| n.value().as_deref() == Some(text));
        assert!(typed(&harness, "2026-10-16") && says(&harness, "One of a series") && bar_rect(&harness, w, "recurrence").is_none());
        // Something other than its title is changed, and something else is selected meanwhile.
        bar(&mut harness, w, "private");
        run(&mut harness, "calendar.select", json!({"id": null}));
        bar(&mut harness, w, "save");
        // Saved for that one, with no question: it is an event of its own that day, and the
        // series is as it was but for that day.
        assert!(harness.state().dialog.is_none() && harness.state().windows.is_empty());
        assert_eq!(events(&harness), count + 1);
        let friday = mailcraft_time::Date::new(2026, 10, 16).unwrap();
        let one = harness.state().session.mailbox.events.values().find(|e| e.title == "Team stand-up" && e.recurrence.is_none()).cloned().unwrap();
        assert_eq!((one.start.date, one.private, one.start.hour), (friday, !series.private, series.start.hour));
        let after = harness.state().session.mailbox.event(standup).cloned().unwrap();
        assert_eq!((after.private, after.start, after.exceptions.contains(&friday)), (series.private, series.start, true));
        // The entire series, the same way: the series changes, and nothing more is asked.
        run(&mut harness, "calendar.select", json!({"id": standup, "day": "2026-10-15"}));
        let _ = harness.state_mut().run("ui.openWindow", json!({"event": standup}));
        harness.run_steps(3);
        click_label(&mut harness, "The Entire Series");
        let w = harness.state().windows.of_event(standup).expect("its window opened");
        assert!(typed(&harness, &series.start.date.iso()) && bar_rect(&harness, w, "recurrence").is_some());
        bar(&mut harness, w, "private");
        run(&mut harness, "calendar.select", json!({"id": null}));
        bar(&mut harness, w, "save");
        assert!(harness.state().dialog.is_none() && harness.state().windows.is_empty());
        let after = harness.state().session.mailbox.event(standup).cloned().unwrap();
        assert_eq!((after.private, after.recurrence.is_some(), events(&harness)), (!series.private, true, count + 1));
        // Delete, from the window of one occurrence, deletes that one.
        run(&mut harness, "calendar.select", json!({"id": standup, "day": "2026-10-20"}));
        let _ = harness.state_mut().run("ui.openWindow", json!({"event": standup}));
        harness.run_steps(3);
        click_label(&mut harness, "Just This One");
        let w = harness.state().windows.of_event(standup).expect("its window opened");
        bar(&mut harness, w, "delete");
        let after = harness.state().session.mailbox.event(standup).cloned().expect("the series is still there");
        assert!(after.exceptions.contains(&mailcraft_time::Date::new(2026, 10, 20).unwrap()) && harness.state().windows.is_empty());
    }
}
