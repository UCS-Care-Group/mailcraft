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

#[test]
fn a_short_name_leaves_the_address_the_room_that_is_left() {
    assert_eq!(header_widths(400.0, 60.0, 450.0), (60.0, 340.0));
}

/// (#630) At a narrow width the name, the address, the External tag and the date are drawn
/// apart, never over one another.
#[test]
fn a_narrow_pane_draws_the_sender_the_tag_and_the_date_apart() {
    use egui_kittest::kittest::Queryable as _;
    let email = "newsletter.monthly.updates@harborbooks-and-stationery.example";
    for width in [960.0, 1060.0, 1280.0] {
        let mut m = super::tests::web_message(0, email, "Long", &[]);
        m.from = mailcraft_model::Address::new("Harbor Books and Stationery Supplies", email);
        let (app, ids) = super::tests::app_with(vec![m]);
        let mut harness = super::tests::window(app, (width, 800.0));
        super::tests::select(&mut harness, ids[0]);
        harness.run_steps(3);
        let tag = harness.state().hit.rect("externalTag", ids[0]).expect("the External tag is shown");
        let name = harness.get_by_label_contains("Harbor Books").rect();
        let address = harness.get_by_label_contains("<newsletter").rect();
        let date = harness.get_by_label_contains("2026").rect();
        let parts = [("name", name), ("address", address), ("tag", tag), ("date", date)];
        for (i, (a, ra)) in parts.iter().enumerate() {
            for (b, rb) in &parts[i + 1..] {
                assert!(!ra.shrink(0.5).intersects(rb.shrink(0.5)), "{width}: {a} {ra:?} over {b} {rb:?}");
            }
        }
    }
}
