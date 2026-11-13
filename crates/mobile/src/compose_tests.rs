// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing a message through the binding: starting one (new, reply, reply all, forward, a draft, a
//! `mailto:` link), editing in UTF-16 units, the looks, lists, links and pictures, the recipients, the
//! account it is sent from, files, drafts and sending. Invented people and `example.org` addresses only;
//! nothing leaves the process.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::{Address, Attachment, FolderKind, Message};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::{AddressRecord, ComposeFormat, ComposeKind, ComposeStart, ComposeView, Core, CoreError, ListMark, RecipientLine};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

/// A mailbox with one local account, a message from Ada (with a cc), and a saved signature for the account.
fn core() -> (Arc<Core>, u64, u64) {
    let core = Core::start(FakePlatform::new(), || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder: inbox, subject: "Garden plan".into(), ..Message::default() };
        m.from = Address::new("Ada Quill", "ada@example.org");
        m.to = vec![Address::new("Avery", "avery@example.org")];
        m.cc = vec![Address::new("Bo Marsh", "bo@example.org")];
        m.body.text = "See you at ten.".into();
        mb.add_message(m).unwrap();
        let mut s = Session::at(mb, now(), 0);
        s.run("signature.set", &json!({"name": "Avery", "text": "Avery Stone"})).unwrap();
        s.run("signature.set", &json!({"name": "Plain", "text": "Sent from my phone"})).unwrap();
        let account = s.mailbox.accounts[0].id;
        s.run("signature.defaults", &json!({"account": account, "new": "Avery", "reply": "Avery"})).unwrap();
        s
    })
    .unwrap();
    let folders = block_on(core.folders(0)).unwrap();
    let inbox = folders.folders.iter().find(|f| f.kind == "inbox").unwrap().id;
    let message = block_on(core.messages(inbox, "all".into(), 0, 5)).unwrap().rows[0].id;
    let account = folders.accounts[0].id;
    (core, message, account)
}

fn start(core: &Core, how: ComposeStart) -> ComposeView {
    block_on(core.compose_start(how)).unwrap()
}

fn new(core: &Core) -> ComposeView {
    start(core, ComposeStart::New { account: 0, mailto: None })
}

fn type_in(core: &Core, v: &ComposeView, at: u32, text: &str) -> ComposeView {
    block_on(core.compose_edit(v.compose, at, at, text.to_string())).unwrap()
}

fn people(list: &[&str]) -> Vec<AddressRecord> {
    list.iter().map(|a| AddressRecord { name: String::new(), address: (*a).to_string() }).collect()
}

#[test]
fn a_new_message_starts_with_the_accounts_signature_after_a_blank_line() {
    let (core, _, _) = core();
    let v = new(&core);
    assert_eq!(v.kind, ComposeKind::New);
    assert_eq!(v.signature, "Avery");
    assert!(v.html);
    assert!(v.text.ends_with("Avery Stone"), "{:?}", v.text);
    // The owner's own text comes first, then the signature, and only the signature is marked as one.
    let marked: Vec<bool> = v.paragraphs.iter().map(|p| p.signature).collect();
    assert_eq!(marked.first(), Some(&false));
    assert_eq!(marked.last(), Some(&true));
    assert!(v.to.is_empty() && v.cc.is_empty() && v.bcc.is_empty());
    assert!(!v.dirty);
}

#[test]
fn a_reply_quotes_the_original_and_goes_to_its_sender() {
    let (core, message, _) = core();
    let v = start(&core, ComposeStart::Reply { id: message });
    assert_eq!(v.kind, ComposeKind::Reply);
    assert_eq!(v.source, message);
    assert_eq!(v.subject, "RE: Garden plan");
    assert_eq!(v.to.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), vec!["ada@example.org"]);
    assert!(v.cc.is_empty());
    // The original is quoted: in the body (plain text original) where the owner can edit it.
    assert!(v.text.contains("See you at ten."), "{:?}", v.text);
    assert!(v.text.contains("Ada Quill"), "{:?}", v.text);
    assert_eq!(v.signature, "Avery");
}

#[test]
fn reply_all_keeps_the_others_and_never_the_owner() {
    let (core, message, _) = core();
    let v = start(&core, ComposeStart::ReplyAll { id: message });
    assert_eq!(v.kind, ComposeKind::ReplyAll);
    assert_eq!(v.to.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), vec!["ada@example.org"]);
    assert_eq!(v.cc.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), vec!["bo@example.org"]);
    assert!(v.to.iter().chain(&v.cc).all(|p| p.address != "avery@example.org"));
}

