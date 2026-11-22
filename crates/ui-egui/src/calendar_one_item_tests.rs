// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A plain click selects one item, a typed set of one: what the owner then does to it is what
//! was done to the one appointment in focus before sets (benandez on #1665). One day of a series
//! is asked about as it always was: Delete and the ribbon's Delete ask "Delete Recurring Item",
//! a Cut pasted on the same day asks "Move Recurring Item", and each answer does what it did
//! then (just that day, or the entire series).
//!
//! One module of `calendar_selection_tests`, so `super::*` is that file's fixtures (`calendar`,
//! `event`, `occurrence`, `selected`, `focused`, `click`, `chord`, `after_noon`, `date`, `at`,
//! `COMMAND`, `Window`, `Id`, `Modifiers`, `PointerButton`). The window is the real one, on the
//! sample's invented "Team stand-up" (every weekday); every act is a pointer event, a key, or the
//! command the ribbon's button runs. No file, server, real account or wall clock.

use egui_kittest::kittest::{NodeT as _, Queryable as _};
use serde_json::json;

use super::*;

/// The title of the question that is open, when the one that is open is a question.
fn question(w: &Window) -> Option<String> {
    match &w.state().dialog {
        Some(crate::dialogs::Dialog::Confirm { ask, .. }) => Some(ask.title.clone()),
        _ => None,
    }
}

/// Choose the one button of the window called `label`: a question's own answer.
fn choose_button(w: &mut Window, label: &str) {
    let mut found: Vec<egui::Rect> = w
        .query_all_by_label_contains(label)
        .filter(|node| {
            node.accesskit_node().role() == egui::accesskit::Role::Button && node.accesskit_node().label().is_some_and(|said| said.trim() == label)
        })
        .map(|node| node.rect())
        .collect();
    assert_eq!(found.len(), 1, "one button says {label}");
    let spot = found.pop().expect("one button").center();
    click(w, spot, Modifiers::NONE, PointerButton::Primary);
    w.run_steps(3);
}

/// The stand-up of `day`, clicked alone, as the owner clicks it: a typed set of one, in focus.
fn click_the_standup(w: &mut Window, day: Date) -> Id {
    let standup = event(w, "Team stand-up").id;
    let spot = occurrence(w, standup, day).center();
    click(w, spot, Modifiers::NONE, PointerButton::Primary);
    assert_eq!(selected(w), [(standup, Some(day))], "fixture: a plain click makes a set of one");
    assert_eq!(focused(w), (Some(standup), Some(day)), "fixture: and that one has the focus");
    standup
}

/// The stand-ups that are events of their own (a day changed on its own), earliest first.
fn on_their_own(w: &Window) -> Vec<(DateTime, DateTime)> {
    titled(w, "Team stand-up").into_iter().filter(|e| e.recurrence.is_none()).map(|e| (e.start, e.end)).collect()
}

/// How the owner deletes what is selected.
#[derive(Clone, Copy, Debug)]
enum Route {
    /// The Delete key.
    Key,
    /// The ribbon's Delete, which runs `calendar.delete` with no parameters (`ribbon.rs`,
    /// `Item::big("delete", "Delete", "calendar.delete", no())`) as the owner's own command.
    Ribbon,
}

