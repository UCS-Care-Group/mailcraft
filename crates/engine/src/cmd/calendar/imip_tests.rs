//! Meetings the owner organizes by mail (`imip.rs`) and the answers that come back
//! (`tracking.rs`): what goes into the Outbox, when, and to whom; what is taken from a reply.

use mailcraft_model::{Address, Attachment, FolderKind, Id, Message, Response};
use mailcraft_time::{Date, DateTime, Timestamp};
use serde_json::{Value, json};

use super::{Mail, mail_by_outbox, mail_of};
use crate::{CmdError, Session};

/// Wednesday 2026-10-14 16:00, by a clock in UTC.
fn now() -> Timestamp {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

/// The sample mailbox with an account on a mail server, `ada@example.org`.
fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(CmdError::Failed(why)) => why,
        other => panic!("{id} {params}: expected a refusal, got {other:?}"),
    }
}

/// What waits in the account's Outbox: subject, recipients (To, then Cc), the calendar part.
fn outbox(s: &Session, account: Id) -> Vec<(String, Vec<String>, Vec<String>, String)> {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let mut waiting: Vec<&Message> = s.mailbox.messages_in(folder).map(AsRef::as_ref).collect();
    waiting.sort_by_key(|m| m.id);
    waiting
        .into_iter()
        .map(|m| {
            // (Unfolded: a long line goes on over several.)
            let part = m.attachments.first().map(|a| String::from_utf8_lossy(&a.data).replace("\r\n ", "")).unwrap_or_default();
            (m.subject.clone(), m.to.iter().map(|a| a.email.clone()).collect(), m.cc.iter().map(|a| a.email.clone()).collect(), part)
        })
        .collect()
}

/// A meeting on Thursday the 15th, 14:00–15:00, in the calendar on this computer, Bo required
/// and Cy optional; sent when `send` says so.
fn meeting(s: &mut Session, send: Option<bool>) -> Value {
    let mut params = json!({"title": "Design review", "start": "2026-10-15T14:00", "end": "2026-10-15T15:00", "location": "Room 4",
        "attendees": "Bo Marsh <bo@example.org>", "optional": "cy@example.org"});
    if let Some(send) = send {
        params["send"] = json!(send);
    }
    run(s, "calendar.newMeeting", params)
}

/// A message to Ada from `from` with a calendar part: `method` for the meeting `uid`, the one
/// attendee `attendee` with `partstat`, and `more` lines.
fn answer(s: &mut Session, account: Id, from: &str, method: &str, uid: &str, attendee: &str, partstat: &str, more: &str) -> Id {
    let times = if more.contains("DTSTART") { "" } else { "DTSTART:20261015T140000Z\r\nDTEND:20261015T150000Z\r\n" };
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Design review\r\n{times}\
         ORGANIZER;CN=Ada:mailto:ada@example.org\r\nATTENDEE;PARTSTAT={partstat}:mailto:{attendee}\r\n{more}END:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        folder: inbox,
        message_id: format!("{}.{method}@example.org", ics.len()),
        from: Address::new("", from),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Re: Design review".into(),
        attachments: vec![Attachment::new("reply.ics", "text/calendar", ics.into_bytes())],
        ..Default::default()
    };
    s.mailbox.add_message(message).unwrap()
}

fn uid_of(s: &Session, id: Id) -> String {
    s.mailbox.event(id).unwrap().uid.clone()
}

