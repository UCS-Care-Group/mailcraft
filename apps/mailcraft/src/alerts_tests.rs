// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's notification service: a click and the token it came with, put together.

use super::freedesktop::Heard;

#[test]
fn a_click_takes_the_token_the_service_sent_for_that_notification_just_before() {
    let mut heard = Heard::default();
    heard.token(7, "gnome-shell/MailCraft/2741-5-host_TIME812".into());
    heard.token(8, "for-another".into());
    assert_eq!(heard.invoked(7).as_deref(), Some("gnome-shell/MailCraft/2741-5-host_TIME812"));
    assert_eq!(heard.invoked(7), None, "once");
    assert_eq!(heard.invoked(9), None, "none came for it");
    assert_eq!(heard.invoked(8).as_deref(), Some("for-another"));
}

#[test]
fn a_token_that_is_not_one_is_dropped_and_only_a_few_wait() {
    let mut heard = Heard::default();
    // What a service sends is outside input: a token is short, printable text.
    heard.token(1, "x".repeat(Heard::LONGEST + 1));
    heard.token(2, "a\nb".into());
    heard.token(3, String::new());
    assert_eq!((heard.invoked(1), heard.invoked(2), heard.invoked(3)), (None, None, None));
    // Tokens for notifications nobody clicks don't pile up: the oldest go.
    for id in 0..(Heard::WAITING as u32 + 5) {
        heard.token(id, format!("t{id}"));
    }
    assert_eq!(heard.invoked(0), None);
    let last = Heard::WAITING as u32 + 4;
    assert_eq!(heard.invoked(last), Some(format!("t{last}")));
}

#[test]
fn a_token_goes_with_the_next_action_on_its_notification_whichever_it_is() {
    let mut heard = Heard::default();
    heard.token(7, "first".into());
    // Another action than a click on the notification: not a click, and its token is spent.
    assert_eq!(heard.signal("ActionInvoked", 7, "snooze".into()), None);
    assert_eq!(heard.signal("ActionInvoked", 7, "default".into()), Some((7, None)));
    heard.signal("ActivationToken", 7, "second".into());
    assert_eq!(heard.signal("ActionInvoked", 7, "default".into()), Some((7, Some("second".into()))));
    // Other signals of the service are not clicks.
    assert_eq!(heard.signal("NotificationClosed", 7, String::new()), None);
}

#[test]
fn clicks_waiting_for_the_window_are_few() {
    let (clicks, rx) = super::Clicks::new(egui::Context::default());
    for key in 0..1000 {
        clicks.clicked(key, Some("t".into()));
    }
    let mut waiting = 0;
    while !super::drain(&rx).is_empty() {
        waiting += 20;
    }
    assert!(waiting <= super::Clicks::WAITING + 20, "{waiting}");
}
