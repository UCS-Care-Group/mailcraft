// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox file named by itself alone (`mailcraft-cli run --file mailbox.mailcraft`), with no
//! folder before its name, keeps its file from before a newer format as one named with a folder
//! does: the copy just kept was taken for an older one (`./m.before-…` against `m.before-…`) and
//! removed. The one test in this file, because it changes the working folder of the process.

use mailcraft_engine::store_before::keep_before_newer_format;
use std::fs;
use std::path::Path;

#[test]
fn a_file_named_without_a_folder_keeps_its_file_from_before() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("before-relative-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    std::env::set_current_dir(&dir).unwrap();
    // An index file of format 3: its magic, its format, nothing else.
    let mut old = mailcraft_engine::io::INDEX_MAGIC.to_vec();
    old.extend_from_slice(&3u32.to_le_bytes());
    old.extend_from_slice(&[0; 4]);
    fs::write("m.mailcraft", &old).unwrap();
    let kept = keep_before_newer_format(Path::new("m.mailcraft"), 4, "0.12.0").unwrap().unwrap();
    assert_eq!(kept, Path::new("m.mailcraft.before-0.12.0"));
    assert_eq!(fs::read(&kept).unwrap(), old);
    let _ = fs::remove_dir_all(&dir);
}
