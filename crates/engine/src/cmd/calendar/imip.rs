//! Meetings the owner organizes by mail: for an account whose calendar no server keeps (a plain
//! IMAP and SMTP account), the invitations, updates and cancellations of a meeting in a calendar
//! on this computer go out as iCalendar messages from the Outbox, as RFC 5546 (iTIP) says what
//! they hold and RFC 6047 (iMIP) how they travel: `METHOD:REQUEST` for an invitation or an update,
//! `METHOD:CANCEL` for a cancellation (to everyone, or to the people who were taken off the
//! meeting), each with the meeting's `UID` and a `SEQUENCE` that counts up with every update.
//! The answers come back as `REPLY` and `COUNTER` messages, which `calendar.takeReply` takes
//! into the meeting's list of people and proposals; a proposal the owner turns down is answered
//! with `DECLINECOUNTER` (`calendar.declineProposal`).
//!
//! **Nothing here goes unless the command was told `"send": true`.** A message is only put into
//! the Outbox; the host sends it at the next Send/Receive, as any other mail. What the owner made
//! of a meeting for their own use (the reminder, categories, the proposals they were sent) stays
//! here.

use mailcraft_ical::WriteOptions;
use mailcraft_model::{Account, AccountKind, Address, Attachment, Attendee, Body, Event, FolderKind, Id, Message, Origin, Response};
use mailcraft_time::{Date, DateTime};

use super::super::{fail, remote};
use crate::{CmdError, Session};

/// What a message about a meeting the owner organizes says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// The first invitation (to everyone, or to people who were added).
    Invite,
    /// The meeting changed: everybody hears of it again.
    Update,
    /// It is called off (for everyone, or for the people who were taken off it).
    Cancel,
}

/// The longest title that goes into a subject.
const MAX_SUBJECT: usize = 200;

/// Whether an account can send a meeting by mail: it has a mail server, and an Outbox.
fn sends(s: &Session, account: &Account) -> bool {
    account.kind == AccountKind::Imap && s.mailbox.folder_of_kind(account.id, FolderKind::Outbox).is_some()
}

/// Whether an address is one of the owner's own accounts'.
fn is_mine(s: &Session, who: &Address) -> bool {
    let email = who.email.trim();
    !email.is_empty() && s.mailbox.accounts.iter().any(|account| account.email.trim().eq_ignore_ascii_case(email))
}

/// The account a meeting kept on this computer is organized from, by mail: the one whose address
/// is its organizer's; for a meeting that names none, or names an account of the owner's that has
/// no mail server (the mailbox's own, which nobody can write to), the mailbox's first account
/// that can send (its default one first). `None` when no account here can send, or when the
/// meeting's organizer is somebody else.
pub(super) fn sender(s: &Session, e: &Event) -> Option<Account> {
    let organizer = e.organizer.as_ref().filter(|who| !who.email.trim().is_empty());
    if let Some(who) = organizer {
        let named = s.mailbox.accounts.iter().find(|a| a.email.trim().eq_ignore_ascii_case(who.email.trim()) && sends(s, a));
        if named.is_some() || !is_mine(s, who) {
            return named.cloned();
        }
    }
    let default = s.mailbox.default_account().filter(|a| sends(s, a));
    default.or_else(|| s.mailbox.accounts.iter().find(|a| sends(s, a))).cloned()
}

/// Whether `e` is a meeting the owner organizes by mail: it is kept on this computer (no
/// account's server keeps it), it has people invited, and it is not somebody else's.
pub(super) fn by_mail(s: &Session, e: &Event) -> bool {
    remote::keeper(&s.mailbox, &e.origin).is_none()
        && !e.attendees.is_empty()
        && e.organizer.as_ref().is_none_or(|who| who.email.trim().is_empty() || is_mine(s, who))
}

/// Why a meeting kept here can't be sent by mail; `None` when it can.
pub(super) fn refusal(s: &Session, e: &Event) -> Option<String> {
    if !by_mail(s, e) {
        return Some("Somebody else organizes this meeting: only its organizer sends it to people.".into());
    }
    if sender(s, e).is_none() {
        return Some("No account here has a mail server to send the invitation through.".into());
    }
    None
}

/// The `RECURRENCE-ID` an occurrence of a series is named by in mail: the moment the series
/// would have it start, in UTC (`20261026T140000Z`), or for a series of whole days its day
/// (`20261026`).
pub(super) fn recurrence_id(s: &Session, series: &Event, day: Date) -> String {
    if series.all_day {
        return format!("{:04}{:02}{:02}", day.year, day.month, day.day);
    }
    let instant = remote::occurrence_instant(series, day, &remote::offset_of(s));
    let t = DateTime::from_timestamp(instant, 0);
    format!("{:04}{:02}{:02}T{:02}{:02}{:02}Z", t.date.year, t.date.month, t.date.day, t.hour, t.minute, t.second)
}