#[test]
fn a_forward_quotes_and_has_nobody_to_send_to() {
    let (core, message, _) = core();
    let v = start(&core, ComposeStart::Forward { id: message });
    assert_eq!(v.kind, ComposeKind::Forward);
    assert_eq!(v.subject, "FW: Garden plan");
    assert!(v.to.is_empty());
    assert!(v.text.contains("See you at ten."), "{:?}", v.text);
}

#[test]
fn a_message_that_is_not_there_cannot_be_answered() {
    let (core, _, _) = core();
    for how in
        [ComposeStart::Reply { id: 0 }, ComposeStart::ReplyAll { id: u64::MAX }, ComposeStart::Forward { id: 123_456 }, ComposeStart::Draft { id: 0 }]
    {
        assert!(block_on(core.compose_start(how)).is_err());
    }
    // Nothing was opened by the refusals.
    assert!(block_on(core.compose_view(0)).is_err());
}

#[test]
fn a_mailto_link_prefills_and_nothing_else_of_it_is_used() {
    let (core, _, _) = core();
    let link = "mailto:ada@example.org?cc=bo@example.org&subject=Hi%20there&body=Hello%0Aworld&attach=/etc/passwd&from=evil@example.org";
    let v = start(&core, ComposeStart::New { account: 0, mailto: Some(link.to_string()) });
    assert_eq!(v.to.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), vec!["ada@example.org"]);
    assert_eq!(v.cc.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(), vec!["bo@example.org"]);
    assert_eq!(v.subject, "Hi there");
    assert!(v.text.starts_with("Hello\nworld"), "{:?}", v.text);
    assert!(v.files.is_empty());
    assert_ne!(v.account, 0);
    // It is only ever a draft in the making: nothing was sent.
    assert!(block_on(core.compose_view(v.compose)).is_ok());
}

#[test]
fn a_mailto_link_that_is_not_one_or_is_huge_is_refused() {
    let (core, _, _) = core();
    for bad in ["", "http://example.org/", "mailto", "javascript:alert(1)"] {
        assert!(block_on(core.compose_start(ComposeStart::New { account: 0, mailto: Some(bad.to_string()) })).is_err(), "{bad:?}");
    }
    let huge = format!("mailto:ada@example.org?body={}", "a".repeat(100_000));
    assert!(matches!(block_on(core.compose_start(ComposeStart::New { account: 0, mailto: Some(huge) })), Err(CoreError::BadRequest { .. })));
}

#[test]
fn typing_counts_utf16_units_and_never_splits_an_emoji() {
    let (core, _, _) = core();
    let v = new(&core);
    let sig = v.text.clone();
    // Typed in front of the signature, after the blank line's start.
    let v = type_in(&core, &v, 0, "a\u{1F600}b");
    assert!(v.text.starts_with("a\u{1F600}b"));
    assert_eq!(v.text.chars().count(), sig.chars().count() + 3);
    // The emoji is two UTF-16 units: deleting units 1..3 takes it away whole.
    let v = block_on(core.compose_edit(v.compose, 1, 3, String::new())).unwrap();
    assert!(v.text.starts_with("ab"), "{:?}", v.text);
    // An edit that lands inside a pair takes the whole character, never half of one.
    let v = type_in(&core, &v, 0, "\u{1F600}");
    let v = block_on(core.compose_edit(v.compose, 1, 1, "x".to_string())).unwrap();
    assert!(v.text.contains('x') && v.text.starts_with('\u{1F600}'), "{:?}", v.text);
    assert!(v.dirty);
}

#[test]
fn an_edit_replaces_a_range_and_a_backwards_range_is_the_same() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "hello world");
    let v = block_on(core.compose_edit(v.compose, 11, 6, "there".to_string())).unwrap();
    assert!(v.text.starts_with("hello there"), "{:?}", v.text);
    // Past the end is the end; an empty edit changes nothing.
    let before = v.text.clone();
    let v = block_on(core.compose_edit(v.compose, 9_000, 9_999, String::new())).unwrap();
    assert_eq!(v.text, before);
    let v = block_on(core.compose_edit(v.compose, u32::MAX, u32::MAX, "!".to_string())).unwrap();
    assert!(v.text.ends_with('!'), "{:?}", v.text);
}

