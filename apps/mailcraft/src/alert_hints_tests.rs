// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a desktop alert asks the notification service for, besides its text (#1309).

use mailcraft_ui_egui::alerts::AlertKind;

use super::freedesktop::hints;

#[test]
fn new_mail_asks_for_the_desktops_new_mail_sound() {
    let mail = hints(AlertKind::Mail);
    assert!(mail.contains(&("desktop-entry", "io.github.billsecond.mailcraft")));
    assert!(mail.contains(&("category", "email.arrived")));
    // A name from the desktop's sound theme: whether and how loud it plays is the notification
    // service's to decide.
    assert!(mail.contains(&("sound-name", "message-new-email")));
}

#[test]
fn a_reminder_asks_for_the_alarm_sound_and_nothing_names_a_file() {
    let reminder = hints(AlertKind::Reminder);
    assert!(reminder.contains(&("desktop-entry", "io.github.billsecond.mailcraft")));
    assert!(reminder.contains(&("sound-name", "alarm-clock-elapsed")));
    assert!(!reminder.iter().any(|(name, _)| *name == "category"), "the specification has no category for a reminder");
    for kind in [AlertKind::Mail, AlertKind::Reminder] {
        assert!(!hints(kind).iter().any(|(name, _)| *name == "sound-file" || *name == "suppress-sound"), "{kind:?}");
    }
}
