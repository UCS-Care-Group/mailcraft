// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The From line's list (#797): each account's address in the folder pane's order, its other
//! sender addresses under it, and a choice offered even with one account when it has some.

use mailcraft_model::{Account, AccountKind, Mailbox, SenderAddress};

use super::from_list::{FromChoice, choices, current, has_choice, is_current};

fn sender(email: &str, name: &str) -> SenderAddress {
    SenderAddress { email: email.into(), display_name: name.into(), signature: String::new() }
}

fn imap(email: &str, senders: Vec<SenderAddress>) -> Account {
    Account {
        name: email.into(),
        email: email.into(),
        display_name: "Jorgen".into(),
        kind: AccountKind::Imap,
        sender_addresses: senders,
        ..Default::default()
    }
}

fn choice(account: u64, from: &str, label: &str) -> FromChoice {
    FromChoice { account, from: from.into(), label: label.into() }
}

#[test]
fn one_account_without_other_addresses_offers_no_choice() {
    let mut mb = Mailbox::default();
    let id = mb.add_account(imap("jorgen@company.example", Vec::new()));
    assert!(!has_choice(&mb));
    assert_eq!(choices(&mb), [choice(id, "", "Jorgen <jorgen@company.example>")]);
    // With one, it does.
    mb.account_mut(id).unwrap().sender_addresses = vec![sender("support@product.example", "Product Support")];
    assert!(has_choice(&mb));
    assert_eq!(current(&mb, id, "SUPPORT@product.example"), "Product Support <support@product.example>");
    assert_eq!(current(&mb, id, ""), "Jorgen <jorgen@company.example>");
}

#[test]
fn each_account_is_followed_by_its_other_addresses_in_the_folder_panes_order() {
    let mut mb = Mailbox::default();
    let first = mb.add_account(imap(
        "jorgen@company.example",
        vec![sender("jorgen@product.example", ""), sender("support@product.example", "Product Support")],
    ));
    let second = mb.add_account(imap("jorgen@home.example", Vec::new()));
    // A Microsoft account's list is never offered: it sends only from its own address.
    let graph = mb.add_account(Account { kind: AccountKind::Microsoft, ..imap("jorgen@contoso.example", vec![sender("sales@contoso.example", "")]) });
    mb.move_account(second, Some(first)).unwrap();
    assert_eq!(
        choices(&mb),
        [
            choice(second, "", "Jorgen <jorgen@home.example>"),
            choice(first, "", "Jorgen <jorgen@company.example>"),
            choice(first, "jorgen@product.example", "Jorgen <jorgen@product.example>"),
            choice(first, "support@product.example", "Product Support <support@product.example>"),
            choice(graph, "", "Jorgen <jorgen@contoso.example>"),
        ]
    );
    let list = choices(&mb);
    assert!(is_current(&list[3], first, "Support@Product.example") && !is_current(&list[1], first, "support@product.example"));
}

#[test]
fn many_sender_addresses_scroll_inside_the_window() {
    use egui_kittest::kittest::Queryable as _;
    use serde_json::json;

    let mut a = super::tests::app();
    let acc = a.session.mailbox.accounts[0].id;
    a.session.mailbox.account_mut(acc).unwrap().kind = AccountKind::Imap;
    a.session.mailbox.account_mut(acc).unwrap().sender_addresses =
        (0..mailcraft_model::MAX_SENDER_ADDRESSES).map(|n| sender(&format!("alias{n}@company.example"), "")).collect();
    a.execute("mail.new", json!({"from": "alias49@company.example"})).unwrap();
    let mut h = super::tests::window(a);
    h.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    h.run_steps(3);
    // Open the From menu by clicking the From picker rect.
    let pick = h.state().hit.rect("composeFrom", 0).expect("From picker is shown when an account has sender addresses");
    let click_pos = pick.center();
    h.event(egui::Event::PointerMoved(click_pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: click_pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    let screen = h.ctx.content_rect();
    let menu = h.state().hit.rect("composeFromMenu", 0).expect("From popup menu opens and renders its scroll area");
    let items: Vec<_> = h.query_all_by_label_contains("@company.example").collect();
    assert!(!items.is_empty(), "From popup menu should list sender addresses");
    assert!(menu.height() <= 320.5 && menu.max.y <= screen.max.y, "From popup scroll area {menu:?} overflows window bottom {}", screen.max.y);
    let last = h.query_all_by_label_contains("alias49@company.example").next().expect("last sender address is present");
    assert!(
        last.rect().min.y >= menu.min.y - 1.0 && last.rect().max.y <= menu.max.y + 1.0,
        "selected sender address {:?} is scrolled into the menu viewport {:?}",
        last.rect(),
        menu
    );
    // Clicking the scrollbar area inside the popup keeps the menu open (`CloseOnClickOutside`).
    let scrollbar_pos = egui::pos2(menu.max.x - 2.0, menu.center().y);
    h.event(egui::Event::PointerMoved(scrollbar_pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: scrollbar_pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
    assert!(h.state().hit.rect("composeFromMenu", 0).is_some(), "From popup menu stays open after clicking inside its scroll area");
}
