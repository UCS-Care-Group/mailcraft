// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn list() -> Allowlist {
    parse_allowlist("# permissive\nallow apache\nallow MIT\nallow bsd\nunlisted org.example:bare Apache-2.0 # checked\n").unwrap()
}

#[test]
fn parses_the_allowlist_and_refuses_a_typo() {
    let l = list();
    assert_eq!(l.words, vec!["apache", "mit", "bsd"]);
    assert_eq!(l.unlisted, vec![("org.example:bare".to_string(), "Apache-2.0".to_string())]);
    assert!(parse_allowlist("alow mit\n").is_err());
}

#[test]
fn licence_names_are_read_by_their_words() {
    let l = list();
    assert!(licence_allowed("The Apache Software License, Version 2.0", &l));
    assert!(licence_allowed("MIT License", &l));
    assert!(licence_allowed("BSD-3-Clause", &l));
    assert!(!licence_allowed("GNU Lesser General Public License", &l));
    assert!(!licence_allowed("Eclipse Public License 1.0", &l));
    // "mit" inside a longer word is no licence.
    assert!(!licence_allowed("Permit-Everything", &l));
}

#[test]
fn a_dual_licence_passes_on_its_allowed_half() {
    let report = "net.java.dev.jna:jna:5.17.0\tLGPL-2.1-or-later;Apache-2.0\n";
    assert!(check_report(report, &list()).is_empty());
}

#[test]
fn flags_a_bad_licence_a_missing_one_and_a_banned_group() {
    let report = "a:b:1\tGPL-3.0\nc:d:1\t\norg.example:bare:2\t\ncom.google.firebase:firebase-core:1\tApache-2.0\ncom.google.android.gms:play-services-base:1\tApache-2.0\ncom.google.android.material:material:1\tApache-2.0\n";
    let problems = check_report(report, &list());
    assert_eq!(problems.len(), 4, "{problems:?}");
    assert!(problems.iter().any(|p| p.starts_with("a:b:1")));
    assert!(problems.iter().any(|p| p.starts_with("c:d:1")));
    assert!(problems.iter().any(|p| p.starts_with("com.google.firebase")));
    assert!(problems.iter().any(|p| p.starts_with("com.google.android.gms")));
}

#[test]
fn finds_banned_groups_anywhere_in_the_tree() {
    let tree = "+--- androidx.core:core:1.0\n|    \\--- com.google.android.gms:play-services-basement:18.0.0 -> 18.1.0\n\\--- com.google.firebase:firebase-common:1\n";
    let found = banned_in_tree(tree);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(banned_in_tree("+--- androidx.core:core:1.0\n").is_empty());
}

#[test]
fn finds_banned_subgroups_in_the_tree() {
    let tree = "+--- com.google.firebase.tools:helper:1\n|    \\--- com.google.android.gms.ads:ads-lite:2 -> 3\n+--- com.google.firebasefoo:x:1\n";
    let found = banned_in_tree(tree);
    assert_eq!(found, vec!["com.google.firebase.tools:helper:1", "com.google.android.gms.ads:ads-lite:2"]);
}
