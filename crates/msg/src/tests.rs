use proptest::prelude::*;

use super::*;
use crate::writer::{Node, Props, compound, compress_rtf};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n not really a picture";

fn recipient(kind: i32, name: &str, smtp: &str) -> Props {
    let r = Props::new().int(RECIPIENT_TYPE, kind).unicode(DISPLAY_NAME, name);
    if smtp.is_empty() {
        r.unicode(ADDRESS_TYPE, "EX").unicode(EMAIL_ADDRESS, "/O=EXAMPLE/OU=FIRST ADMINISTRATIVE GROUP/CN=RECIPIENTS/CN=NOBODY")
    } else {
        r.unicode(ADDRESS_TYPE, "SMTP").unicode(EMAIL_ADDRESS, smtp)
    }
}

/// A message such as Outlook saves, with synthetic content.
fn sample() -> Props {
    let big: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    Props::new()
        .unicode(0x001A, "IPM.Note")
        .unicode(SUBJECT, "Quarterly figures, café edition")
        .unicode(SENDER_NAME, "Avery Quinn")
        .unicode(SENDER_ADDRESS_TYPE, "EX")
        .unicode(SENDER_EMAIL, "/O=EXAMPLE/OU=EXCHANGE/CN=RECIPIENTS/CN=AVERY")
        .unicode(SENDER_SMTP, "avery@example.org")
        .time(CLIENT_SUBMIT_TIME, 1_700_000_000)
        .time(MESSAGE_DELIVERY_TIME, 1_700_000_060)
        .unicode(INTERNET_MESSAGE_ID, "<figures.1@example.org>")
        .unicode(IN_REPLY_TO, "<question.1@example.org>")
        .unicode(INTERNET_REFERENCES, "<start.1@example.org> <question.1@example.org>")
        .int(IMPORTANCE, 2)
        .int(MESSAGE_FLAGS, 0x11)
        .int(FLAG_STATUS, 2)
        .int(LAST_VERB_EXECUTED, 102)
        .int(INTERNET_CODEPAGE, 65001)
        .unicode(BODY, "Hello team,\r\nthe figures are attached.\r\n")
        .binary(HTML, "<html><body><p>Hello team, the figures are <b>attached</b>.</p><img src=\"cid:chart@example.org\"></body></html>".as_bytes())
        .unicode(
            TRANSPORT_HEADERS,
            "Received: from mx.example.org by mail.example.org\r\nReply-To: Finance <finance@example.org>\r\nX-Mailer: Example Mailer 1.0\r\nKeywords: Finance\r\n",
        )
        .recipient(recipient(1, "Blake Rivera", "blake@example.org"))
        .recipient(recipient(1, "Exchange Only", ""))
        .recipient(recipient(2, "Casey Moreau", "casey@example.org").int(RECIPIENT_TYPE, 2))
        .recipient(recipient(3, "Drew Park", "drew@example.org"))
        .attachment(
            Props::new()
                .int(ATTACH_METHOD, 1)
                .unicode(ATTACH_LONG_FILENAME, "figures.pdf")
                .unicode(ATTACH_FILENAME, "FIGURES.PDF")
                .unicode(ATTACH_MIME_TAG, "application/pdf")
                .binary(ATTACH_DATA, &big),
        )
        .attachment(
            Props::new()
                .int(ATTACH_METHOD, 1)
                .unicode(ATTACH_LONG_FILENAME, "chart.png")
                .unicode(ATTACH_CONTENT_ID, "<chart@example.org>")
                .bool(ATTACHMENT_HIDDEN, true)
                .binary(ATTACH_DATA, PNG),
        )
        .attachment(Props::new().int(ATTACH_METHOD, 2).unicode(ATTACH_LONG_FILENAME, "elsewhere.docx").unicode(0x3708, "\\\\server.example\\share\\elsewhere.docx"))
}

