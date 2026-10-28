// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! [`Message`] to RFC 5322 + MIME bytes.

use mailcraft_model::{Address, Attachment, FlagStatus, Importance, Message};

use crate::header::{clean, is_token, write_field};
use crate::words::{WORD_BYTES, encode_b, has_words, is_plain};
use crate::{WriteOptions, address, charset, media, transfer};

/// Header fields the writer makes itself; fields of these names in `Message::headers` are skipped.
const OWN_FIELDS: [&str; 17] = [
    "date",
    "from",
    "to",
    "cc",
    "bcc",
    "reply-to",
    "subject",
    "message-id",
    "in-reply-to",
    "references",
    "importance",
    "x-priority",
    "keywords",
    "status",
    "x-status",
    "x-mailcraft-flag",
    "mime-version",
];

/// Whether a kept header (`lower`, in lower case) is a note of MailCraft's own that stays in
/// the mailbox: what was answered to a meeting (with this mailbox's mark), the file a message was
/// opened from. Neither goes into a file, a server's copy or what is sent. `X-MailCraft-Flag` is
/// written from the message's flag, and `X-MailCraft-Not-Downloaded` travels: it tells another
/// program that what it has is only the header of a message too large to download.
fn private(lower: &str) -> bool {
    lower.starts_with("x-mailcraft-") && lower != "x-mailcraft-not-downloaded"
}

/// Longest physical line a body may have without a transfer encoding (RFC 5322 §2.1.1).
const MAX_LINE: usize = 998;

/// Longest piece of a file name parameter, so that its header line folds within 78 characters.
const NAME_PIECE: usize = 52;

/// Write a message as RFC 5322 + MIME bytes, with CRLF line ends.
///
/// Header fields come in this order: `Date`, `From`, `To`, `Cc`, `Bcc` (only with
/// [`WriteOptions::include_bcc`]), `Reply-To`, `Subject`, `Message-ID`, `In-Reply-To`,
/// `References`, `Importance` and `X-Priority` (unless normal), `Keywords` (the categories),
/// `Status` and `X-Status` (read, answered, flagged), then `Message::headers` (`Resent-Bcc` only
/// with the Bcc), then `MIME-Version` and the content fields. Empty fields are left out.
///
/// No value can add a header field: CR, LF and NUL are removed from every value, addresses and
/// message ids lose every character that could end them early, and a header in
/// `Message::headers` whose name isn't a valid field name is dropped. Text that isn't printable
/// ASCII is written as encoded-words, and long lines are folded.
///
/// The body is `text/plain` when there is only text; `multipart/alternative` with the HTML when
/// there is both (only `text/html` when the text is blank); the HTML and the inline attachments
/// go in a `multipart/related`; other attachments wrap everything in a `multipart/mixed`. An
/// iCalendar part that is the body in another form (an invitation, the answer to one: an inline
/// `text/calendar` attachment that names its `METHOD`) is one more alternative,
/// `text/calendar; method=…`, which is where mail programs look for it. Text is
/// UTF-8, `7bit` when it can be and `quoted-printable` otherwise, with every line end written as
/// CRLF. Attachments are base64 in 76-column lines; an attached message (`message/rfc822`) that is
/// clean 7-bit text goes as it is, as RFC 2046 asks. Boundaries are made from the content, so the
/// same message always gives the same bytes.
pub fn write(message: &Message, opts: &WriteOptions) -> Vec<u8> {
    let mut out = Vec::new();
    write_field(&mut out, "Date", &mailcraft_time::format_rfc5322(message.date, opts.offset_minutes));
    addresses(&mut out, "From", std::slice::from_ref(&message.from));
    addresses(&mut out, "To", &message.to);
    addresses(&mut out, "Cc", &message.cc);
    if opts.include_bcc {
        addresses(&mut out, "Bcc", &message.bcc);
    }
    addresses(&mut out, "Reply-To", &message.reply_to);
    if !clean(&message.subject).is_empty() {
        write_field(&mut out, "Subject", &unstructured("Subject", &message.subject));
    }
    let ids = |list: &[String]| list.iter().map(|i| message_id(i)).filter(|i| !i.is_empty()).map(|i| format!("<{i}>")).collect::<Vec<_>>().join(" ");
    for (name, value) in [
        ("Message-ID", ids(std::slice::from_ref(&message.message_id))),
        ("In-Reply-To", ids(std::slice::from_ref(&message.in_reply_to))),
        ("References", ids(&message.references)),
    ] {
        if !value.is_empty() {
            write_field(&mut out, name, &value);
        }
    }
    match message.importance {
        Importance::High => {
            write_field(&mut out, "Importance", "high");
            write_field(&mut out, "X-Priority", "1");
        }
        Importance::Low => {
            write_field(&mut out, "Importance", "low");
            write_field(&mut out, "X-Priority", "5");
        }
        Importance::Normal => {}
    }
    let keywords: Vec<String> = message.categories.iter().map(|c| clean(c)).filter(|c| !c.is_empty()).map(|c| keyword(&c)).collect();
    if !keywords.is_empty() {
        write_field(&mut out, "Keywords", &keywords.join(", "));
    }
    if message.read {
        write_field(&mut out, "Status", "RO");
    }
    let status: String =
        [(message.answered, 'A'), (message.flag == FlagStatus::Flagged, 'F')].iter().filter(|(on, _)| *on).map(|(_, c)| *c).collect();
    if !status.is_empty() {
        write_field(&mut out, "X-Status", &status);
    }
    if message.flag == FlagStatus::Complete {
        // `X-Status` has no letter for a finished follow-up.
        write_field(&mut out, "X-MailCraft-Flag", "complete");
    }
    for (name, value) in &message.headers {
        let lower = name.to_ascii_lowercase();
        // Who got a blind copy of a resend goes only where the Bcc does.
        let blind = lower == "resent-bcc" && !opts.include_bcc;
        if is_token(name.as_bytes()) && !OWN_FIELDS.contains(&lower.as_str()) && !lower.starts_with("content-") && !private(&lower) && !blind {
            let value = if has_words(&lower) { unstructured(name, value) } else { kept(name, value) };
            write_field(&mut out, name, &value);
        }
    }
    write_field(&mut out, "MIME-Version", "1.0");
    let body = body(message);
    for (name, value) in &body.fields {
        write_field(&mut out, name, value);
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&body.content);
    out.extend_from_slice(b"\r\n");
    out
}

