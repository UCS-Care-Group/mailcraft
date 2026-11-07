// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Files on the system's clipboard (#807).

use super::*;

#[test]
fn only_whole_paths_are_taken_and_not_too_many() {
    let root = std::env::temp_dir();
    let whole = root.join("report.pdf");
    let listed = vec![PathBuf::from("relative/report.pdf"), whole.clone(), PathBuf::from(""), PathBuf::from("..")];
    assert_eq!(local_files(listed), vec![whole]);
    let many: Vec<PathBuf> = (0..MOST_FILES + 50).map(|i| root.join(format!("{i}.txt"))).collect();
    assert_eq!(local_files(many).len(), MOST_FILES);
    assert!(local_files(Vec::new()).is_empty());
}

/// The real clipboard of the machine the test runs on: a file put there comes back as a file, as
/// a file manager reads it, and text put there afterwards is no file. Run by hand, in each kind of
/// session: `cargo test -p mailcraft clipboard_tests -- --ignored`.
#[test]
#[ignore = "uses the system's clipboard"]
fn a_file_goes_on_the_system_clipboard_and_comes_back() {
    let dir = std::env::temp_dir().join(format!("mailcraft-clipboard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("white space.txt");
    std::fs::write(&file, b"x").unwrap();
    let board = Board::default();
    board.put_files(std::slice::from_ref(&file)).unwrap();
    let other = Board::default();
    assert_eq!(other.files(), vec![file]);
    arboard::Clipboard::new().unwrap().set_text("plain").unwrap();
    assert!(other.files().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