/// Delete on one clicked day of a series, by the key and by the ribbon, asks which of the series
/// is meant, as it did before sets; nothing goes until it is answered, and each answer does what
/// it did then: just that day, or the entire series.
#[test]
fn delete_on_one_clicked_day_of_a_series_asks_which_and_does_what_the_answer_says() {
    let tuesday = date(10, 13);
    for route in [Route::Key, Route::Ribbon] {
        for just_this_one in [true, false] {
            let case = format!("{route:?}, just this one: {just_this_one}");
            let mut w = calendar();
            let standup = click_the_standup(&mut w, tuesday);
            let before = w.state().session.mailbox.event(standup).cloned().expect("fixture: the series is there");
            match route {
                Route::Key => chord(&mut w, Modifiers::NONE, egui::Key::Delete),
                Route::Ribbon => {
                    let answer = w.state_mut().run("calendar.delete", json!({}));
                    assert_eq!(
                        answer.as_ref().ok().and_then(|said| said.get("pending")).and_then(|p| p.as_str()),
                        Some("confirm"),
                        "{case}: {answer:?}"
                    );
                    w.run_steps(3);
                }
            }
            assert_eq!(question(&w).as_deref(), Some("Delete Recurring Item"), "{case}: the series question is asked");
            assert_eq!(w.state().session.mailbox.event(standup).cloned().as_ref(), Some(&before), "{case}: nothing goes until it is answered");
            choose_button(&mut w, if just_this_one { "Just This One" } else { "The Entire Series" });
            let after = w.state().session.mailbox.event(standup).cloned();
            if just_this_one {
                let after = after.unwrap_or_else(|| panic!("{case}: the series stays"));
                assert_eq!(after.exceptions, vec![tuesday], "{case}: just Tuesday's goes");
                assert_eq!((after.start, after.end), (before.start, before.end), "{case}: the series is where it was");
            } else {
                assert!(after.is_none(), "{case}: the entire series goes");
            }
            assert!(on_their_own(&w).is_empty(), "{case}: nothing is made of it");
            assert!(w.state_mut().session.undo(), "{case}: one step");
            w.run_steps(2);
            assert_eq!(w.state().session.mailbox.event(standup).cloned().as_ref(), Some(&before), "{case}: one Undo brings it back");
        }
    }
}

/// A cut of one clicked day of a series, pasted at another time of that same day, asks which of
/// the series is meant, as Cut and Paste did before sets; just this one moves that day alone, and
/// the entire series moves the series by as much.
#[test]
fn a_cut_of_one_clicked_day_of_a_series_pasted_on_that_day_asks_which() {
    let thursday = date(10, 15);
    for just_this_one in [true, false] {
        let case = format!("just this one: {just_this_one}");
        let mut w = calendar();
        let standup = click_the_standup(&mut w, thursday);
        let before = w.state().session.mailbox.event(standup).cloned().expect("fixture: the series is there");
        chord(&mut w, COMMAND, egui::Key::X);
        let spot = egui::pos2(occurrence(&w, standup, thursday).center().x, after_noon(&w, 4.0).y);
        click(&mut w, spot, Modifiers::NONE, PointerButton::Primary);
        chord(&mut w, COMMAND, egui::Key::V);
        w.run_steps(3);
        assert_eq!(question(&w).as_deref(), Some("Move Recurring Item"), "{case}: the series question is asked");
        assert_eq!(w.state().session.mailbox.event(standup).cloned().as_ref(), Some(&before), "{case}: nothing moves until it is answered");
        choose_button(&mut w, if just_this_one { "Just This One" } else { "The Entire Series" });
        let after = w.state().session.mailbox.event(standup).cloned().unwrap_or_else(|| panic!("{case}: the series stays"));
        let length = before.start.minutes_until(before.end);
        if just_this_one {
            assert_eq!(after.exceptions, vec![thursday], "{case}: Thursday's left the series");
            assert_eq!((after.start, after.end), (before.start, before.end), "{case}: the series stays where it was");
            assert_eq!(on_their_own(&w), [(at(thursday, 16, 0), at(thursday, 16, 0).add_minutes(length))], "{case}: and Thursday's is at four");
        } else {
            assert!(after.exceptions.is_empty() && on_their_own(&w).is_empty(), "{case}: the series moves whole");
            assert_eq!((after.start.hour, after.start.minute), (16, 0), "{case}: every day of it at four now");
            assert_eq!(after.start.date, before.start.date, "{case}: on the days it had");
            assert_eq!(after.start.minutes_until(after.end), length, "{case}: as long as it was");
        }
    }
}
