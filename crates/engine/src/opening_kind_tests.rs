// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Linux and the BSDs type a file by what is in it when they don't know its name: there only a
//! kind the desktop knows by name ([`KNOWN`]) is opened, whatever else the lists say.

use crate::opening::{ACTIVE, BLOCKED, Chooser, KNOWN, MACROS, SCRIPTS, SEVERAL, Verdict, desktop_types, verdict, verdict_for};

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
fn markdown_keys_certificates_and_signatures_ask_first_everywhere() {
    for kind in ACTIVE {
        // Where the computer goes by the name alone, every one asks; on Linux and the BSDs one the
        // desktop knows by name asks, any other is refused there as every unknown kind is.
        let on_linux = if desktop_types(kind).is_empty() { Verdict::Blocked } else { Verdict::Active };
        for (chooser, want) in [(Chooser::Name, Verdict::Active), (Chooser::NameOrContent, on_linux)] {
            assert_eq!(verdict_for(&format!("x.{kind}"), chooser), want, "{kind}");
            assert_eq!(verdict_for(&format!("X.{}", kind.to_uppercase()), chooser), want, "{kind}");
        }
        assert!(crate::opening::is_risky(kind), "{kind}");
        assert!(!BLOCKED.contains(kind) && !MACROS.contains(kind) && !SCRIPTS.contains(kind), "{kind}");
    }
    assert!(Verdict::Active.asks());
    assert_eq!(verdict_for("notes.md", Chooser::Name), Verdict::Active);
    assert_eq!(verdict_for("key.asc", Chooser::Name), Verdict::Active);
    for name in [
        "notes.mkd",
        "a.mdown",
        "a.mdwn",
        "a.mkdn",
        "a.mmd",
        "a.mdx",
        "id.p12",
        "id.pfx",
        "a.p7b",
        "a.p7c",
        "a.gpg",
        "a.pgp",
        "a.key",
        "a.csr",
        "a.crl",
    ] {
        assert_eq!(verdict_for(name, Chooser::Name), Verdict::Active, "{name}");
    }
    // Certificates Windows installs from a double-click stay refused.
    for name in ["a.crt", "a.cer", "a.der", "a.p7r", "a.sst", "a.stl", "a.spc", "A.SST"] {
        assert_eq!(verdict_for(name, Chooser::Name), Verdict::Blocked, "{name}");
    }
}

#[test]
fn recent_kinds_are_known_kinds_of_one_type() {
    for kind in crate::opening::RECENT {
        assert!(!desktop_types(kind).is_empty() && !SEVERAL.contains(kind), "{kind}");
    }
}

#[test]
fn this_computer_is_judged_its_own_way() {
    let linux_or_bsd = cfg!(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))));
    assert_eq!(Chooser::THIS == Chooser::NameOrContent, linux_or_bsd);
    for name in ["invoice.qzx", "report.pdf", "x.exe", "noextension"] {
        assert_eq!(verdict(name), verdict_for(name, Chooser::THIS), "{name}");
    }
}

/// Types a kind's name, a doubled kind ending in it, or a more particular type of its types can
/// be given, which are refused on purpose: PostScript, a compressed drawing that runs scripts, a
/// disk image, an HTML application, fonts, packages and the like, and a game cartridge's `.md`.
const REFUSED_ON_PURPOSE: &[&str] = &[
    "image/x-gzeps",
    "image/x-bzeps",
    "application/x-gzpostscript",
    "application/x-bzpostscript",
    "image/svg+xml-compressed",
    "application/x-raw-disk-image-xz-compressed",
    "application/hta",
    "application/x-mozilla-bookmarks",
    "application/x-ipynb+json",
    "application/x-uml",
    "application/x-qtiplot",
    "application/x-font-pcf",
    "application/x-font-snf",
    "application/x-gz-font-linux-psf",
    "application/vnd.kde.fontspackage",
    "application/x-gzdvi",
    "application/x-bzdvi",
    "text/vnd.kde.kcrash-report",
    "application/x-freedesktop-appstream-component",
    "application/x-freedesktop-appstream-releases",
    "application/x-gerber-job",
    "model/gltf+json",
    "application/x-genesis-rom",
    "application/x-sega-pico-rom",
];

