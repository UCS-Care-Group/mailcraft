// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::device_limits::*;
use crate::{Account, AccountKind, DownloadInFull, KeepOffline, Mailbox};

const NOW: i64 = 1_792_000_000; // 2026-10-15

#[test]
fn a_phone_starts_with_its_defaults() {
    let l = DeviceLimits::phone(None);
    assert_eq!((l.storage, l.messages, l.defaults), (PHONE_STORAGE, DEFAULT_MESSAGES, PHONE_DEFAULTS));
    assert_eq!(PHONE_STORAGE, 500 << 20);
    assert_eq!(PHONE_KEEP, KeepOffline::Month1);
    assert_eq!(PHONE_DOWNLOAD.limit(), Some(1 << 20));
}

#[test]
fn the_message_cap_follows_the_memory_budget_within_bounds() {
    assert_eq!(messages_for(Some(256 << 20)), (256 << 20) / BYTES_PER_MESSAGE);
    assert_eq!(messages_for(Some(0)), MIN_MESSAGES);
    assert_eq!(messages_for(Some(u64::MAX)), MAX_MESSAGES);
}

#[test]
fn limits_from_a_file_are_clamped_and_junk_reads_as_none() {
    let l = DeviceLimits { storage: 1, messages: u64::MAX, defaults: 7 }.clamped();
    assert_eq!((l.storage, l.messages, l.defaults), (MIN_STORAGE, MAX_MESSAGES, 7));
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    mb.device_limits = Some(DeviceLimits::phone(Some(64 << 20)));
    let json = serde_json::to_value(&mb).unwrap();
    let back: Mailbox = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back.device_limits, mb.device_limits);
    let mut junk = json;
    junk["deviceLimits"] = serde_json::json!("lots");
    let back: Mailbox = serde_json::from_value(junk).unwrap();
    assert_eq!(back.device_limits, None, "what isn't limits is none");
    // A computer's mailbox writes nothing of them.
    let plain = serde_json::to_value(Mailbox::with_local_account("a@example.org", "A")).unwrap();
    assert!(plain.get("deviceLimits").is_none());
}

#[test]
fn only_a_device_with_limits_lists_from_the_window() {
    let mut mb = Mailbox::with_local_account("avery@mail.example", "Avery");
    let account = mb.add_account(Account {
        email: "noor@example.org".into(),
        kind: AccountKind::Microsoft,
        keep_offline: KeepOffline::Month1,
        ..Default::default()
    });
    assert_eq!(mb.listed_since(account, NOW), None, "a computer lists everything");
    mb.device_limits = Some(DeviceLimits::default());
    assert_eq!(mb.listed_since(account, NOW), KeepOffline::Month1.since(NOW));
    mb.accounts.iter_mut().find(|a| a.id == account).unwrap().keep_offline = KeepOffline::All;
    assert_eq!(mb.listed_since(account, NOW), None);
}

#[test]
fn the_one_megabyte_step_round_trips() {
    let v = serde_json::to_value(DownloadInFull::UpTo1Mb).unwrap();
    assert_eq!(v, serde_json::json!("upTo1Mb"));
    assert_eq!(serde_json::from_value::<DownloadInFull>(v).unwrap(), DownloadInFull::UpTo1Mb);
    assert_eq!(DownloadInFull::default(), DownloadInFull::UpTo10Mb, "a computer's default is unchanged");
}
