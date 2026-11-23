// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The downloads and the sender-picture rules' facts, with hostile input and no network: every
//! address here is one the downloader refuses before it connects.

use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Message};

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::pictures::{PictureStop, fetch_picture, fetch_sender_picture, picture_accepted, sender_picture_urls};
use crate::{Core, CoreError};

#[test]
fn a_download_refuses_what_is_not_the_public_web() {
    for url in [
        "",
        "   ",
        "ftp://example.org/x.png",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:image/png;base64,AAAA",
        "http://user:pass@93.184.216.34/x.png",
        "http://127.0.0.1:1/x.png",
        "http://localhost/x.png",
        "http://[::1]/x.png",
        "http://10.0.0.1/x.png",
        "http://192.168.1.1/x.png",
        "http://169.254.169.254/latest/meta-data",
        "http://[::ffff:127.0.0.1]/x.png",
        "https://\0/x.png",
    ] {
        assert!(fetch_picture(url.to_string(), PictureStop::new()).is_err(), "{url:?}");
    }
    let long = format!("https://93.184.216.34/{}", "a".repeat(20_000));
    assert!(matches!(fetch_picture(long, PictureStop::new()), Err(CoreError::BadRequest { .. })));
    assert!(fetch_picture("\u{202e}\u{0}é".repeat(5_000), PictureStop::new()).is_err());
}

#[test]
fn a_sender_picture_is_asked_of_the_three_places_only() {
    for url in ["", "https://example.org/avatar.png", "http://gravatar.com/avatar/x", "https://gravatar.com.evil.example/x", "http://127.0.0.1/"] {
        assert!(matches!(fetch_sender_picture(url.to_string()), Err(CoreError::BadRequest { .. })), "{url:?}");
    }
    // A place it may ask, on a private address, is refused by the downloader (no network is used).
    assert!(fetch_sender_picture("https://gravatar.com@127.0.0.1/avatar/x".to_string()).is_err());
}

#[test]
fn the_places_for_a_sender_are_the_desktops() {
    let urls = sender_picture_urls("Ada@Mail.Nutrient.io".to_string());
    assert_eq!(urls.len(), 3);
    assert!(urls[0].starts_with("https://gravatar.com/avatar/") && urls[0].ends_with("?s=96&d=404"));
    // The hash, never the address.
    assert!(!urls[0].contains("ada") && !urls[0].contains('@'));
    assert_eq!(urls[1], "https://favicon.im/nutrient.io?larger=true&throw-error-on-404=true");
    assert_eq!(urls[2], "https://www.google.com/s2/favicons?domain=nutrient.io&sz=96");
    // A mail provider's own icon says nothing of the sender; a hosting platform's would wear its own.
    assert_eq!(sender_picture_urls("ada@gmail.com".to_string()).len(), 1);
    assert_eq!(sender_picture_urls("ada@contoso.onmicrosoft.com".to_string()).len(), 1);
    for junk in ["", "@", "ada", "ada@", "@example.org", "a da@example.org", "ada@exa mple.org", "ada@\u{0}.org", "ada@localhost"] {
        assert!(sender_picture_urls(junk.to_string()).is_empty(), "{junk:?}");
    }
    assert!(sender_picture_urls(format!("{}@example.org", "a".repeat(400))).is_empty());
}

fn core_with_two() -> (std::sync::Arc<Core>, u64, u64) {
    let core = Core::start(FakePlatform::new(), || {
        let mut mb = mailcraft_model::Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        for (kind, subject) in [(FolderKind::Inbox, "Inbox one"), (FolderKind::Junk, "Junk one")] {
            let folder = mb.folder_of_kind(account, kind).unwrap();
            let mut m = Message { account, folder, subject: subject.into(), ..Message::default() };
            m.from = mailcraft_model::Address::new("Bo", "Bo@Example.org");
            m.body.text = "Hello".into();
            mb.add_message(m).unwrap();
        }
        Session::at(mb, now(), 0)
    })
    .unwrap();
    let mut ids = [0u64; 2];
    for (n, kind) in ["inbox", "junk"].iter().enumerate() {
        let folders = block_on(core.folders(0)).unwrap();
        let folder = folders.folders.iter().find(|f| f.kind == *kind).unwrap().id;
        ids[n] = block_on(core.messages(folder, "all".into(), 0, 5)).unwrap().rows[0].id;
    }
    (core, ids[0], ids[1])
}

#[test]
fn the_facts_for_a_sender_come_from_the_mailbox() {
    let (core, inbox, junk) = core_with_two();
    let facts = block_on(core.sender_facts(inbox)).unwrap();
    assert_eq!(facts.address, "Bo@Example.org");
    assert!(!facts.junk && !facts.vouched && !facts.known);
    assert!(block_on(core.sender_facts(junk)).unwrap().junk);
    assert!(block_on(core.sender_facts(u64::MAX)).is_err());
    assert!(block_on(core.sender_facts(0)).is_err());
}

#[test]
fn what_may_be_shown_is_decided_by_its_own_bytes() {
    let png = b"\x89PNG\r\n\x1a\n....".to_vec();
    let ico = b"\0\0\x01\0\x01\0....".to_vec();
    let empty_ico = b"\0\0\x01\0\0\0....".to_vec();
    assert!(picture_accepted(png.clone(), false) && picture_accepted(png, true));
    assert!(picture_accepted(vec![0xFF, 0xD8, 0xFF, 0xE0], false));
    assert!(picture_accepted(b"GIF89a..".to_vec(), false));
    assert!(picture_accepted(b"RIFF\0\0\0\0WEBPVP8 ".to_vec(), false));
    // An ICO is a sender's picture only, and only with a picture in it.
    assert!(!picture_accepted(ico.clone(), false) && picture_accepted(ico, true));
    assert!(!picture_accepted(empty_ico, true));
    for hostile in [vec![], vec![0], b"RIFF".to_vec(), b"RIFF\0\0\0\0WEB".to_vec(), b"\0\0\x01\0".to_vec(), b"<html>".to_vec()] {
        assert!(!picture_accepted(hostile.clone(), false) && !picture_accepted(hostile, true));
    }
}

#[test]
fn a_stop_given_before_ends_a_download_that_never_begins() {
    let stop = PictureStop::new();
    stop.stop();
    // (A public address that is never reached: the signal is read before anything is asked.)
    assert!(fetch_picture("https://93.184.216.34/x.png".to_string(), stop).is_err());
}