fn addresses(out: &mut Vec<u8>, name: &str, list: &[Address]) {
    let value = address::format_list(list);
    if !value.is_empty() {
        write_field(out, name, &value);
    }
}

/// Free text for a header field called `name`: as it is when it is plain, else encoded-words.
/// Text that begins or ends with a space is encoded too, because readers trim header values.
fn unstructured(name: &str, value: &str) -> String {
    let value = clean(value);
    let unfoldable = value.split(' ').any(|word| word.len() > MAX_LINE - 100);
    if is_plain(&value) && !value.starts_with(' ') && !value.ends_with(' ') && !unfoldable {
        return value;
    }
    // Leave room for the name on the first line.
    let room = 76usize.saturating_sub(name.len() + 2 + 12) / 4 * 3;
    encode_b(&value, if room < 9 { WORD_BYTES } else { room })
}

/// A kept field that is not unstructured text, which is read as it is written ([`has_words`]):
/// printable ASCII (and tabs) as it is, `=?` and all; anything else as [`unstructured`] text, so that what
/// is written stays seven-bit.
fn kept(name: &str, value: &str) -> String {
    let value = clean(value);
    let unfoldable = value.split(' ').any(|word| word.len() > MAX_LINE - 100);
    let edge = |c: char| c == ' ' || c == '\t';
    if value.bytes().all(|b| b == b'\t' || (0x20..=0x7E).contains(&b)) && !value.starts_with(edge) && !value.ends_with(edge) && !unfoldable {
        return value;
    }
    unstructured(name, &value)
}

/// One category for the `Keywords` field, where a comma separates entries.
fn keyword(category: &str) -> String {
    if is_plain(category) && !category.contains(',') && !category.starts_with(' ') && !category.ends_with(' ') {
        category.to_string()
    } else {
        encode_b(category, WORD_BYTES)
    }
}

/// The `Message-ID` field [`write`] gives a message whose id is `raw`, as it goes on the wire
/// (`<id>`); `None` when it gives it none.
pub fn written_message_id(raw: &str) -> Option<String> {
    let id = message_id(raw);
    (!id.is_empty()).then(|| format!("<{id}>"))
}

/// A message id or content id without anything that could end it early.
fn message_id(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control() && !c.is_whitespace() && !matches!(c, '<' | '>')).collect()
}

/// A MIME part ready to be written.
struct Part {
    fields: Vec<(&'static str, String)>,
    /// The body, encoded (so always 7-bit), without a line end after it.
    content: Vec<u8>,
}

impl Part {
    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.content.len() + 256);
        for (name, value) in &self.fields {
            write_field(&mut out, name, value);
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&self.content);
        out
    }
}

/// The attachment of a message that is its body in iCalendar form, and the `METHOD` it carries:
/// an invitation, or the answer to one. Mail programs look for that beside the text, as one more
/// form of the body (`text/calendar; method=REPLY`, RFC 6047), and not among the files. Such a
/// part is marked as shown in place, is `text/calendar`, belongs to no picture, is text, and
/// says its method in one plain word (which is all of it that reaches a header).
fn calendar_form(message: &Message) -> Option<(usize, String)> {
    message.attachments.iter().enumerate().find_map(|(index, part)| {
        if !part.inline || !part.content_id.is_empty() || media::safe_media_type(&part.mime) != "text/calendar" {
            return None;
        }
        let text = std::str::from_utf8(&part.data).ok()?;
        let method = text.lines().take(100).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim().eq_ignore_ascii_case("METHOD").then(|| value.trim())
        })?;
        let word = !method.is_empty() && method.len() <= 16 && method.bytes().all(|b| b.is_ascii_alphabetic());
        word.then(|| (index, method.to_ascii_uppercase()))
    })
}

