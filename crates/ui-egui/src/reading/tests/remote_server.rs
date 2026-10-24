// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Whose `Authentication-Results` header is believed when a remembered sender's pictures are
//! downloaded: only the topmost one, and only when the account's own server wrote it.
//!
//! The hosts and addresses are invented.

use mailcraft_model::{Account, AccountKind, Address, Message, Security, Server};

use crate::reading::remote::vouched;

fn account(kind: AccountKind, host: &str) -> Account {
    let incoming = (!host.is_empty()).then(|| Server { host: host.into(), port: 993, security: Security::Tls });
    Account { kind, incoming, ..Account::default() }
}

fn imap(host: &str) -> Account {
    account(AccountKind::Imap, host)
}

fn message(headers: &[&str]) -> Message {
    Message {
        from: Address::new("Shop", "news@shop.example"),
        headers: headers.iter().map(|value| ("Authentication-Results".to_string(), value.to_string())).collect(),
        ..Message::default()
    }
}

const PASS: &str = "dmarc=pass header.from=shop.example";

#[test]
fn gmail_vouches_with_its_own_id() {
    let gmail = imap("imap.gmail.com");
    assert!(vouched(Some(&gmail), &message(&[&format!("mx.google.com; dkim=pass header.i=@shop.example; {PASS}")])));
    // Case and a trailing dot are the same name.
    assert!(vouched(Some(&gmail), &message(&[&format!("MX.Google.com.; {PASS}")])));
}

#[test]
fn a_forged_google_id_is_nothing_on_another_account() {
    let forged = message(&[&format!("mx.google.com; {PASS}")]);
    assert!(!vouched(Some(&imap("imap.lumen.example")), &forged));
    assert!(!vouched(Some(&imap("imap.fastmail.example")), &forged));
    assert!(!vouched(Some(&account(AccountKind::Local, "")), &forged));
    assert!(!vouched(None, &forged));
}

#[test]
fn no_id_counts_for_a_microsoft_account_alone() {
    let bare = message(&["spf=pass smtp.mailfrom=shop.example; dmarc=pass action=none header.from=shop.example; compauth=pass reason=100"]);
    assert!(vouched(Some(&account(AccountKind::Microsoft, "")), &bare));
    assert!(vouched(Some(&imap("outlook.office365.com")), &bare));
    assert!(vouched(Some(&imap("outlook.office.com")), &bare));
    assert!(!vouched(Some(&imap("imap.lumen.example")), &bare));
    assert!(!vouched(Some(&imap("imap.gmail.com")), &bare));
}

#[test]
fn a_company_host_vouches_for_names_under_its_domain() {
    let lumen = imap("imap.lumen.example");
    assert!(vouched(Some(&lumen), &message(&[&format!("mx.lumen.example; {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("lumen.example; {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("a.b.lumen.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("mx.other.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("mx.notlumen.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("lumen.example.evil.example; {PASS}")])));
}

#[test]
fn a_bare_suffix_is_not_a_company() {
    for host in ["imap.co.uk", "mail.com.au", "imap.github.io", "localhost", "lumen.example", "imap"] {
        let account = imap(host);
        for id in ["mx.co.uk", "co.uk", "mx.com.au", "mx.example", "example", "evil.co.uk", "evil.github.io"] {
            assert!(!vouched(Some(&account), &message(&[&format!("{id}; {PASS}")])), "{host} {id}");
        }
    }
    // Under a suffix, the domain itself is a company.
    assert!(vouched(Some(&imap("imap.lumen.co.uk")), &message(&[&format!("mx.lumen.co.uk; {PASS}")])));
    assert!(!vouched(Some(&imap("imap.lumen.co.uk")), &message(&[&format!("mx.other.co.uk; {PASS}")])));
}

#[test]
fn comments_and_a_version_do_not_hide_the_id() {
    let lumen = imap("imap.lumen.example");
    assert!(vouched(Some(&lumen), &message(&[&format!("(a (nested) note) mx.lumen.example 1 (v1); {PASS}")])));
    assert!(vouched(Some(&lumen), &message(&[&format!("mx.lumen.example (note; with a semicolon); {PASS}")])));
    // A comment is not the id, and the id is not hidden inside one.
    assert!(!vouched(Some(&lumen), &message(&[&format!("(mx.lumen.example) mx.other.example; {PASS}")])));
    assert!(!vouched(Some(&lumen), &message(&[&format!("(mx.lumen.example; {PASS})")])));
}

#[test]
fn only_the_topmost_header_counts() {
    let lumen = imap("imap.lumen.example");
    // The server's line is on top and says no; a forged line below says yes.
    assert!(!vouched(Some(&lumen), &message(&["mx.lumen.example; dmarc=fail header.from=shop.example", &format!("mx.lumen.example; {PASS}")])));
    // The sender's line is on top, the server added none.
    assert!(!vouched(Some(&lumen), &message(&[&format!("forged.example; {PASS}"), &format!("mx.lumen.example; {PASS}")])));
}

#[test]
fn an_id_that_is_no_host_name_is_nobody() {
    let lumen = imap("imap.lumen.example");
    for value in
        ["", ";", "mx.lumen.example@x; dmarc=pass", "mx lumen.example; dmarc=pass header.from=shop.example", "dmarc=pass header.from=shop.example"]
    {
        assert!(!vouched(Some(&lumen), &message(&[value])), "{value}");
    }
}
