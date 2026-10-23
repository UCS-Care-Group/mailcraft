// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! mbox files (RFC 4155, in the `mboxrd` flavour): messages one after another, each after a
//! `From ` line.

use mailcraft_model::Message;
use mailcraft_time::{DateTime, month_short};

use crate::{WriteOptions, header, parse, write};

/// Read an mbox file.
///
/// A message starts at a line that begins with `From ` and either follows an empty line (or
/// starts the file) or holds a time (`12:08:34`), as the separator lines of every mbox writer
/// do. In the messages `>From ` lines lose one `>` (the `mboxrd` rule, which also reads `mboxo`
/// files). Bytes before the first separator are read as a message too, so a lone `.eml` gives one
/// message. Each message is read with [`parse()`], which never fails.
pub fn parse_mbox(bytes: &[u8]) -> Vec<Message> {
    mbox_messages(bytes).collect()
}

/// The messages of an mbox file one at a time, read as [`parse_mbox`] reads them. Lets a caller
/// stop early (an import caps how many it takes) without every message being built first.
pub fn mbox_messages(bytes: &[u8]) -> MboxMessages<'_> {
    MboxMessages { bytes, pos: 0, current: None, previous_blank: true, done: false }
}

/// The iterator behind [`mbox_messages`].
pub struct MboxMessages<'a> {
    bytes: &'a [u8],
    pos: usize,
    current: Option<Vec<u8>>,
    previous_blank: bool,
    done: bool,
}

impl Iterator for MboxMessages<'_> {
    type Item = Message;

    fn next(&mut self) -> Option<Message> {
        while !self.done {
            if self.pos >= self.bytes.len() {
                self.done = true;
                if let Some(m) = finish(self.current.take()) {
                    return Some(m);
                }
                break;
            }
            let (line, next) = header::line_at(self.bytes, self.pos);
            let mut out = None;
            if line.starts_with(b"From ") && (self.previous_blank || has_time(line)) {
                out = finish(self.current.take());
                self.current = Some(Vec::new());
            } else {
                let whole = self.bytes.get(self.pos..next).unwrap_or_default();
                // `>From ` was `From ` before it was stored; `>>From ` was `>From `.
                let quoted =
                    line.iter().position(|b| *b != b'>').is_some_and(|i| i > 0 && line.get(i..).is_some_and(|rest| rest.starts_with(b"From ")));
                let unquoted = if quoted { whole.get(1..).unwrap_or_default() } else { whole };
                match &mut self.current {
                    Some(message) => message.extend_from_slice(unquoted),
                    // Before any separator: blank lines are nothing, anything else is a message.
                    None if line.is_empty() => {}
                    None => self.current = Some(unquoted.to_vec()),
                }
            }
            self.previous_blank = line.is_empty();
            self.pos = next;
            if out.is_some() {
                return out;
            }
        }
        None
    }
}

/// Parse a message cut out of an mbox, without the empty line that ended it there.
fn finish(message: Option<Vec<u8>>) -> Option<Message> {
    let mut bytes = message?;
    if bytes.ends_with(b"\n\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n\n") {
        bytes.truncate(bytes.len() - 1);
    }
    if bytes.iter().all(u8::is_ascii_whitespace) { None } else { Some(parse(&bytes)) }
}

/// Whether a line holds a clock time (`hh:mm`), as mbox separator lines do.
fn has_time(line: &[u8]) -> bool {
    line.windows(5).any(|w| matches!(w, [a, b, b':', c, d] if a.is_ascii_digit() && b.is_ascii_digit() && c.is_ascii_digit() && d.is_ascii_digit()))
}