#[test]
fn what_is_refused_on_purpose_is_never_accepted() {
    for (kind, types) in KNOWN {
        for t in *types {
            assert!(!REFUSED_ON_PURPOSE.iter().any(|r| r.eq_ignore_ascii_case(t)), "{kind}: {t}");
        }
    }
    let has = |kind: &str, t: &str| desktop_types(kind).contains(&t);
    // Doubled kinds: a compressed tar file is typed by the longer name.
    assert!(
        has("gz", "application/x-compressed-tar") && has("bz2", "application/x-bzip2-compressed-tar") && has("xz", "application/x-xz-compressed-tar")
    );
    // An Opus `.ogg`, which GIO types by what is in it.
    assert!(has("ogg", "audio/x-opus+ogg") && has("oga", "audio/x-opus+ogg"));
    assert!(!has("xz", "application/x-raw-disk-image-xz-compressed") && !has("md", "application/x-genesis-rom"));
    for kind in SEVERAL {
        assert!(!desktop_types(kind).is_empty(), "{kind} is not known");
    }
}

/// Where this computer has the shared MIME database, every type it can give a file of a known kind
/// is one the host accepts for it (or one refused on purpose), so that a file that is what its
/// name says is never refused: the types of `*.kind`, of every doubled `*.x.kind`, and, for a name
/// with several types (which GIO tells apart by what is in the file, [`SEVERAL`]), every more
/// particular type of those.
#[test]
fn the_known_types_cover_what_this_desktop_says() {
    let Ok(globs) = std::fs::read_to_string("/usr/share/mime/globs2") else {
        // Linux CI has the database (it comes with GTK): its absence there is a fault.
        assert!(!(cfg!(target_os = "linux") && std::env::var_os("CI").is_some()), "no /usr/share/mime/globs2 on Linux CI");
        return;
    };
    let subclasses = std::fs::read_to_string("/usr/share/mime/subclasses").unwrap_or_default();
    let globs: Vec<(&str, String)> = globs
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split(':').skip(1);
            Some((parts.next()?, parts.next()?.to_ascii_lowercase()))
        })
        .collect();
    let plain = |p: &str| p.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c));
    for (kind, types) in KNOWN {
        let accepted = |t: &str| types.iter().chain(REFUSED_ON_PURPOSE).any(|k| k.eq_ignore_ascii_case(t));
        let own: std::collections::BTreeSet<&str> = globs.iter().filter(|(_, p)| *p == format!("*.{kind}")).map(|(t, _)| *t).collect();
        assert!(!own.is_empty(), "{kind}: the desktop doesn't know this kind by name");
        for t in &own {
            assert!(accepted(t), "{kind}: {t} is missing");
        }
        let suffix = format!(".{kind}");
        for (t, p) in &globs {
            if let Some(stem) = p.strip_prefix("*.").and_then(|r| r.strip_suffix(suffix.as_str()))
                && !stem.is_empty()
                && plain(stem)
            {
                assert!(accepted(t), "{kind}: {t} ({p}) is missing");
            }
        }
        if own.len() > 1 {
            assert!(SEVERAL.contains(kind), "{kind} has several types: {own:?}");
            let mut parents: Vec<&str> = own.iter().copied().filter(|t| *t != "text/plain").collect();
            let mut seen = std::collections::BTreeSet::new();
            while let Some(parent) = parents.pop() {
                for line in subclasses.lines() {
                    if let Some((child, p)) = line.split_once(' ')
                        && p == parent
                        && seen.insert(child)
                    {
                        assert!(accepted(child), "{kind}: {child} (a {parent}) is missing");
                        parents.push(child);
                    }
                }
            }
        }
    }
}
