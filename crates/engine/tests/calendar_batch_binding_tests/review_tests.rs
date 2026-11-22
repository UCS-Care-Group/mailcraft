// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regressions for the agent gate: the approval reach of an implicit `calendar.delete`, and the
//! guard/binding parities the agent gate keeps with the window.
//!
//! Wired from `crates/engine/tests/calendar_batch_binding_tests.rs` as
//!
//! ```ignore
//! #[path = "calendar_batch_binding_tests/review_tests.rs"]
//! mod review_tests;
//! ```
//!
//! so that this file is one module of that test binary and shares its fixture (`fixture()`, `d`,
//! `at`, `NOW`, `START`) and its imports through `use super::*`.
//!
//! Everything here is synthetic: `example.org`/`.example` people, events added through the public
//! `Mailbox::add_event`, and account servers that are names nothing contacts. No files, no
//! network, no wall clock, no real mailbox.
//!
//! Each test pins one finding to observable behaviour of the public engine/agent API. The
//! assertions are written against the contract (paste and delete, and the UI) and against the
//! documented params of the commands, never against a chosen implementation of the fix.

use super::*;

use mailcraft_engine::SelectedEvent;
use mailcraft_engine::model::{AccountKind, Security, Server};

/// The reviewed session's clock offset: the fixture is at UTC, so an occurrence's wall times are
/// the times of that offset. (The parent fixture's `Session::at(.., NOW, 0)`.)
fn utc() -> impl Fn(i64) -> i32 {
    |_| 0
}

/// The parent fixture's second account: the one its server-kept calendar belongs to.
fn second_account(f: &Fixture) -> Id {
    f.s.mailbox
        .accounts
        .iter()
        .find(|a| a.email == "second@example.org")
        .map(|a| a.id)
        .expect("fixture: the parent fixture has a second account, second@example.org")
}

/// The parent fixture, with that second account given the servers an IMAP account has, so that
/// `account.edit` can move where the account's calendar writes would go while its id, address and
/// the mailbox generation stay the same. The hosts are `example.org` names: nothing is sent.
fn imap_fixture() -> Fixture {
    let mut f = fixture();
    let account = second_account(&f);
    // (The parent fixture clones the sample mailbox's own account, which is a local one: that is
    // the fixture this module is wired to, and the guard says so if it ever changes.)
    let a = f.s.mailbox.account_mut(account).expect("fixture: the second account is there");
    assert_eq!(a.kind, AccountKind::Local, "fixture: the second account is the parent fixture's clone of the sample account");
    a.kind = AccountKind::Imap;
    a.incoming = Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls });
    a.outgoing = Some(Server { host: "smtp.example.org".into(), port: 465, security: Security::Tls });
    // The calendar the parent fixture pushes is this account's, on its server.
    let calendar = f.s.mailbox.calendar(f.remote).expect("fixture: the second account's calendar is there");
    assert_eq!(calendar.origin.account, account, "fixture: the second calendar is kept by the second account");
    assert_eq!(calendar.rights, mailcraft_engine::model::Rights::Edit, "fixture: it may be changed here");
    f
}

/// An appointment already on the second account's server: a source whose keeper is that account.
fn keep_remote(f: &mut Fixture) -> Id {
    let account = second_account(f);
    let start = at(d(2026, 11, 12), 11, 0);
    f.s.mailbox
        .add_event(Event {
            title: "Second-account standup".into(),
            start,
            end: start.add_minutes(30),
            calendar: f.remote,
            origin: Origin { account, id: "e-second-standup".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("a server calendar takes an event")
}

/// The question an agent's parameterless `calendar.delete` puts to the owner when one typed day of
/// the series is what is selected. Asserts the selection is the typed key, never a legacy focus.
fn question_for_one_day(day: Date) -> String {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"items": [{"id": f.series, "day": day.iso()}]})).expect("one typed day of the series is selected");
    assert_eq!(
        f.s.sel.events,
        vec![SelectedEvent { id: f.series, day: Some(day) }],
        "fixture: `calendar.select` made the typed key, so the test asks about a typed selection"
    );
    // (A set of one in focus is the older Delete of the focus, benandez on #1665: unscoped it is
    // the entire series, and says so; told the day is meant, it is that day.)
    match agent::check(&f.s, "calendar.delete", &json!({}), &[]) {
        Verdict::Ask(Class::Delete, said) => assert!(said.contains("all of its occurrences"), "unscoped, the whole series, and said: {said}"),
        other => panic!("a parameterless delete over the owner's selection is theirs to approve, not {other:?}"),
    }
    match agent::check(&f.s, "calendar.delete", &json!({"scope": "occurrence"}), &[]) {
        Verdict::Ask(Class::Delete, said) => said,
        other => panic!("a delete of the day in focus is theirs to approve, not {other:?}"),
    }
}