/// Give a meeting that is about to go by mail a `UID` of its own, when it has none yet: one that
/// is unique in the world, as RFC 5545 asks, made the way a message's id is. Answers it.
pub(super) fn ensure_uid(s: &mut Session, id: Id) -> Result<String, CmdError> {
    let e = s.mailbox.event(id).ok_or_else(|| fail("the event is gone"))?;
    if !e.uid.trim().is_empty() {
        return Ok(e.uid.trim().to_string());
    }
    let domain = sender(s, e).map(|a| a.email.rsplit_once('@').map_or(String::new(), |(_, d)| d.to_string())).unwrap_or_default();
    let uid = mailcraft_mime::new_message_id(&domain, s.now(), s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ id.rotate_left(29) ^ 0x6d63_6d74);
    s.mailbox.event_mut(id)?.uid = uid.clone();
    Ok(uid)
}

/// The meeting as the people invited are sent it: organized from `from`, without what the owner
/// keeps of it for their own use.
fn as_sent(e: &Event, from: &Account) -> Event {
    Event {
        organizer: Some(from.address()),
        categories: Vec::new(),
        reminder_minutes: None,
        proposals: Vec::new(),
        origin: Origin::default(),
        ..e.clone()
    }
}

/// One line of text from an event: no control characters, cut to `max` characters.
fn one_line(text: &str, max: usize) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(max).collect::<String>().trim().to_string()
}

/// When a meeting is, in words: `Monday, October 12, 2026 9:00 AM – 10:00 AM`.
pub(super) fn when(s: &Session, e: &Event) -> String {
    let twelve = s.prefs.twelve_hour;
    if e.all_day {
        let last = e.end.add_minutes(-1).date.max(e.start.date);
        return if last == e.start.date { e.start.date.long() } else { format!("{} – {}", e.start.date.long(), last.long()) };
    }
    let until = if e.end.date == e.start.date { e.end.time_short(twelve) } else { format!("{} {}", e.end.date.long(), e.end.time_short(twelve)) };
    let mut said = format!("{} {} – {until}", e.start.date.long(), e.start.time_short(twelve));
    if let Some(rule) = &e.recurrence {
        said = format!("{said} ({})", rule.describe(e.start.date));
    }
    said
}

/// The message that sends `e` (as it is now) to `to` with `method`, from the Outbox of `from`.
/// `instance` names one occurrence of a series (`RECURRENCE-ID`); `lead` is the line the text
/// begins with.
#[allow(clippy::too_many_arguments)]
pub(super) fn message(
    s: &Session,
    e: &Event,
    from: &Account,
    method: &str,
    to: &[Address],
    cc: &[Address],
    subject: String,
    lead: &str,
    note: &str,
) -> Result<Message, CmdError> {
    let outbox = s.mailbox.folder_of_kind(from.id, FolderKind::Outbox).ok_or_else(|| fail("this account has no Outbox folder"))?;
    let now = s.now();
    let ics = mailcraft_ical::write_in(&[e], &[], &WriteOptions { method: method.into(), now }, Some(s.zone()));
    let mut text = if note.is_empty() { String::new() } else { format!("{note}\n\n") };
    text.push_str(&format!("{lead}\n\n{}\n{}\n", one_line(&e.title, MAX_SUBJECT), when(s, e)));
    let place = one_line(&e.location, 500);
    if !place.is_empty() {
        text.push_str(&format!("{place}\n"));
    }
    let domain = from.email.rsplit_once('@').map_or("", |(_, domain)| domain);
    let seed = s.rev().wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ e.id.rotate_left(19) ^ u64::from(e.sequence).rotate_left(41) ^ method.len() as u64;
    Ok(Message {
        account: from.id,
        folder: outbox,
        message_id: mailcraft_mime::new_message_id(domain, now, seed),
        from: from.address(),
        to: to.to_vec(),
        cc: cc.to_vec(),
        subject,
        date: now,
        received: now,
        body: Body { text, html: None },
        // The meeting is the message in another form, beside the text, where the people's
        // programs look for it (`mailcraft-mime` writes it `text/calendar; method=…`).
        attachments: vec![Attachment { inline: true, ..Attachment::new("invite.ics", "text/calendar", ics.into_bytes()) }],
        read: true,
        ..Default::default()
    })
}

