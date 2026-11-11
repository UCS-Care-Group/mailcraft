// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn legacy_outlook_custom_domain_mx_uses_the_consumer_preset() {
    for host in ["123456789.pamx1.hotmail.com", " 123456789.PAMX1.HOTMAIL.COM. ", "pamx1.hotmail.com"] {
        assert_eq!(by_mx(host), Some(microsoft(true)), "{host}");
    }
}

#[test]
fn legacy_outlook_mx_matching_requires_a_domain_boundary() {
    for host in ["evilpamx1.hotmail.com", "123456789.pamx1.hotmail.com.evil.example", "mx.hotmail.com", "hotmail.com"] {
        assert_eq!(by_mx(host), None, "{host}");
    }
    assert_eq!(by_mx("example-org.mail.protection.outlook.com"), Some(microsoft(false)));
}