/// **S-1** (blocking). An agent's `calendar.delete {}` is dispatched to the whole typed selection
/// (`cmd/calendar.rs:762-764` → `selection::delete_selected`, `selection.rs:713-721`), and the
/// owner's question must therefore name every item of it, with the day of each occurrence
/// (`agent.rs:1567`, `agent.rs:1415-1425`) — never only the focused appointment
/// (`agent.rs:1586` → `event_title` → `named_event`).
#[test]
fn a_delete_that_names_nothing_asks_about_the_whole_typed_selection() {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"items": [{"id": f.kickoff}, {"id": f.series, "day": "2026-11-18"}]}))
        .expect("two typed items are selected: an ordinary appointment and one day of the series");
    let before = f.s.mailbox.clone();
    let series_before = f.s.mailbox.event(f.series).cloned().expect("fixture: the series is there");
    let unrelated_before = f.s.mailbox.event(f.unrelated).cloned().expect("fixture: the unrelated appointment is there");

    let Verdict::Ask(class, what) = agent::check(&f.s, "calendar.delete", &json!({}), &[]) else {
        panic!("a parameterless delete over a typed selection is the owner's to approve")
    };
    assert_eq!(class, Class::Delete, "it is asked about as a delete");
    assert!(
        what.contains("Kickoff") && what.contains("Weekly sync"),
        "the question names every appointment the yes would delete, not only the one in focus: {what}"
    );
    assert!(what.contains("2026-11-18"), "and the day of the series it would cancel, which is the scope the owner is approving: {what}");

    // While the question is up nothing of the mailbox changes, and the yes is bound to what it said.
    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &json!({}), &what).expect("a delete is shown about what it acts on");
    assert!(f.s.mailbox == before, "asking the owner changes nothing");
    assert!(shown.check(&f.s, "calendar.delete", &json!({})).is_ok(), "nothing changed behind the question, so the yes still stands");

    f.s.agent = Some(vec![Class::Delete]);
    let answered = f.s.run("calendar.delete", &json!({}));
    f.s.agent = None;
    let answered = answered.expect("the granted delete runs");
    assert_eq!(answered["deleted"], json!(2), "exactly the two items the question named: {answered}");

    assert!(f.s.mailbox.event(f.kickoff).is_none(), "the ordinary appointment the question named is gone");
    let series = f.s.mailbox.event(f.series).expect("the series' master still stands");
    assert_eq!((series.start, series.end), (series_before.start, series_before.end), "with the same start and end");
    assert_eq!(series.exceptions, vec![d(2026, 11, 18)], "and exactly the named day cancelled, not the series");
    assert_eq!(f.s.mailbox.event(f.unrelated), Some(&unrelated_before), "an appointment the batch never named is untouched");
    let here = utc();
    assert_eq!(
        series.occurrence_times(d(2026, 11, 25), &here),
        (at(d(2026, 11, 25), 9, 0), at(d(2026, 11, 25), 9, 30)),
        "the week after happens where it always did"
    );

    assert!(f.s.undo(), "one undo takes the batch back");
    assert!(f.s.mailbox == before, "and it puts the whole mailbox back: the series, its cancelled days and the appointments around it");
}

