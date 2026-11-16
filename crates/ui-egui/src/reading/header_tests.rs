// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How the message header shares its row between the sender's name, address, tag and date (#630).

use super::header_date_on_own_line;

#[test]
fn a_very_narrow_row_puts_the_date_on_its_own_line() {
    assert!(header_date_on_own_line(250.0, 130.0));
    assert!(!header_date_on_own_line(600.0, 130.0));
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

/// A short sender name keeps its width when the address needs a separate line.
#[test]
fn a_compact_pane_gives_the_address_its_own_line() {
    use egui_kittest::kittest::Queryable as _;

    let email = "maya.okafor@long-name.example";
    let mut message = super::tests::web_message(0, email, "Sender details", &[]);
    message.from = mailcraft_model::Address::new("Maya Okafor", email);
    let (app, ids) = super::tests::app_with(vec![message]);
    let mut h = super::tests::window(app, (1280.0, 800.0));
    super::tests::select(&mut h, ids[0]);
    let name = |h: &egui_kittest::Harness<'static, crate::MailApp>| h.get_by_label("Maya Okafor").rect();
    let wide = name(&h);
    h.set_size(egui::vec2(850.0, 700.0));
    h.run_steps(5);
    let narrow = name(&h);
    let address = h.get_by_label("<maya.okafor@long-name.example>").rect();
    assert!((wide.width() - narrow.width()).abs() < 1.0, "{wide:?} → {narrow:?}");
    assert!(address.min.y >= narrow.max.y - 0.5, "{narrow:?} and {address:?}");
}