#[test]
fn a_message_outlook_saved() {
    let bytes = sample().msg();
    assert!(is_compound(&bytes));
    let m = parse(&bytes).unwrap();
    assert_eq!(m.subject, "Quarterly figures, café edition");
    assert_eq!(m.from, Address::new("Avery Quinn", "avery@example.org"));
    assert_eq!(m.to, vec![Address::new("Blake Rivera", "blake@example.org"), Address::new("Exchange Only", "")]);
    assert_eq!(m.cc, vec![Address::new("Casey Moreau", "casey@example.org")]);
    assert_eq!(m.bcc, vec![Address::new("Drew Park", "drew@example.org")]);
    assert_eq!((m.date, m.received), (1_700_000_000, 1_700_000_060));
    assert_eq!(m.message_id, "figures.1@example.org");
    assert_eq!(m.in_reply_to, "question.1@example.org");
    assert_eq!(m.references, ["start.1@example.org", "question.1@example.org"]);
    assert_eq!(m.importance, Importance::High);
    assert!(m.read && m.answered && !m.forwarded);
    assert_eq!(m.flag, FlagStatus::Flagged);
    assert_eq!(m.body.text, "Hello team,\nthe figures are attached.\n");
    assert!(m.body.html.as_deref().unwrap().contains("<b>attached</b>"));
    // Transport headers: what the properties don't have.
    assert_eq!(m.reply_to, vec![Address::new("Finance", "finance@example.org")]);
    assert_eq!(m.categories, ["Finance"]);
    assert!(m.headers.iter().any(|(k, v)| k == "X-Mailer" && v == "Example Mailer 1.0"));
    // The attachment by reference is left out.
    assert_eq!(m.attachments.len(), 2);
    let pdf = &m.attachments[0];
    assert_eq!((pdf.name.as_str(), pdf.mime.as_str(), pdf.size(), pdf.inline), ("figures.pdf", "application/pdf", 20_000, false));
    assert!(pdf.data.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    let png = &m.attachments[1];
    assert_eq!((png.name.as_str(), png.mime.as_str(), png.content_id.as_str(), png.inline), ("chart.png", "image/png", "chart@example.org", true));
    assert_eq!(png.data.as_slice(), PNG);
    assert_eq!(m.listed_attachments().count(), 1);
}

#[test]
fn version_4_files_read_the_same() {
    assert_eq!(parse(&sample().msg_v4()).unwrap(), parse(&sample().msg()).unwrap());
}

#[test]
fn eight_bit_strings_in_the_message_code_page() {
    let bytes = Props::new()
        .int(MESSAGE_CODEPAGE, 1251)
        .string8(SUBJECT, b"\xcf\xf0\xe8\xe2\xe5\xf2")
        .string8(SENDER_NAME, b"\xc8\xe2\xe0\xed")
        .string8(SENDER_ADDRESS_TYPE, b"SMTP")
        .string8(SENDER_EMAIL, b"ivan@example.org")
        .string8(BODY, b"\xcf\xf0\xe8\xe2\xe5\xf2!\r\n")
        .msg();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.subject, "Привет");
    assert_eq!(m.from, Address::new("Иван", "ivan@example.org"));
    assert_eq!(m.body.text, "Привет!\n");
    assert_eq!(m.body.html, None);
}

#[test]
fn html_out_of_compressed_rtf() {
    let rtf = br"{\rtf1\ansi\ansicpg1252\fromhtml1 \deff0{\fonttbl{\f0\fswiss Arial;}}{\*\htmltag19 <html>}{\*\htmltag50 <body>}\htmlrtf {\htmlrtf0 Caf\'e9 time\htmlrtf\par}\htmlrtf0{\*\htmltag58 </body>}{\*\htmltag27 </html>}}";
    let m = parse(&Props::new().unicode(SUBJECT, "rtf").binary(RTF_COMPRESSED, &compress_rtf(rtf)).msg()).unwrap();
    assert_eq!(m.body.html.as_deref(), Some("<html><body>Café time</body></html>"));
    assert_eq!(m.body.text.trim(), "Café time");
}

#[test]
fn plain_rtf_falls_back_to_the_text_body() {
    let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}\pard Only RTF here\par}";
    let with_body = parse(&Props::new().unicode(BODY, "The text body").binary(RTF_COMPRESSED, &compress_rtf(rtf)).msg()).unwrap();
    assert_eq!((with_body.body.text.as_str(), with_body.body.html.as_deref()), ("The text body", None));
    let without = parse(&Props::new().binary(RTF_COMPRESSED, &compress_rtf(rtf)).msg()).unwrap();
    assert_eq!(without.body.text, "Only RTF here\n");
}

