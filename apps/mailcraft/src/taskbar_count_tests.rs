// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tests of `taskbar_count.rs`.

#[test]
fn the_dot_is_a_red_point_on_a_clear_square() {
    let bytes = super::dot_icon();
    assert_eq!(&bytes[..6], &[0, 0, 1, 0, 1, 0]);
    assert_eq!(bytes.len(), 22 + 40 + 16 * 16 * 4 + 4 * 16);
    let at = |x: usize, y: usize| {
        let row = 15 - y;
        let i = 62 + row * 64 + x * 4;
        [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]
    };
    assert_eq!(at(0, 0), [0, 0, 0, 0]);
    assert_eq!(at(15, 15), [0, 0, 0, 0]);
    assert_eq!(at(7, 7), [32, 32, 220, 255]);
}

#[test]
fn the_same_dot_waits_to_be_sent_again_and_a_change_does_not() {
    let now = std::time::Instant::now();
    let soon = Some(now + std::time::Duration::from_secs(30));
    let later = Some(now + std::time::Duration::from_secs(3));
    assert!(!super::send_overlay(true, true, now, None, soon));
    assert!(super::send_overlay(true, true, now + std::time::Duration::from_secs(30), None, soon));
    assert!(super::send_overlay(false, true, now, None, soon));
    assert!(!super::send_overlay(false, true, now, later, None));
    assert!(!super::send_overlay(false, false, now, None, None));
}
