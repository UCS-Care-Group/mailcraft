// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

const ROADMAP: &str = "# Roadmap\n\nWords.\n\n## Progress log\n\n| Date | What landed |\n|---|---|\n| 2026-11-01 | Old work |\n\n## Revision history\n\n| Date | Change |\n|---|---|\n| 2026-11-01 | Old change |\n";
const GAPS: &str = "# Gaps\n\n## Revision history\n\n| Date | Change | Summary |\n|---|---|---|\n| 2026-11-01 | minor | Old |\n\nAfter.\n";

fn fragment(doc: &str, table: &str, name: &str, lines: &[&str]) -> Fragment {
    Fragment {
        path: PathBuf::from(format!("{DIR}/{}/{table}/{name}.md", doc.trim_end_matches(".md"))),
        doc: doc.into(),
        table: table.into(),
        name: name.into(),
        lines: lines.iter().map(|l| l.to_string()).collect(),
    }
}

#[test]
fn fragments_go_at_the_top_of_their_table_newest_date_first() {
    let fragments = vec![
        fragment("ROADMAP.md", "progress", "700", &["| 2026-11-02 | Middle |"]),
        fragment("ROADMAP.md", "progress", "800", &["| 2026-11-04 | Newest |", "| 2026-11-03 | Its second row |"]),
        fragment("ROADMAP.md", "revisions", "800", &["| 2026-11-04 | A change |"]),
        fragment("docs/gaps.md", "revisions", "800", &["| 2026-11-04 | patch | Not this document |"]),
    ];
    let joined = join("ROADMAP.md", ROADMAP, &fragments).unwrap();
    assert_eq!(
        joined,
        "# Roadmap\n\nWords.\n\n## Progress log\n\n| Date | What landed |\n|---|---|\n| 2026-11-04 | Newest |\n| 2026-11-03 | Its second row |\n| 2026-11-02 | Middle |\n| 2026-11-01 | Old work |\n\n## Revision history\n\n| Date | Change |\n|---|---|\n| 2026-11-04 | A change |\n| 2026-11-01 | Old change |\n"
    );
}

#[test]
fn a_tie_puts_the_last_name_first_by_number_and_nothing_to_join_changes_nothing() {
    let fragments = vec![
        fragment("docs/gaps.md", "revisions", "999", &["| 2026-11-04 | patch | A |"]),
        fragment("docs/gaps.md", "revisions", "1000", &["| 2026-11-04 | patch | B |"]),
    ];
    assert!(
        join("docs/gaps.md", GAPS, &fragments)
            .unwrap()
            .contains("|---|---|---|\n| 2026-11-04 | patch | B |\n| 2026-11-04 | patch | A |\n| 2026-11-01 | minor | Old |\n\nAfter.\n")
    );
    assert_eq!(join("docs/gaps.md", GAPS, &[]).unwrap(), GAPS);
}

#[test]
fn a_row_older_than_the_top_goes_in_where_its_date_is() {
    let doc = "## Revision history\n\n| Date | Change |\n|---|---|\n| 2026-11-03 | Third |\n| 2026-11-01 | First |\n\nAfter.\n";
    let middle = fragment("a.md", "revisions", "1", &["| 2026-11-02 | Second |"]);
    assert_eq!(
        join("a.md", doc, &[middle]).unwrap(),
        "## Revision history\n\n| Date | Change |\n|---|---|\n| 2026-11-03 | Third |\n| 2026-11-02 | Second |\n| 2026-11-01 | First |\n\nAfter.\n"
    );
    let same_day = fragment("a.md", "revisions", "1", &["| 2026-11-01 | Also first |"]);
    assert!(join("a.md", doc, &[same_day]).unwrap().contains("| 2026-11-03 | Third |\n| 2026-11-01 | Also first |\n| 2026-11-01 | First |\n"));
    let oldest = fragment("a.md", "revisions", "1", &["| 2026-10-30 | Before all |"]);
    assert!(join("a.md", doc, &[oldest]).unwrap().contains("| 2026-11-01 | First |\n| 2026-10-30 | Before all |\n\nAfter.\n"));
    let split = fragment("a.md", "revisions", "1", &["| 2026-11-04 | Newest |", "| 2026-11-02 | Second |"]);
    assert!(
        join("a.md", doc, &[split])
            .unwrap()
            .contains("|---|---|\n| 2026-11-04 | Newest |\n| 2026-11-03 | Third |\n| 2026-11-02 | Second |\n| 2026-11-01")
    );
}