#[test]
fn a_line_break_starts_a_paragraph_and_carriage_returns_are_not_kept() {
    let (core, _, _) = core();
    let v = new(&core);
    let before = v.paragraphs.len();
    let v = type_in(&core, &v, 0, "one\r\ntwo\rthree");
    assert_eq!(v.paragraphs.len(), before + 2);
    assert!(!v.text.contains('\r'));
    assert!(v.text.starts_with("one\ntwo\nthree"), "{:?}", v.text);
}

#[test]
fn an_edit_over_the_caps_is_refused_and_changes_nothing() {
    let (core, _, _) = core();
    let v = new(&core);
    let big = "x".repeat(200_001);
    assert!(matches!(block_on(core.compose_edit(v.compose, 0, 0, big)), Err(CoreError::Failed { .. })));
    // Many edits that each fit are held to the whole body's cap.
    let mut v = v;
    let chunk = "y".repeat(200_000);
    let mut refused = false;
    for _ in 0..8 {
        match block_on(core.compose_edit(v.compose, 0, 0, chunk.clone())) {
            Ok(next) => v = next,
            Err(CoreError::Failed { .. }) => {
                refused = true;
                break;
            }
            Err(other) => panic!("{other:?}"),
        }
    }
    assert!(refused);
    assert!(v.text.chars().count() <= crate::compose::MAX_BODY_CHARS);
}

#[test]
fn bold_italic_underline_and_links_are_spans_of_the_text() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "hello world");
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Bold, 0, 5)).unwrap();
    assert!(v.spans.iter().any(|s| s.bold && (s.start, s.end) == (0, 5)), "{:?}", v.spans);
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Italic, 6, 11)).unwrap();
    assert!(v.spans.iter().any(|s| s.italic && (s.start, s.end) == (6, 11)));
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Underline, 0, 11)).unwrap();
    assert!(v.spans.iter().any(|s| s.underline));
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Link { url: Some("https://example.org/a".into()) }, 6, 11)).unwrap();
    assert!(v.spans.iter().any(|s| s.link.as_deref() == Some("https://example.org/a")));
    // Toggled off again, and the link taken away.
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Bold, 0, 5)).unwrap();
    assert!(!v.spans.iter().any(|s| s.bold && (s.start, s.end) == (0, 5)));
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Link { url: None }, 6, 11)).unwrap();
    assert!(v.spans.iter().all(|s| s.link.is_none()));
}

#[test]
fn spans_count_utf16_units_after_an_emoji() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "\u{1F600} bold");
    // The emoji is two units, the space one: "bold" is units 3..7.
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Bold, 3, 7)).unwrap();
    assert!(v.spans.iter().any(|s| s.bold && (s.start, s.end) == (3, 7)), "{:?}", v.spans);
}

#[test]
fn a_link_that_runs_something_is_refused() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "click");
    for url in ["javascript:alert(1)", "data:text/html,x", "file:///etc/passwd", "cid:x", "vbscript:x"] {
        assert!(block_on(core.compose_format(v.compose, ComposeFormat::Link { url: Some(url.into()) }, 0, 5)).is_err(), "{url}");
    }
    let long = format!("https://example.org/{}", "a".repeat(10_000));
    assert!(matches!(block_on(core.compose_format(v.compose, ComposeFormat::Link { url: Some(long) }, 0, 5)), Err(CoreError::BadRequest { .. })));
}

#[test]
fn lists_are_paragraph_marks_with_their_numbers() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "one\ntwo\nthree");
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Numbering, 0, 13)).unwrap();
    let numbered: Vec<u32> = v.paragraphs.iter().filter(|p| p.list == ListMark::Number).map(|p| p.number).collect();
    assert_eq!(numbered, vec![1, 2, 3]);
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Bullets, 0, 13)).unwrap();
    assert!(v.paragraphs.iter().filter(|p| p.list == ListMark::Bullet).count() >= 3);
    let v = block_on(core.compose_format(v.compose, ComposeFormat::Indent, 0, 3)).unwrap();
    assert!(v.paragraphs.iter().any(|p| p.level == 1));
    // Clearing takes every look and list away.
    let v = block_on(core.compose_format(v.compose, ComposeFormat::ClearFormatting, 0, 13)).unwrap();
    assert!(v.paragraphs.iter().all(|p| p.list == ListMark::None));
}

