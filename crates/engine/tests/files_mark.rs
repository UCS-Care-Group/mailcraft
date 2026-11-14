// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every file of a sender's bytes that MailCraft writes is marked as from the Internet (AU3-V6):
//! the writers are listed here, each writes through `mailcraft_engine::mark`, and a file of the
//! program that writes files and isn't listed fails this test until it says which it is.

use std::path::{Path, PathBuf};

/// The files that write a sender's bytes (a message, an attachment, an export of mail, a
/// calendar or contacts) to a file the owner or an agent named, and the call each makes.
const SENDER_WRITERS: &[(&str, &str)] = &[
    // `deliver`: every export (`file.exportMessage`, `exportFolder`, `exportCalendar`,
    // `exportContacts`) and Save Attachment.
    ("crates/engine/src/cmd/mod.rs", "mark::write_from_mail("),
    // Save All Attachments.
    ("crates/engine/src/cmd/save_all.rs", "mark::write_new_from_mail("),
    // `mailcraft-cli mcp --files DIR`: what an agent saves.
    ("crates/mcp/src/files.rs", "mark::write_new_from_mail("),
    // `ui.saveWindowMessage`: Save As of a message file opened in a window.
    ("crates/ui-egui/src/windows.rs", "mark::write_from_mail("),
    // An attachment written for the program that opens it: read-only, for this user only.
    ("apps/mailcraft/src/opened_files.rs", "mark::mark_from_internet("),
    // A private copy for the optional preview renderer.
    ("apps/mailcraft/src/attachment_render.rs", "mark::write_new_from_mail("),
];

/// The files that write files that are not a sender's bytes, and what they write.
const OTHER_WRITERS: &[(&str, &str)] = &[
    ("crates/engine/src/mark.rs", "the mark itself, and the writers of the sender's bytes"),
    ("crates/engine/src/store.rs", "the mailbox file and its content folder"),
    ("crates/engine/src/store_stream.rs", "the mailbox file's time"),
    ("crates/engine/src/store_pack.rs", "the packs of header blocks in the mailbox's content folder"),
    ("crates/engine/src/store_before.rs", "its tests"),
    ("crates/engine/src/words.rs", "the search index"),
    ("crates/engine/src/io.rs", "reads a file named from outside"),
    ("crates/engine/src/import_pst.rs", "the import's own report"),
    ("crates/net/src/update_download.rs", "a new version of the program, checked by its signature"),
    ("crates/ui-egui/src/theme.rs", "its tests"),
    ("apps/mailcraft/src/control_server.rs", "the control channel's key"),
    ("apps/mailcraft/src/desktop_icons.rs", "the desktop's entry for the program"),
    ("apps/mailcraft/src/diagnostics.rs", "its tests"),
    ("apps/mailcraft/src/favorites_organisation_flow.rs", "its tests"),
    ("apps/mailcraft/src/install.rs", "a probe of where the program is installed"),
    ("apps/mailcraft/src/logging.rs", "the log"),
    ("apps/mailcraft/src/picture_cache.rs", "pictures kept for the window, never handed out"),
    ("apps/mailcraft/src/self_update.rs", "the program's own update"),
    ("apps/mailcraft/src/single_instance.rs", "a lock"),
    ("apps/mailcraft-cli/src/main.rs", "its tests"),
];

/// What writes a file.
const WRITES: &[&str] = &["fs::write(", "File::create(", "OpenOptions", "AtomicFile::create("];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The program's source files under `dir`, but for tests.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name != "tests" {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
            out.push(path);
        }
    }
}

fn relative(path: &Path) -> String {
    let root = root().canonicalize().unwrap();
    path.canonicalize().unwrap().strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/")
}

#[test]
fn every_writer_of_a_senders_bytes_marks_the_file() {
    for (file, call) in SENDER_WRITERS {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        assert!(text.contains(call), "{file} writes a sender's bytes through `{call}`");
    }
}

#[test]
fn every_file_of_the_program_that_writes_files_is_listed() {
    let mut files = Vec::new();
    for top in ["crates", "apps"] {
        let Ok(entries) = std::fs::read_dir(root().join(top)) else { continue };
        for e in entries.flatten() {
            sources(&e.path().join("src"), &mut files);
        }
    }
    assert!(files.len() > 100, "the sources were found");
    let listed = |f: &str| SENDER_WRITERS.iter().chain(OTHER_WRITERS).any(|(l, _)| *l == f);
    let mut unlisted: Vec<String> = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = relative(&path);
        if WRITES.iter().any(|w| text.contains(w)) && !listed(&name) {
            unlisted.push(name);
        }
    }
    assert!(
        unlisted.is_empty(),
        "these write files: list each in SENDER_WRITERS (a sender's bytes, written through mailcraft_engine::mark) or OTHER_WRITERS (say what): {unlisted:?}"
    );
    // And no writer of a sender's bytes writes one any other way.
    for (file, _) in SENDER_WRITERS.iter().filter(|(f, _)| !f.ends_with("opened_files.rs") && !f.ends_with("mcp/src/files.rs")) {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        assert!(!WRITES.iter().any(|w| text.contains(w)), "{file} writes a file without the mark");
    }
}

#[test]
fn the_writers_write_the_bytes_and_a_new_file_is_never_written_over() {
    let dir = std::env::temp_dir().join(format!("mc-files-mark-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("saved.eml");
    mailcraft_engine::mark::write_from_mail(&file, b"Subject: x\r\n\r\ny").unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"Subject: x\r\n\r\ny");
    let again = mailcraft_engine::mark::write_new_from_mail(&file, b"other");
    assert_eq!(again.unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&file).unwrap(), b"Subject: x\r\n\r\ny");
    let new = dir.join("new.pdf");
    mailcraft_engine::mark::write_new_from_mail(&new, b"%PDF").unwrap();
    assert_eq!(std::fs::read(&new).unwrap(), b"%PDF");
    for f in [&file, &new] {
        assert_marked(f);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_export_through_deliver_is_marked_where_the_system_has_a_mark() {
    // `file.exportMessage` with a path writes through `deliver`, which marks it.
    let dir = std::env::temp_dir().join(format!("mc-files-mark-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("message.eml");
    let mut s = mailcraft_engine::Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0);
    let id = *s.mailbox.messages.keys().next().unwrap();
    s.run("file.exportMessage", &serde_json::json!({"id": id, "path": path.to_string_lossy()})).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 0);
    assert_marked(&path);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The file carries the system's "from the Internet" mark, where the system has one (AU3-V6).
fn assert_marked(path: &std::path::Path) {
    #[cfg(windows)]
    assert_eq!(std::fs::read(mailcraft_engine::mark::zone_stream(path)).unwrap(), mailcraft_engine::mark::ZONE_IDENTIFIER);
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/bin/xattr").arg("-p").arg("com.apple.quarantine").arg(path).output().unwrap();
        assert!(out.status.success(), "{} isn't quarantined", path.display());
        let value = String::from_utf8_lossy(&out.stdout);
        assert!(value.starts_with("0083;") && value.trim_end().ends_with(";MailCraft;"), "quarantine value: {value}");
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
}