/// **S-1**, the single typed occurrence. One day of a series is one item
/// (`selection.rs:204-246`), and the question must name that intrinsic day — the day
/// `Occurrence::day` and the exceptions key use — and must never say the whole series goes.
#[test]
fn a_delete_of_one_typed_occurrence_asks_about_that_day_and_never_the_series() {
    let eleventh = question_for_one_day(d(2026, 11, 11));
    let eighteenth = question_for_one_day(d(2026, 11, 18));
    assert!(eleventh.contains("2026-11-11"), "the day the yes would cancel is named: {eleventh}");
    assert!(eighteenth.contains("2026-11-18"), "the day the yes would cancel is named: {eighteenth}");
    assert_ne!(eleventh, eighteenth, "two days of one series are two different questions, so the words must differ");
    for said in [&eleventh, &eighteenth] {
        assert!(!said.contains("all of its occurrences"), "one day is not the series, and the owner is not told it is: {said}");
    }
}

/// **N-1** (concern A). The yes is pinned to the items and the calendars (`agent/bind.rs:350-385`),
/// and the calendars name the account that keeps them only by id (`model/src/calendar.rs:130-161`).
/// An `account.edit` that moves where that account's calendar writes would go, under the same id,
/// address and mailbox generation, changes what the yes reaches — so it must retract the yes.
/// The contrast: an account the operation never touches is not part of it.
#[test]
fn an_approval_does_not_reach_an_account_whose_calendar_writes_moved() {
    let mut f = imap_fixture();
    let remote_event = keep_remote(&mut f);
    // A paste into a calendar the second account's server keeps...
    let into_remote = json!({"items": [{"id": f.kickoff}], "start": START, "calendar": f.remote, "cut": true});
    // ...and a cut of an appointment that account's server already keeps.
    let in_remote = json!({"items": [{"id": remote_event}], "start": START, "cut": true});
    let into_what = agent::describe(&f.s, "calendar.paste", &into_remote);
    let source_what = agent::describe(&f.s, "calendar.paste", &in_remote);
    let into_shown = agent::bind::Shown::new(&f.s, "calendar.paste", &into_remote, &into_what).expect("a paste is shown about the items it moves");
    let source_shown = agent::bind::Shown::new(&f.s, "calendar.paste", &in_remote, &source_what).expect("a paste is shown about the items it moves");
    assert!(into_shown.check(&f.s, "calendar.paste", &into_remote).is_ok(), "fixture: nothing has changed yet");
    assert!(source_shown.check(&f.s, "calendar.paste", &in_remote).is_ok(), "fixture: nothing has changed yet");

    let account = second_account(&f);
    let openings = f.s.openings();
    let kickoff_before = f.s.mailbox.event(f.kickoff).cloned().expect("fixture: the source appointment is there");
    let calendar_before = f.s.mailbox.calendar(f.remote).cloned().expect("fixture: the destination calendar is there");
    // The same account, the same address, the same mailbox generation; only its server address moves.
    f.s.run(
        "account.edit",
        &json!({"id": account, "email": "second@example.org", "incoming": {"host": "imap.changed.example.org", "port": 993, "security": "tls"}}),
    )
    .expect("an IMAP account's server may be changed while its address stays");

    assert_eq!(f.s.openings(), openings, "fixture: the mailbox generation is the one the question was asked in");
    assert_eq!(f.s.mailbox.account(account).map(|a| a.email.clone()), Some("second@example.org".to_string()), "fixture: the account is the same one");
    assert_eq!(f.s.mailbox.event(f.kickoff), Some(&kickoff_before), "fixture: the source appointment is unchanged");
    assert_eq!(f.s.mailbox.calendar(f.remote), Some(&calendar_before), "fixture: the destination calendar is unchanged");
    assert_eq!(
        agent::describe(&f.s, "calendar.paste", &into_remote),
        into_what,
        "the question's words cannot see this: only the account's settings moved, not its id or address"
    );
    assert!(
        into_shown.check(&f.s, "calendar.paste", &into_remote).is_err(),
        "the account whose server would receive the move is part of the yes, so it must be shown again"
    );
    assert!(
        source_shown.check(&f.s, "calendar.paste", &in_remote).is_err(),
        "and so is the account that keeps the source, whose server would be told of the change"
    );

    // The contrast: an account this paste never touches is not part of the yes.
    let mut c = imap_fixture();
    let into_remote = json!({"items": [{"id": c.kickoff}], "start": START, "calendar": c.remote, "cut": true});
    let what = agent::describe(&c.s, "calendar.paste", &into_remote);
    let shown = agent::bind::Shown::new(&c.s, "calendar.paste", &into_remote, &what).expect("a paste is shown about the items it moves");
    let touched = second_account(&c);
    let unrelated = c.s.mailbox.accounts.iter().find(|a| a.id != touched).map(|a| a.id).expect("fixture: the mailbox has another account");
    c.s.run("account.edit", &json!({"id": unrelated, "incoming": {"host": "imap.elsewhere.example.org", "port": 993, "security": "tls"}}))
        .expect("that account's server may be changed too");
    assert!(
        shown.check(&c.s, "calendar.paste", &into_remote).is_ok(),
        "an account the operation doesn't touch must not retract its yes, or every edit would cancel every question"
    );
}

