// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Delivery status notifications (RFC 3461): asked for only when EHLO offers DSN.

use super::*;
use crate::script::Script;

const MESSAGE: &[u8] = b"Subject: Hi\r\n\r\nHello.\r\n";

#[test]
fn a_server_that_offers_dsn_is_asked_with_ret_hdrs_and_notify() {
    let mut s = Script::new(
        "220 ready\r\n250-mail.example.org\r\n250-SIZE 100000\r\n250-DSN\r\n250 HELP\r\n250 OK\r\n250 OK\r\n250 OK\r\n354 go\r\n250 queued\r\n",
    );
    let mut smtp = Smtp::connect(&mut s).unwrap();
    let ext = smtp.hello("localhost").unwrap();
    assert!(ext.dsn);
    assert!(smtp.send_with("ada@example.org", &["bob@example.org", "carol@example.org"], MESSAGE, true).unwrap());
    assert_eq!(
        s.sent(),
        "EHLO localhost\r\n\
         MAIL FROM:<ada@example.org> SIZE=23 RET=HDRS\r\n\
         RCPT TO:<bob@example.org> NOTIFY=SUCCESS,FAILURE\r\n\
         RCPT TO:<carol@example.org> NOTIFY=SUCCESS,FAILURE\r\n\
         DATA\r\n\
         Subject: Hi\r\n\r\nHello.\r\n.\r\n"
    );
}

#[test]
fn a_server_that_does_not_offer_dsn_is_not_asked_and_the_message_still_goes() {
    let mut s = Script::new("220 ready\r\n250-mail.example.org\r\n250-SIZE 100000\r\n250 HELP\r\n250 OK\r\n250 OK\r\n354 go\r\n250 queued\r\n");
    let mut smtp = Smtp::connect(&mut s).unwrap();
    assert!(!smtp.hello("localhost").unwrap().dsn);
    assert!(!smtp.send_with("ada@example.org", &["bob@example.org"], MESSAGE, true).unwrap());
    let sent = s.sent();
    assert!(sent.contains("MAIL FROM:<ada@example.org> SIZE=23\r\n"), "{sent}");
    assert!(sent.contains("RCPT TO:<bob@example.org>\r\n"), "{sent}");
    assert!(!sent.contains("RET=") && !sent.contains("NOTIFY="), "{sent}");
}

#[test]
fn a_message_that_asks_for_none_sends_no_dsn_parameters() {
    let mut s = Script::new("220 ready\r\n250-mail.example.org\r\n250-dsn\r\n250 HELP\r\n250 OK\r\n250 OK\r\n354 go\r\n250 queued\r\n");
    let mut smtp = Smtp::connect(&mut s).unwrap();
    assert!(smtp.hello("localhost").unwrap().dsn);
    smtp.send("ada@example.org", &["bob@example.org"], MESSAGE).unwrap();
    let sent = s.sent();
    assert!(!sent.contains("RET=") && !sent.contains("NOTIFY="), "{sent}");
}
