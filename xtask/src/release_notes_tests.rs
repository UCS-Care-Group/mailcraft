// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

const NOTES: &str = "**MailCraft 1.0 in short**\n- Things.\n\n**What's new**\n- **Old feature.**\n\n**Fixes**\n- Old fix.\n\nClosing words.\n";

fn fragment(name: &str, added: u64, lines: &[&str]) -> Fragment {
    Fragment { path: PathBuf::from(name), added, lines: lines.iter().map(|l| l.to_string()).collect() }
}

#[test]
fn fragments_go_at_the_top_of_their_section_newest_first() {
    let new = vec![fragment("new/300.md", 10, &["- **New feature.**"])];
    let fixes = vec![
        fragment("fixes/100.md", 5, &["- Older fix."]),
        fragment("fixes/200.md", 20, &["- Newer fix.", "- Its second line."]),
        fragment("fixes/a.md", u64::MAX, &["- Not committed yet."]),
    ];
    let joined = join(NOTES, &[new, fixes]).unwrap();
    assert_eq!(
        joined,
        "**MailCraft 1.0 in short**\n- Things.\n\n**What's new**\n- **New feature.**\n- **Old feature.**\n\n**Fixes**\n- Not committed yet.\n- Newer fix.\n- Its second line.\n- Older fix.\n- Old fix.\n\nClosing words.\n"
    );
}

#[test]
fn a_tie_puts_the_last_name_first_and_nothing_to_join_changes_nothing() {
    let fixes = vec![fragment("fixes/101.md", 7, &["- A."]), fragment("fixes/102.md", 7, &["- B."])];
    assert!(join(NOTES, &[Vec::new(), fixes]).unwrap().contains("**Fixes**\n- B.\n- A.\n- Old fix."));
    assert_eq!(join(NOTES, &[Vec::new(), Vec::new()]).unwrap(), NOTES);
}

#[test]
fn a_missing_heading_is_an_error() {
    let fixes = vec![fragment("fixes/1.md", 1, &["- A."])];
    assert!(join("**What's new**\n- x\n", &[Vec::new(), fixes]).unwrap_err().contains("**Fixes**"));
}

#[test]
fn a_fragment_holds_list_lines_only() {
    assert_eq!(lines("- One.\n\n- Two.  \n").unwrap(), vec!["- One.", "- Two."]);
    assert!(lines("").is_err());
    assert!(lines("\n  \n").is_err());
    assert!(lines("- One.\nNot a line of the list.\n").unwrap_err().contains("Not a line"));
    assert!(lines("**Fixes**\n- One.\n").is_err());
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-xtask-release-notes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("packaging")).unwrap();
    std::fs::write(dir.join(super::NOTES), NOTES).unwrap();
    dir
}

#[test]
fn write_joins_the_fragments_and_removes_them_and_a_preview_changes_nothing() {
    let root = scratch("write");
    let fixes = root.join(DIR).join("fixes");
    std::fs::create_dir_all(&fixes).unwrap();
    std::fs::write(root.join(DIR).join("README.md"), "How fragments work.\n").unwrap();
    std::fs::write(fixes.join("445.md"), "- Opening an attachment works without xdg-open.\n").unwrap();
    run(&root, false).unwrap();
    assert_eq!(std::fs::read_to_string(root.join(super::NOTES)).unwrap(), NOTES);
    assert!(fixes.join("445.md").exists());
    run(&root, true).unwrap();
    let notes = std::fs::read_to_string(root.join(super::NOTES)).unwrap();
    assert!(notes.contains("**Fixes**\n- Opening an attachment works without xdg-open.\n- Old fix.\n"), "{notes}");
    assert!(!fixes.join("445.md").exists());
    assert!(root.join(DIR).join("README.md").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_is_not_a_fragment_is_refused() {
    for (place, text, says) in [
        ("fixes/note.txt", "- A.\n", "`.md`"),
        ("other/1.md", "- A.\n", "only `fixes/`"),
        ("fixes/2.md", "Just words.\n", "doesn't start"),
        ("new/3.md", "\n", "empty"),
    ] {
        let root = scratch(&place.replace('/', "-"));
        let path = root.join(DIR).join(place);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        let err = gather(&root).unwrap_err();
        assert!(err.contains(says), "{place}: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn no_fragment_folder_is_nothing_to_join() {
    let root = scratch("none");
    assert_eq!(gather(&root).unwrap(), vec![Vec::new(), Vec::new()]);
    let _ = std::fs::remove_dir_all(&root);
}
