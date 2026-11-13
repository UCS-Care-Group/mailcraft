// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::SecretError;

#[test]
fn errors_say_nothing_of_the_secret() {
    let e = SecretError::Other("could not keep it");
    assert_eq!(e.to_string(), "could not keep it");
    assert_eq!(format!("{e:?}"), "Other(..)");
    assert_eq!(format!("{:?}", SecretError::Refused), "Refused");
    assert!(SecretError::TooLong.to_string().contains("too long"));
}
