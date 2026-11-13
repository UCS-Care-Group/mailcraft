// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::{Account, AccountKind, Server};

use crate::{legacy_secret_key, microsoft_secret_key, secret_key};

#[test]
fn a_microsoft_account_is_named_by_its_address_alone() {
    let account = Account { kind: AccountKind::Microsoft, email: " Ada@Lumen.Example ".to_string(), ..Default::default() };
    assert_eq!(secret_key("refresh", &account), "refresh:ada@lumen.example|graph");
    assert_eq!(secret_key("refresh", &account), microsoft_secret_key("refresh", "ada@lumen.example"));
}

#[test]
fn an_account_that_names_servers_is_named_by_them() {
    let server = |host: &str| Some(Server { host: host.to_string(), port: 993, ..Default::default() });
    let one = Account { email: "ada@lumen.example".to_string(), incoming: server("Imap.Lumen.Example"), ..Default::default() };
    let other = Account { incoming: server("imap.other.example"), ..one.clone() };
    assert!(secret_key("password", &one).contains("imap=imap.lumen.example:993"));
    assert_ne!(secret_key("password", &one), secret_key("password", &other));
    assert_eq!(legacy_secret_key("password", " Ada@Lumen.Example"), "password:ada@lumen.example");
}