/// The people of `list` who have an address, each once, never the owner.
fn people(s: &Session, list: impl IntoIterator<Item = Address>) -> Vec<Address> {
    let mut out: Vec<Address> = Vec::new();
    for who in list {
        if who.is_valid() && !is_mine(s, &who) && !out.iter().any(|x| x.same_mailbox(&who)) && out.len() < 500 {
            out.push(who);
        }
    }
    out
}

/// Put the message about the meeting `id` (as the mailbox has it now) into the Outbox: `kind`
/// to everyone invited, or to `only` when it names somebody (people added to an existing
/// meeting are invited, people taken off it are told it is cancelled for them). `day` names one
/// occurrence of a series. How many people it goes to.
pub(super) fn send(s: &mut Session, id: Id, kind: Kind, day: Option<Date>, only: &[Address], note: &str) -> Result<usize, CmdError> {
    let e = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    // (A cancellation for the people taken off may be for a meeting that has nobody left.)
    let probe = if e.attendees.is_empty() && !only.is_empty() {
        Event { attendees: only.iter().map(|who| Attendee { address: who.clone(), ..Default::default() }).collect(), ..e.clone() }
    } else {
        e.clone()
    };
    if let Some(why) = refusal(s, &probe) {
        return Err(fail(why));
    }
    let from = sender(s, &e).ok_or_else(|| fail("No account here has a mail server to send the invitation through."))?;
    let uid = ensure_uid(s, id)?;
    // The owner organizes it from that account from now on: the answers come back there.
    if e.organizer.as_ref().is_none_or(|who| !who.email.trim().eq_ignore_ascii_case(from.email.trim())) {
        s.mailbox.event_mut(id)?.organizer = Some(from.address());
    }
    // The meeting as it goes: one occurrence named as the occurrence of its series, the series
    // otherwise.
    let mut item = as_sent(&Event { uid: uid.clone(), ..e.clone() }, &from);
    if let Some(day) = day {
        let rid = recurrence_id(s, &e, day);
        item = Event { uid: format!("{uid}#{rid}"), ..as_sent(&remote::occurrence_in(&e, day, &remote::offset_of(s)), &from) };
        item.recurrence = None;
        item.exceptions.clear();
    }
    if kind == Kind::Cancel {
        item.cancelled = true;
    }
    // Those it goes to: the ones named, or everyone invited (required in To, optional in Cc).
    let (to, cc) = if only.is_empty() {
        let required = people(s, e.attendees.iter().filter(|a| !a.optional).map(|a| a.address.clone()));
        let optional = people(s, e.attendees.iter().filter(|a| a.optional).map(|a| a.address.clone()));
        (required, optional)
    } else {
        (people(s, only.iter().cloned()), Vec::new())
    };
    if to.is_empty() && cc.is_empty() {
        return Ok(0);
    }
    // A cancellation for some of the people lists them alone (RFC 5546 §3.2.5): they may be
    // off the meeting already.
    if kind == Kind::Cancel && !only.is_empty() {
        item.attendees = to.iter().map(|who| Attendee { address: who.clone(), optional: false, response: Response::None }).collect();
    }
    let title = one_line(&e.title, MAX_SUBJECT);
    let name = from.address().display().trim().to_string();
    let (method, subject, lead) = match kind {
        Kind::Invite => ("REQUEST", format!("Invitation: {title}"), format!("{name} invites you to this meeting.")),
        Kind::Update => ("REQUEST", format!("Updated: {title}"), format!("{name} has changed this meeting.")),
        Kind::Cancel => ("CANCEL", format!("Canceled: {title}"), format!("{name} has cancelled this meeting.")),
    };
    let lead = if day.is_some() && kind != Kind::Invite { format!("{lead} This is about one occurrence of a series.") } else { lead };
    let message = message(s, &item, &from, method, &to, &cc, subject, &lead, note)?;
    s.mailbox.add_message(message)?;
    Ok(to.len() + cc.len())
}

