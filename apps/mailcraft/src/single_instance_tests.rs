// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn temp_mailbox(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-instance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir.join("mailbox.mailcraft")
}

/// A file beside the mailbox, with something in it; its whole path.
fn file_beside(mailbox: &Path, name: &str) -> String {
    let dir = mailbox.parent().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    path.to_string_lossy().into_owned()
}

fn first_of(start: Start) -> Guard {
    match start {
        Start::First(g) => g,
        Start::Handed => panic!("handed over"),
        Start::Unreachable(e) => panic!("unreachable: {e}"),
    }
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn opened(open: &[&str]) -> Handed {
    Handed { open: strings(open), refused: 0 }
}

#[test]
fn the_second_start_hands_over_to_the_first_and_the_next_first_starts_afresh() {
    let mailbox = temp_mailbox("hand");
    let first = first_of(start(&mailbox, &[]));
    assert!(beside(&mailbox, "lock").exists() && beside(&mailbox, "instance").exists());
    assert_eq!(first.handed_over(), None, "nobody asked yet");

    // A later start with a link and a file named from where it was started: the first gets
    // both, the file by its whole path.
    let note = file_beside(&mailbox, "note.eml");
    let here = std::env::current_dir().unwrap();
    let relative = pathdiff(Path::new(&note), &here);
    let link = "mailto:maya@lumen.example?subject=Hi";
    assert!(matches!(start(&mailbox, &[link.to_string(), relative]), Start::Handed));
    let got = first.handed_over().unwrap();
    assert_eq!(got.open.first().map(String::as_str), Some(link));
    let file = Path::new(got.open.get(1).unwrap());
    assert!(file.is_absolute() && file.ends_with("note.eml"), "{file:?}");
    assert_eq!(got.refused, 0);
    // A later start with nothing still asks for the window.
    assert!(matches!(start(&mailbox, &[]), Start::Handed));
    assert_eq!(first.handed_over(), Some(Handed::default()));
    assert_eq!(first.handed_over(), None);

    // The first closes: the next start is a first again, even over what the last one left (a
    // crashed run's `.instance`, naming a port nobody answers on).
    first.close();
    assert!(!beside(&mailbox, "instance").exists());
    drop(first);
    std::fs::write(beside(&mailbox, "instance"), json!({"port": 1, "key": "stale"}).to_string()).unwrap();
    let again = first_of(start(&mailbox, &[]));
    let ics = file_beside(&mailbox, "a.ics");
    assert!(matches!(start(&mailbox, std::slice::from_ref(&ics)), Start::Handed));
    assert_eq!(again.handed_over(), Some(opened(&[&ics])));
    drop(again);
    let _ = std::fs::remove_dir_all(mailbox.parent().unwrap());
}

/// `path` as named from `base`, by as many `..` as it takes.
fn pathdiff(path: &Path, base: &Path) -> String {
    let (p, b): (Vec<_>, Vec<_>) = (path.components().collect(), base.components().collect());
    let same = p.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in same..b.len() {
        out.push("..");
    }
    for c in p.iter().skip(same) {
        out.push(c);
    }
    out.to_string_lossy().into_owned()
}

#[test]
fn mailboxes_are_guarded_one_by_one() {
    let (a, b) = (temp_mailbox("one"), temp_mailbox("two"));
    let (first_a, first_b) = (first_of(start(&a, &[])), first_of(start(&b, &[])));
    let x = file_beside(&a, "x.eml");
    assert!(matches!(start(&a, &[x]), Start::Handed));
    assert_eq!(first_b.handed_over(), None);
    assert!(first_a.handed_over().is_some());
    for m in [a, b] {
        let _ = std::fs::remove_dir_all(m.parent().unwrap());
    }
}

#[test]
fn a_first_that_cannot_be_reached_is_said_so_and_the_mailbox_is_not_opened_twice() {
    let mailbox = temp_mailbox("deaf");
    let first = first_of(start(&mailbox, &[]));
    // No word of where it listens, then a port nobody answers on, then a wrong key.
    std::fs::remove_file(beside(&mailbox, "instance")).unwrap();
    assert!(matches!(start_within(&mailbox, &[], Duration::from_millis(300)), Start::Unreachable(_)));
    let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    std::fs::write(beside(&mailbox, "instance"), json!({"port": closed, "key": "k"}).to_string()).unwrap();
    assert!(matches!(start_within(&mailbox, &[], Duration::from_millis(300)), Start::Unreachable(_)));
    for junk in ["", "{", "[]", r#"{"port": 0, "key": "k"}"#, r#"{"port": 70000, "key": "k"}"#, r#"{"port": "x"}"#] {
        std::fs::write(beside(&mailbox, "instance"), junk).unwrap();
        assert!(matches!(start_within(&mailbox, &[], Duration::from_millis(50)), Start::Unreachable(_)), "{junk}");
    }
    assert_eq!(first.handed_over(), None);
    drop(first);
    let _ = std::fs::remove_dir_all(mailbox.parent().unwrap());
}

#[test]
fn only_the_key_holder_is_listened_to_and_only_within_bounds() {
    let mailbox = temp_mailbox("key");
    let first = first_of(start(&mailbox, &[]));
    let info: Value = serde_json::from_slice(&std::fs::read(beside(&mailbox, "instance")).unwrap()).unwrap();
    let (port, key) = (u16::try_from(info["port"].as_u64().unwrap()).unwrap(), info["key"].as_str().unwrap().to_string());
    assert_eq!(key.len(), 64);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(beside(&mailbox, "instance")).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the key is the owner's alone: {mode:o}");
    }
    let ask = |line: String| {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let _ = s.write_all(line.as_bytes());
        let mut answer = String::new();
        let _ = BufReader::new(s).read_line(&mut answer);
        answer
    };
    // No key, a wrong key, a key of another length, junk, a line that never ends: nothing
    // reaches the window.
    assert!(ask(format!("{}\n", json!({"open": ["mailto:x@evil.example"]}))).contains("false"));
    assert!(ask(format!("{}\n", json!({"key": "0".repeat(64), "open": ["mailto:x@evil.example"]}))).contains("false"));
    assert!(ask(format!("{}\n", json!({"key": &key[..63], "open": ["mailto:x@evil.example"]}))).contains("false"));
    assert!(ask(format!("{}\n", json!({"key": 7, "open": ["mailto:x@evil.example"]}))).contains("false"));
    assert_eq!(ask("GET / HTTP/1.1\r\n\r\n".to_string()), "");
    assert_eq!(ask("\u{0}\u{ff}garbage\n".to_string()), "");
    assert_eq!(ask("x".repeat(MAX_LINE + 10)), "");
    assert_eq!(first.handed_over(), None);
    // The key holder: what it names is looked at, within bounds, and only what passes is taken.
    let ok = file_beside(&mailbox, "ok.vcf");
    let many: Vec<String> = (0..MAX_ARGS + 50).map(|_| ok.clone()).collect();
    assert!(ask(format!("{}\n", json!({"key": key, "open": many}))).contains("true"));
    assert_eq!(first.handed_over().map(|h| (h.open.len(), h.refused)), Some((MAX_ARGS, 50)));
    let missing = mailbox.parent().unwrap().join("missing.eml").to_string_lossy().into_owned();
    let junk = json!(["", 7, "x".repeat(MAX_ARG + 1), "relative.eml", missing, ok]);
    assert!(ask(format!("{}\n", json!({"key": key, "open": junk}))).contains("true"));
    assert_eq!(first.handed_over(), Some(Handed { open: vec![ok], refused: 5 }));
    // Not a list at all: refused whole, and the window still comes forward.
    assert!(ask(format!("{}\n", json!({"key": key, "open": "mailto:x@lumen.example"}))).contains("true"));
    assert_eq!(first.handed_over(), Some(Handed { open: vec![], refused: 1 }));
    drop(first);
    let _ = std::fs::remove_dir_all(mailbox.parent().unwrap());
}

