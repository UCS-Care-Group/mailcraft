// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Linux and the BSDs type a file by what is in it when they don't know its name: there only a
//! kind the desktop knows by name ([`KNOWN`]) is opened, whatever else the lists say.

use crate::opening::{BLOCKED, Chooser, KNOWN, MACROS, SCRIPTS, Verdict, desktop_types, verdict, verdict_for};

/// Every kind the lists name, every kind of [`KNOWN`], and every kind of one to three letters.
fn every_kind() -> Vec<String> {
    let mut kinds: Vec<String> = BLOCKED.iter().chain(MACROS).chain(SCRIPTS).chain(KNOWN.iter().map(|(k, _)| k)).map(|k| k.to_string()).collect();
    let letters: Vec<char> = ('a'..='z').chain('0'..='9').collect();
    for a in &letters {
        kinds.push(a.to_string());
        for b in &letters {
            kinds.push(format!("{a}{b}"));
            for c in &letters {
                kinds.push(format!("{a}{b}{c}"));
            }
        }
    }
    kinds.extend(["qzx", "invoice", "p\u{0434}f", "ſh", "tar.gz"].map(String::from));
    kinds
}

#[test]
fn a_kind_the_desktop_may_not_know_is_never_opened_there() {
    // A program renamed to a made-up kind: the desktop would type it by what is in it.
    assert_eq!(verdict_for("invoice.qzx", Chooser::NameOrContent), Verdict::Blocked);
    assert_eq!(verdict_for("invoice.qzx", Chooser::Name), Verdict::Open);
    for kind in every_kind() {
        let name = format!("x.{kind}");
        let (by_name, by_content) = (verdict_for(&name, Chooser::Name), verdict_for(&name, Chooser::NameOrContent));
        let known = !desktop_types(&crate::opening::extension(&crate::opening::file_name(&name))).is_empty();
        if known {
            assert_eq!(by_content, by_name, "{name}: a kind the desktop knows is judged as anywhere else");
        } else {
            assert_eq!(by_content, Verdict::Blocked, "{name}: a kind the desktop may not know");
        }
        // Never more than where the computer goes by the name alone.
        if by_name == Verdict::Blocked {
            assert_eq!(by_content, Verdict::Blocked, "{name}");
        }
    }
}

#[test]
fn the_known_kinds_are_kinds_that_open_as_documents() {
    let mut seen = std::collections::HashSet::new();
    for (kind, types) in KNOWN {
        assert!(seen.insert(*kind), "{kind} twice");
        assert_eq!(*kind, kind.to_ascii_lowercase(), "{kind}");
        assert!(!BLOCKED.contains(kind), "{kind} is a program");
        assert!(!types.is_empty(), "{kind}");
        for t in *types {
            assert!(t.contains('/') && !t.contains(char::is_whitespace), "{kind}: {t}");
            for program in ["exec", "msi", "shortcut", "desktop", "script", "java", "sharedlib", "appimage", "msdownload", "dosexec"] {
                assert!(!t.to_ascii_lowercase().contains(program), "{kind}: {t}");
            }
        }
    }
    // The kinds that ask first still ask.
    assert_eq!(verdict_for("a.docm", Chooser::NameOrContent), Verdict::Macros);
    assert_eq!(verdict_for("a.html", Chooser::NameOrContent), Verdict::Scripts);
    assert_eq!(verdict_for("a.png", Chooser::NameOrContent), Verdict::Open);
    assert_eq!(desktop_types("png"), ["image/apng", "image/vnd.mozilla.apng", "image/png"]);
    assert!(desktop_types("qzx").is_empty() && desktop_types("").is_empty());
}

#[test]
fn this_computer_is_judged_its_own_way() {
    let linux_or_bsd = cfg!(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))));
    assert_eq!(Chooser::THIS == Chooser::NameOrContent, linux_or_bsd);
    for name in ["invoice.qzx", "report.pdf", "x.exe", "noextension"] {
        assert_eq!(verdict(name), verdict_for(name, Chooser::THIS), "{name}");
    }
}

/// Where this computer has the shared MIME database, every type it gives a known kind's name is
/// one the host accepts for it, so that a file that is what its name says is never refused.
#[test]
fn the_known_types_cover_what_this_desktop_says() {
    let Ok(globs) = std::fs::read_to_string("/usr/share/mime/globs2") else { return };
    for (kind, types) in KNOWN {
        let glob = format!("*.{kind}");
        for line in globs.lines().filter(|l| !l.starts_with('#')) {
            let mut parts = line.split(':');
            let (_, ty, pattern) = (parts.next(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
            if pattern.eq_ignore_ascii_case(&glob) {
                assert!(types.iter().any(|t| t.eq_ignore_ascii_case(ty)), "{kind}: {ty} is missing");
            }
        }
    }
}