#[test]
fn html_bytes_in_the_charset_they_declare() {
    let html = b"<html><head><meta charset=\"windows-1252\"></head><body>na\xefve</body></html>";
    let m = parse(&Props::new().binary(HTML, html).msg()).unwrap();
    assert!(m.body.html.unwrap().contains("naïve"));
    assert_eq!(m.body.text.trim(), "naïve");
}

#[test]
fn an_attached_message_is_an_attachment() {
    let inner = Props::new()
        .unicode(SUBJECT, "The original")
        .unicode(SENDER_NAME, "Emery Stone")
        .unicode(SENDER_SMTP, "emery@example.org")
        .unicode(BODY, "Inner text")
        .recipient(recipient(1, "Avery Quinn", "avery@example.org"))
        .attachment(Props::new().unicode(ATTACH_LONG_FILENAME, "note.txt").binary(ATTACH_DATA, b"a note"));
    let bytes =
        Props::new().unicode(SUBJECT, "FW: The original").attachment(Props::new().unicode(DISPLAY_NAME, "The original").embedded(inner)).msg();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.attachments.len(), 1);
    let a = &m.attachments[0];
    assert_eq!((a.name.as_str(), a.mime.as_str()), ("The original.eml", "message/rfc822"));
    let inner = mailcraft_mime::parse(&a.data);
    assert_eq!(inner.subject, "The original");
    assert_eq!(inner.from.email, "emery@example.org");
    assert_eq!(inner.to[0].email, "avery@example.org");
    assert_eq!(inner.body.text.trim(), "Inner text");
    assert_eq!(inner.attachments[0].name, "note.txt");
    assert_eq!(inner.attachments[0].data.as_slice(), b"a note");
}

#[test]
fn attached_messages_nest_only_so_deep() {
    let mut message = Props::new().unicode(SUBJECT, "bottom");
    for i in 0..20 {
        message = Props::new().unicode(SUBJECT, &format!("level {i}")).attachment(Props::new().embedded(message));
    }
    let m = parse(&message.msg()).unwrap();
    let mut depth = 0;
    let mut current = m;
    while let Some(a) = current.attachments.first() {
        current = mailcraft_mime::parse(&a.data);
        depth += 1;
    }
    assert_eq!(depth, MAX_DEPTH - 1);
}

#[test]
fn an_exchange_sender_without_smtp_takes_the_transport_from() {
    let bytes = Props::new()
        .unicode(SENDER_NAME, "Finley Grey")
        .unicode(SENDER_ADDRESS_TYPE, "EX")
        .unicode(SENDER_EMAIL, "/O=EXAMPLE/CN=FINLEY")
        .unicode(
            TRANSPORT_HEADERS,
            "From: Finley Grey <finley@example.org>\nTo: team@example.org\nMessage-ID: <x.1@example.org>\nDate: Tue, 14 Nov 2023 22:13:20 +0000\n",
        )
        .msg();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.from, Address::new("Finley Grey", "finley@example.org"));
    assert_eq!(m.to, vec![Address::new("", "team@example.org")]);
    assert_eq!(m.message_id, "x.1@example.org");
    assert_eq!(m.date, 1_700_000_000);
    // Without the headers, the name alone.
    let bare =
        Props::new().unicode(SENDER_NAME, "Finley Grey").unicode(SENDER_ADDRESS_TYPE, "EX").unicode(SENDER_EMAIL, "/O=EXAMPLE/CN=FINLEY").msg();
    assert_eq!(parse(&bare).unwrap().from, Address::new("Finley Grey", ""));
}

