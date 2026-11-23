// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account's other sender addresses (#797): kept in its file, checked, and which of them a
//! reply is from.

use crate::senders::{SenderProblem, clean_sender_addresses};
use crate::{Account, AccountKind, Address, Mailbox, Message, SenderAddress};

fn sender(email: &str, name: &str) -> SenderAddress {
    SenderAddress { email: email.into(), display_name: name.into(), signature: String::new() }
}

/// An IMAP account (`ada@example.org`, "Ada Lovelace") with two other sender addresses.
fn ada() -> Account {
    Account {
        id: 1,
        email: "ada@example.org".into(),
        display_name: "Ada Lovelace".into(),
        kind: AccountKind::Imap,
        sender_addresses: vec![sender("Support@Product.example", "Product Support"), sender("ada@company.example", "")],
        ..Default::default()
    }
}

fn to(list: &[&str]) -> Vec<Address> {
    list.iter().map(|a| Address::new("", *a)).collect()
}

#[test]
fn a_file_from_before_has_none_and_an_account_without_any_writes_none() {
    let old = Mailbox::with_local_account("Ada", "ada@example.org");
    let json = serde_json::to_string(&old).unwrap();
    assert!(!json.contains("senderAddresses"), "{json}");
    let again: Mailbox = serde_json::from_str(&json).unwrap();
    assert!(again.accounts[0].sender_addresses.is_empty());
}