/// Write messages as an mbox file.
///
/// Each message is what [`write()`] gives (CRLF line ends, kept so attachments survive byte for
/// byte), after a `From sender date` line and before an empty line. Lines that begin with
/// `From `, or with `>From `, get one more `>` (the `mboxrd` rule), which [`parse_mbox`] takes
/// off again.
pub fn write_mbox(messages: &[&Message], opts: &WriteOptions) -> Vec<u8> {
    let mut out = Vec::new();
    for message in messages {
        let sender: String = message.from.email.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect();
        let t = DateTime::from_timestamp(message.date, 0);
        let (date, sender) = (t.date, if sender.is_empty() { "MAILER-DAEMON" } else { sender.as_str() });
        let stamp = format!(
            "{} {} {:>2} {:02}:{:02}:{:02} {:04}",
            date.weekday().short(),
            month_short(date.month),
            date.day,
            t.hour,
            t.minute,
            t.second,
            date.year
        );
        out.extend_from_slice(format!("From {sender} {stamp}\r\n").as_bytes());
        let bytes = write(message, opts);
        let mut pos = 0;
        while pos < bytes.len() {
            let (line, next) = header::line_at(&bytes, pos);
            let text = line.iter().position(|b| *b != b'>').and_then(|i| line.get(i..)).unwrap_or_default();
            if text.starts_with(b"From ") {
                out.push(b'>');
            }
            out.extend_from_slice(bytes.get(pos..next).unwrap_or_default());
            pos = next;
        }
        if !out.ends_with(b"\n") {
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use mailcraft_model::{Address, Body};

    use super::*;

    fn message(subject: &str, text: &str) -> Message {
        Message {
            from: Address { name: "Ada".into(), email: "ada@example.org".into() },
            subject: subject.into(),
            date: 1_057_049_557,
            received: 1_057_049_557,
            body: Body { text: text.into(), html: None },
            ..Message::default()
        }
    }

    #[test]
    fn reads_a_unix_mbox() {
        let mbox = b"From ada@example.org Tue Jul  1 08:52:37 2003\n\
Subject: One\n\
Status: RO\n\
\n\
First body\n\
>From the start, quoted\n\
>>From twice\n\
> From not quoted\n\
\n\
From bob@example.org Wed Jul  2 09:00:00 2003\n\
Subject: Two\n\
\n\
Second body\n";
        let got = parse_mbox(mbox);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].subject.as_str(), got[0].read), ("One", true));
        assert_eq!(got[0].body.text, "First body\nFrom the start, quoted\n>From twice\n> From not quoted");
        assert_eq!((got[1].subject.as_str(), got[1].read), ("Two", false));
        assert_eq!(got[1].body.text, "Second body");
    }

    #[test]
    fn separators_need_a_blank_line_or_a_time() {
        // No blank line between the messages, but the separator is dated.
        let tight =
            b"From a@example.org Tue Jul  1 08:52:37 2003\nSubject: One\n\nx\nFrom b@example.org Wed Jul  2 09:00:00 2003\nSubject: Two\n\ny\n";
        assert_eq!(parse_mbox(tight).len(), 2);
        // A body line that starts with "From " in the middle of a paragraph is text.
        let prose = b"From a@example.org Tue Jul  1 08:52:37 2003\nSubject: One\n\nHello\nFrom now on we meet on Fridays\n";
        let got = parse_mbox(prose);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body.text, "Hello\nFrom now on we meet on Fridays");
    }

    #[test]
    fn empty_and_odd_files() {
        assert!(parse_mbox(b"").is_empty());
        assert!(parse_mbox(b"\n\n\r\n").is_empty());
        assert!(parse_mbox(b"From a@example.org Tue Jul  1 08:52:37 2003\n").is_empty());
        assert!(parse_mbox(b"From \n\nFrom \n\nFrom ").is_empty());
        // A `From ` line in the middle of a paragraph, without a time, is content.
        assert_eq!(parse_mbox(b"From \nFrom \n\nFrom ").len(), 1);
        // A lone message without a separator.
        let got = parse_mbox(b"\nSubject: Lone\n\nbody\n");
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].subject.as_str(), got[0].body.text.as_str()), ("Lone", "body"));
    }

    #[test]
    fn writes_separators_and_quotes_from_lines() {
        let m = message("Hi", "From here\n>From there\nplain");
        let out = String::from_utf8(write_mbox(&[&m], &WriteOptions::default())).unwrap();
        assert!(
            out.starts_with(
                "From ada@example.org Tue Jul  1 08:52:37 2003\r\nDate: Tue, 01 Jul 2003 08:52:37 +0000\r\nFrom: Ada <ada@example.org>\r\n"
            ),
            "{out}"
        );
        assert!(out.contains("\r\n\r\n>From here\r\n>>From there\r\nplain\r\n\r\n"), "{out}");
        assert!(out.ends_with("plain\r\n\r\n"));
        let nobody = Message { from: Address::default(), date: 0, ..m.clone() };
        let out = String::from_utf8(write_mbox(&[&nobody], &WriteOptions::default())).unwrap();
        assert!(out.starts_with("From MAILER-DAEMON Thu Jan  1 00:00:00 1970\r\n"));
        assert!(write_mbox(&[], &WriteOptions::default()).is_empty());
    }

    #[test]
    fn a_written_mbox_reads_back() {
        let one = message("One", "From the top\n\nFrom after a blank line\n>From quoted already\n");
        let two = message("Two — ünïcode", "Grüße\n\nFrom München");
        let three = message("", "");
        let out = write_mbox(&[&one, &two, &three], &WriteOptions::default());
        let got = parse_mbox(&out);
        assert_eq!(got, [one, two, three]);
    }

    #[test]
    fn hostile_senders_cannot_break_the_separator() {
        let mut m = message("x", "y");
        m.from.email = "a@example.org\r\nFrom evil@example.org Tue Jul  1 08:52:37 2003".into();
        let out = write_mbox(&[&m], &WriteOptions::default());
        assert_eq!(parse_mbox(&out).len(), 1);
    }

    proptest::proptest! {
        #[test]
        fn any_bytes_read(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..600)) {
            let _ = parse_mbox(&data);
        }

        #[test]
        fn from_soup_reads(s in "(From |>From |>|\n|\r\n|a|12:34| |: ){0,60}") {
            let _ = parse_mbox(s.as_bytes());
        }
    }
}