#[test]
fn typing_dash_space_starts_a_list_when_it_is_asked_for() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "-");
    let v = type_in(&core, &v, 1, " ");
    // The keyboard's space after `-` is the engine's own auto-list: the dash is gone and the paragraph is an item.
    assert!(v.paragraphs.iter().any(|p| p.list == ListMark::Bullet) || v.text.starts_with("- "), "{:?}", v.text);
}

#[test]
fn a_message_in_plain_text_has_no_formatting() {
    let (core, _, _) = core();
    let v = new(&core);
    block_on(core.execute("compose.formatPlain".into(), "{\"compose\":".to_string() + &v.compose.to_string() + "}")).unwrap();
    let v = block_on(core.compose_view(v.compose)).unwrap();
    assert!(!v.html);
    assert!(block_on(core.compose_format(v.compose, ComposeFormat::Bold, 0, 0)).is_err());
    assert!(block_on(core.compose_picture(v.compose, PNG.to_vec(), String::new(), 0)).is_err());
}

#[test]
fn a_picture_is_one_object_character_in_the_text() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = type_in(&core, &v, 0, "ab");
    let v = block_on(core.compose_picture(v.compose, PNG.to_vec(), "A fern".into(), 1)).unwrap();
    assert_eq!(v.pictures.len(), 1);
    assert_eq!(v.pictures[0].at, 1);
    assert_eq!(v.pictures[0].alt, "A fern");
    assert!(v.text.starts_with("a\u{fffc}b"), "{:?}", v.text);
    let bytes = block_on(core.compose_picture_bytes(v.compose, v.pictures[0].id)).unwrap();
    assert_eq!(bytes.mime, "image/png");
    assert!(bytes.bytes.starts_with(b"\x89PNG"));
    assert!(block_on(core.compose_picture_bytes(v.compose, 9_999)).is_err());
}

#[test]
fn only_a_picture_by_its_own_bytes_goes_in_the_text() {
    let (core, _, _) = core();
    let v = new(&core);
    for bytes in [b"MZ this is a program".to_vec(), Vec::new(), b"GIF".to_vec(), vec![0u8; 100]] {
        assert!(block_on(core.compose_picture(v.compose, bytes, String::new(), 0)).is_err());
    }
    let huge = {
        let mut b = PNG.to_vec();
        b.resize(21 * 1024 * 1024, 0);
        b
    };
    assert!(matches!(block_on(core.compose_picture(v.compose, huge, String::new(), 0)), Err(CoreError::Failed { .. })));
    assert!(block_on(core.compose_view(v.compose)).unwrap().pictures.is_empty());
}

#[test]
fn recipients_are_lines_of_people_and_come_back_parsed() {
    let (core, _, _) = core();
    let v = new(&core);
    let who = vec![
        AddressRecord { name: "Quill, Ada".into(), address: "ada@example.org".into() },
        AddressRecord { name: String::new(), address: "bo@example.org".into() },
        AddressRecord { name: String::new(), address: "not an address".into() },
    ];
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, who)).unwrap();
    assert_eq!(v.to.len(), 3);
    assert_eq!((v.to[0].name.as_str(), v.to[0].address.as_str(), v.to[0].valid), ("Quill, Ada", "ada@example.org", true));
    assert!(v.to[1].valid && !v.to[2].valid);
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::Cc, people(&["cy@example.org"]))).unwrap();
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::Bcc, people(&["di@example.org"]))).unwrap();
    assert_eq!((v.cc.len(), v.bcc.len()), (1, 1));
    // The whole line is replaced.
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, Vec::new())).unwrap();
    assert!(v.to.is_empty());
}

#[test]
fn a_line_of_recipients_never_carries_a_header_of_its_own() {
    let (core, _, _) = core();
    let v = new(&core);
    let evil = vec![AddressRecord { name: "Ada\r\nBcc: spy@example.org".into(), address: "ada@example.org\r\nBcc: spy@example.org".into() }];
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, evil)).unwrap();
    for p in v.to.iter().chain(&v.cc).chain(&v.bcc) {
        assert!(!p.name.contains(['\r', '\n']) && !p.address.contains(['\r', '\n']), "{p:?}");
    }
    assert!(v.bcc.is_empty());
    let v = block_on(core.compose_subject(v.compose, "Hi\r\nBcc: spy@example.org".into())).unwrap();
    assert!(!v.subject.contains(['\r', '\n']));
    let many: Vec<AddressRecord> = (0..2_000).map(|n| AddressRecord { name: String::new(), address: format!("p{n}@example.org") }).collect();
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, many)).unwrap();
    assert!(v.to.len() <= 500);
}

