// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One source-bound regression for the question an **implicit** typed `calendar.delete` asks when
//! it is to write to the people invited (`{"send": true}`): the recipients it names must be every
//! member's, never only the focused appointment's.
//!
//! One module of `calendar_batch_binding_tests` (the integration test of that name), so
//! `use super::*` is its fixture (`fixture()`, `d`, `at`, `NOW`, `START`), `agent`, `json!` and
//! `Id`.
//!
//! What the source did when this was written (2026-11-15), and why the test holds it:
//! - `agent::what_it_does` describes an implicit typed delete as a batch only from
//!   `params.get("items")` (agent.rs:1567), so today the sentence is built from the focus
//!   (`agent.rs:1568-1576` → `invitees` → `named_event`, agent.rs:1352-1363).
//! - Even with the batch arm reached (`is_batch`), `invitees` (agent.rs:1352)
//!   collects people only when `params` has `items`, else from the one named/focused event. For
//!   an implicit `{}` delete over a typed set the recipient list therefore stays the focus's,
//!   while `delete_selected` writes to every member's attendees and asks about them all
//!   (selection.rs:748-759, 792-803). The question would name fewer people than the command
//!   writes to.
//!
//! Contract basis: "Mail-to-attendees approval must aggregate all affected members with one
//! accurate question", and "question titles/counts/recipient descriptions inspect all explicit
//! items, never an unrelated focus". The implicit delete's members are the typed selection the
//! command acts on: "Implicit engine Delete targets ANY nonempty typed selection".
//!
//! Pure describe: nothing is sent, nothing is run, and the mailbox is asserted unchanged. All
//! data is synthetic (`example.org` people, the parent fixture's events). No files, no network,
//! no wall clock, no real mailbox.

use super::*;

/// An implicit typed delete that would write to the people invited names every recipient of the
/// batch — including the person invited to the appointment that is not in focus — and both
/// appointments, before the owner is asked.
#[test]
fn an_implicit_typed_delete_that_writes_to_people_names_every_recipient() {
    let mut f = fixture();
    // The shared fixture's meetings start local. Give both a real synthetic keeper before
    // asserting who a cancellation reaches: local appointments would send nobody mail.
    let added =
        f.s.run(
            "account.add",
            &json!({
                "email": "zoe@example.org", "name": "Zoe",
                "incoming": {"host": "imap.example.org"}
            }),
        )
        .expect("a synthetic IMAP account");
    let account = added["id"].as_u64().expect("the account id");
    for id in [f.kickoff, f.review] {
        let event = f.s.mailbox.event_mut(id).expect("the fixture meeting exists");
        event.origin = Origin { account, id: format!("server-{id}"), ..Default::default() };
        event.organizer = Some(Address::new("Zoe Example", "zoe@example.org"));
        assert!(!event.origin.guest && !event.origin.id.is_empty());
        assert_eq!(event.attendees.len(), 1, "one distinct recipient per kept meeting");
    }
    assert_eq!(
        mailcraft_engine::cmd::calendar::mail_of(
            &f.s,
            "calendar.delete",
            &json!({
                "items": [{"id": f.kickoff}, {"id": f.review}]
            })
        ),
        Some(mailcraft_engine::cmd::calendar::Mail::Cancellation(2)),
        "fixture: both selected meetings really require cancellation mail"
    );
    f.s.run("calendar.select", &json!({"items": [{"id": f.kickoff}, {"id": f.review}]})).expect("two typed meetings are selected");
    assert_eq!(f.s.sel.event, Some(f.review), "fixture: the last typed item has the focus, so a question built from the focus names Bo alone");
    let before = f.s.mailbox.clone();

    let said = agent::describe(&f.s, "calendar.delete", &json!({"send": true}));
    // The targeted assertion first: the person invited to the appointment that is not in focus.
    assert!(
        said.contains("alice@example.org"),
        "the question must name every person the cancellation is written to, the non-focused meeting's included: {said}"
    );
    assert!(said.contains("bo@example.org"), "and the focused meeting's person too: {said}");
    assert!(said.contains("Kickoff") && said.contains("Review"), "and both appointments, as the delete's own typed batch describes them: {said}");
    assert!(f.s.mailbox == before, "describing the delete changes nothing, and nothing is sent");
}
