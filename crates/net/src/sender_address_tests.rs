// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail from one of an account's other sender addresses (#797) goes through the account's own
//! SMTP sign-in, from that address on the envelope and in its From line, and its copy is kept in
//! the account's Sent Items. A From that isn't one of the account's never reaches the envelope.

use mailcraft_model::SenderAddress;

use super::*;

const ALIAS: &str = "support@product.example";

/// The account of the other tests, which sends from `ALIAS` too.
fn with_alias(imap: Option<&FakeImap>, smtp: Option<&FakeSmtp>) -> Account {
    let alias = SenderAddress { email: ALIAS.into(), display_name: "Product Support".into(), signature: String::new() };
    Account { sender_addresses: vec![alias], ..account(imap, smtp) }
}

fn from(address: Address) -> Message {
    Message { from: address, date: SENT_AT, message_id: "797@product.example".into(), ..hello() }
}

#[test]
fn mail_from_a_sender_address_goes_from_it_over_the_same_sign_in() {
    let smtp = Fake::smtp(smtp_world());
    let account = with_alias(None, Some(&smtp));
    assert_eq!(send(&account, PASSWORD, &from(Address::new("Product Support", "Support@Product.example"))), Ok(()));
    let world = smtp.world();
    // Signed in as the account, as always.
    assert_eq!(world.sign_ins, 1);
    let went = &world.delivered[0];
    // The envelope says the address as the account keeps it; the message says it as written.
    assert_eq!(went.from, ALIAS);
    assert!(went.data.contains("From: Product Support <Support@Product.example>\r\n"), "{}", went.data);
}

#[test]
fn a_from_that_isnt_the_accounts_never_reaches_the_envelope() {
    let smtp = Fake::smtp(smtp_world());
    let account = with_alias(None, Some(&smtp));
    for (written, envelope) in [
        (Address::new("Ada", "ada@example.org"), "ada@example.org"),
        (Address::new("", ""), "ada@example.org"),
        (Address::new("Boss", "ceo@example.org"), "ada@example.org"),
        (Address::new("", "support+x@product.example"), "ada@example.org"),
    ] {
        assert_eq!(send(&account, PASSWORD, &from(written.clone())), Ok(()), "{written:?}");
        assert_eq!(smtp.world().delivered.last().unwrap().from, envelope, "{written:?}");
    }
    // An account that doesn't take sender addresses sends from its own whatever its list says.
    let local = Account { kind: AccountKind::Local, ..with_alias(None, Some(&smtp)) };
    assert_eq!(send(&local, PASSWORD, &from(Address::new("", ALIAS))), Ok(()));
    assert_eq!(smtp.world().delivered.last().unwrap().from, "ada@example.org");
}

#[test]
fn its_copy_is_kept_in_the_accounts_sent_items() {
    let imap = two_way(true, true);
    let smtp = Fake::smtp(smtp_world());
    let folders = synced_folders(&imap);
    let outbox = vec![Message { id: 501, ..from(Address::new("Product Support", ALIAS)) }];
    let result = sync(&SyncRequest { outbox, ..request(with_alias(Some(&imap), Some(&smtp)), folders) });
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!((result.sent.as_slice(), result.filed.as_slice()), (&[501][..], &[501][..]));
    assert_eq!(smtp.world().delivered[0].from, ALIAS);
    let kept = imap.world().mailbox("Sent Items").messages.last().map(|m| String::from_utf8_lossy(&m.body).into_owned()).unwrap();
    assert!(kept.contains(&format!("From: Product Support <{ALIAS}>\r\n")), "{kept}");
}
