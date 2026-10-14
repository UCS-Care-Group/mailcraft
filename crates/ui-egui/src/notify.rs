//! Notices from the operating system: new mail and reminders that come due while the window is
//! elsewhere ([`Services::notify`](crate::Services::notify)).
//!
//! The host shows them the way its system does (the freedesktop notification service on Linux
//! and the BSDs, a toast on Windows, the Notification Center on macOS); the window only says what
//! they say. What they say is kept short on purpose, because the system shows it over whatever
//! is on screen, on a locked screen too: for new mail how many messages came, and the newest
//! one's sender and subject only when the owner switched that on in File › Options; never a
//! word of a message's body. A reminder says its appointment's subject and time. When the window
//! is in front nothing is sent: the owner sees it there. A notice the system won't show is the
//! host's to log quietly; it is never an error to the owner.

use mailcraft_engine::Session;
use mailcraft_engine::cmd::view::Reminder;
use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use crate::MailApp;

/// What a notice is about, for a host that files them differently (a system that groups or
/// silences notices by kind).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    NewMail,
    Reminder,
}

/// One notice for the operating system to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub kind: NoticeKind,
    /// The first line: `3 new messages`, or the appointment's subject.
    pub title: String,
    /// What follows: the newest message's sender and subject (when the owner wants them), or
    /// the appointment's time. Can be empty.
    pub body: String,
}

/// Most reminders announced one by one at a time; the rest are counted in one more notice.
const REMINDERS_ONE_BY_ONE: usize = 3;
/// Longest a sender or subject is in a notice, in characters.
const MAX_FIELD: usize = 120;

/// New mail taken in during the Send/Receive that is under way, to be told in one notice when
/// it is over rather than one for each folder it brings.
#[derive(Default)]
pub struct State {
    /// How many new messages have come down so far.
    new: usize,
    /// The newest of them.
    newest: Option<Id>,
}

/// The highest message id in the mailbox: what a piece of a Send/Receive adds comes after it.
pub(crate) fn last_id(session: &Session) -> Id {
    session.mailbox.messages.keys().next_back().copied().unwrap_or(0)
}

/// Note the mail a piece of a Send/Receive brought: the unread messages after `before` in the
/// folders where mail arrives (the Inbox and the owner's own folders; not Junk E-mail, Sent
/// Items, Drafts, Deleted Items, Archive or the Outbox).
pub(crate) fn took_in(app: &mut MailApp, before: Id) {
    let mb = &app.session.mailbox;
    let arriving = |folder: Id| mb.folder(folder).is_some_and(|f| matches!(f.kind, FolderKind::Inbox | FolderKind::Custom));
    let fresh = mb.messages.range(before.saturating_add(1)..).map(|(_, m)| m).filter(|m| !m.read && arriving(m.folder));
    let mut newest = app.notices.newest.and_then(|id| mb.messages.get(&id)).map(|m| (m.received, m.id));
    let mut count = 0usize;
    for m in fresh {
        count = count.saturating_add(1);
        if newest.is_none_or(|(at, _)| m.received >= at) {
            newest = Some((m.received, m.id));
        }
    }
    app.notices.new = app.notices.new.saturating_add(count);
    app.notices.newest = newest.map(|(_, id)| id);
}

/// The Send/Receive is over: tell the system about the new mail it brought, if any, when the
/// owner wants notices and the window isn't in front.
pub(crate) fn run_over(app: &mut MailApp) {
    let State { new, newest } = std::mem::take(&mut app.notices);
    if new == 0 || !app.ui.notifications || window_in_front(app) {
        return;
    }
    let title = if new == 1 { tl!("1 new message").to_string() } else { crate::i18n::fmt(tl!("{n} new messages"), &[("n", &new.to_string())]) };
    let body = match newest.and_then(|id| app.session.mailbox.messages.get(&id)) {
        Some(m) if app.ui.notification_details => {
            let from = if m.from.name.trim().is_empty() { m.from.email.as_str() } else { m.from.name.as_str() };
            let subject = if m.subject.trim().is_empty() { tl!("(no subject)") } else { m.subject.as_str() };
            format!("{}\n{}", clip(from), clip(subject))
        }
        _ => String::new(),
    };
    send(app, Notice { kind: NoticeKind::NewMail, title, body });
}