/// The MIME structure of a message's body and attachments.
fn body(message: &Message) -> Part {
    let mut counter = 0u32;
    let text = text_part("plain", &message.body.text);
    let calendar = calendar_form(message);
    let rest = message.attachments.iter().enumerate().filter(|(index, _)| calendar.as_ref().is_none_or(|(form, _)| form != index)).map(|(_, a)| a);
    let (inline, files): (Vec<&Attachment>, Vec<&Attachment>) = match message.body.html {
        Some(_) => rest.partition(|a| a.inline),
        // Without HTML nothing can show an inline picture: it is a file like the others.
        None => (Vec::new(), rest.collect()),
    };
    // The forms the body comes in: its text, its HTML, and its iCalendar form when it has one.
    let mut forms: Vec<Part> = Vec::new();
    match &message.body.html {
        None => forms.push(text),
        Some(html) => {
            let mut rich = text_part("html", html);
            if !inline.is_empty() {
                let mut parts = vec![rich];
                parts.extend(inline.iter().map(|a| attachment_part(a)));
                rich = multipart("related", " type=\"text/html\";", &parts, &mut counter);
            }
            // A blank text part would hide the text a reader can make from the HTML.
            if !message.body.text.trim().is_empty() {
                forms.push(text);
            }
            forms.push(rich);
        }
    }
    if let Some((index, method)) = &calendar
        && let Some(part) = message.attachments.get(*index)
    {
        let mut form = text_part("calendar", &String::from_utf8_lossy(&part.data));
        if let Some(kind) = form.fields.first_mut() {
            kind.1 = format!("text/calendar; method={method}; charset=utf-8");
        }
        forms.push(form);
    }
    let mut root =
        if forms.len() > 1 { multipart("alternative", "", &forms, &mut counter) } else { forms.pop().unwrap_or_else(|| text_part("plain", "")) };
    if !files.is_empty() {
        let mut parts = vec![root];
        parts.extend(files.iter().map(|a| attachment_part(a)));
        root = multipart("mixed", "", &parts, &mut counter);
    }
    root
}

fn text_part(subtype: &str, text: &str) -> Part {
    let text = charset::unix_lines(text);
    let seven_bit = text.bytes().all(|b| matches!(b, b'\n' | b'\t' | 0x20..=0x7E)) && text.split('\n').all(|line| line.len() <= MAX_LINE);
    let (encoding, content) =
        if seven_bit { ("7bit", text.replace('\n', "\r\n").into_bytes()) } else { ("quoted-printable", transfer::qp_encode(text.as_bytes())) };
    Part { fields: vec![("Content-Type", format!("text/{subtype}; charset=utf-8")), ("Content-Transfer-Encoding", encoding.to_string())], content }
}

fn attachment_part(attachment: &Attachment) -> Part {
    let mime = media::safe_media_type(&attachment.mime);
    let mut name = clean(&attachment.name);
    if name.is_empty() && (mime == "text/plain" || mime == "text/html") {
        // A text part without a name would be read as the body.
        name = media::default_name(&mime);
    }
    let mut content_type = mime.clone();
    let mut disposition = String::from(if attachment.inline { "inline" } else { "attachment" });
    if !name.is_empty() {
        // Old readers only look at `name`, and only understand encoded-words there.
        content_type.push_str("; name=");
        // The first word is shorter, to fit on a line after ` name="`.
        content_type.push_str(&quoted(&if is_plain(&name) { name.clone() } else { encode_b(&name, 42) }));
        disposition.push_str(&file_name_params(&name));
    }
    // RFC 2046 allows no base64 for an attached message: it goes as it is when it is clean 7-bit
    // text. Bodies are always written 7-bit, so one with 8-bit bytes is base64 all the same.
    let (encoding, content) = if mime == "message/rfc822" && is_seven_bit_message(&attachment.data) {
        ("7bit", attachment.data.to_vec())
    } else {
        ("base64", transfer::base64_lines(&attachment.data))
    };
    let mut fields = vec![("Content-Type", content_type), ("Content-Transfer-Encoding", encoding.to_string()), ("Content-Disposition", disposition)];
    let content_id = message_id(&attachment.content_id);
    if !content_id.is_empty() {
        fields.push(("Content-ID", format!("<{content_id}>")));
    }
    Part { fields, content }
}

/// Whether bytes can travel as they are: ASCII without NUL, CRLF line ends only, no long lines.
fn is_seven_bit_message(data: &[u8]) -> bool {
    let mut line = 0usize;
    let mut prev = 0u8;
    for (i, &b) in data.iter().enumerate() {
        match b {
            0 | 0x80..=0xFF => return false,
            b'\n' if prev != b'\r' => return false,
            b'\n' => line = 0,
            b'\r' if data.get(i + 1) != Some(&b'\n') => return false,
            b'\r' => {}
            _ => {
                line += 1;
                if line > MAX_LINE {
                    return false;
                }
            }
        }
        prev = b;
    }
    true
}

/// A quoted-string: only `"` and `\` need a backslash.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The `filename` parameters of `Content-Disposition`: a quoted name when it is plain ASCII, else
/// the RFC 2231 form (`filename*=utf-8''…`, in numbered pieces when long) after an ASCII stand-in
/// for readers that don't know it.
fn file_name_params(name: &str) -> String {
    if is_plain(name) && name.len() <= 200 {
        return format!("; filename={}", quoted(name));
    }
    let stand_in: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).take(NAME_PIECE).collect();
    let mut out = format!("; filename=\"{stand_in}\"");
    let mut pieces: Vec<String> = vec![String::new()];
    for b in name.bytes() {
        let token = if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) { char::from(b).to_string() } else { format!("%{b:02X}") };
        match pieces.last_mut() {
            Some(piece) if piece.len() + token.len() <= NAME_PIECE => piece.push_str(&token),
            _ => pieces.push(token),
        }
    }
    match pieces.as_slice() {
        [only] => out.push_str(&format!("; filename*=utf-8''{only}")),
        many => {
            for (i, piece) in many.iter().enumerate() {
                out.push_str(&format!("; filename*{i}*={}{piece}", if i == 0 { "utf-8''" } else { "" }));
            }
        }
    }
    out
}

