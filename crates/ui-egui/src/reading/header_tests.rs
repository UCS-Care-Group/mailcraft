// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How the message header shares its row between the sender's name, address, tag and date (#630).

use super::{header_date_on_own_line, header_widths};

#[test]
fn name_and_address_keep_their_natural_width_when_they_fit() {
    assert_eq!(header_widths(500.0, 100.0, 150.0), (100.0, 150.0));
}

#[test]
fn the_address_gives_way_first_and_the_name_keeps_the_rest() {
    let (name, address) = header_widths(300.0, 120.0, 400.0);
    assert_eq!(address, 180.0);
    assert_eq!(name, 120.0);
    let (name, address) = header_widths(300.0, 250.0, 400.0);
    assert!(address < 400.0 && name < 250.0 && name + address <= 300.0 + 0.001);
}

#[test]
fn nothing_ever_adds_up_to_more_than_the_room() {
    for room in [-5.0, 0.0, 1.0, 40.0, 200.0, 900.0] {
        for name in [0.0, 30.0, 300.0, 5000.0] {
            for address in [0.0, 80.0, 700.0, f32::MAX] {
                let (n, a) = header_widths(room, name, address);
                assert!(n >= 0.0 && a >= 0.0 && n + a <= room.max(0.0) + 0.001, "{room} {name} {address}: {n} {a}");
            }
        }
    }
}

#[test]
fn a_very_narrow_row_puts_the_date_on_its_own_line() {
    assert!(header_date_on_own_line(250.0, 130.0));
    assert!(!header_date_on_own_line(600.0, 130.0));
}