/// **N-2** (concern B). `selection::paste` reads its destination strictly (`selection.rs:433-436`,
/// `p::id_strict`), so a malformed one is refused — and the gate must refuse it too, rather than
/// class and describe it from each item's own calendar (`agent.rs:1273-1287` reads `items`,
/// `start` and `id`, never `calendar`).
#[test]
fn a_destination_that_isnt_a_calendar_is_refused_by_the_gate_whatever_it_was_granted() {
    let mut f = fixture();
    let before = f.s.mailbox.clone();
    let no_grants: &[Class] = &[];
    let every_class: &[Class] = &Class::ALL;
    for calendar in [json!(null), json!("7"), json!(0), json!(999_999)] {
        for cut in [false, true] {
            let params = json!({"items": [{"id": f.kickoff}], "start": START, "cut": cut, "calendar": calendar.clone()});
            for allow in [no_grants, every_class] {
                let said = agent::check(&f.s, "calendar.paste", &params, allow);
                assert!(matches!(said, Verdict::Refuse(_)), "`calendar.paste` with {params} must be refused, never asked about or run: {said:?}");
            }
        }
    }
    // The command already refuses these before any change; that is not the finding, and this
    // control keeps it so.
    let bad = json!({"items": [{"id": f.kickoff}], "start": START, "cut": true, "calendar": "7"});
    assert!(f.s.run("calendar.paste", &bad).is_err(), "the command refuses a destination that isn't an id");
    assert!(f.s.mailbox == before, "and nothing was pasted");

    // The id policy around those cases is not part of the finding: keep it as it is. A whole
    // number written as a float is an id; a fraction is not one.
    assert_eq!(mailcraft_engine::p::id_strict(&json!({"calendar": 12.0}), "calendar").ok(), Some(Some(12)), "12.0 is the id 12");
    assert!(mailcraft_engine::p::id_strict(&json!({"calendar": 0.5}), "calendar").is_err(), "a fraction is no id");
}

/// **N-3** (concern C). `calendar.delete` documents `items` with `scope: "series"`
/// (`calendar.rs:50`), and `delete_selected` works it (`selection.rs:705-709`,
/// `selection.rs:222`), but `agent::batch` always resolves with `Allow::Occurrences`
/// (`agent.rs:1286`), so the typed whole-series form is refused while the legacy `id` form is
/// asked about. Both forms are the same documented operation and must be asked about the same way.
#[test]
fn a_whole_series_named_by_typed_items_is_asked_about_like_the_legacy_id_is() {
    let mut f = fixture();
    let typed = json!({"items": [{"id": f.series}], "scope": "series"});
    let legacy = json!({"id": f.series, "scope": "series"});
    assert!(
        matches!(agent::check(&f.s, "calendar.delete", &legacy, &[]), Verdict::Ask(..)),
        "the documented legacy form of a whole-series delete is asked about"
    );
    let Verdict::Ask(class, what) = agent::check(&f.s, "calendar.delete", &typed, &[]) else {
        panic!("`items` with `scope: \"series\"` is the documented form of the same delete, so the gate must ask about it, not refuse it")
    };
    assert_eq!(class, Class::Delete);
    assert!(
        what.contains("Weekly sync") && what.contains("all of its occurrences"),
        "the question says the whole series goes, which is what the yes would do: {what}"
    );

    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &typed, &what).expect("a delete is shown about what it acts on");
    assert!(shown.check(&f.s, "calendar.delete", &typed).is_ok(), "nothing changed behind the question");
    f.s.agent = Some(vec![Class::Delete]);
    let answered = f.s.run("calendar.delete", &typed);
    f.s.agent = None;
    let answered = answered.expect("the approved whole-series delete runs");
    assert_eq!(answered["deleted"], json!(1), "one item, the series: {answered}");
    assert!(f.s.mailbox.event(f.series).is_none(), "the whole series is gone, as the question said");
    for untouched in [f.kickoff, f.review, f.unrelated] {
        assert!(f.s.mailbox.event(untouched).is_some(), "an appointment the batch didn't name is untouched");
    }
}

