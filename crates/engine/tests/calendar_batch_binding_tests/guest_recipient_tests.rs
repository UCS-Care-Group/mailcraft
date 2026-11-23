// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One source-bound regression for the question a typed `calendar.delete` asks when the owner is
//! to **decline meetings somebody else organizes** and the answer is to be sent
//! (`{"decline": true, "send": true}`): an answer reaches each meeting's **organizer**, never the
//! other people invited to it.
//!
//! One module of `calendar_batch_binding_tests`, sharing that fixture (`fixture()`, `d`, `at`,
//! `NOW`, `START`, `agent` and `json!`) through `use super::*`.
//!
//! What the source did when this was written (2026-11-15), and why the test holds it:
//! - `agent::classify` asks the stronger question for any `calendar.delete` with `"send": true`
//!   (agent.rs:741, 752-754), guest meetings included.
//! - An explicit typed set is described by `batch_text` (agent.rs:1567), whose recipient sentence
//!   is built from `invitees` (agent.rs:1403-1410); `invitees` counts only `event.attendees`
//!   (agent.rs:1352-1363). It never reads `event.organizer`, and it never reads `decline`.
//! - `delete_selected` sends no cancellation for a guest meeting: with `decline` true it calls
//!   `remote::note_responded(.., Response::Declined, send, ..)` (selection.rs:766, 785-789), which
//!   queues `EventChangeKind::Respond` with `send` (remote.rs:1903-1930). `invited()` is 0 for a
//!   guest meeting (calendar.rs:182-184), so that answer is queued even though the batch's own
//!   people count is 0 — and the `write to …` sentence is the only recipient wording the question
//!   gets, so it is the wrong one.
//! - An answer is mail to the organizer: `EventChange::send` is "an answer to the organizer"
//!   (model/src/remote.rs:535-539), and the upload carries no recipient list for `Respond` (`to`
//!   is filled for forwards only, remote.rs:1420-1423); the Graph call is the event's own decline
//!   action (net/src/graph.rs:1406), which tells its organizer.
//!
//! Contract basis: "Mail-to-attendees approval must aggregate all affected members with one
//! accurate question", "guest deletion/decline keeps explicit choices and never silently tells
//! the organizer", and "question titles/counts/recipient descriptions inspect all explicit
//! items, never an unrelated focus".
//!
//! The original mailbox is only described and asserted unchanged. A separate synthetic engine
//! session first queues the same operation as a fixture precondition, without host I/O. All
//! data is synthetic; no server is contacted, and no real mailbox, files or wall clock is used.

use super::*;