/// Put the answer to a proposal of another time that the owner turns down into the Outbox: a
/// `DECLINECOUNTER` to the one who proposed it (RFC 5546 §3.2.8), with the meeting as it stays
/// (`day`: the occurrence of a series it was about).
pub(super) fn decline_counter(s: &mut Session, id: Id, day: Option<Date>, to: &Address, proposed: &str, note: &str) -> Result<(), CmdError> {
    let e = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    if let Some(why) = refusal(s, &e) {
        return Err(fail(why));
    }
    let from = sender(s, &e).ok_or_else(|| fail("No account here has a mail server to send the answer through."))?;
    let uid = ensure_uid(s, id)?;
    let mut item = as_sent(&Event { uid: uid.clone(), ..e.clone() }, &from);
    if let Some(day) = day {
        let rid = recurrence_id(s, &e, day);
        item = Event { uid: format!("{uid}#{rid}"), ..as_sent(&remote::occurrence_in(&e, day, &remote::offset_of(s)), &from) };
        item.recurrence = None;
        item.exceptions.clear();
    }
    // The one it answers, as the meeting lists them.
    let listed = e.attendees.iter().find(|a| a.address.same_mailbox(to)).cloned();
    item.attendees = vec![listed.unwrap_or(Attendee { address: to.clone(), optional: false, response: Response::None })];
    let title = one_line(&e.title, MAX_SUBJECT);
    let name = from.address().display().trim().to_string();
    let lead = format!("{name} has declined the other time you proposed ({proposed}). The meeting stays as it is.");
    let message = message(s, &item, &from, "DECLINECOUNTER", std::slice::from_ref(to), &[], format!("Proposal declined: {title}"), &lead, note)?;
    s.mailbox.add_message(message)?;
    Ok(())
}

/// Count the meeting's `SEQUENCE` up: it is about to be sent again with a change (RFC 5546
/// §2.1.4). When it is rescheduled, everyone's answer is asked for again.
pub(super) fn count_up(s: &mut Session, id: Id, rescheduled: bool) -> Result<(), CmdError> {
    let e = s.mailbox.event_mut(id)?;
    e.sequence = e.sequence.saturating_add(1);
    if rescheduled {
        for a in &mut e.attendees {
            a.response = Response::None;
        }
    }
    Ok(())
}

/// Whether a change moved the meeting in time (its start, its end, how it repeats): those
/// invited are then asked again.
pub(super) fn rescheduled(before: &Event, after: &Event) -> bool {
    before.start != after.start
        || before.end != after.end
        || before.all_day != after.all_day
        || before.recurrence != after.recurrence
        || before.start_zone != after.start_zone
        || before.end_zone != after.end_zone
}

/// The people of `after` who were not invited in `before`, and those of `before` who are no
/// longer in `after`.
pub(super) fn added_and_dropped(before: &Event, after: &Event) -> (Vec<Address>, Vec<Address>) {
    let has = |e: &Event, who: &Address| e.attendees.iter().any(|a| a.address.same_mailbox(who));
    let added = after.attendees.iter().map(|a| a.address.clone()).filter(|who| !has(before, who)).collect();
    let dropped = before.attendees.iter().map(|a| a.address.clone()).filter(|who| !has(after, who)).collect();
    (added, dropped)
}

/// After a change to a meeting the owner organizes by mail (`before` as it was; the mailbox has
/// it as it is now), with the owner's yes: whoever was taken off is told it is cancelled for them;
/// when the change is one people see, everyone still invited gets the update (and a meeting that
/// had nobody invited before gets its first invitation), else only the people who were added get
/// the invitation. How many people were written to.
pub(super) fn after_change(s: &mut Session, before: &Event, note: &str) -> Result<usize, CmdError> {
    let id = before.id;
    let after = s.mailbox.event(id).cloned().ok_or_else(|| fail("the event is gone"))?;
    // A meeting that never went by mail (it has no `UID` yet) has nobody to update: everyone
    // invited now gets the first invitation.
    if before.uid.trim().is_empty() {
        return if after.attendees.is_empty() { Ok(0) } else { send(s, id, Kind::Invite, None, &[], note) };
    }
    let (added, dropped) = added_and_dropped(before, &after);
    let seen = remote::seen_by_others(&remote::changed_fields(before, &after, &remote::offset_of(s)));
    // Whether the people are all that changed.
    let only_people = Event { attendees: after.attendees.clone(), ..before.clone() } == after;
    let update = seen && !only_people && !before.attendees.is_empty() && !after.attendees.is_empty();
    if update || !dropped.is_empty() {
        count_up(s, id, update && rescheduled(before, &after))?;
    }
    let mut told = 0;
    if !dropped.is_empty() {
        told += send(s, id, Kind::Cancel, None, &dropped, note)?;
    }
    if after.attendees.is_empty() {
        return Ok(told);
    }
    if update {
        told += send(s, id, Kind::Update, None, &[], note)?;
    } else if seen && before.attendees.is_empty() {
        told += send(s, id, Kind::Invite, None, &[], note)?;
    } else if !added.is_empty() {
        told += send(s, id, Kind::Invite, None, &added, note)?;
    }
    Ok(told)
}