#[test]
fn a_row_must_have_the_tables_columns_and_a_date() {
    for (row, says) in [
        ("| 2026-11-04 | Two cells |", "2 cells"),
        ("| 2026-11-04 | minor | Three | Four |", "4 cells"),
        ("| minor | 2026-11-04 | Date in the wrong column |", "not a date"),
        ("| 2026-13-01 | minor | No such month |", "not a date"),
        ("| 4 Nov 2026 | minor | Words |", "not a date"),
    ] {
        let err = join("docs/gaps.md", GAPS, &[fragment("docs/gaps.md", "revisions", "1", &[row])]).unwrap_err();
        assert!(err.contains(says), "{row}: {err}");
    }
    let escaped = fragment("docs/gaps.md", "revisions", "1", &[r"| 2026-11-04 | minor | `a \| b` |"]);
    assert!(join("docs/gaps.md", GAPS, &[escaped]).is_ok());
}

#[test]
fn the_date_column_is_found_by_its_name() {
    let cla = "## Revision history\n\n| Version | Date | Change |\n|---|---|---|\n| 1.0 | 2026-10-19 | First |\n";
    let ok = fragment("CLA.md", "revisions", "1", &["| 1.1 | 2026-11-04 | Second |"]);
    assert!(join("CLA.md", cla, &[ok]).unwrap().contains("|---|---|---|\n| 1.1 | 2026-11-04 | Second |\n| 1.0"));
    let bad = fragment("CLA.md", "revisions", "1", &["| 2026-11-04 | 1.1 | Second |"]);
    assert!(join("CLA.md", cla, &[bad]).unwrap_err().contains("not a date"));
}

#[test]
fn a_missing_table_is_an_error() {
    let progress = fragment("docs/gaps.md", "progress", "1", &["| 2026-11-04 | x |"]);
    assert!(join("docs/gaps.md", GAPS, &[progress]).unwrap_err().contains("## Progress log"));
    let no_table = "## Revision history\n\nNo table.\n\n## Next\n\n| Date | Change |\n|---|---|\n";
    let fragment = fragment("a.md", "revisions", "1", &["| 2026-11-04 | x |"]);
    assert!(join("a.md", no_table, std::slice::from_ref(&fragment)).unwrap_err().contains("no table"));
    let no_separator = "## Revision history\n\n| Date | Change |\n| 2026-11-01 | x |\n";
    assert!(join("a.md", no_separator, std::slice::from_ref(&fragment)).unwrap_err().contains("`|---|`"));
    let no_date = "## Revision history\n\n| When | Change |\n|---|---|\n";
    assert!(join("a.md", no_date, &[fragment]).unwrap_err().contains("no Date column"));
}

#[test]
fn a_fragment_holds_rows_only() {
    assert_eq!(lines("| 2026-11-04 | A |\n\n| 2026-11-03 | B |  \n").unwrap(), vec!["| 2026-11-04 | A |", "| 2026-11-03 | B |"]);
    assert!(lines("").is_err());
    assert!(lines("\n  \n").is_err());
    assert!(lines("| 2026-11-04 | A |\nNot a row.\n").unwrap_err().contains("Not a row"));
    assert!(lines("- A line of the release notes.\n").is_err());
    assert!(lines("| 2026-11-04 | ends escaped \\|\n").is_err());
}

#[test]
fn dates_are_checked() {
    assert!(is_date("2026-11-04"));
    assert!(!is_date("2026-11-4"));
    assert!(!is_date("2026-00-04"));
    assert!(!is_date("2026-11-32"));
    assert!(!is_date("2026-02-29"));
    assert!(!is_date("2026-04-31"));
    assert!(!is_date("1900-02-29"));
    assert!(is_date("2028-02-29"));
    assert!(is_date("2000-02-29"));
    assert!(is_date("2026-12-31"));
    assert!(!is_date("2026/11/04"));
    assert!(!is_date("２０２６-11-04"));
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mailcraft-xtask-docs-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("ROADMAP.md"), ROADMAP).unwrap();
    std::fs::write(dir.join("docs/gaps.md"), GAPS).unwrap();
    dir
}

