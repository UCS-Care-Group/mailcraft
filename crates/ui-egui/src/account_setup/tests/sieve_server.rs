// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #2057: Account Settings names the ManageSieve server an IMAP account's Automatic Replies are
//! kept on, when it isn't the incoming server on port 4190.

use super::*;

fn settings() -> (Harness<'static, MailApp>, Id) {
    let mut a = app(true);
    let id = a.execute("account.add", json!({"email": "noor@lumen.example", "incoming": {"host": "imap.lumen.example", "security": "tls"}})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    (open(a, json!({"id": id})), id)
}

#[test]
fn the_owner_names_another_sieve_server_and_only_that_is_changed() {
    let (mut h, id) = settings();
    h.run();
    assert!(h.query_by_label("Sieve server (Automatic Replies)").is_some());
    // Nothing typed: nothing to change.
    assert_eq!(wizard(&h).unwrap().edit_command(), None);
    wizard_mut(&mut h).sieve_server = "sieve.lumen.example:4191".into();
    h.run();
    assert_eq!(wizard(&h).unwrap().edit_command(), Some(("account.edit", json!({"id": id, "sieve": {"host": "sieve.lumen.example", "port": 4191}}))));
    wizard_mut(&mut h).sieve_server = "sieve.lumen.example".into();
    assert_eq!(wizard(&h).unwrap().edit_command(), Some(("account.edit", json!({"id": id, "sieve": {"host": "sieve.lumen.example", "port": 0}}))));
    // What can't be a server is said, and nothing goes.
    for wrong in ["a b", "x/y", "host:0", "host:99999", "host:port"] {
        wizard_mut(&mut h).sieve_server = wrong.into();
        assert_eq!(wizard(&h).unwrap().edit_command(), None, "{wrong}");
    }
}

#[test]
fn a_named_sieve_server_is_shown_again_and_cleared_back_to_the_incoming_one() {
    let mut a = app(true);
    let id = a.execute("account.add", json!({"email": "noor@lumen.example", "incoming": {"host": "imap.lumen.example", "security": "tls"}})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    a.execute("account.edit", json!({"id": id, "sieve": {"host": "sieve.lumen.example", "port": 2000}})).unwrap();
    let mut h = open(a, json!({"id": id}));
    h.run();
    assert_eq!(wizard(&h).unwrap().sieve_server, "sieve.lumen.example:2000");
    assert_eq!(wizard(&h).unwrap().edit_command(), None);
    wizard_mut(&mut h).sieve_server.clear();
    assert_eq!(wizard(&h).unwrap().edit_command(), Some(("account.edit", json!({"id": id, "sieve": null}))));
}

/// An IPv6 address named for the Sieve server, with a port, is shown in brackets, so that the port
/// is read back as the port and nothing seems changed (#2058).
#[test]
fn an_ipv6_sieve_server_with_a_port_is_shown_in_brackets() {
    let mut a = app(true);
    let id = a.execute("account.add", json!({"email": "noor@lumen.example", "incoming": {"host": "imap.lumen.example", "security": "tls"}})).unwrap()
        ["id"]
        .as_u64()
        .unwrap();
    a.execute("account.edit", json!({"id": id, "sieve": {"host": "2001:db8::1", "port": 4191}})).unwrap();
    let mut h = open(a, json!({"id": id}));
    h.run();
    assert_eq!(wizard(&h).unwrap().sieve_server, "[2001:db8::1]:4191");
    assert_eq!(wizard(&h).unwrap().edit_command(), None);
    assert_eq!(host_port("sieve.lumen.example", 2000), "sieve.lumen.example:2000");
    assert_eq!(host_port("[2001:db8::1]", 2000), "[2001:db8::1]:2000");
}
