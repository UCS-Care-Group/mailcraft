// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the phone keeps (MB3.16): its defaults, the one move to them, and the owner's choices.

use mailcraft_engine::Session;
use mailcraft_model::{Account, AccountKind, DownloadInFull, KeepOffline, Mailbox, PHONE_DEFAULTS, PHONE_STORAGE};

use crate::limits::{DownloadSettings, KeepWindow, settle};

/// A mailbox as an earlier version of the phone left it: a Microsoft account at a computer's
/// defaults, no device limits.
fn before_the_limits() -> (Session, u64) {
    let mut mb = Mailbox::with_local_account("", "");
    let account = mb.add_account(Account {
        email: "noor@example.org".into(),
        kind: AccountKind::Microsoft,
        keep_offline: KeepOffline::Months12,
        ..Default::default()
    });
    (Session::new(mb), account)
}

#[test]
fn a_mailbox_from_before_the_limits_is_moved_to_the_phones_defaults_once() {
    let (mut s, account) = before_the_limits();
    assert!(settle(&mut s, Some(64 << 20)));
    let a = s.mailbox.account(account).unwrap();
    assert_eq!((a.keep_offline, a.download_in_full), (KeepOffline::Month1, DownloadInFull::UpTo1Mb));
    let l = s.mailbox.device_limits.unwrap();
    assert_eq!((l.storage, l.messages, l.defaults), (PHONE_STORAGE, (64 << 20) / 4096, PHONE_DEFAULTS));
    // The owner's later choice stands: it is never moved again.
    s.mailbox.account_mut(account).unwrap().keep_offline = KeepOffline::Months6;
    assert!(!settle(&mut s, Some(64 << 20)));
    assert_eq!(s.mailbox.account(account).unwrap().keep_offline, KeepOffline::Months6);
}

#[test]
fn a_storage_cap_the_owner_had_chosen_is_kept_by_the_move() {
    let (mut s, _) = before_the_limits();
    s.run("account.deviceLimits", &serde_json::json!({"storage": 2u64 << 30})).unwrap();
    assert!(settle(&mut s, None));
    assert_eq!(s.mailbox.device_limits.unwrap().storage, 2 << 30);
}

#[test]
fn the_settings_of_a_new_account_are_applied_by_commands() {
    let (mut s, account) = before_the_limits();
    let settings = DownloadSettings { window: KeepWindow::Weeks2, storage_mb: 1024 };
    crate::limits::apply_to(&mut s, account, settings).unwrap();
    assert_eq!(s.mailbox.account(account).unwrap().keep_offline, KeepOffline::Weeks2);
    assert_eq!(s.mailbox.device_limits.unwrap().storage, 1 << 30);
    // A cap out of bounds is brought within them, never refused nor overflowing.
    crate::limits::set_storage(&mut s, u64::MAX).unwrap();
    assert_eq!(s.mailbox.device_limits.unwrap().storage, mailcraft_model::device_limits::MAX_STORAGE);
    crate::limits::set_storage(&mut s, 0).unwrap();
    assert_eq!(s.mailbox.device_limits.unwrap().storage, mailcraft_model::device_limits::MIN_STORAGE);
    assert_eq!(DownloadSettings::default(), DownloadSettings { window: KeepWindow::Month1, storage_mb: 500 });
}

#[test]
fn every_window_goes_both_ways() {
    for k in KeepOffline::STEPS {
        assert_eq!(KeepOffline::from(KeepWindow::from(k)), k);
    }
}