/// A multipart holding `parts`. `params` goes between the type and the boundary.
fn multipart(subtype: &str, params: &str, parts: &[Part], counter: &mut u32) -> Part {
    let children: Vec<Vec<u8>> = parts.iter().map(Part::bytes).collect();
    let boundary = boundary(&children, counter);
    let mut content = Vec::with_capacity(children.iter().map(|c| c.len() + boundary.len() + 8).sum::<usize>() + boundary.len() + 8);
    for child in &children {
        content.extend_from_slice(b"--");
        content.extend_from_slice(boundary.as_bytes());
        content.extend_from_slice(b"\r\n");
        content.extend_from_slice(child);
        content.extend_from_slice(b"\r\n");
    }
    content.extend_from_slice(b"--");
    content.extend_from_slice(boundary.as_bytes());
    content.extend_from_slice(b"--");
    Part { fields: vec![("Content-Type", format!("multipart/{subtype};{params} boundary=\"{boundary}\""))], content }
}

/// A boundary that occurs in none of the parts. It is an FNV-1a hash of the parts and a counter
/// (which sets the multiparts of one message apart, and moves on when a part happens to hold the
/// boundary), so the same content always gets the same boundary.
fn boundary(children: &[Vec<u8>], counter: &mut u32) -> String {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for child in children {
        for b in child {
            hash = (hash ^ u64::from(*b)).wrapping_mul(PRIME);
        }
        // The edge between two parts counts too.
        hash = (hash ^ 0x1ff).wrapping_mul(PRIME);
    }
    free_boundary(hash, children, counter)
}