#[test]
fn the_subject_is_held_to_its_length() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = block_on(core.compose_subject(v.compose, "s".repeat(10_000))).unwrap();
    assert!(v.subject.chars().count() <= crate::text::MAX_SUBJECT);
}

#[test]
fn files_have_safe_names_their_kind_from_their_bytes_and_caps() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = block_on(core.compose_attach(v.compose, "../../etc/report.pdf".into(), b"%PDF-1.4".to_vec())).unwrap();
    assert_eq!(v.files[0].name, "report.pdf");
    assert_eq!(v.files[0].mime, "application/pdf");
    // A name that says picture over bytes that are not is no picture; bytes that are a picture are one whatever the name.
    let v = block_on(core.compose_attach(v.compose, "photo.jpg".into(), b"MZ program".to_vec())).unwrap();
    assert_eq!(v.files[1].mime, "application/octet-stream");
    let v = block_on(core.compose_attach(v.compose, "pic.dat".into(), PNG.to_vec())).unwrap();
    assert_eq!(v.files[2].mime, "image/png");
    // A name with controls, an odd length or nothing at all.
    let v = block_on(core.compose_attach(v.compose, "a\0b\u{202e}.txt".into(), vec![1])).unwrap();
    assert!(!v.files[3].name.contains(['\0', '\u{202e}']));
    let v = block_on(core.compose_attach(v.compose, String::new(), vec![1])).unwrap();
    assert_eq!(v.files[4].name, "attachment");
    assert_eq!(v.files.len(), 5);
    let v = block_on(core.compose_remove_file(v.compose, 1)).unwrap();
    assert_eq!(v.files.len(), 4);
    assert!(block_on(core.compose_remove_file(v.compose, 99)).is_err());
}

#[test]
fn one_file_and_all_files_are_held_to_their_caps() {
    let (core, _, _) = core();
    let v = new(&core);
    let too_big = vec![0u8; crate::compose::MAX_COMPOSE_FILE + 1];
    assert!(matches!(block_on(core.compose_attach(v.compose, "big.bin".into(), too_big)), Err(CoreError::Failed { .. })));
    let big = vec![1u8; crate::compose::MAX_COMPOSE_FILE];
    let v = block_on(core.compose_attach(v.compose, "one.bin".into(), big.clone())).unwrap();
    let v = block_on(core.compose_attach(v.compose, "two.bin".into(), big.clone())).unwrap();
    // Together they are at the total's cap: one more byte is too many.
    assert!(matches!(block_on(core.compose_attach(v.compose, "three.bin".into(), vec![1])), Err(CoreError::Failed { .. })));
    assert_eq!(block_on(core.compose_view(v.compose)).unwrap().files.len(), 2);
}