#[test]
fn a_meeting_kept_here_is_sent_by_mail_only_when_that_is_said() {
    let (mut s, account) = session();
    // Asked about first, in the words of mail from the Outbox.
    let params = json!({"title": "Design review", "attendees": "bo@example.org; cy@example.org"});
    assert_eq!(mail_of(&s, "calendar.newMeeting", &params), Some(Mail::Invitation(2)));
    assert!(mail_by_outbox(&s, "calendar.newMeeting", &params));
    // Without a yes: kept here, and nobody is written to.
    let quiet = meeting(&mut s, None)["id"].as_u64().unwrap();
    assert!(outbox(&s, account).is_empty());
    assert!(uid_of(&s, quiet).is_empty(), "a meeting that never went has no name in the world yet");
    let kept = meeting(&mut s, Some(false));
    assert!(kept.get("byMail").is_none() && outbox(&s, account).is_empty());
    // With it: one invitation, to Bo (required) and Cy (optional, in Cc), from Ada's Outbox.
    let made = meeting(&mut s, Some(true));
    assert_eq!((made["byMail"].clone(), made["mailed"].clone()), (json!(true), json!(2)));
    let id = made["id"].as_u64().unwrap();
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), 1);
    let (subject, to, cc, ics) = &sent[0];
    assert_eq!(
        (subject.as_str(), to.as_slice(), cc.as_slice()),
        ("Invitation: Design review", &["bo@example.org".to_string()][..], &["cy@example.org".to_string()][..])
    );
    let uid = uid_of(&s, id);
    assert!(uid.ends_with("@example.org") && !uid.contains('#'), "{uid}");
    for line in [
        "\r\nMETHOD:REQUEST\r\n",
        &format!("\r\nUID:{uid}\r\n"),
        "\r\nORGANIZER:mailto:ada@example.org\r\n",
        "mailto:bo@example.org",
        "ROLE=OPT-PARTICIPANT",
    ] {
        assert!(ics.contains(line), "{line} in {ics}");
    }
    // What the owner keeps for their own use doesn't go: no reminder.
    assert!(!ics.contains("VALARM") && !ics.contains("SEQUENCE"), "{ics}");
    // The owner organizes it from that account now.
    assert_eq!(s.mailbox.event(id).unwrap().organizer.as_ref().unwrap().email, "ada@example.org");
    // Written out, it is a meeting request where mail programs look for one.
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    let message = s.mailbox.messages_in(folder).next().unwrap().as_ref().clone();
    let wire = String::from_utf8(mailcraft_mime::write(&message, &mailcraft_mime::WriteOptions::default())).unwrap();
    assert!(wire.contains("\r\nContent-Type: text/calendar; method=REQUEST; charset=utf-8\r\n"), "{wire}");
    // Undo takes the message back with the meeting.
    assert!(s.undo());
    assert!(outbox(&s, account).is_empty() && s.mailbox.event(id).is_none());
}

#[test]
fn without_an_account_that_can_send_nothing_is_made() {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    let before = s.mailbox.events.len();
    let why = refused(&mut s, "calendar.newMeeting", json!({"title": "Review", "attendees": "bo@example.org", "send": true}));
    assert!(why.contains("No account here has a mail server"), "{why}");
    assert_eq!(s.mailbox.events.len(), before);
    // And nothing asks about mail that can't go.
    assert_eq!(mail_of(&s, "calendar.newMeeting", &json!({"title": "Review", "attendees": "bo@example.org"})), None);
}