fn put(root: &Path, place: &str, text: &str) -> PathBuf {
    let path = root.join(DIR).join(place);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn write_joins_the_fragments_and_removes_them_and_a_preview_changes_nothing() {
    let root = scratch("write");
    put(&root, "README.md", "How fragments work.\n");
    let progress = put(&root, "ROADMAP/progress/795.md", "| 2026-11-04 | New work |\n");
    let gaps = put(&root, "docs/gaps/revisions/795.md", "| 2026-11-04 | patch | New |\n");
    let gaps_too = put(&root, "docs/gaps/revisions/796.md", "| 2026-11-04 | minor | Newer |\n");
    check(&root).unwrap();
    run(&root, &[]).unwrap();
    run(&root, &["--check"]).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("ROADMAP.md")).unwrap(), ROADMAP);
    assert_eq!(std::fs::read_to_string(root.join("docs/gaps.md")).unwrap(), GAPS);
    assert!(progress.exists() && gaps.exists() && gaps_too.exists());
    run(&root, &["--write"]).unwrap();
    let roadmap = std::fs::read_to_string(root.join("ROADMAP.md")).unwrap();
    assert!(roadmap.contains("|---|---|\n| 2026-11-04 | New work |\n| 2026-11-01 | Old work |\n"), "{roadmap}");
    assert!(roadmap.ends_with("|---|---|\n| 2026-11-01 | Old change |\n"), "{roadmap}");
    let gaps_doc = std::fs::read_to_string(root.join("docs/gaps.md")).unwrap();
    assert!(
        gaps_doc.contains("|---|---|---|\n| 2026-11-04 | minor | Newer |\n| 2026-11-04 | patch | New |\n| 2026-11-01 | minor | Old |\n"),
        "{gaps_doc}"
    );
    assert!(!progress.exists() && !gaps.exists() && !gaps_too.exists());
    assert!(!root.join(DIR).join("ROADMAP").exists() && !root.join(DIR).join("docs").exists());
    assert!(root.join(DIR).join("README.md").exists());
    assert!(gather(&root).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_bad_fragment_leaves_every_document_as_it_was() {
    let root = scratch("bad");
    let good = put(&root, "ROADMAP/progress/1.md", "| 2026-11-04 | Fine |\n");
    put(&root, "docs/gaps/revisions/2.md", "| 2026-11-04 | Two cells |\n");
    let err = run(&root, &["--write"]).unwrap_err();
    assert!(err.starts_with("docs/log.d/docs/gaps/revisions/2.md: ") && err.contains("2 cells"), "{err}");
    assert_eq!(std::fs::read_to_string(root.join("ROADMAP.md")).unwrap(), ROADMAP);
    assert!(good.exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn what_is_not_a_fragment_is_refused() {
    for (place, text, says) in [
        ("ROADMAP/progress/note.txt", "| 2026-11-04 | A |\n", "`.md`"),
        ("ROADMAP/history/1.md", "| 2026-11-04 | A |\n", "isn't a table"),
        ("progress/1.md", "| 2026-11-04 | A |\n", "<document without .md>"),
        ("stray.md", "| 2026-11-04 | A |\n", "<document without .md>"),
        ("docs/README/revisions/1.md", "| 2026-11-04 | A |\n", "there is no docs/README.md"),
        ("ROADMAP/progress/2.md", "Just words.\n", "isn't a table row"),
        ("ROADMAP/progress/3.md", "\n", "empty"),
        ("ROADMAP/progress/README.md", "Not at the top.\n", "isn't a table row"),
    ] {
        let root = scratch(&place.replace('/', "-"));
        put(&root, place, text);
        let err = gather(&root).unwrap_err();
        assert!(err.contains(says), "{place}: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn a_fragment_too_long_or_too_deep_is_refused() {
    let root = scratch("long");
    put(&root, "ROADMAP/progress/1.md", &"| 2026-11-04 | A |\n".repeat(4000));
    assert!(gather(&root).unwrap_err().contains("longer than"));
    let _ = std::fs::remove_dir_all(&root);
    let root = scratch("deep");
    put(&root, "a/b/c/d/e/f/g/h/i/revisions/1.md", "| 2026-11-04 | A |\n");
    assert!(gather(&root).unwrap_err().contains("deeper than"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn every_table_in_the_real_documents_takes_fragments() {
    let root = crate::root();
    let out = std::process::Command::new("git").current_dir(&root).args(["ls-files", "*.md"]).output().unwrap();
    let mut seen = 0;
    for doc in String::from_utf8_lossy(&out.stdout).lines() {
        let text = std::fs::read_to_string(root.join(doc)).unwrap();
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        for (_, heading) in TABLES {
            if lines.iter().any(|l| l.trim_end() == heading) {
                let table = table(&lines, heading).unwrap_or_else(|e| panic!("{doc} {e}"));
                let rows: Vec<&String> = lines.iter().skip(table.first_row).take_while(|l| l.starts_with('|')).collect();
                if let Some(first) = rows.first() {
                    check_row(first, &table).unwrap_or_else(|e| panic!("{doc}: {e}"));
                }
                // Newest first, so that rows joined in at the top stay in order (a date may carry
                // a word after it, `2026-10-12 (night)`).
                let dates: Vec<String> = rows.iter().filter_map(|r| cells(r)?.get(table.date).map(|d| d.chars().take(10).collect())).collect();
                for pair in dates.windows(2) {
                    if let [newer, older] = pair {
                        assert!(newer >= older, "{doc} `{heading}`: {older} is under {newer}; the table runs newest first");
                    }
                }
                seen += 1;
            }
        }
    }
    assert!(seen >= 10, "only {seen} tables found");
}

#[test]
fn no_fragment_folder_is_nothing_to_join() {
    let root = scratch("none");
    assert!(gather(&root).unwrap().is_empty());
    check(&root).unwrap();
    let _ = std::fs::remove_dir_all(&root);
}