#[test]
fn the_list_round_trips_and_junk_in_a_file_is_dropped() {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let id = mb.add_account(ada());
    let json = serde_json::to_string(&mb).unwrap();
    assert!(
        json.contains(r#""senderAddresses":[{"email":"Support@Product.example","displayName":"Product Support"},{"email":"ada@company.example"}]"#),
        "{json}"
    );
    let again: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(again.account(id).unwrap().sender_addresses, ada().sender_addresses);
    // An entry that isn't one is left out; the rest of the account reads as it was.
    let junk = json.replace(r#"{"email":"ada@company.example"}"#, r#"42,{"email":["x"]},{"email":"ada@company.example"}"#);
    let again: Mailbox = serde_json::from_str(&junk).unwrap();
    assert_eq!(again.account(id).unwrap().sender_addresses, ada().sender_addresses);
    // Something that isn't a list at all is none.
    let junk = json.replace(r#""senderAddresses":["#, r#""senderAddresses":"no","x":["#);
    let again: Mailbox = serde_json::from_str(&junk).unwrap();
    assert!(again.account(id).unwrap().sender_addresses.is_empty());
}

#[test]
fn a_list_is_checked_before_it_is_kept() {
    let typed = [
        sender("  ada@product.example ", " Ada at\nProduct "),
        sender("ADA@product.example", "twice"),
        sender("Ada@Example.org", "the account's own"),
        sender("help@product.example", ""),
    ];
    let kept = clean_sender_addresses("ada@example.org", &typed).unwrap();
    assert_eq!(kept, [sender("ada@product.example", "Ada atProduct"), sender("help@product.example", "")]);
    for bad in ["", "ada", "ada@", "two words@example.org", "ada@example.org\r\nBcc: x@evil.example", "<ada@example.org>", "ädä@example.org"] {
        assert!(matches!(clean_sender_addresses("ada@example.org", &[sender(bad, "")]), Err(SenderProblem::NotAnAddress(_))), "{bad:?}");
    }
    let many: Vec<SenderAddress> = (0..=crate::MAX_SENDER_ADDRESSES).map(|n| sender(&format!("a{n}@example.org"), "")).collect();
    assert_eq!(clean_sender_addresses("ada@example.org", &many), Err(SenderProblem::TooMany));
    assert_eq!(clean_sender_addresses("ada@example.org", &many[1..]).unwrap().len(), crate::MAX_SENDER_ADDRESSES);
}

#[test]
fn only_an_imap_account_uses_them_and_only_those_that_are_addresses() {
    let mut a = ada();
    assert_eq!(a.sender_addresses().count(), 2);
    assert_eq!(a.from_address("support@product.example"), Address::new("Product Support", "Support@Product.example"));
    // Without a name of its own, the account's.
    assert_eq!(a.from_address("ada@company.example"), Address::new("Ada Lovelace", "ada@company.example"));
    for main in ["", "ada@example.org", "someone@else.example"] {
        assert_eq!(a.from_address(main), a.address(), "{main}");
    }
    assert!(a.sends_as("ADA@example.org") && a.sends_as("support@product.example") && !a.sends_as("") && !a.sends_as("x@example.org"));
    // One a file wrote that isn't an address is never used.
    a.sender_addresses.push(sender("bad\r\n@example.org", ""));
    assert_eq!(a.sender_addresses().count(), 2);
    for kind in [AccountKind::Local, AccountKind::Microsoft, AccountKind::Jmap, AccountKind::Exchange, AccountKind::Unknown] {
        let other = Account { kind, ..ada() };
        assert_eq!(other.sender_addresses().count(), 0, "{kind:?}");
        assert_eq!(other.from_address("support@product.example"), other.address());
    }
    let shared = Account { shared_of: Some(9), ..ada() };
    assert!(!shared.takes_sender_addresses());
}

#[test]
fn a_reply_is_from_the_address_the_original_was_sent_to() {
    let a = ada();
    let from = |m: &Message| a.reply_from(m);
    let customer = Address::new("Customer", "customer@example.net");
    // To, in any letter case.
    let m = Message { from: customer.clone(), to: to(&["SUPPORT@product.example"]), ..Default::default() };
    assert_eq!(from(&m), "Support@Product.example");
    // Cc.
    let m = Message { from: customer.clone(), to: to(&["someone@example.net"]), cc: to(&["ada@company.example"]), ..Default::default() };
    assert_eq!(from(&m), "ada@company.example");
    // The first of the account's addresses found: To before Cc, the account's own included.
    let m = Message { from: customer.clone(), to: to(&["ada@example.org", "support@product.example"]), ..Default::default() };
    assert_eq!(from(&m), "");
    let m = Message { from: customer.clone(), to: to(&["support@product.example"]), cc: to(&["ada@example.org"]), ..Default::default() };
    assert_eq!(from(&m), "Support@Product.example");
    // A message the owner sent from one is answered from it.
    let m = Message { from: Address::new("", "ada@company.example"), to: to(&["customer@example.net"]), ..Default::default() };
    assert_eq!(from(&m), "ada@company.example");
    // Plus-addressing matches only as it is kept.
    let m = Message { from: customer.clone(), to: to(&["support+urgent@product.example"]), ..Default::default() };
    assert_eq!(from(&m), "");
    // Nothing of the account's: its own address.
    let m = Message { from: customer.clone(), to: to(&["list@example.net"]), ..Default::default() };
    assert_eq!(from(&m), "");
}

#[test]
fn mail_to_an_alias_in_bcc_is_answered_from_it_by_its_delivery_headers() {
    let a = ada();
    let customer = Address::new("Customer", "customer@example.net");
    let headers = |list: &[(&str, &str)]| list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect::<Vec<_>>();
    // The mailbox's own address in Delivered-To says nothing; the address it was sent to does.
    let m = Message {
        from: customer.clone(),
        to: to(&["list@example.net"]),
        headers: headers(&[("X-Original-To", "support@product.example"), ("Delivered-To", "ada@example.org")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "Support@Product.example");
    let m = Message { from: customer.clone(), headers: headers(&[("delivered-to", "<ada@company.example>")]), ..Default::default() };
    assert_eq!(a.reply_from(&m), "ada@company.example");
    // To and Cc come first.
    let m = Message {
        from: customer.clone(),
        to: to(&["ada@example.org"]),
        headers: headers(&[("Delivered-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // Only those two headers, and a plus address only as it is.
    let m = Message {
        from: customer,
        headers: headers(&[("Received", "for support@product.example"), ("Delivered-To", "support+x@product.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // An account without sender addresses always answers from its own.
    let plain = Account { sender_addresses: Vec::new(), ..ada() };
    let m = Message { to: to(&["support@product.example"]), ..Default::default() };
    assert_eq!(plain.reply_from(&m), "");
}

#[test]
fn a_planted_delivery_header_lower_down_never_picks_the_reply_address() {
    let a = ada();
    let headers = |list: &[(&str, &str)]| list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect::<Vec<_>>();
    let customer = Address::new("Customer", "customer@example.net");
    // The server wrote the main address on top; a value the sender planted below must not win.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[
            ("X-Original-To", "ada@example.org"),
            ("X-Original-To", "ada@company.example"),
            ("Delivered-To", "support@product.example"),
        ]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // The same for Delivered-To alone.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[("Delivered-To", "ada@example.org"), ("delivered-to", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // A topmost value that is no sender address of the account is not looked past either.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[("X-Original-To", "list@example.net"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // A server that writes only Delivered-To (the main address, on top): an X-Original-To below it
    // came with the message.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[("Delivered-To", "ada@example.org"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // The topmost alias still wins.
    let m = Message {
        from: customer,
        headers: headers(&[("X-Original-To", "support@product.example"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "Support@Product.example");
}

#[test]
fn delivery_headers_below_a_received_header_are_never_read() {
    let a = ada();
    let headers = |list: &[(&str, &str)]| list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect::<Vec<_>>();
    let customer = Address::new("Customer", "customer@example.net");
    // A server that writes no delivery header: the one below the Received lines came with the message.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[("Received", "from a"), ("Received", "from b"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // A server that writes only Delivered-To, with an internal address that is neither the main
    // nor a sender address: the X-Original-To below its Received line is not looked at.
    let m = Message {
        from: customer.clone(),
        headers: headers(&[("Delivered-To", "ada@mail.internal.example"), ("Received", "from a"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "");
    // Above the first Received, an alias is still found.
    let m = Message {
        from: customer,
        headers: headers(&[("X-Original-To", "support@product.example"), ("Received", "from a"), ("X-Original-To", "ada@company.example")]),
        ..Default::default()
    };
    assert_eq!(a.reply_from(&m), "Support@Product.example");
}

#[test]
fn a_list_read_from_a_file_is_clamped_when_the_mailbox_is_repaired() {
    let mut mb = Mailbox::with_local_account("Ada", "ada@example.org");
    let mut many: Vec<SenderAddress> = vec![
        sender(
            "bad
@example.org",
            "",
        ),
        sender("ADA@example.org", ""),
        sender(
            "x@example.org",
            "Line
one",
        ),
    ];
    many.extend((0..80).map(|n| sender(&format!("a{n}@example.org"), &"n".repeat(500))));
    let id = mb.add_account(Account { sender_addresses: many, ..ada() });
    let local = mb.accounts[0].id;
    mb.account_mut(local).unwrap().sender_addresses = (0..80).map(|n| sender(&format!("kept{n}"), "")).collect();
    mb.repair();
    let kept = &mb.account(id).unwrap().sender_addresses;
    assert_eq!(kept.len(), crate::MAX_SENDER_ADDRESSES);
    assert_eq!(kept[0], sender("x@example.org", "Lineone"));
    assert!(kept.iter().all(|s| s.display_name.chars().count() <= crate::MAX_NAME));
    // Another kind's list is kept as its file had it, but for the bound.
    // (cut to the bound, never unbounded).
    let other = &mb.account(local).unwrap().sender_addresses;
    assert_eq!(other.len(), crate::MAX_SENDER_ADDRESSES);
    assert_eq!(other[0], sender("kept0", ""));
}