/// The first boundary for `hash`, counting up from `counter`, that none of the parts holds.
fn free_boundary(hash: u64, children: &[Vec<u8>], counter: &mut u32) -> String {
    loop {
        let candidate = format!("=_mc_{hash:016x}_{counter}");
        *counter = counter.wrapping_add(1);
        if !children.iter().any(|c| contains(c, candidate.as_bytes())) {
            return candidate;
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((first, rest)) = needle.split_first() else { return true };
    let mut from = 0;
    while let Some(i) = haystack.get(from..).and_then(|h| h.iter().position(|b| b == first)) {
        let at = from + i + 1;
        if haystack.get(at..at + rest.len()) == Some(rest) {
            return true;
        }
        from = at;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(name: &str, email: &str) -> Address {
        Address { name: name.into(), email: email.into() }
    }

    fn text(message: &Message) -> String {
        String::from_utf8(write(message, &WriteOptions::default())).unwrap()
    }

    fn sample() -> Message {
        Message {
            message_id: "abc.123@example.org".into(),
            in_reply_to: "parent.1@example.org".into(),
            references: vec!["root.0@example.org".into(), "parent.1@example.org".into()],
            from: a("Ada Lovelace", "ada@example.org"),
            reply_to: vec![a("", "replies@example.org")],
            to: vec![a("Bob", "bob@example.org"), a("", "carol@example.org")],
            cc: vec![a("Hopper, Grace", "grace@example.org")],
            bcc: vec![a("", "secret@example.org")],
            subject: "Lunch on Friday?".into(),
            date: 1_057_049_557,
            body: mailcraft_model::Body { text: "Hello Bob,\n\nLunch?\n".into(), html: None },
            ..Message::default()
        }
    }

    #[test]
    fn a_plain_message_byte_for_byte() {
        let want = "Date: Tue, 01 Jul 2003 08:52:37 +0000\r\n\
                    From: Ada Lovelace <ada@example.org>\r\n\
                    To: Bob <bob@example.org>, carol@example.org\r\n\
                    Cc: \"Hopper, Grace\" <grace@example.org>\r\n\
                    Reply-To: replies@example.org\r\n\
                    Subject: Lunch on Friday?\r\n\
                    Message-ID: <abc.123@example.org>\r\n\
                    In-Reply-To: <parent.1@example.org>\r\n\
                    References: <root.0@example.org> <parent.1@example.org>\r\n\
                    MIME-Version: 1.0\r\n\
                    Content-Type: text/plain; charset=utf-8\r\n\
                    Content-Transfer-Encoding: 7bit\r\n\
                    \r\n\
                    Hello Bob,\r\n\
                    \r\n\
                    Lunch?\r\n\
                    \r\n";
        assert_eq!(text(&sample()), want);
    }

    #[test]
    fn bcc_only_when_asked_and_the_zone_is_used() {
        assert!(!text(&sample()).contains("secret@example.org"));
        let out = String::from_utf8(write(&sample(), &WriteOptions { include_bcc: true, offset_minutes: 120 })).unwrap();
        assert!(out.contains("\r\nCc: \"Hopper, Grace\" <grace@example.org>\r\nBcc: secret@example.org\r\nReply-To:"));
        assert!(out.starts_with("Date: Tue, 01 Jul 2003 10:52:37 +0200\r\n"));
    }

    #[test]
    fn empty_fields_are_left_out() {
        let out = text(&Message::default());
        assert_eq!(
            out,
            "Date: Thu, 01 Jan 1970 00:00:00 +0000\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\n\r\n"
        );
    }

    #[test]
    fn flags_importance_and_keywords() {
        let m = Message {
            read: true,
            answered: true,
            flag: FlagStatus::Flagged,
            importance: Importance::High,
            categories: vec!["Work".into(), "Café".into(), "a, b".into(), "".into()],
            ..Message::default()
        };
        let out = text(&m);
        assert!(out.contains("\r\nImportance: high\r\nX-Priority: 1\r\nKeywords: Work, =?UTF-8?B?Q2Fmw6k=?=, =?UTF-8?B?YSwgYg==?=\r\nStatus: RO\r\nX-Status: AF\r\n"), "{out}");
        let out = text(&Message { importance: Importance::Low, flag: FlagStatus::Complete, ..Message::default() });
        assert!(out.contains("\r\nImportance: low\r\nX-Priority: 5\r\nX-MailCraft-Flag: complete\r\n"), "{out}");
        assert!(!out.contains("Status:"));
    }

    #[test]
    fn extra_headers_are_kept_unless_they_clash() {
        let m = Message {
            subject: "Real".into(),
            headers: vec![
                ("X-Mailer".into(), "Synthetic 1.0".into()),
                ("Subject".into(), "Fake".into()),
                ("BCC".into(), "leak@example.org".into()),
                ("Content-Type".into(), "text/evil".into()),
                ("content-transfer-encoding".into(), "base64".into()),
                ("MIME-Version".into(), "9.9".into()),
                ("Bad Name".into(), "x".into()),
                ("Bad:Name".into(), "x".into()),
                ("".into(), "x".into()),
                ("X-Injected\r\nBcc".into(), "x".into()),
                ("X-Unicode".into(), "héllo".into()),
                ("List-Unsubscribe".into(), "<mailto:leave@example.org>".into()),
            ],
            ..Message::default()
        };
        let out = text(&m);
        assert!(out.contains("\r\nSubject: Real\r\nX-Mailer: Synthetic 1.0\r\nX-Unicode: =?UTF-8?B?aMOpbGxv?=\r\nList-Unsubscribe: <mailto:leave@example.org>\r\nMIME-Version: 1.0\r\n"), "{out}");
        for gone in ["Fake", "leak@example.org", "text/evil", "9.9", "Bad", "X-Injected", "base64"] {
            assert!(!out.contains(gone), "{gone}");
        }
    }

    #[test]
    fn header_values_cannot_inject_fields() {
        let m = Message {
            subject: "Hi\r\nBcc: eve@example.org\r\n\r\nbody".into(),
            message_id: "id@example.org>\r\nX-Evil: 1\r\n<x".into(),
            references: vec!["a@example.org> <b".into(), "\r\n".into()],
            from: a("Ada\nX-Evil: 2", "ada@example.org\r\nX-Evil: 3"),
            categories: vec!["one\r\nX-Evil: 4".into()],
            headers: vec![("X-Note".into(), "fine\r\nX-Evil: 5".into())],
            attachments: vec![Attachment {
                name: "a.txt\r\nX-Evil: 6".into(),
                mime: "text/plain\r\nX-Evil: 7".into(),
                content_id: "c\r\nX-Evil: 8".into(),
                ..Attachment::default()
            }],
            ..Message::default()
        };
        let out = text(&m);
        for line in out.split("\r\n") {
            assert!(!line.starts_with("X-Evil") && !line.starts_with("Bcc"), "{line}");
            assert!(!line.contains(['\r', '\n', '\0']));
        }
        assert!(out.contains("\r\nSubject: HiBcc: eve@example.orgbody\r\n"));
        assert!(out.contains("\r\nMessage-ID: <id@example.orgX-Evil:1x>\r\n"));
        assert!(out.contains("\r\nReferences: <a@example.orgb>\r\n"));
    }

    #[test]
    fn long_and_non_ascii_subjects_fold() {
        let m = Message { subject: "Ünïcödé subject ".repeat(12), ..Message::default() };
        let out = text(&m);
        let headers = out.split("\r\n\r\n").next().unwrap();
        for line in headers.split("\r\n") {
            assert!(line.len() <= 78, "{} {line}", line.len());
        }
        assert!(headers.contains("Subject: =?UTF-8?B?"));
        let m = Message { subject: "plain words ".repeat(30).trim().to_string(), ..Message::default() };
        let out = text(&m);
        assert!(out.split("\r\n").all(|l| l.len() <= 78));
        assert!(out.contains("Subject: plain words plain words"));
        // A subject that could not fold is encoded rather than written as one endless line.
        let m = Message { subject: "x".repeat(3000), ..Message::default() };
        assert!(text(&m).split("\r\n").all(|l| l.len() <= 78));
    }

    #[test]
    fn text_encodings() {
        let part = |t: &str| {
            let p = text_part("plain", t);
            (p.fields.get(1).map(|f| f.1.clone()).unwrap(), String::from_utf8(p.content).unwrap())
        };
        assert_eq!(part("plain\ntext\r\nmixed\rends"), ("7bit".into(), "plain\r\ntext\r\nmixed\r\nends".into()));
        assert_eq!(part("café"), ("quoted-printable".into(), "caf=C3=A9".into()));
        assert_eq!(part("nul\0"), ("quoted-printable".into(), "nul=00".into()));
        let (encoding, content) = part(&"x".repeat(1200));
        assert_eq!(encoding, "quoted-printable");
        assert!(content.split("\r\n").all(|l| l.len() <= 76));
        assert_eq!(part(&"x".repeat(998)).0, "7bit");
        assert_eq!(part("").0, "7bit");
    }

    #[test]
    fn alternative_related_and_mixed_nest_as_they_should() {
        let mut m = sample();
        m.body.html = Some("<p>Hello <img src=\"cid:logo@example.org\"></p>".into());
        m.attachments = vec![
            Attachment {
                name: "report.pdf".into(),
                mime: "application/pdf".into(),
                data: std::sync::Arc::new(b"%PDF-1.4\n".to_vec()),
                ..Attachment::default()
            },
            Attachment {
                name: "logo.png".into(),
                mime: "image/png".into(),
                content_id: "logo@example.org".into(),
                inline: true,
                data: std::sync::Arc::new(vec![0x89, b'P', b'N', b'G']),
                kept: None,
            },
        ];
        let out = text(&m);
        let types: Vec<&str> = out.split("\r\n").filter(|l| l.starts_with("Content-Type:")).collect();
        assert_eq!(types.len(), 7, "{out}");
        assert!(types[0].starts_with("Content-Type: multipart/mixed; boundary=\"=_mc_"));
        assert!(types[1].starts_with("Content-Type: multipart/alternative; boundary=\"=_mc_"));
        assert_eq!(types[2], "Content-Type: text/plain; charset=utf-8");
        assert!(types[3].starts_with("Content-Type: multipart/related; type=\"text/html\";"));
        assert_eq!(types[4], "Content-Type: text/html; charset=utf-8");
        assert_eq!(types[5], "Content-Type: image/png; name=\"logo.png\"");
        assert_eq!(types[6], "Content-Type: application/pdf; name=\"report.pdf\"");
        assert!(out.contains("Content-Transfer-Encoding: base64\r\nContent-Disposition: inline; filename=\"logo.png\"\r\nContent-ID: <logo@example.org>\r\n\r\niVBORw==\r\n--=_mc_"));
        assert!(out.contains("Content-Disposition: attachment; filename=\"report.pdf\"\r\n\r\nJVBERi0xLjQK\r\n--=_mc_"));
        assert!(out.ends_with("--\r\n"));
        // Three multiparts, three different boundaries.
        let mut boundaries: Vec<&str> = out.split("boundary=\"").skip(1).filter_map(|s| s.split('"').next()).collect();
        boundaries.sort_unstable();
        boundaries.dedup();
        assert_eq!(boundaries.len(), 3);
    }

    #[test]
    fn html_without_text_is_html_only() {
        let m = Message { body: mailcraft_model::Body { text: " \n".into(), html: Some("<p>x</p>".into()) }, ..Message::default() };
        let out = text(&m);
        assert!(out.contains("\r\nContent-Type: text/html; charset=utf-8\r\n"));
        assert!(!out.contains("multipart"));
        assert!(!out.contains("text/plain"));
    }

    /// An iCalendar text of `method`, as the answer to an invitation has it.
    fn calendar(method: &str) -> Attachment {
        let text = format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\n{method}BEGIN:VEVENT\r\nUID:uid-1\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n");
        Attachment {
            name: "response.ics".into(),
            mime: "text/calendar".into(),
            inline: true,
            data: std::sync::Arc::new(text.into_bytes()),
            ..Attachment::default()
        }
    }

    #[test]
    fn the_calendar_form_of_a_body_goes_beside_its_text() {
        // The answer to an invitation: text, and the same in iCalendar, as alternatives.
        let mut m = sample();
        m.attachments = vec![calendar("METHOD:reply\r\n")];
        let out = text(&m);
        let types: Vec<&str> = out.split("\r\n").filter(|l| l.starts_with("Content-Type:")).collect();
        assert_eq!(types.len(), 3, "{out}");
        assert!(types[0].starts_with("Content-Type: multipart/alternative; boundary=\"=_mc_"));
        assert_eq!((types[1], types[2]), ("Content-Type: text/plain; charset=utf-8", "Content-Type: text/calendar; method=REPLY; charset=utf-8"));
        // As text, not as a file: readable as it is, with no name and no disposition.
        assert!(
            out.contains("method=REPLY; charset=utf-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\nBEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:reply\r\n"),
            "{out}"
        );
        assert!(!out.contains("Content-Disposition") && !out.contains("response.ics") && !out.contains("base64"));
        // A reader finds the text and the calendar part again.
        let back = crate::parse(out.as_bytes());
        assert_eq!(back.body.text, m.body.text);
        assert_eq!(back.attachments.len(), 1);
        assert_eq!(back.attachments[0].mime, "text/calendar");
        let calendar_text = String::from_utf8_lossy(&back.attachments[0].data).into_owned();
        assert!(calendar_text.starts_with("BEGIN:VCALENDAR\r\n") && calendar_text.trim_end().ends_with("END:VCALENDAR"), "{calendar_text}");
        // With HTML it is the third form, and files are still files beside them all.
        m.body.html = Some("<p>Lunch?</p>".into());
        m.attachments.push(Attachment { name: "agenda.pdf".into(), mime: "application/pdf".into(), ..Attachment::default() });
        let out = text(&m);
        let types: Vec<&str> = out.split("\r\n").filter(|l| l.starts_with("Content-Type:")).map(|l| l.split(';').next().unwrap()).collect();
        assert_eq!(
            types,
            [
                "Content-Type: multipart/mixed",
                "Content-Type: multipart/alternative",
                "Content-Type: text/plain",
                "Content-Type: text/html",
                "Content-Type: text/calendar",
                "Content-Type: application/pdf"
            ]
        );
        // Text that is not plain ASCII still travels as text.
        let mut wide = calendar("METHOD:REQUEST\r\n");
        wide.data = std::sync::Arc::new("METHOD:REQUEST\r\nSUMMARY:Café\r\n".as_bytes().to_vec());
        let out = text(&Message { attachments: vec![wide], ..sample() });
        assert!(out.contains("text/calendar; method=REQUEST; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nMETHOD:REQUEST\r\nSUMMARY:Caf=C3=A9"), "{out}");
    }

    #[test]
    fn a_calendar_file_is_a_file_unless_it_is_the_body_and_says_what_for() {
        let attached = |a: Attachment| {
            let out = text(&Message { attachments: vec![a], ..sample() });
            assert!(!out.contains("method="), "{out}");
            out.contains("Content-Type: text/calendar; name=\"response.ics\"")
                || out.contains("Content-Type: application/octet-stream; name=\"response.ics\"")
        };
        // Not shown in place; a picture's part; another kind of file; no method, an empty one,
        // one that is not a word, one too long to be one; bytes that are no text.
        assert!(attached(Attachment { inline: false, ..calendar("METHOD:REPLY\r\n") }));
        assert!(attached(Attachment { content_id: "c@example.org".into(), ..calendar("METHOD:REPLY\r\n") }));
        assert!(attached(Attachment { mime: "application/octet-stream".into(), ..calendar("METHOD:REPLY\r\n") }));
        for method in [
            "",
            "METHOD:\r\n",
            "METHOD:RE PLY\r\n",
            "METHOD:REPLY; charset=x\r\n",
            "METHOD:RÉPLY\r\n",
            "METHOD:ABCDEFGHIJKLMNOPQ\r\n",
            "X-METHOD:REPLY\r\n",
        ] {
            assert!(attached(calendar(method)), "{method}");
        }
        let mut bytes = calendar("METHOD:REPLY\r\n");
        bytes.data = std::sync::Arc::new(b"METHOD:REPLY\r\n\xff\xfe".to_vec());
        assert!(attached(bytes));
        // Nothing a method says can add a header field: it is one word, or the part is a file.
        for method in ["METHOD:REPLY\rBcc: eve@example.org\r\n", "METHOD:REPLY\0\r\n", "METHOD:REPLY\"\r\n", "METHOD:REPLY\r\n\r\nX-Evil: 1\r\n"] {
            let out = text(&Message { attachments: vec![calendar(method)], ..sample() });
            let head = out.split("\r\n\r\n").next().unwrap();
            assert!(!head.contains("eve@example.org") && !head.contains("X-Evil"), "{head}");
            for line in out.split("\r\n").filter(|line| line.starts_with("Content-Type: text/calendar")) {
                assert!(line == "Content-Type: text/calendar; method=REPLY; charset=utf-8" || line.contains("name="), "{line}");
            }
        }
        // Two of them: the first is the body's, the second a file.
        let out = text(&Message { attachments: vec![calendar("METHOD:REPLY\r\n"), calendar("METHOD:CANCEL\r\n")], ..sample() });
        assert_eq!(
            (out.matches("method=REPLY").count(), out.matches("method=CANCEL").count(), out.matches("text/calendar; name=\"response.ics\"").count()),
            (1, 0, 1)
        );
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name_params("report.pdf"), "; filename=\"report.pdf\"");
        assert_eq!(file_name_params("my \"best\" file\\x.txt"), r#"; filename="my \"best\" file\\x.txt""#);
        assert_eq!(file_name_params("résumé €.txt"), "; filename=\"r_sum___.txt\"; filename*=utf-8''r%C3%A9sum%C3%A9%20%E2%82%AC.txt");
        // Something that looks like an encoded-word must not be read as one.
        assert!(file_name_params("=?UTF-8?Q?x?=.txt").contains("filename*=utf-8''%3D%3FUTF-8%3FQ%3Fx%3F%3D.txt"));
        let long = file_name_params(&"ü".repeat(40));
        assert!(long.contains("; filename*0*=utf-8''%C3%BC") && long.contains("; filename*4*=%") && !long.contains("filename*5"), "{long}");
        // Each piece fits a folded line: a space before it, a semicolon after.
        for piece in long.split("; ") {
            assert!(piece.len() + 2 <= 78, "{piece}");
        }
        let m = Message {
            attachments: vec![Attachment { name: "ü".repeat(200), mime: "image/png".into(), ..Attachment::default() }],
            ..Message::default()
        };
        assert!(text(&m).split("\r\n").all(|l| l.len() <= 78));
    }

    #[test]
    fn unsafe_media_types_and_nameless_text_attachments() {
        let part = |mime: &str, name: &str| attachment_part(&Attachment { name: name.into(), mime: mime.into(), ..Attachment::default() }).fields;
        assert_eq!(part("multipart/mixed; boundary=x", "a.bin").first().map(|f| f.1.as_str()), Some("application/octet-stream; name=\"a.bin\""));
        assert_eq!(part("", "").first().map(|f| f.1.as_str()), Some("application/octet-stream"));
        assert_eq!(part("application/pdf", "").get(2).map(|f| f.1.as_str()), Some("attachment"));
        // A nameless text part would be taken for the body.
        assert_eq!(part("text/plain", "").get(2).map(|f| f.1.as_str()), Some("attachment; filename=\"attachment.txt\""));
        assert_eq!(part("text/html", "").first().map(|f| f.1.as_str()), Some("text/html; name=\"attachment.html\""));
        assert_eq!(part("image/png", "ünï.png").first().map(|f| f.1.as_str()), Some("image/png; name=\"=?UTF-8?B?w7xuw68ucG5n?=\""));
    }

    #[test]
    fn attached_messages_travel_as_they_are_when_they_can() {
        let part = |data: &[u8]| {
            let p = attachment_part(&Attachment {
                name: "m.eml".into(),
                mime: "message/rfc822".into(),
                data: std::sync::Arc::new(data.to_vec()),
                ..Attachment::default()
            });
            (p.fields.get(1).map(|f| f.1.clone()).unwrap(), p.content)
        };
        assert_eq!(part(b"Subject: x\r\n\r\nbody\r\n"), ("7bit".into(), b"Subject: x\r\n\r\nbody\r\n".to_vec()));
        assert_eq!(part(b"").0, "7bit");
        assert_eq!(part(&[b'x'; 998]).0, "7bit");
        // Anything that could not travel as it is: base64, which loses nothing.
        assert_eq!(part("Subject: x\r\n\r\ncafé".as_bytes()), ("base64".into(), b"U3ViamVjdDogeA0KDQpjYWbDqQ==".to_vec()));
        assert_eq!(part(b"Subject: x\n\nbare lf").0, "base64");
        assert_eq!(part(b"bare\rcr").0, "base64");
        assert_eq!(part(b"nul\0").0, "base64");
        assert_eq!(part(&[b'x'; 999]).0, "base64");
        // Other attachments are always base64, however plain they are.
        let p = attachment_part(&Attachment {
            name: "a.txt".into(),
            mime: "text/plain".into(),
            data: std::sync::Arc::new(b"plain".to_vec()),
            ..Attachment::default()
        });
        assert_eq!(p.fields.get(1).map(|f| f.1.as_str()), Some("base64"));
    }

    #[test]
    fn everything_written_is_seven_bit() {
        let m = Message {
            from: a("Ünï Cödé", "ada@example.org"),
            subject: "Grüße 🙋".into(),
            categories: vec!["Été".into()],
            headers: vec![("X-Note".into(), "ñ".into())],
            body: mailcraft_model::Body { text: "Grüße\n".into(), html: Some("<p>Grüße</p>".into()) },
            attachments: vec![
                Attachment {
                    name: "ü.eml".into(),
                    mime: "message/rfc822".into(),
                    data: std::sync::Arc::new("Subject: é\r\n\r\nü".as_bytes().to_vec()),
                    ..Attachment::default()
                },
                Attachment {
                    name: "b.bin".into(),
                    mime: "application/octet-stream".into(),
                    data: std::sync::Arc::new(vec![0, 128, 255]),
                    ..Attachment::default()
                },
            ],
            ..Message::default()
        };
        assert!(write(&m, &WriteOptions::default()).is_ascii());
    }

    #[test]
    fn boundaries_are_deterministic_and_avoid_the_content() {
        let mut m = sample();
        m.body.html = Some("<p>x</p>".into());
        assert_eq!(write(&m, &WriteOptions::default()), write(&m, &WriteOptions::default()));
        let first = text(&m);
        let boundary = first.split("boundary=\"").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
        // Put that very boundary in the text: the writer must pick another.
        m.body.text = format!("before\n--{boundary}\nafter");
        let second = text(&m);
        let new_boundary = second.split("boundary=\"").nth(1).and_then(|s| s.split('"').next()).unwrap().to_string();
        assert_ne!(new_boundary, boundary);
        assert_eq!(second.matches(&format!("--{new_boundary}")).count(), 3);
    }

    #[test]
    fn a_boundary_found_in_a_part_is_passed_over() {
        let mut counter = 0;
        assert_eq!(free_boundary(42, &[b"nothing here".to_vec()], &mut counter), "=_mc_000000000000002a_0");
        assert_eq!(counter, 1);
        // The parts hold the first two candidates, one of them in the second part.
        let parts = [b"text --=_mc_000000000000002a_1 text".to_vec(), b"=_mc_000000000000002a_2".to_vec()];
        let mut counter = 1;
        assert_eq!(free_boundary(42, &parts, &mut counter), "=_mc_000000000000002a_3");
        assert_eq!(counter, 4);
        // The counter sets apart two multiparts with the same content.
        let mut counter = 0;
        let same = [b"same".to_vec()];
        assert_ne!(boundary(&same, &mut counter), boundary(&same, &mut counter));
    }

    #[test]
    fn substring_search() {
        assert!(contains(b"hello world", b"lo w"));
        assert!(contains(b"aaab", b"aab"));
        assert!(contains(b"abc", b""));
        assert!(contains(b"abc", b"abc"));
        assert!(!contains(b"abc", b"abcd"));
        assert!(!contains(b"", b"a"));
        assert!(!contains(b"ababab", b"abb"));
    }
}