/// A typed guest decline that sends names the organizer of every meeting it answers, says that an
/// answer is sent, and does not claim the other people invited are written to.
#[test]
fn an_explicit_guest_decline_that_sends_names_each_organizer_and_says_it_answers() {
    let mut f = fixture();
    // A copy somebody else organizes only reaches a server when the mailbox has the account that
    // keeps it; `account.add` makes one here without contacting anything.
    let added =
        f.s.run(
            "account.add",
            &json!({
                "email": "owner@example.org", "name": "Owner",
                "incoming": {"host": "imap.example.org"}
            }),
        )
        .expect("a synthetic IMAP account");
    let account = added["id"].as_u64().expect("the account id");
    assert!(f.s.mailbox.account(account).is_some(), "fixture: the account that keeps both copies is in the mailbox");
    let meetings = [
        (f.kickoff, "Kickoff", "server-kickoff", "Zoe Organizer", "zoe@example.org", "alice@example.org"),
        (f.review, "Review", "server-review", "Yves Organizer", "yves@example.org", "bo@example.org"),
    ];
    for (id, title, remote, organizer_name, organizer_email, invitee) in meetings {
        let event = f.s.mailbox.event_mut(id).expect("the fixture meeting exists");
        assert_eq!(event.title, title, "fixture: the meeting's own title, as the question must still name it");
        event.origin = Origin { account, id: remote.to_string(), guest: true, ..Default::default() };
        event.organizer = Some(Address::new(organizer_name, organizer_email));
        assert!(event.origin.guest && !event.origin.id.is_empty(), "fixture: an invited copy of a server's meeting, so a decline can be sent");
        assert_eq!(event.attendees.len(), 1, "fixture: one other person is invited to {title}");
        assert_eq!(
            event.attendees.first().map(|a| a.address.email.as_str()),
            Some(invitee),
            "fixture: the person invited to {title} is not its organizer"
        );
        assert_ne!(organizer_email, invitee, "fixture: the organizer is a different person from the one invited");
    }
    let organizers: Vec<&str> = [f.kickoff, f.review]
        .iter()
        .flat_map(|id| f.s.mailbox.event(*id))
        .filter_map(|event| event.organizer.as_ref())
        .map(|organizer| organizer.email.as_str())
        .collect();
    assert_eq!(organizers, vec!["zoe@example.org", "yves@example.org"], "fixture: each meeting answers its own, different organizer");

    let params = json!({
        "items": [{"id": f.kickoff}, {"id": f.review}],
        "decline": true,
        "send": true
    });
    // Establish actual outgoing semantics in a separate, synthetic engine session. This
    // only queues in memory: the engine performs no host I/O and the original is unchanged.
    // If this precondition fails, the fixture has not reproduced a recipient-text defect.
    let mut probe = Session::at(f.s.mailbox.clone(), NOW, 0);
    let queued = probe.run("calendar.delete", &params).expect("fixture: both guest declines can be queued");
    assert_eq!(queued["deleted"], json!(2), "fixture: both meetings are actually answered");
    let changes: Vec<_> = probe.mailbox.event_changes.iter().filter(|change| change.event == f.kickoff || change.event == f.review).collect();
    assert_eq!(changes.len(), 2, "fixture: one outgoing change for each meeting");
    let sent_to: Vec<_> = changes
        .iter()
        .map(|change| {
            assert_eq!(change.kind, mailcraft_engine::model::remote::EventChangeKind::Respond, "fixture: an answer, not a cancellation to invitees");
            assert!(change.send, "fixture: each answer is meant to be sent");
            assert_eq!(change.response, mailcraft_engine::model::Response::Declined, "fixture: each answer is a decline");
            change.base.as_ref().and_then(|event| event.organizer.as_ref()).map(|who| who.email.as_str())
        })
        .collect();
    assert_eq!(
        sent_to,
        vec![Some("zoe@example.org"), Some("yves@example.org")],
        "fixture: the real queued answers name these organizers, not the invitees"
    );

    assert_eq!(
        agent::classify(&f.s, "calendar.delete", &params),
        Access::Owner(Class::Send),
        "an answer that is sent is the owner's stronger question, whoever organizes the meetings"
    );
    let before = f.s.mailbox.clone();

    let Verdict::Ask(class, said) = agent::check(&f.s, "calendar.delete", &params, &[]) else {
        panic!("a typed guest decline that sends must be asked about, never refused or run unasked");
    };
    assert_eq!(class, Class::Send, "the question is the one about other people: {said}");

    // The targeted assertion first: an answer reaches the organizer of each meeting, and this
    // batch has two meetings with two different organizers.
    assert!(said.contains("zoe@example.org"), "the question must name the organizer the answer goes to, the first meeting's: {said}");
    assert!(said.contains("yves@example.org"), "and the second meeting's organizer: {said}");
    let low = said.to_ascii_lowercase();
    assert!(
        low.contains("declin") || low.contains("answer") || low.contains("respond") || low.contains("reply"),
        "the question must say an answer is sent (a decline), not only that something is written: {said}"
    );
    // And it must not claim the other people invited are written to: a decline answers the
    // organizer, and those people receive nothing from this batch.
    if let Some(at) = said.find("write to") {
        let written = &said[at..];
        assert!(
            !written.contains("alice@example.org") && !written.contains("bo@example.org"),
            "the question must not name the people invited as the recipients of a decline, which writes to the organizers: {said}"
        );
    }
    assert!(
        said.contains("Kickoff") && said.contains("Review"),
        "and it still names both meetings, as the delete's own typed batch describes them: {said}"
    );
    assert!(f.s.mailbox == before, "asking about the decline changes nothing, and nothing is sent");
}