#[test]
fn from_lists_the_accounts_and_shared_mailboxes_and_switching_swaps_the_signature() {
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org"), now(), 0);
        s.run("signature.set", &json!({"name": "Avery", "text": "Avery Stone"})).unwrap();
        s.run("signature.set", &json!({"name": "Desk", "text": "The support desk"})).unwrap();
        let first = s.mailbox.accounts[0].id;
        let second = s.run("account.add", &json!({"email": "support@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        s.run("signature.defaults", &json!({"account": first, "new": "Avery", "reply": "Avery"})).unwrap();
        s.run("signature.defaults", &json!({"account": second, "new": "Desk", "reply": "Desk"})).unwrap();
        s
    })
    .unwrap();
    let choices = block_on(core.compose_accounts()).unwrap();
    assert_eq!(choices.len(), 2);
    assert!(choices.iter().all(|c| !c.shared));
    let (first, second) = (choices[0].account, choices[1].account);
    let v = start(&core, ComposeStart::New { account: first, mailto: None });
    assert_eq!((v.account, v.signature.as_str()), (first, "Avery"));
    assert!(v.text.ends_with("Avery Stone"));
    let v = block_on(core.compose_from(v.compose, second)).unwrap();
    assert_eq!((v.account, v.signature.as_str()), (second, "Desk"));
    assert!(v.text.ends_with("The support desk"), "{:?}", v.text);
    assert!(!v.text.contains("Avery Stone"));
    // An account that is not there is refused, and the message stays as it was.
    assert!(block_on(core.compose_from(v.compose, 99_999)).is_err());
    assert_eq!(block_on(core.compose_view(v.compose)).unwrap().account, second);
}

#[test]
fn a_signature_the_owner_edited_is_not_swapped_for_another_accounts() {
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org"), now(), 0);
        s.run("signature.set", &json!({"name": "Avery", "text": "Avery Stone"})).unwrap();
        s.run("signature.set", &json!({"name": "Desk", "text": "The support desk"})).unwrap();
        let first = s.mailbox.accounts[0].id;
        let second = s.run("account.add", &json!({"email": "support@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        s.run("signature.defaults", &json!({"account": first, "new": "Avery"})).unwrap();
        s.run("signature.defaults", &json!({"account": second, "new": "Desk"})).unwrap();
        s
    })
    .unwrap();
    let choices = block_on(core.compose_accounts()).unwrap();
    let v = start(&core, ComposeStart::New { account: choices[0].account, mailto: None });
    let at = u32::try_from(v.text.len()).unwrap();
    let v = block_on(core.compose_edit(v.compose, at, at, " (edited)".into())).unwrap();
    let v = block_on(core.compose_from(v.compose, choices[1].account)).unwrap();
    assert!(v.text.contains("Avery Stone (edited)"), "{:?}", v.text);
}

#[test]
fn suggestions_come_from_who_mail_was_exchanged_with_and_never_panic() {
    let (core, _, _) = core();
    let found = block_on(core.suggest_recipients("ad".into(), 5)).unwrap();
    assert!(found.iter().any(|s| s.address == "ada@example.org"), "{found:?}");
    assert!(found.len() <= 5);
    assert!(block_on(core.suggest_recipients("zzzz".into(), 5)).unwrap().is_empty());
    for junk in ["", " ", "\0", "\u{202e}", "%", "\"", "a".repeat(10_000).as_str(), "<<<>>>", "\r\nBcc: x"] {
        assert!(block_on(core.suggest_recipients(junk.to_string(), 5)).is_ok(), "{junk:?}");
    }
    assert!(block_on(core.suggest_recipients("a".into(), 0)).unwrap().len() <= 20);
    assert!(block_on(core.suggest_recipients("a".into(), u32::MAX)).unwrap().len() <= 20);
}

#[test]
fn sending_files_the_message_as_sent_for_a_local_account_and_closes_it() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, people(&["avery@example.org"]))).unwrap();
    let v = block_on(core.compose_subject(v.compose, "Note to self".into())).unwrap();
    let v = type_in(&core, &v, 0, "Remember the fern.");
    let sent = block_on(core.compose_send(v.compose)).unwrap();
    assert!(!sent.queued, "a local account has no server to queue for");
    assert!(block_on(core.compose_view(v.compose)).is_err(), "the message is closed once sent");
}

#[test]
fn a_message_for_an_account_that_sends_waits_in_the_outbox_for_the_next_check() {
    let core = Core::start(FakePlatform::new(), || {
        let mut s = Session::at(mailcraft_model::Mailbox::default(), now(), 0);
        s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap();
        s
    })
    .unwrap();
    let v = new(&core);
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, people(&["ada@example.org"]))).unwrap();
    let v = block_on(core.compose_subject(v.compose, "Hello".into())).unwrap();
    let sent = block_on(core.compose_send(v.compose)).unwrap();
    assert!(sent.queued);
    let outbox = block_on(core.folders(0)).unwrap().folders.into_iter().find(|f| f.kind == "outbox").unwrap();
    assert_eq!(outbox.total, 1);
}

#[test]
fn a_send_with_nobody_to_send_to_is_refused_and_the_message_stays_open() {
    let (core, _, _) = core();
    let v = new(&core);
    assert!(block_on(core.compose_send(v.compose)).is_err());
    assert!(block_on(core.compose_view(v.compose)).is_ok());
    // Nor to a line with only what is no address.
    let v = block_on(core.compose_recipients(v.compose, RecipientLine::To, people(&["not an address"]))).unwrap();
    assert!(block_on(core.compose_send(v.compose)).is_err());
    assert!(block_on(core.compose_view(v.compose)).is_ok());
}

