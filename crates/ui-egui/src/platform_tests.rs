// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;

use mailcraft_host::{Notification, NotificationKind, Platform, PlatformError};

use crate::Services;

fn note(kind: NotificationKind) -> Notification {
    Notification { key: 1, kind, title: "t".into(), body: "b".into() }
}

#[test]
fn services_with_nothing_attached_say_so() {
    let s = Services::default();
    assert!(s.secrets().is_none());
    assert_eq!(s.open_sign_in_page("https://login.example.org/"), Err(PlatformError::Unsupported));
    assert_eq!(s.open_file("a.txt", b"x"), Err(PlatformError::Unsupported));
    assert_eq!(s.notify(&note(NotificationKind::Reminder)), Err(PlatformError::Unsupported));
}

#[test]
fn services_hand_the_platform_entries_to_what_the_host_gave_them() {
    let opened = Rc::new(RefCell::new(Vec::new()));
    let seen = opened.clone();
    let s = Services {
        open_page: Some(Box::new(move |url| {
            seen.borrow_mut().push(url.to_string());
            url.starts_with("https://")
        })),
        open_file: Some(Box::new(|_, _| Err("nothing opens it".to_string()))),
        ..Default::default()
    };
    assert_eq!(s.open_sign_in_page("https://login.example.org/"), Ok(()));
    assert!(matches!(s.open_sign_in_page("nope"), Err(PlatformError::Failed(_))));
    assert_eq!(opened.borrow().len(), 2);
    assert_eq!(s.open_file("a.txt", b"x"), Err(PlatformError::Failed("the file could not be opened")));
    // Mail alerts are the window's own.
    assert_eq!(s.notify(&note(NotificationKind::Mail)), Err(PlatformError::Unsupported));
}
