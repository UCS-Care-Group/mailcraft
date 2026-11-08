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
