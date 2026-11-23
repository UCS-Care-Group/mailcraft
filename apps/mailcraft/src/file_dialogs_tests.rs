// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signature picker starts in the owner's home, independently of the last attachment picked.

use super::signature_home;

#[test]
fn signature_picker_uses_the_platforms_home_directory() {
    let home = Some("/home/example".into());
    let profile = Some("C:\\Users\\Example".into());
    assert_eq!(signature_home(home.clone(), profile.clone(), false), Some("/home/example".into()));
    assert_eq!(signature_home(home, profile, true), Some("C:\\Users\\Example".into()));
    assert_eq!(signature_home(None, None, false), None);
    assert_eq!(signature_home(Some("".into()), None, false), None);
}
