// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drawing without the graphics card: the repaint cap and the one-time line.

use std::time::Duration;

use serde_json::json;

use super::{SOFTWARE_MIN_WAIT, capped, notice_due, spinner_wake};
use crate::people_view::testing;

#[test]
fn software_holds_repaints_to_four_a_second() {
    let ms = Duration::from_millis;
    assert_eq!(capped(ms(16), true), SOFTWARE_MIN_WAIT);
    assert_eq!(capped(ms(100), true), SOFTWARE_MIN_WAIT);
    assert_eq!(capped(ms(250), true), ms(250));
    assert_eq!(capped(Duration::from_secs(5), true), Duration::from_secs(5));
    assert_eq!(capped(Duration::ZERO, true), SOFTWARE_MIN_WAIT);
    // A graphics card: as asked.
    assert_eq!(capped(ms(16), false), ms(16));
    assert_eq!(capped(Duration::ZERO, false), Duration::ZERO);
}

#[test]
fn the_line_is_said_once_and_only_in_software() {
    assert!(notice_due(true, false));
    assert!(!notice_due(true, true));
    assert!(!notice_due(false, false));
}

#[test]
fn dismissing_the_line_is_kept_in_the_settings() {
    let mut w = testing::window("view.mail");
    w.state_mut().software_drawing = true;
    w.run_steps(3);
    assert_eq!(w.state_mut().execute("ui.dismissSoftwareNotice", json!({})).unwrap()["dismissed"], true);
    assert!(w.state_mut().prefs().software_told);
    assert_eq!(w.state_mut().execute("ui.dismissSoftwareNotice", json!({})).unwrap()["dismissed"], false);
}

#[test]
fn a_still_spinner_keeps_the_window_looking() {
    // Polling that rode on the real spinner's every-frame repaint goes on, four times a second.
    assert_eq!(spinner_wake(true), Some(SOFTWARE_MIN_WAIT));
    assert_eq!(spinner_wake(false), None);
}