#[test]
fn only_mail_links_and_readable_files_of_kinds_mailcraft_opens_pass() {
    let mailbox = temp_mailbox("checked");
    let dir = mailbox.parent().unwrap().to_path_buf();
    let eml = file_beside(&mailbox, "a.eml");
    let ics = file_beside(&mailbox, "b.ICS");
    let vcf = file_beside(&mailbox, "c.vcf");
    let mbox = file_beside(&mailbox, "d.mbox");
    let kept = file_beside(&mailbox, "e.mailcraft");
    // Links: mailto in any case, as it came (the compose command reads it).
    assert_eq!(checked("mailto:maya@lumen.example?subject=Hi").as_deref(), Some("mailto:maya@lumen.example?subject=Hi"));
    assert_eq!(checked("MAILTO:maya@lumen.example").as_deref(), Some("MAILTO:maya@lumen.example"));
    for path in [&eml, &ics, &vcf, &mbox, &kept] {
        assert_eq!(checked(path).as_deref(), Some(path.as_str()), "{path}");
    }
    // A file URL as a desktop writes one is its path.
    #[cfg(unix)]
    assert_eq!(checked(&format!("file://{}", eml.replace(' ', "%20"))).as_deref(), Some(eml.as_str()));
    let pdf = file_beside(&mailbox, "f.pdf");
    let json_mailbox = file_beside(&mailbox, "g.json");
    let folder = dir.join("h.eml");
    std::fs::create_dir_all(&folder).unwrap();
    let refused = [
        String::new(),
        "https://lumen.example/a.eml".to_string(),
        "javascript:alert(1)".to_string(),
        "mailto:x@lumen.example\r\nBcc: y@evil.example".to_string(),
        "mailto:x@lumen.example\u{0}".to_string(),
        format!("mailto:{}", "a".repeat(MAX_ARG)),
        // Not a kind MailCraft opens from outside: it would be attached to a message being written.
        pdf,
        // A mailbox only by its own extension.
        json_mailbox,
        // Not there, a folder, relative, with a line break in it.
        dir.join("missing.eml").to_string_lossy().into_owned(),
        folder.to_string_lossy().into_owned(),
        "a.eml".to_string(),
        format!("{eml}\n"),
    ];
    for r in refused {
        assert_eq!(checked(&r), None, "{r:?}");
    }
    #[cfg(unix)]
    {
        // A file the owner can't read is no file to open.
        use std::os::unix::fs::PermissionsExt as _;
        let locked = file_beside(&mailbox, "locked.ics");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&locked).is_err() {
            assert_eq!(checked(&locked), None);
        }
        let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600));
        let link = dir.join("link.vcf");
        std::os::unix::fs::symlink(&vcf, &link).unwrap();
        // A link to a readable file is the file.
        assert!(checked(&link.to_string_lossy()).is_some());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_is_handed_over_is_sorted_strictly() {
    assert_eq!(sorted(None), Handed::default());
    assert_eq!(sorted(Some(&json!(null))), Handed { open: vec![], refused: 1 });
    assert_eq!(sorted(Some(&json!({"a": 1}))), Handed { open: vec![], refused: 1 });
    assert_eq!(
        sorted(Some(&json!(["mailto:a@lumen.example", [1], {"x": "y"}, null]))),
        Handed { open: strings(&["mailto:a@lumen.example"]), refused: 3 }
    );
    let many: Vec<&str> = std::iter::repeat_n("mailto:a@lumen.example", MAX_ARGS + 3).collect();
    let h = sorted(Some(&json!(many)));
    assert_eq!((h.open.len(), h.refused), (MAX_ARGS, 3));
}