#[test]
fn a_draft_is_kept_and_continues_where_it_was_left() {
    let (core, _, _) = core();
    let v = new(&core);
    let v = block_on(core.compose_subject(v.compose, "Work in progress".into())).unwrap();
    let v = type_in(&core, &v, 0, "Half a thought");
    block_on(core.compose_save(v.compose)).unwrap();
    block_on(core.compose_close(v.compose, false)).unwrap();
    assert!(block_on(core.compose_view(v.compose)).is_err());
    let folders = block_on(core.folders(0)).unwrap();
    let drafts = folders.folders.iter().find(|f| f.kind == "drafts").unwrap().id;
    let rows = block_on(core.messages(drafts, "all".into(), 0, 10)).unwrap().rows;
    assert_eq!(rows.len(), 1, "saved once, however many times it was saved");
    let again = start(&core, ComposeStart::Draft { id: rows[0].id });
    assert_eq!(again.kind, ComposeKind::Draft);
    assert_eq!(again.subject, "Work in progress");
    assert!(again.text.contains("Half a thought"), "{:?}", again.text);
}

#[test]
fn closing_keeps_changes_as_a_draft_and_discarding_does_not() {
    let (core, _, _) = core();
    let drafts_of = |core: &Core| {
        let folders = block_on(core.folders(0)).unwrap();
        let drafts = folders.folders.iter().find(|f| f.kind == "drafts").unwrap().id;
        block_on(core.messages(drafts, "all".into(), 0, 10)).unwrap().rows.len()
    };
    let v = new(&core);
    block_on(core.compose_close(v.compose, false)).unwrap();
    assert_eq!(drafts_of(&core), 0, "nothing was written, so nothing is kept");
    let v = new(&core);
    let v = type_in(&core, &v, 0, "kept");
    block_on(core.compose_close(v.compose, false)).unwrap();
    assert_eq!(drafts_of(&core), 1);
    let v = new(&core);
    let v = type_in(&core, &v, 0, "thrown away");
    block_on(core.compose_close(v.compose, true)).unwrap();
    assert_eq!(drafts_of(&core), 1);
}

#[test]
fn several_messages_can_be_open_and_each_is_named() {
    let (core, message, _) = core();
    let a = new(&core);
    let b = start(&core, ComposeStart::Reply { id: message });
    assert_ne!(a.compose, b.compose);
    let a2 = block_on(core.compose_subject(a.compose, "First".into())).unwrap();
    assert_eq!(a2.subject, "First");
    assert_eq!(block_on(core.compose_view(b.compose)).unwrap().subject, "RE: Garden plan");
    // 0 is the current one: the one started last.
    assert_eq!(block_on(core.compose_view(0)).unwrap().compose, b.compose);
    // The most that can be open at once is refused with words, not a crash.
    let mut opened = 2;
    while block_on(core.compose_start(ComposeStart::New { account: 0, mailto: None })).is_ok() {
        opened += 1;
        assert!(opened < 100);
    }
}

