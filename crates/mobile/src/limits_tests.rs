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

/// A phone's mailbox with a Microsoft account whose Inbox the server has, holding mail received
/// `days_ago`; the account.
fn with_mail(days_ago: &[i64]) -> Session {
    use mailcraft_model::sync::ServerMessage;
    use mailcraft_model::{FolderKind, Message, ServerFolder};
    let (mut s, account) = before_the_limits();
    s.prefs.sync_minutes = 0;
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "in".into(), ..Default::default() });
    let now = s.now();
    for (n, d) in days_ago.iter().enumerate() {
        let server = ServerMessage { id: format!("M{n}"), folder: "in".into(), ..Default::default() };
        let m =
            Message { account, folder: inbox, received: now - d * 86_400, subject: format!("{d} days"), server: Some(server), ..Default::default() };
        s.mailbox.add_message(m).unwrap();
    }
    s
}

#[test]
fn a_phone_opened_with_years_of_headers_is_thinned_to_its_window_between_runs() {
    let platform = crate::fake_tests::FakePlatform::new();
    let core = crate::Core::start_with(platform, || with_mail(&[1, 10, 100, 1000]), mailcraft_host::sync::SendReceive::new()).unwrap();
    let sync = core.sync.clone();
    let subjects = crate::fake_tests::block_on(core.call(move |s| {
        sync.tick(s);
        let mut left: Vec<String> = s.mailbox.messages.values().map(|m| m.subject.clone()).collect();
        left.sort();
        Ok(left)
    }))
    .unwrap();
    assert_eq!(subjects, ["1 days", "10 days"], "the month is kept; what is older left the phone");
}

#[test]
fn a_trimming_step_is_due_once_and_again_when_asked() {
    let mut s = with_mail(&[100]);
    settle(&mut s, None);
    let mut t = crate::limits::Trimming::default();
    assert!(!t.step(&mut s), "nothing more is left");
    assert!(s.mailbox.messages.is_empty());
    let mut more = with_mail(&[100]);
    settle(&mut more, None);
    assert!(!t.step(&mut more), "not due again so soon");
    assert_eq!(more.mailbox.messages.len(), 1);
    t.due = true;
    t.step(&mut more);
    assert!(more.mailbox.messages.is_empty());
}
