// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_engine::Session;
use mailcraft_engine::io::OpenProgress;
use mailcraft_engine::store::{self, INLINE_MAX};
use mailcraft_model::{FolderKind, Mailbox, Message};

use crate::Core;
use crate::fake_tests::{FakePlatform, block_on};

/// A mailbox file whose Sent Items message has a body too long to be kept in the file itself, so a
/// window's opening leaves it on disk.
fn on_disk(name: &str) -> (std::path::PathBuf, u64, u64) {
    let dir = std::env::temp_dir().join(format!("mailcraft-mobile-preview-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let account = mb.accounts[0].id;
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
    let mut m = Message { account, folder: sent, subject: "Long".into(), ..Message::default() };
    m.body.text = format!("Opening words of an old letter. {}", "x".repeat(INLINE_MAX * 2));
    let id = mb.add_message(m).unwrap();
    store::save(&path, &mb).unwrap();
    (path, id, sent)
}

#[test]
fn a_message_kept_on_disk_has_its_preview_and_stays_on_disk() {
    let (path, id, sent) = on_disk("kept");
    let opened = store::open_for_window(&path, &OpenProgress::default()).unwrap();
    assert!(opened.mailbox.message(id).unwrap().body_is_kept());
    let at = path.clone();
    let core = Core::start(FakePlatform::new(), move || {
        let mut s = Session::new(opened.mailbox);
        s.path = Some(at);
        s
    })
    .unwrap();
    let window = block_on(core.messages(sent, "all".into(), 0, 5)).unwrap();
    assert!(window.rows[0].preview.starts_with("Opening words of an old letter."), "{:?}", window.rows[0].preview);
    // Asking changed nothing: the body is still where it was.
    assert!(block_on(core.call(move |s| Ok(s.mailbox.message(id).unwrap().body_is_kept()))).unwrap());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_body_that_cannot_be_read_leaves_the_row_with_an_empty_preview() {
    let (path, id, sent) = on_disk("unreadable");
    let opened = store::open_for_window(&path, &OpenProgress::default()).unwrap();
    assert!(opened.mailbox.message(id).unwrap().body_is_kept());
    // No file to read it from: the session has no path.
    let core = Core::start(FakePlatform::new(), move || Session::new(opened.mailbox)).unwrap();
    let window = block_on(core.messages(sent, "all".into(), 0, 5)).unwrap();
    assert_eq!(window.rows.len(), 1);
    assert_eq!(window.rows[0].subject, "Long");
    assert!(!window.rows[0].preview.contains("Opening words"), "{:?}", window.rows[0].preview);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_body_over_the_preview_cap_is_not_read_and_one_under_it_is() {
    use crate::queries::MAX_PREVIEW_READ;
    for (name, size, read) in [("under", MAX_PREVIEW_READ as usize - 1024, true), ("over", MAX_PREVIEW_READ as usize + 1, false)] {
        let dir = std::env::temp_dir().join(format!("mailcraft-mobile-preview-{}-cap-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mailbox.mailcraft");
        let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
        let account = mb.accounts[0].id;
        let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
        let mut m = Message { account, folder: sent, subject: "Long".into(), ..Message::default() };
        m.body.text = format!("Opening words. {}", "x".repeat(size));
        mb.add_message(m).unwrap();
        store::save(&path, &mb).unwrap();
        let opened = store::open_for_window(&path, &OpenProgress::default()).unwrap();
        let at = path.clone();
        let core = Core::start(FakePlatform::new(), move || {
            let mut s = Session::new(opened.mailbox);
            s.path = Some(at);
            s
        })
        .unwrap();
        let window = block_on(core.messages(sent, "all".into(), 0, 5)).unwrap();
        assert_eq!(window.rows[0].preview.starts_with("Opening words."), read, "{name}: {:?}", window.rows[0].preview);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
