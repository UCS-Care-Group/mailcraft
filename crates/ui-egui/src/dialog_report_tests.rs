// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A report (Check Calendar Connection, Remove MailCraft's Data): every line is shown when there
//! is room, and its buttons sit right under the last one, not at the foot of a dialog as tall as
//! the window with the report cut short above a gap.

use egui_kittest::kittest::Queryable;

use crate::dialogs::Dialog;
use crate::people_view::testing;

#[test]
fn a_report_is_as_tall_as_its_lines_and_its_buttons_follow_them() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
    let mut lines = vec!["account@example.org: The calendar connection works.".to_string()];
    for n in 1..=7 {
        lines.push(format!(
            "{n}. Step number {n} of the check, said in your time zone: OK (the server answered 200). sent: 2027-11-11 15:00:00 to 2027-11-11 15:30:00 in the zone \"W. Europe Standard Time\", which is 2027-11-11 14:00:00 UTC to 2027-11-11 14:30:00 UTC here"
        ));
    }
    lines.push("The last line of the report.".to_string());
    h.state_mut().dialog = Some(Dialog::Report { name: "calendarCheck", title: "Calendar Connection Check".to_string(), lines });
    h.run_steps(4);
    let last = h.get_by_label("The last line of the report.").rect();
    let close = h.get_by_label("Close").rect();
    // The whole report fits in a window 800 points high: nothing is scrolled out of sight.
    assert!(last.bottom() <= close.top(), "the last line is under the buttons: {last:?} {close:?}");
    // And the buttons follow it, with no gap the height of the window between them.
    assert!(close.top() - last.bottom() < 40.0, "a gap above the buttons: {last:?} {close:?}");
}

/// A short report after a tall dialog: the dialogs share one window, which keeps the size it
/// last had, and the buttons stayed at its foot, a gap the height of the earlier dialog below
/// the report.
#[test]
fn a_short_report_after_a_tall_one_has_no_gap_above_its_buttons() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", serde_json::json!({"all": true}));
    let tall: Vec<String> = (1..=40).map(|n| format!("Line {n} of a long report")).collect();
    h.state_mut().dialog = Some(Dialog::Report { name: "dataRemoval", title: "Remove".to_string(), lines: tall });
    h.run_steps(4);
    let lines = vec!["The only line of the report.".to_string()];
    h.state_mut().dialog = Some(Dialog::Report { name: "calendarCheck", title: "Calendar Connection Check".to_string(), lines });
    h.run_steps(4);
    let last = h.get_by_label("The only line of the report.").rect();
    let close = h.get_by_label("Close").rect();
    assert!(close.top() - last.bottom() < 40.0, "a gap above the buttons: {last:?} {close:?}");
}