/// **N-4**. A typed item carries its own day, and `delete_selected` keeps that day even under
/// `scope: "series"` (`selection.rs:215-226`, `selection.rs:767-780`): the question must name the
/// day, and must not say the whole series goes (`agent.rs:1415-1420` says it does).
#[test]
fn a_dated_typed_item_keeps_its_day_even_under_scope_series() {
    let mut f = fixture();
    let params = json!({"items": [{"id": f.series, "day": "2026-11-11"}], "scope": "series"});
    let said = agent::describe(&f.s, "calendar.delete", &params);
    assert!(said.contains("2026-11-11"), "the question names the day the yes would cancel: {said}");
    assert!(!said.contains("all of its occurrences"), "an item that names its own day is not the whole series, whatever `scope` says: {said}");

    let answered = f.s.run("calendar.delete", &params).expect("the day is deleted");
    assert_eq!(answered["deleted"], json!(1), "one day of the series: {answered}");
    let series = f.s.mailbox.event(f.series).expect("the series' master stands");
    assert_eq!(series.exceptions, vec![d(2026, 11, 11)], "exactly the day named is cancelled");
    let here = utc();
    assert_eq!(
        series.occurrence_times(d(2026, 11, 18), &here),
        (at(d(2026, 11, 18), 9, 0), at(d(2026, 11, 18), 9, 30)),
        "the next week happens where it always did"
    );
    assert!(f.s.mailbox.event(f.unrelated).is_some(), "and an appointment the batch never named is untouched");
}

// (No test for guest-decline parity: the source disproves the premise. `delete_selected` counts
// people with `super::invited`
// (`selection.rs:748-754`), which is 0 for a meeting somebody else organizes
// (`calendar.rs:182-184`: `kept_by(..).is_some() && !e.origin.guest`), so a batch that names only
// such a meeting needs no `send` — exactly as the legacy path does at `calendar.rs:775, 789-801`.
// The asymmetry the review saw needs a second, invited appointment of the owner's in the same
// batch, where the one aggregate question is about that appointment's mail, not about the guest.)

#[test]
fn an_implicit_typed_delete_refuses_a_replaced_member_even_when_its_caption_stays() {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"items": [{"id": f.kickoff}, {"id": f.review}]})).unwrap();
    let params = json!({});
    let what = agent::describe(&f.s, "calendar.delete", &params);
    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &params, &what).unwrap();
    assert!(shown.check(&f.s, "calendar.delete", &params).is_ok(), "the unchanged question can be answered");
    let selection = f.s.sel.clone();
    let view = f.s.view.clone();
    let opening = f.s.openings();
    let old_title = f.s.mailbox.event(f.kickoff).unwrap().title.clone();
    // A host can update a record without changing the owner's selection. Keep its public
    // caption and recipients, but replace its identity: this is not the event that was shown.
    f.s.mailbox.event_mut(f.kickoff).unwrap().uid = "replacement-identity@example.org".into();
    assert_eq!(f.s.sel, selection, "fixture: neither the typed selection nor its focus changed");
    assert_eq!(f.s.view, view, "fixture: the view is unchanged");
    assert_eq!(f.s.openings(), opening, "fixture: this is still the same mailbox generation");
    assert_eq!(f.s.mailbox.event(f.kickoff).unwrap().title, old_title, "fixture: the caption did not expose this change");
    assert_eq!(agent::describe(&f.s, "calendar.delete", &params), what, "fixture: describing again cannot detect the replacement");
    let before_answer = f.s.mailbox.clone();
    assert!(
        shown.check(&f.s, "calendar.delete", &params).is_err(),
        "an implicit typed batch must bind each source's state, not only its visible caption and selection"
    );
    assert!(f.s.mailbox == before_answer, "refusing the changed approval deletes nothing");
}
