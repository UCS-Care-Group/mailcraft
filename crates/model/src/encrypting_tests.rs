// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Bcc copies of an Encrypted message (#30 part 7): their Message-IDs, who has had theirs, and
//! the message that goes to everybody else. Every name is invented.

use super::*;

fn message(to: &[&str], bcc: &[&str], sent: &[&str]) -> Message {
    let people = |all: &[&str]| all.iter().map(|a| Address::new("", *a)).collect::<Vec<_>>();
    let body = Original::new(b"Content-Type: application/pkcs7-mime\r\n\r\nx".to_vec()).unwrap_or_default();
    Message {
        message_id: "rota.1@example.org".into(),
        to: people(to),
        bcc: people(bcc),
        blind_copies: bcc.iter().filter(|a| !sent.contains(a)).map(|a| BlindCopy { address: (*a).to_string(), original: body.clone() }).collect(),
        blind_sent: sent.iter().map(|a| (*a).to_string()).collect(),
        ..Message::default()
    }
}

#[test]
fn each_copy_has_a_message_id_of_its_own_and_the_message_keeps_its_own() {
    let m = message(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &[]);
    let ids: Vec<String> = m.blind_messages(true).into_iter().map(|(_, c)| c.message_id).collect();
    assert_eq!(ids, ["rota.1-b1@example.org", "rota.1-b2@example.org"]);
    assert_eq!(m.message_id, "rota.1@example.org");
    assert_eq!(blind_message_id("", 0), "");
    assert_eq!(blind_message_id("no-host", 1), "no-host-b2");
}

#[test]
fn those_who_had_theirs_are_not_sent_another_and_the_main_message_leaves_the_bcc_line() {
    let m = message(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &["erin@example.org"]);
    assert_eq!(m.blind_addresses(), ["finn@example.org"]);
    let main = m.for_main();
    assert!(main.bcc.is_empty() && main.blind_copies.is_empty());
}

#[test]
fn a_message_to_bcc_recipients_alone_has_no_main_message() {
    assert!(message(&["zoe@example.org"], &["erin@example.org"], &[]).has_main_recipients());
    assert!(!message(&[], &["erin@example.org"], &[]).has_main_recipients());
    // (Nor after the first of them had theirs.)
    assert!(!message(&[], &["erin@example.org", "finn@example.org"], &["erin@example.org"]).has_main_recipients());
}

#[test]
fn a_copy_keeps_its_number_after_a_send_that_stopped_half_way() {
    // Erin's went; Finn's is made again and is never numbered as Erin's was.
    let m = message(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &["erin@example.org"]);
    let ids: Vec<String> = m.blind_messages(true).into_iter().map(|(_, c)| c.message_id).collect();
    assert_eq!(ids, ["rota.1-b2@example.org"]);
}

#[test]
fn a_message_that_goes_unencrypted_keeps_the_bcc_recipients_who_have_not_had_theirs() {
    // (No messages of their own to send: whoever hasn't had one is still on the Bcc line.)
    let mut m = message(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &["erin@example.org"]);
    m.blind_copies.clear();
    let main = m.for_main();
    assert_eq!(main.bcc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["finn@example.org"]);
}

#[test]
fn a_bcc_copys_message_id_names_its_message() {
    assert_eq!(blind_copy_base_message_id("<rota.1-b2@example.org>").as_deref(), Some("<rota.1@example.org>"));
    assert_eq!(blind_copy_base_message_id("rota-b10@example.org").as_deref(), Some("<rota@example.org>"));
    for not in ["<rota.1@example.org>", "<-b1@example.org>", "<rota-bx@example.org>", "<rota-b@example.org>", ""] {
        assert!(blind_copy_base_message_id(not).is_none(), "{not}");
    }
}
