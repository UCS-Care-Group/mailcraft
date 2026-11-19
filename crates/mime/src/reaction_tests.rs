// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A reaction (RFC 9078, #1544): one emoji as a part marked `Content-Disposition: reaction`, with
//! the same emoji as plain text beside it; never read into a message that comes in.
//! (`read_back_the_emoji_is_the_text_once_and_nothing_is_marked` is a round trip of our own text and
//! also passes without the writer's change; the foreign-style test below reads another writer's.)

use crate::{Address, Body, Message};

use crate::{WriteOptions, parse, write};

const UP: &str = "\u{1F44D}";

fn message(text: &str, reaction: bool) -> Message {
    Message {
        from: Address { name: String::new(), email: "ada@example.org".into() },
        to: vec![Address { name: String::new(), email: "bo@example.org".into() }],
        subject: "RE: Plans".into(),
        in_reply_to: "plans@example.org".into(),
        body: Body { text: text.into(), html: None },
        reaction,
        ..Default::default()
    }
}

fn text(m: &Message) -> String {
    String::from_utf8(write(m, &WriteOptions::default())).unwrap()
}

#[test]
fn a_reaction_is_an_alternative_with_the_reaction_part_first() {
    let out = text(&message(UP, true));
    assert!(out.contains("Content-Type: multipart/alternative"), "{out}");
    assert_eq!(out.matches("Content-Disposition: reaction\r\n").count(), 1, "{out}");
    let reaction = out.find("Content-Disposition: reaction").unwrap();
    let last_plain = out.rmatch_indices("Content-Type: text/plain").next().unwrap().0;
    assert!(reaction < last_plain, "the reaction part comes first: {out}");
    // Both parts carry the emoji: UTF-8 text, quoted-printable.
    assert_eq!(out.matches("=F0=9F=91=8D").count(), 2, "{out}");
    assert_eq!(out.matches("Content-Type: text/plain; charset=utf-8").count(), 2, "{out}");
}

#[test]
fn what_is_not_one_is_written_as_the_plain_message_it_is() {
    assert!(!text(&message(UP, false)).contains("reaction"));
    for not_one in ["", "  ", "\u{1F44D} \u{2764}", "yes\nno", &"x".repeat(65)] {
        let out = text(&message(not_one, true));
        assert!(!out.contains("Content-Disposition"), "{not_one:?}: {out}");
    }
    // With HTML or a file it is more than a reaction.
    let mut m = message(UP, true);
    m.body.html = Some("<p>x</p>".into());
    assert!(!text(&m).contains("Content-Disposition: reaction"));
}

#[test]
fn read_back_the_emoji_is_the_text_once_and_nothing_is_marked() {
    let m = parse(text(&message(UP, true)).as_bytes());
    assert_eq!(m.body.text, UP);
    assert!(m.attachments.is_empty());
    assert!(!m.reaction, "mail that comes in is never taken for one");
}

/// Mail from another writer: a lone part marked as a reaction, and the alternative in another order
/// (plain text first). It is read as the plain message it is: the emoji as text, no attachment, and
/// nothing marked.
#[test]
fn a_reaction_another_program_wrote_is_read_as_plain_text() {
    let head = "From: bo@example.org\r\nTo: ada@example.org\r\nSubject: Re: Plans\r\nMessage-ID: <x@example.org>\r\nMIME-Version: 1.0\r\n";
    let single =
        format!("{head}Content-Type: text/plain; charset=utf-8\r\nContent-Disposition: reaction\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{UP}\r\n");
    let m = parse(single.as_bytes());
    assert_eq!(m.body.text.trim(), UP, "{m:?}");
    assert!(m.attachments.is_empty(), "{:?}", m.attachments);
    assert!(!m.reaction);
    let both = format!(
        "{head}Content-Type: multipart/alternative; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{UP}\r\n--b\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Disposition: reaction\r\n\r\n{UP}\r\n--b--\r\n"
    );
    let m = parse(both.as_bytes());
    assert_eq!(m.body.text.trim(), UP, "{m:?}");
    assert!(m.attachments.is_empty(), "{:?}", m.attachments);
    assert!(!m.reaction);
}