#[test]
fn an_update_goes_to_everyone_and_a_cancellation_to_whoever_is_taken_off() {
    let (mut s, account) = session();
    let id = meeting(&mut s, Some(true))["id"].as_u64().unwrap();
    // Cy accepted meanwhile.
    s.mailbox.event_mut(id).unwrap().attendees[1].response = Response::Accepted;
    // An hour later: asked about as an update, sent with the next sequence, the answers asked again.
    let later = json!({"id": id, "start": "2026-10-15T15:00", "end": "2026-10-15T16:00"});
    assert_eq!(mail_of(&s, "calendar.move", &later), Some(Mail::Update(2)));
    let moved = run(&mut s, "calendar.move", json!({"id": id, "start": "2026-10-15T15:00", "send": true}));
    assert_eq!(moved["mailed"], json!(2));
    let e = s.mailbox.event(id).unwrap();
    assert_eq!(e.sequence, 1);
    assert!(e.attendees.iter().all(|a| a.response == Response::None));
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].0, "Updated: Design review");
    assert!(sent[1].3.contains("\r\nSEQUENCE:1\r\n") && sent[1].3.contains("\r\nMETHOD:REQUEST\r\n"), "{}", sent[1].3);
    // A change nobody else sees (the reminder) sends nothing, even when told to.
    run(&mut s, "calendar.edit", json!({"id": id, "reminder": 5, "send": true}));
    assert_eq!(outbox(&s, account).len(), 2);
    // Without the yes, a change is made and nobody is told.
    run(&mut s, "calendar.edit", json!({"id": id, "location": "Room 5"}));
    assert_eq!(outbox(&s, account).len(), 2);
    // Cy is taken off: Cy alone is told it is cancelled; nobody else hears of it.
    let edited = run(&mut s, "calendar.edit", json!({"id": id, "attendees": "Bo Marsh <bo@example.org>", "optional": "", "send": true}));
    assert_eq!(edited["mailed"], json!(1));
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), 3);
    let (subject, to, cc, ics) = &sent[2];
    assert_eq!((subject.as_str(), to.as_slice(), cc.is_empty()), ("Canceled: Design review", &["cy@example.org".to_string()][..], true));
    assert!(ics.contains("\r\nMETHOD:CANCEL\r\n") && ics.contains("mailto:cy@example.org") && !ics.contains("bo@example.org"), "{ics}");
    assert!(ics.contains("\r\nSEQUENCE:2\r\n") && ics.contains("STATUS:CANCELLED"), "{ics}");
    // Dan is added: Dan alone gets the invitation.
    run(&mut s, "calendar.edit", json!({"id": id, "attendees": "Bo Marsh <bo@example.org>", "optional": "dan@example.org", "send": true}));
    let sent = outbox(&s, account);
    assert_eq!((sent.len(), sent[3].0.as_str(), sent[3].1.as_slice()), (4, "Invitation: Design review", &["dan@example.org".to_string()][..]));
    // Deleted: asked about as a cancellation, which everyone still invited gets.
    assert_eq!(mail_of(&s, "calendar.delete", &json!({"id": id})), Some(Mail::Cancellation(2)));
    let gone = run(&mut s, "calendar.delete", json!({"id": id, "send": true, "comment": "Postponed."}));
    assert_eq!(gone["mailed"], json!(2));
    let sent = outbox(&s, account);
    assert_eq!(sent.len(), 5);
    assert!(sent[4].3.contains("\r\nMETHOD:CANCEL\r\n") && sent[4].3.contains("\r\nSEQUENCE:3\r\n"), "{}", sent[4].3);
    assert!(s.mailbox.event(id).is_none());
}

#[test]
fn one_occurrence_is_sent_as_that_occurrence() {
    let (mut s, account) = session();
    let made = run(
        &mut s,
        "calendar.newMeeting",
        json!({"title": "Stand-up", "start": "2026-10-15T09:00", "end": "2026-10-15T09:15", "attendees": "bo@example.org",
            "recurrence": {"freq": "daily", "count": 5}, "send": true}),
    );
    let series = made["id"].as_u64().unwrap();
    let uid = uid_of(&s, series);
    assert!(outbox(&s, account)[0].3.contains("RRULE:FREQ=DAILY;COUNT=5"));
    // Friday's is half an hour later: that occurrence alone, named by when it would have been.
    let one = run(&mut s, "calendar.move", json!({"id": series, "occurrence": "2026-10-16", "start": "2026-10-16T09:30", "send": true}));
    let split = one["id"].as_u64().unwrap();
    assert_eq!(uid_of(&s, split), format!("{uid}#20261016T090000Z"));
    let sent = outbox(&s, account);
    let ics = &sent[1].3;
    assert!(ics.contains(&format!("\r\nUID:{uid}\r\nRECURRENCE-ID:20261016T090000Z\r\n")), "{ics}");
    assert!(ics.contains("\r\nSEQUENCE:1\r\n") && !ics.contains("RRULE"), "{ics}");
    // Saturday's is called off.
    run(&mut s, "calendar.delete", json!({"id": series, "occurrence": "2026-10-17", "send": true}));
    let sent = outbox(&s, account);
    let ics = &sent[2].3;
    assert!(ics.contains("\r\nMETHOD:CANCEL\r\n") && ics.contains("RECURRENCE-ID:20261017T090000Z"), "{ics}");
    assert!(s.mailbox.event(series).unwrap().exceptions.contains(&Date::new(2026, 10, 17).unwrap()));
}