#[test]
fn a_message_with_attachments_forwards_them() {
    let core = Core::start(FakePlatform::new(), || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
        let mut m = Message { account, folder: inbox, subject: "Files".into(), ..Message::default() };
        m.attachments = vec![Attachment::new("plan.pdf", "application/pdf", b"%PDF-1.4".to_vec())];
        m.body.text = "Here".into();
        mb.add_message(m).unwrap();
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let folders = block_on(core.folders(0)).unwrap();
    let inbox = folders.folders.iter().find(|f| f.kind == "inbox").unwrap().id;
    let id = block_on(core.messages(inbox, "all".into(), 0, 5)).unwrap().rows[0].id;
    let v = start(&core, ComposeStart::Forward { id });
    assert_eq!(v.files.len(), 1);
    assert_eq!(v.files[0].name, "plan.pdf");
}

#[test]
fn every_call_with_junk_answers_and_never_panics() {
    let (core, message, _) = core();
    let v = new(&core);
    let c = v.compose;
    let ids = [0, c, c + 1, u64::MAX, 7_777_777];
    for id in ids {
        for (from, to) in [(0, 0), (u32::MAX, 0), (5, 3), (u32::MAX, u32::MAX), (1, 1_000_000)] {
            for text in ["", "x", "\u{1F600}", "\0", "\u{202e}", "a\r\nb", "\u{fffc}"] {
                let r = block_on(core.compose_edit(id, from, to, text.to_string()));
                assert!(!matches!(r, Err(CoreError::Internal)), "{id} {from} {to} {text:?}");
            }
            for format in [ComposeFormat::Bold, ComposeFormat::Bullets, ComposeFormat::Indent, ComposeFormat::Outdent, ComposeFormat::ClearFormatting]
            {
                let r = block_on(core.compose_format(id, format, from, to));
                assert!(!matches!(r, Err(CoreError::Internal)));
            }
            let r = block_on(core.compose_select(id, from, to));
            assert!(!matches!(r, Err(CoreError::Internal)));
        }
        for bytes in [Vec::new(), vec![0u8; 3], PNG.to_vec()] {
            assert!(!matches!(block_on(core.compose_attach(id, "x".into(), bytes.clone())), Err(CoreError::Internal)));
            assert!(!matches!(block_on(core.compose_picture(id, bytes, "\0".into(), u32::MAX)), Err(CoreError::Internal)));
        }
        assert!(!matches!(block_on(core.compose_remove_file(id, u32::MAX)), Err(CoreError::Internal)));
        assert!(!matches!(block_on(core.compose_from(id, u64::MAX)), Err(CoreError::Internal)));
        assert!(!matches!(block_on(core.compose_picture_bytes(id, u32::MAX)), Err(CoreError::Internal)));
        assert!(!matches!(block_on(core.compose_view(id)), Err(CoreError::Internal)));
    }
    for how in [
        ComposeStart::Reply { id: message },
        ComposeStart::Draft { id: message },
        ComposeStart::New { account: u64::MAX, mailto: Some("mailto:\0@\u{202e}?subject=%ZZ&body=%".into()) },
        ComposeStart::New { account: 0, mailto: Some("MAILTO:a@example.org?cc=&&&==&subject".into()) },
        ComposeStart::Shared { account: u64::MAX, subject: Some("\0\r\n".into()), text: "https://\0 http:// \u{202e}https://x".into() },
        ComposeStart::Shared { account: 0, subject: None, text: String::new() },
    ] {
        assert!(!matches!(block_on(core.compose_start(how)), Err(CoreError::Internal)));
    }
    // Whatever was refused, the first message is still whole.
    assert!(block_on(core.compose_view(c)).is_ok());
}

#[test]
fn a_signature_written_on_the_phone_is_the_accounts_for_new_messages_and_replies() {
    let (core, message, account) = core();
    assert_eq!(block_on(core.account_signature(account)).unwrap(), "Avery Stone");
    block_on(core.set_account_signature(account, "Avery\r\nGarden club".into())).unwrap();
    assert_eq!(block_on(core.account_signature(account)).unwrap(), "Avery\nGarden club");
    assert!(new(&core).text.ends_with("Avery\nGarden club"));
    // In a reply the signature goes between what the owner writes and the quoted original.
    let reply = start(&core, ComposeStart::Reply { id: message });
    let (sig, quoted) = (reply.text.find("Avery\nGarden club").unwrap(), reply.text.find("-----Original Message-----").unwrap());
    assert!(sig < quoted, "{:?}", reply.text);
    assert!(reply.paragraphs.iter().any(|p| p.signature));
    // The signatures the owner made on the computer stay as they were.
    let names = block_on(core.call(|s| Ok(s.mailbox.signatures.iter().map(|g| g.name.clone()).collect::<Vec<_>>()))).unwrap();
    assert!(names.contains(&"Avery".to_string()) && names.contains(&"Plain".to_string()), "{names:?}");
    // Written again it is replaced, not added; empty is none.
    block_on(core.set_account_signature(account, "Av".into())).unwrap();
    assert_eq!(block_on(core.account_signature(account)).unwrap(), "Av");
    block_on(core.set_account_signature(account, "  \n ".into())).unwrap();
    assert_eq!(block_on(core.account_signature(account)).unwrap(), "");
    assert_eq!(new(&core).signature, "");
}

#[test]
fn a_signature_for_nobody_or_too_long_is_held() {
    let (core, _, account) = core();
    assert!(block_on(core.account_signature(0)).is_err());
    assert!(block_on(core.account_signature(u64::MAX)).is_err());
    assert!(block_on(core.set_account_signature(u64::MAX, "x".into())).is_err());
    block_on(core.set_account_signature(account, "s".repeat(50_000))).unwrap();
    assert!(block_on(core.account_signature(account)).unwrap().chars().count() <= crate::compose::MAX_SIGNATURE_CHARS);
    for junk in ["\0", "\u{202e}", "a\r\nBcc: x", "\u{fffc}"] {
        assert!(!matches!(block_on(core.set_account_signature(account, junk.to_string())), Err(CoreError::Internal)));
    }
}