/// Each message file is an import and a window: one start hands over no more than
/// [`MAX_MESSAGES`] of them; calendar and contact files and links are not counted with them.
#[test]
fn message_files_are_bounded_per_hand_over() {
    let mailbox = temp_mailbox("messages");
    let eml = file_beside(&mailbox, "m.eml");
    let ics = file_beside(&mailbox, "m.ics");
    let mut list: Vec<String> = std::iter::repeat_n(eml.clone(), MAX_MESSAGES + 8).collect();
    list.push(ics.clone());
    list.push("mailto:a@lumen.example?subject=x.eml".to_string());
    let h = sorted(Some(&json!(list)));
    assert_eq!(h.refused, 8);
    assert_eq!(h.open.iter().filter(|o| **o == eml).count(), MAX_MESSAGES);
    assert!(h.open.contains(&ics) && h.open.iter().any(|o| o.starts_with("mailto:")));
    let _ = std::fs::remove_dir_all(mailbox.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn a_pipe_where_a_file_was_named_is_refused_at_once() {
    let mailbox = temp_mailbox("pipe");
    std::fs::create_dir_all(mailbox.parent().unwrap()).unwrap();
    let fifo = mailbox.parent().unwrap().join("p.eml");
    if std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
        // (Opening it to read would wait for somebody to write; it is opened without waiting.)
        assert_eq!(checked(&fifo.to_string_lossy()), None);
    }
    let _ = std::fs::remove_dir_all(mailbox.parent().unwrap());
}