#[test]
fn an_answer_in_the_mail_is_taken_into_the_meeting_from_the_one_it_answers_for() {
    let (mut s, account) = session();
    let id = meeting(&mut s, Some(true))["id"].as_u64().unwrap();
    let uid = uid_of(&s, id);
    let reply = answer(&mut s, account, "cy@example.org", "REPLY", &uid, "cy@example.org", "ACCEPTED", "");
    let told = run(&mut s, "calendar.invitation", json!({"message": reply}));
    assert_eq!((told["kind"].clone(), told["trackable"].clone(), told["tracked"].clone()), (json!("reply"), json!(true), json!(false)));
    let taken = run(&mut s, "calendar.takeReply", json!({"message": reply}));
    assert_eq!((taken["changed"].clone(), taken["response"].clone()), (json!(true), json!("accepted")));
    assert_eq!(s.mailbox.event(id).unwrap().attendees[1].response, Response::Accepted);
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": reply}))["tracked"], json!(true));
    // Taken again: nothing changes.
    assert_eq!(run(&mut s, "calendar.takeReply", json!({"message": reply}))["changed"], json!(false));
    let tracking = run(&mut s, "calendar.tracking", json!({"id": id}));
    assert_eq!(tracking["counts"], json!({"accepted": 1, "tentative": 0, "declined": 0, "none": 1}));
    assert_eq!((tracking["byMail"].clone(), tracking["organizes"].clone()), (json!(true), json!(true)));

    // Somebody who says they answer for Bo, and isn't Bo: not taken.
    let forged = answer(&mut s, account, "mallory@example.org", "REPLY", &uid, "bo@example.org", "DECLINED", "");
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": forged}))["trackable"], json!(false));
    assert!(refused(&mut s, "calendar.takeReply", json!({"message": forged})).contains("somebody else"));
    assert_eq!(s.mailbox.event(id).unwrap().attendees[0].response, Response::None);
    // Somebody who isn't invited: not taken.
    let stranger = answer(&mut s, account, "eve@example.org", "REPLY", &uid, "eve@example.org", "ACCEPTED", "");
    assert!(refused(&mut s, "calendar.takeReply", json!({"message": stranger})).contains("isn't invited"));
    assert_eq!(s.mailbox.event(id).unwrap().attendees.len(), 2);
    // An answer to a version that was changed since: it answers something else.
    run(&mut s, "calendar.move", json!({"id": id, "start": "2026-10-15T16:00", "send": true}));
    let old = answer(&mut s, account, "bo@example.org", "REPLY", &uid, "bo@example.org", "ACCEPTED", "SEQUENCE:0\r\n");
    let stale = run(&mut s, "calendar.takeReply", json!({"message": old}));
    assert_eq!((stale["outdated"].clone(), stale["changed"].clone()), (json!(true), json!(false)));
    assert_eq!(s.mailbox.event(id).unwrap().attendees[0].response, Response::None);
    let new = answer(&mut s, account, "bo@example.org", "REPLY", &uid, "bo@example.org", "TENTATIVE", "SEQUENCE:1\r\n");
    run(&mut s, "calendar.takeReply", json!({"message": new}));
    assert_eq!(s.mailbox.event(id).unwrap().attendees[0].response, Response::Tentative);
    // A meeting somebody else organizes, or a message that isn't an answer: refused.
    let request = answer(&mut s, account, "bo@example.org", "REQUEST", "other-uid", "ada@example.org", "NEEDS-ACTION", "");
    assert!(refused(&mut s, "calendar.takeReply", json!({"message": request})).contains("isn't an answer"));
}

