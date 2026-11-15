// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Device limits (MB3.16): the setting.

use mailcraft_model::device_limits::{MIN_STORAGE, PHONE_STORAGE};
use serde_json::json;

use crate::Session;

#[test]
fn a_computer_has_none_until_they_are_set_and_off_takes_them_away() {
    let mut s = Session::empty();
    let said = s.run("account.deviceLimits", &json!({})).unwrap();
    assert_eq!(said["on"], false);
    assert!(s.mailbox.device_limits.is_none(), "reading changes nothing");
    let said = s.run("account.deviceLimits", &json!({"messages": 3000})).unwrap();
    assert_eq!((said["on"].clone(), said["storage"].clone(), said["messages"].clone()), (json!(true), json!(PHONE_STORAGE), json!(3000)));
    s.run("account.deviceLimits", &json!({"storage": MIN_STORAGE})).unwrap();
    assert_eq!(s.mailbox.device_limits.unwrap().storage, MIN_STORAGE);
    assert_eq!(s.mailbox.device_limits.unwrap().messages, 3000, "what isn't named stays");
    assert_eq!(s.run("account.deviceLimits", &json!({"off": true})).unwrap()["on"], false);
    assert!(s.mailbox.device_limits.is_none());
}

#[test]
fn values_out_of_bounds_are_refused_and_change_nothing() {
    let mut s = Session::empty();
    s.run("account.deviceLimits", &json!({"storage": PHONE_STORAGE})).unwrap();
    let before = s.mailbox.device_limits;
    for bad in [
        json!({"storage": 1}),
        json!({"storage": u64::MAX}),
        json!({"storage": "500"}),
        json!({"messages": 10}),
        json!({"messages": -1}),
        json!({"defaults": 1.5}),
        json!({"off": "yes"}),
        json!([1, 2]),
    ] {
        assert!(s.run("account.deviceLimits", &bad).is_err(), "{bad}");
        assert_eq!(s.mailbox.device_limits, before, "{bad}");
    }
}

#[test]
fn account_edit_takes_the_one_megabyte_step() {
    let mut s = Session::empty();
    let id = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("account.edit", &json!({"id": id, "downloadInFull": "upTo1Mb"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().download_in_full, mailcraft_model::DownloadInFull::UpTo1Mb);
}
