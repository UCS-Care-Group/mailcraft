// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use crate::pop::looks_like_pop;

#[test]
fn pop_servers_are_recognised() {
    assert!(looks_like_pop("pop.example.org", 995));
    assert!(looks_like_pop("POP3.example.org", 0));
    assert!(looks_like_pop("pop3s.example.org", 993));
    assert!(looks_like_pop("mail.example.org", 110));
    assert!(looks_like_pop("mail.example.org", 995));
    assert!(looks_like_pop(" Pop.example.org ", 0));
}

#[test]
fn imap_servers_are_not() {
    assert!(!looks_like_pop("imap.example.org", 993));
    assert!(!looks_like_pop("popcorn.example.org", 993));
    assert!(!looks_like_pop("mail.example.org", 143));
    assert!(!looks_like_pop("mail.pop.example.org", 993));
    assert!(!looks_like_pop("", 0));
}

#[test]
fn hostile_input_never_panics() {
    let huge = "pop".repeat(100_000);
    assert!(!looks_like_pop(&huge, 993));
    assert!(!looks_like_pop(&format!("pop{}", ".a".repeat(200)), 993));
    for host in ["", ".", "..", "ｐｏｐ.example", "pöp.example", "\u{0}", "pop\u{202e}.example", "💥💥"] {
        let _ = looks_like_pop(host, 0);
        let _ = looks_like_pop(host, u16::MAX);
    }
}
