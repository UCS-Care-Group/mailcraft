// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1200: `--folder` takes a standard folder's kind, a path or an id.

use super::{open, run};

fn strings(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_string()).collect()
}

/// `--folder` takes a standard folder's kind, a path or an id (#1200): the three name the
/// same folder and export the same messages.
#[test]
fn folder_takes_a_kind_a_path_or_an_id() {
    let dir = std::env::temp_dir().join(format!("mailcraft-cli-kinds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.mailcraft").to_string_lossy().to_string();
    run(&strings(&["run", "--sample", "--save", &out])).unwrap();
    let s = open(&out).unwrap();
    let account = s.mailbox.default_account().unwrap().id;
    let sent = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Sent).unwrap();
    let path = s.mailbox.folder_path(sent);
    let export = |folder: &str, name: &str| {
        let mbox = dir.join(name).to_string_lossy().to_string();
        run(&strings(&["export", &out, &mbox, "--folder", folder])).unwrap();
        std::fs::read(&mbox).unwrap()
    };
    let by_kind = export("sent", "kind.mbox");
    assert!(!by_kind.is_empty());
    assert_eq!(export(&path, "path.mbox"), by_kind);
    assert_eq!(export(&sent.to_string(), "id.mbox"), by_kind);
    assert_ne!(export("inbox", "inbox.mbox"), by_kind);
    let missing = dir.join("none.mbox").to_string_lossy().to_string();
    assert!(run(&strings(&["export", &out, &missing, "--folder", "Nowhere"])).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
