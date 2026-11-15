// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox file, from any bytes, in every form it is read in: handed over as text (`file.open`
//! with `data`, the browser's stored mailbox), opened as a file (read whole, or replayed as the
//! form with a log of records when it begins as one), the same file read as a stream, and what
//! reads of it saved in the form with a log of records and opened again. All within a small budget
//! for what parsing builds, so that a file of tiny list elements is refused quickly.
#![no_main]

use std::fs;
use std::path::PathBuf;

use mailcraft_engine::test_support::{Limits, with_limits};

/// A directory of this process's own for the file, emptied after each input.
fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("mailcraft-fuzz-mailbox-{}", std::process::id()))
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let whole = Limits { element_bytes: 8 << 20, stream: false };
    let stream = Limits { stream: true, ..whole };
    let read = with_limits(whole, || mailcraft_engine::io::read_mailbox(data));
    let dir = dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("fuzz.mailcraft");
    if fs::write(&path, data).is_ok() {
        let _ = with_limits(whole, || mailcraft_engine::store::open(&path));
        let _ = with_limits(stream, || mailcraft_engine::store::open(&path));
        // The same bytes as a pack of header blocks beside it (#1376), and whatever the file names
        // read back from it.
        let blobs = mailcraft_engine::store::blob_dir(&path);
        if fs::create_dir_all(&blobs).is_ok()
            && fs::write(blobs.join("headers-1.pack"), data).is_ok()
            && let Ok(mut opened) = with_limits(whole, || mailcraft_engine::store::open(&path))
        {
            let _ = mailcraft_engine::store::load_all(&path, &mut opened.mailbox);
        }
    }
    // What read as a mailbox, in the form with a log of records.
    if let Ok(mailbox) = read {
        let saved = dir.join("saved.mailcraft");
        if mailcraft_engine::store::save(&saved, &mailbox).is_ok() {
            let _ = with_limits(whole, || mailcraft_engine::store::open(&saved));
        }
    }
    let _ = fs::remove_dir_all(&dir);
});
