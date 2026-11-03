// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A POP server is told apart before anything connects (#402).
use super::*;

fn with_incoming(host: &str, port: &str) -> AccountSetup {
    let mut w = AccountSetup::blank(Page::Password);
    w.email = "ada@example.org".into();
    w.incoming = ServerFields { host: host.into(), port: port.into(), ..Default::default() };
    w
}

#[test]
fn a_pop_server_gets_a_plain_message_and_no_account() {
    for (host, port) in [("pop.example.org", "995"), ("POP3.example.org", ""), ("mail.example.org", "110")] {
        let w = with_incoming(host, port);
        let why = w.settings_problem(true).unwrap_or_default();
        assert!(why.starts_with("MailCraft doesn't support POP yet"), "{host}:{port}");
        assert!(w.account().is_none(), "{host}:{port} would have been tried as IMAP");
    }
}

#[test]
fn imap_servers_are_left_alone() {
    for (host, port) in [("imap.example.org", "993"), ("popcorn.example.org", "993"), ("mail.example.org", "143")] {
        assert_eq!(with_incoming(host, port).settings_problem(true), None, "{host}:{port}");
    }
}
