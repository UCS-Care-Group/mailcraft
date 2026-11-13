// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The downloads and the sender-picture rules' facts, with hostile input and no network: every
//! address here is one the downloader refuses before it connects.

use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Message};

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::pictures::{fetch_picture, fetch_sender_picture, sender_picture_urls};
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
        assert!(fetch_picture(url.to_string()).is_err(), "{url:?}");
    }
    let long = format!("https://93.184.216.34/{}", "a".repeat(20_000));
    assert!(matches!(fetch_picture(long), Err(CoreError::BadRequest { .. })));
    assert!(fetch_picture("\u{202e}\u{0}é".repeat(5_000)).is_err());
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