#[test]
fn what_isnt_a_message() {
    assert_eq!(parse(b"From: someone@example.org\r\n\r\nhi"), Err(MsgError::NotCompound));
    assert_eq!(parse(&[]), Err(MsgError::NotCompound));
    assert_eq!(parse(&crate::SIGNATURE), Err(MsgError::NotCompound));
    // A compound file without message properties (an old Word document, say).
    let doc = compound(&[Node::Stream("WordDocument".into(), vec![1; 5000])], false);
    assert_eq!(parse(&doc), Err(MsgError::NotMessage));
}

#[test]
fn a_chain_that_comes_back_on_itself_ends() {
    let mut bytes = sample().msg();
    // Point every FAT entry at sector 0: every chain loops at once.
    let fat_sector = u32::from_le_bytes(bytes[76..80].try_into().unwrap()) as usize;
    let at = (fat_sector + 1) * 512;
    for i in 0..128 {
        bytes[at + i * 4..at + i * 4 + 4].copy_from_slice(&0u32.to_le_bytes());
    }
    let _ = parse(&bytes);
    // And a directory whose siblings point at each other.
    let mut bytes = sample().msg();
    let dir = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;
    let at = (dir + 1) * 512;
    for e in 0..4 {
        let entry = at + e * 128;
        bytes[entry + 68..entry + 72].copy_from_slice(&1u32.to_le_bytes());
        bytes[entry + 72..entry + 76].copy_from_slice(&0u32.to_le_bytes());
        bytes[entry + 76..entry + 80].copy_from_slice(&1u32.to_le_bytes());
    }
    let _ = parse(&bytes);
}

#[test]
fn an_attached_message_that_is_its_own_parent_ends() {
    // The attached message's storage points back at the root's children: every level holds the
    // whole file again, until the depth cap.
    let inner = Props::new().unicode(SUBJECT, "inner");
    let mut bytes =
        Props::new().unicode(SUBJECT, "outer").attachment(Props::new().embedded(inner)).attachment(Props::new().embedded(Props::new())).msg();
    let dir = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;
    let at = (dir + 1) * 512;
    let name: Vec<u8> = "__substg1.0_3701000D".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let root_child = bytes[at + 76..at + 80].to_vec();
    let mut found = 0;
    for e in 0..(bytes.len() - at) / 128 {
        let entry = at + e * 128;
        if bytes[entry..entry + name.len()] == name[..] {
            bytes[entry + 76..entry + 80].copy_from_slice(&root_child);
            found += 1;
        }
    }
    assert_eq!(found, 2);
    let m = parse(&bytes).unwrap();
    assert_eq!(m.subject, "outer");
    assert!(m.attachments.len() <= 2);
}

#[test]
fn a_truncated_file_gives_what_there_is() {
    let bytes = sample().msg();
    for len in (0..bytes.len()).step_by(97) {
        let _ = parse(&bytes[..len]);
    }
}

#[test]
fn absurd_sizes_are_capped() {
    // A stream that claims 4 GB.
    let mut bytes = Props::new().unicode(SUBJECT, "x").msg();
    let dir = u32::from_le_bytes(bytes[48..52].try_into().unwrap()) as usize;
    let at = (dir + 1) * 512;
    for e in 1..4 {
        bytes[at + e * 128 + 120..at + e * 128 + 128].copy_from_slice(&u64::MAX.to_le_bytes());
    }
    let _ = parse(&bytes);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = parse(&bytes);
        let mut with_signature = crate::SIGNATURE.to_vec();
        with_signature.extend_from_slice(&bytes);
        let _ = parse(&with_signature);
    }

    #[test]
    fn a_mutated_message_never_panics(edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..64)) {
        let mut bytes = sample().msg();
        for (at, b) in edits {
            let i = at.index(bytes.len());
            bytes[i] = b;
        }
        let _ = parse(&bytes);
    }

    #[test]
    fn arbitrary_rtf_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = rtf::decompress(&bytes);
        let _ = rtf::read(&bytes);
        let mut lzfu = vec![0u8; 16];
        lzfu[0..4].copy_from_slice(&((bytes.len() + 12) as u32).to_le_bytes());
        lzfu[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        lzfu[8..12].copy_from_slice(&0x7546_5A4Cu32.to_le_bytes());
        lzfu.extend_from_slice(&bytes);
        let _ = rtf::decompress(&lzfu);
    }
}