#[test]
fn a_proposal_by_mail_is_kept_and_can_be_turned_down() {
    let (mut s, account) = session();
    let id = meeting(&mut s, Some(true))["id"].as_u64().unwrap();
    let uid = uid_of(&s, id);
    let counter = answer(
        &mut s,
        account,
        "bo@example.org",
        "COUNTER",
        &uid,
        "bo@example.org",
        "TENTATIVE",
        "DTSTART:20261016T100000Z\r\nDTEND:20261016T110000Z\r\n",
    );
    let told = run(&mut s, "calendar.invitation", json!({"message": counter}));
    assert_eq!((told["kind"].clone(), told["trackable"].clone(), told["declinable"].clone()), (json!("counter"), json!(true), json!(true)));
    let taken = run(&mut s, "calendar.takeReply", json!({"message": counter}));
    assert_eq!(taken["proposal"], json!({"start": "2026-10-16T10:00:00", "end": "2026-10-16T11:00:00"}));
    let e = s.mailbox.event(id).unwrap();
    assert_eq!((e.proposals.len(), e.proposals[0].from.email.as_str()), (1, "bo@example.org"));
    // A COUNTER doesn't answer for the meeting as it is.
    assert_eq!(e.attendees[0].response, Response::None);
    let listed = run(&mut s, "calendar.proposals", json!({"id": id}));
    assert_eq!(listed["proposals"].as_array().unwrap().len(), 1);
    // Turned down: only with the yes, and then to Bo alone, as a DECLINECOUNTER.
    assert!(refused(&mut s, "calendar.declineProposal", json!({"message": counter})).contains("\"send\": true"));
    let declined = run(&mut s, "calendar.declineProposal", json!({"message": counter, "send": true, "comment": "That day is full."}));
    assert_eq!(declined["to"], json!("bo@example.org"));
    let sent = outbox(&s, account);
    let (subject, to, _, ics) = sent.last().unwrap();
    assert_eq!((subject.as_str(), to.as_slice()), ("Proposal declined: Design review", &["bo@example.org".to_string()][..]));
    assert!(ics.contains("\r\nMETHOD:DECLINECOUNTER\r\n") && ics.contains(&format!("\r\nUID:{uid}\r\n")), "{ics}");
    assert!(ics.contains("DTSTART:20261015T140000Z") && !ics.contains("cy@example.org"), "the meeting as it stays, to Bo alone: {ics}");
    assert!(s.mailbox.event(id).unwrap().proposals.is_empty());
}

#[test]
fn cancel_meeting_is_for_a_meeting_the_owner_organizes_and_always_tells_them() {
    let (mut s, account) = session();
    let appointment = run(&mut s, "calendar.new", json!({"title": "Dentist"}))["id"].as_u64().unwrap();
    assert!(refused(&mut s, "calendar.cancel", json!({"id": appointment})).contains("appointment"));
    let id = meeting(&mut s, Some(true))["id"].as_u64().unwrap();
    assert!(refused(&mut s, "calendar.cancel", json!({"id": id})).contains("\"send\": true"));
    assert!(s.mailbox.event(id).is_some());
    let done = run(&mut s, "calendar.cancel", json!({"id": id, "send": true}));
    assert_eq!(done["mailed"], json!(2));
    assert!(s.mailbox.event(id).is_none());
    assert!(outbox(&s, account).last().unwrap().3.contains("\r\nMETHOD:CANCEL\r\n"));
    // Somebody else's meeting (it came by mail) is not the owner's to cancel.
    let request = answer(&mut s, account, "bo@example.org", "REQUEST", "theirs-1", "ada@example.org", "NEEDS-ACTION", "");
    let mut fixed = s.mailbox.message(request).unwrap().clone();
    let text =
        String::from_utf8_lossy(&fixed.attachments[0].data).replace("ORGANIZER;CN=Ada:mailto:ada@example.org", "ORGANIZER:mailto:bo@example.org");
    fixed.attachments[0].data = text.into_bytes().into();
    *s.mailbox.message_mut(request).unwrap() = fixed;
    let theirs = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}))["event"].as_u64().unwrap();
    assert!(refused(&mut s, "calendar.cancel", json!({"id": theirs, "send": true})).contains("Somebody else organizes"));
}
