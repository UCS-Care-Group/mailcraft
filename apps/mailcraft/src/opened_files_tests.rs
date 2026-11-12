// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The files an owner copies (#807): written beside the opened ones, kept when MailCraft quits.

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-copied-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir.join("MailCraft").join("Attachments")
}

#[test]
fn a_copied_file_is_written_safely_and_outlives_the_run() {
    let base = scratch("write");
    let opened = Opened::at(base.clone());
    // A hostile name: a path, an override that turns `fdp.exe` round, a control character.
    let a = opened.copy("..\\..\\evil/re\u{202e}fdp.exe", b"MZ").unwrap();
    let b = opened.copy("../../report.pdf", b"one").unwrap();
    let c = opened.copy("report.pdf", b"two").unwrap();
    let d = opened.copy("notes\r\n", b"x").unwrap();
    for path in [&a, &b, &c, &d] {
        assert!(path.starts_with(&base), "{path:?}");
        let run = path.parent().unwrap().parent().unwrap();
        assert!(run.file_name().unwrap().to_string_lossy().starts_with(COPIES), "{path:?}");
        // Not read-only: a paste keeps that, and the pasted file is the owner's to change.
        assert!(!fs::metadata(path).unwrap().permissions().readonly(), "{path:?}");
    }
    // Programs too (nothing is opened), under the name Save As gives: no folders, nothing turned
    // round, a kind for a file of none.
    assert_eq!(a.file_name().unwrap(), "re_fdp.exe");
    assert_eq!(d.file_name().unwrap(), "notes__.bin");
    assert_eq!((fs::read(&b).unwrap(), fs::read(&c).unwrap()), (b"one".to_vec(), b"two".to_vec()));
    assert_ne!(b, c);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        // The folders are this user's alone.
        assert_eq!(fs::metadata(a.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(a.parent().unwrap().parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }
    // An opened file goes into the run's other folder, which quitting empties; the copies stay.
    let opened_one = opened.write("notes.txt", b"y").unwrap();
    opened.clear();
    assert!(!opened_one.exists());
    assert!(a.exists() && b.exists() && c.exists() && d.exists());
    // Tidying at a start leaves them while they are this run's, and takes them once a later run
    // finds them a day old.
    opened.tidy(Duration::ZERO);
    assert!(a.exists());
    let later = Opened::at(base.clone());
    later.tidy(MAX_AGE);
    assert!(a.exists());
    later.tidy(Duration::ZERO);
    assert!(!a.exists() && !b.exists());
    assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
    let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
}

#[test]
fn the_copies_kept_are_capped_and_the_oldest_go_first() {
    let base = scratch("cap");
    let opened = Opened::at(base.clone()).keeping_at_most(100);
    let a = opened.copy("a.txt", &[1; 40]).unwrap();
    let b = opened.copy("b.txt", &[2; 40]).unwrap();
    assert!(a.exists() && b.exists());
    // Past the cap: the oldest goes, the one just written stays.
    let c = opened.copy("c.txt", &[3; 40]).unwrap();
    assert!(!a.exists() && b.exists() && c.exists());
    // One that is more than the cap alone is kept, for it is what was copied; the next copy takes it.
    let big = opened.copy("big.bin", &[4; 300]).unwrap();
    assert!(big.exists() && !b.exists() && !c.exists());
    let d = opened.copy("d.txt", &[5; 40]).unwrap();
    assert!(d.exists() && !big.exists());
    // The same file copied over and over (a key held down) is as many files as there were Copies
    // asked: the window writes it once (`attachment_open::Copies`), and the cap holds for the rest.
    for _ in 0..50 {
        opened.copy("d.txt", &[5; 40]).unwrap();
    }
    let kept: u64 = fs::read_dir(d.parent().unwrap().parent().unwrap()).unwrap().flatten().map(|e| tree_bytes(&e.path(), 1)).sum();
    assert!(kept <= 100, "{kept}");
    // Earlier runs' copies that are not old yet count too, and go oldest first.
    let g = opened.copy("g.txt", &[7; 60]).unwrap();
    let later = Opened::at(base.clone()).keeping_at_most(100);
    let e = later.copy("e.txt", &[6; 90]).unwrap();
    assert!(e.exists() && !g.exists());
    let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
}

#[cfg(unix)]
#[test]
fn a_copy_never_goes_through_a_link() {
    let base = scratch("link");
    let outside = base.parent().unwrap().parent().unwrap().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(base.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &base).unwrap();
    let opened = Opened::at(base.clone());
    assert!(opened.copy("report.pdf", b"x").is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    let _ = fs::remove_dir_all(base.parent().unwrap().parent().unwrap());
}