/// Reminders that have just come due: one notice each (a few at most, and one more that counts
/// the rest), when the owner wants notices and the window isn't in front.
pub(crate) fn reminders(app: &MailApp, due: &[Reminder]) {
    if due.is_empty() || !app.ui.notifications || window_in_front(app) {
        return;
    }
    let today = app.session.local_now().date;
    let twelve = app.session.prefs.twelve_hour;
    for r in due.iter().take(REMINDERS_ONE_BY_ONE) {
        let at = if r.all_day { tl!("All day").to_string() } else { r.start.time_short(twelve) };
        let at = if r.start.date == today { at } else { format!("{}, {at}", crate::i18n::date_long(r.start.date)) };
        let title = if r.title.trim().is_empty() { tl!("(no subject)").to_string() } else { clip(&r.title) };
        send(app, Notice { kind: NoticeKind::Reminder, title, body: format!("{at} ({})", crate::reminders::when(r.minutes)) });
    }
    if let Some(more) = due.len().checked_sub(REMINDERS_ONE_BY_ONE).filter(|n| *n > 0) {
        let title = crate::i18n::fmt(tl!("{n} more reminders"), &[("n", &more.to_string())]);
        send(app, Notice { kind: NoticeKind::Reminder, title, body: String::new() });
    }
}

/// Whether the owner is looking at the window. Without one (scripts, tests without a window)
/// nobody is.
fn window_in_front(app: &MailApp) -> bool {
    app.ctx.as_ref().is_some_and(|ctx| ctx.input(|i| i.focused))
}

fn send(app: &MailApp, notice: Notice) {
    if let Some(notify) = &app.services.notify {
        notify(notice);
    }
}

/// One line of at most [`MAX_FIELD`] characters: a sender or a subject can be anything.
fn clip(s: &str) -> String {
    let line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= MAX_FIELD {
        return line;
    }
    let mut cut: String = line.chars().take(MAX_FIELD - 1).collect();
    cut.push('…');
    cut
}

/// `ui.notifications` and `ui.notificationDetails`: File › Options.
pub(crate) fn ui_command(app: &mut MailApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let value = p.get("value").and_then(Value::as_bool);
    match id {
        "ui.notifications" => {
            app.ui.notifications = value.unwrap_or(!app.ui.notifications);
            Some(Ok(json!({"value": app.ui.notifications, "available": app.services.notify.is_some()})))
        }
        "ui.notificationDetails" => {
            app.ui.notification_details = value.unwrap_or(!app.ui.notification_details);
            Some(Ok(json!({"value": app.ui.notification_details})))
        }
        _ => None,
    }
}

/// The switches in File › Options.
pub(crate) fn options(app: &mut MailApp, ui: &mut egui::Ui) {
    let available = app.services.notify.is_some();
    let mut on = app.ui.notifications;
    let switch = ui.add_enabled(available, egui::Checkbox::new(&mut on, tl!("Show a notification for new mail and reminders")));
    if switch.on_hover_text(tl!("Only while MailCraft's window isn't in front. A notification for new mail says how many messages came.")).changed() {
        let _ = app.run("ui.notifications", json!({"value": on}));
    }
    ui.indent("notification_details", |ui| {
        let mut details = app.ui.notification_details;
        let switch = ui.add_enabled(available && on, egui::Checkbox::new(&mut details, tl!("Include the sender and subject of the newest message")));
        let tip = tl!("Notifications can show on a locked screen and over whatever is on yours. The message's text is never shown.");
        if switch.on_hover_text(tip).changed() {
            let _ = app.run("ui.notificationDetails", json!({"value": details}));
        }
    });
    if !available {
        ui.weak(tl!("This version of MailCraft can't show notifications here."));
    }
}

#[cfg(test)]
mod tests;
