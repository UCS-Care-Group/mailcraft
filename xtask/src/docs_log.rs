// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask docs-log [--check | --write]`: the rows at the top of the documents' append-only
//! tables (`ROADMAP.md`'s **Progress log**, every document's **Revision history**) come from
//! fragments, one small file per change under `docs/log.d/`, so pull requests don't all add a row
//! on the same line of the same table. They are joined in at release time, as the release notes'
//! fragments are (`release_notes.rs`).
//!
//! A fragment is `docs/log.d/<document without .md>/<table>/<name>.md`: `docs/log.d/ROADMAP/progress/795.md`
//! goes into `ROADMAP.md`'s Progress log, `docs/log.d/docs/gaps/revisions/795.md` into
//! `docs/gaps.md`'s Revision history. It holds the table's row or rows exactly as they read.
//!
//! Without `--write` each document a fragment touches is printed as it would be, and nothing
//! changes; `--check` (and `cargo xtask ci`) only checks that every fragment names a real document
//! and table, that each line is a row with the table's columns and a date where the table has
//! its `Date`, and that they join. `--write`, in the release's pull request, puts each row into its
//! table where its date goes, so the table stays newest first (above the table's own rows of the
//! same date; rows of fragments of one date, the last name first, by number when both are
//! numbers), and removes the fragments. A fragment older than the table's top row (a pull request
//! opened before a release and merged after it) goes in lower down, not refused.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the fragments are, under the workspace root.
pub const DIR: &str = "docs/log.d";
/// The tables: a fragment's folder, and the heading its table is under.
pub const TABLES: [(&str, &str); 2] = [("progress", "## Progress log"), ("revisions", "## Revision history")];
/// The longest fragment read.
const MAX_FRAGMENT: u64 = 64 * 1024;
/// The most fragments read.
const MAX_FRAGMENTS: usize = 10_000;
/// The deepest folder read under [`DIR`] (a document's path, its table, the fragment).
const MAX_DEPTH: usize = 8;

/// A fragment: where it is, the document and table it goes into, its name, and its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub path: PathBuf,
    /// The document's path from the workspace root, `docs/gaps.md`.
    pub doc: String,
    /// The table's folder, one of [`TABLES`].
    pub table: String,
    /// The file's name without `.md`.
    pub name: String,
    pub lines: Vec<String>,
}

impl Fragment {
    /// Where it is from the workspace root, as errors name it.
    pub fn shown(&self) -> String {
        format!("{DIR}/{}/{}/{}.md", self.doc.strip_suffix(".md").unwrap_or(&self.doc), self.table, self.name)
    }
}

/// The lines of a fragment's text, or why it can't go into a table: every line that isn't blank
/// is a row (`| … |`), and there is at least one. Whether the row has the table's columns is
/// [`join`]'s to say.
pub fn lines(text: &str) -> Result<Vec<String>, String> {
    let lines: Vec<String> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    if lines.is_empty() {
        return Err("is empty".into());
    }
    if let Some(bad) = lines.iter().find(|l| cells(l).is_none()) {
        return Err(format!("has a line that isn't a table row (`| … |`): {bad:?}"));
    }
    Ok(lines)
}

/// A row's cells, trimmed, or `None` when the line isn't a row: it starts and ends with `|`, and
/// a `\|` is part of a cell.
pub fn cells(row: &str) -> Option<Vec<String>> {
    let row = row.trim();
    let inner = row.strip_prefix('|')?.strip_suffix('|')?;
    if inner.ends_with('\\') && !inner.ends_with("\\\\") {
        return None;
    }
    let mut out = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for c in inner.chars() {
        if c == '|' && !escaped {
            out.push(cell.trim().to_string());
            cell.clear();
        } else {
            cell.push(c);
        }
        escaped = c == '\\' && !escaped;
    }
    out.push(cell.trim().to_string());
    Some(out)
}

/// Whether `s` is a date, `YYYY-MM-DD`.
pub fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| s.get(r).filter(|p| p.bytes().all(|c| c.is_ascii_digit())).and_then(|p| p.parse::<u32>().ok());
    let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

/// A table under a heading: its column names, which of them is the date, and the line its first
/// row goes at (the one after the `|---|` line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub date: usize,
    pub first_row: usize,
}

/// The table under `heading` in a document's `lines`: the first table after the heading, before
/// the next heading of its level or above.
pub fn table(lines: &[String], heading: &str) -> Result<Table, String> {
    let at = lines.iter().position(|l| l.trim_end() == heading).ok_or_else(|| format!("has no `{heading}` heading"))?;
    let mut rest = lines.iter().enumerate().skip(at + 1);
    let header = loop {
        match rest.next() {
            Some((_, l)) if l.starts_with("# ") || l.starts_with("## ") => return Err(format!("has no table under `{heading}`")),
            Some((i, l)) if l.trim_start().starts_with('|') => break i,
            Some(_) => continue,
            None => return Err(format!("has no table under `{heading}`")),
        }
    };
    let columns = lines.get(header).and_then(|l| cells(l)).ok_or_else(|| format!("`{heading}`: its table's header isn't a row"))?;
    let separator = lines
        .get(header + 1)
        .and_then(|l| cells(l))
        .filter(|c| c.len() == columns.len() && c.iter().all(|c| !c.is_empty() && c.chars().all(|c| matches!(c, '-' | ':'))));
    if separator.is_none() {
        return Err(format!("`{heading}`: its table's header has no `|---|` line under it"));
    }
    let date = columns.iter().position(|c| c.eq_ignore_ascii_case("date")).ok_or_else(|| format!("`{heading}`: its table has no Date column"))?;
    Ok(Table { columns, date, first_row: header + 2 })
}

/// Why `row` can't go into `table`, if it can't: it has the table's columns, a date in its Date
/// column, and it is not the `|---|` line.
pub fn check_row(row: &str, table: &Table) -> Result<(), String> {
    let cells = cells(row).ok_or_else(|| format!("{row:?} isn't a table row"))?;
    if cells.len() != table.columns.len() {
        return Err(format!("{row:?} has {} cells, the table has {} columns ({})", cells.len(), table.columns.len(), table.columns.join(" | ")));
    }
    let date = cells.get(table.date).map(String::as_str).unwrap_or_default();
    if !is_date(date) {
        return Err(format!(
            "{row:?}: its {} column holds {date:?}, not a date (YYYY-MM-DD)",
            table.columns.get(table.date).map(String::as_str).unwrap_or("Date")
        ));
    }
    Ok(())
}

/// Names in order, numbers by their value and before words.
fn by_name(a: &str, b: &str) -> Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => a.cmp(b),
    }
}

/// `text`, the document at `doc`, with the rows of `fragments` (those for `doc`) put into their
/// tables by date, so each stays newest first: a row goes above the first of the table's own rows
/// that is not newer than it. Rows from fragments go in the order of their fragments (newest
/// first, a fragment's date being its newest row's; a tie, the last name first), and then by
/// date, a fragment's own rows of one date as it has them.
pub fn join(doc: &str, text: &str, fragments: &[Fragment]) -> Result<String, String> {
    let mut out: Vec<String> = text.lines().map(str::to_string).collect();
    for (folder, heading) in TABLES {
        let mut found: Vec<&Fragment> = fragments.iter().filter(|f| f.doc == doc && f.table == folder).collect();
        if found.is_empty() {
            continue;
        }
        let table = table(&out, heading).map_err(|e| format!("{doc} {e}"))?;
        let mut dated = Vec::new();
        for fragment in found.drain(..) {
            let shown = fragment.shown();
            for row in &fragment.lines {
                check_row(row, &table).map_err(|e| format!("{shown}: {e}"))?;
            }
            let newest = fragment.lines.iter().filter_map(|r| cells(r)?.get(table.date).cloned()).max().unwrap_or_default();
            dated.push((newest, fragment));
        }
        dated.sort_by(|(a, fa), (b, fb)| b.cmp(a).then_with(|| by_name(&fb.name, &fa.name)));
        let date_of = |row: &str| cells(row).and_then(|c| c.get(table.date).map(|d| d.chars().take(10).collect::<String>())).unwrap_or_default();
        let mut rows: Vec<(String, String)> = dated.into_iter().flat_map(|(_, f)| f.lines.iter().map(|r| (date_of(r), r.clone()))).collect();
        rows.sort_by(|(a, _), (b, _)| b.cmp(a));
        let end = out.iter().skip(table.first_row).take_while(|l| l.trim_start().starts_with('|')).count() + table.first_row;
        let existing: Vec<String> = out.get(table.first_row..end).map(<[String]>::to_vec).unwrap_or_default();
        let mut merged = Vec::with_capacity(existing.len() + rows.len());
        let mut new = rows.into_iter().peekable();
        for old in existing {
            let old_date = date_of(&old);
            while let Some((_, row)) = new.next_if(|(date, _)| *date >= old_date) {
                merged.push(row);
            }
            merged.push(old);
        }
        merged.extend(new.map(|(_, row)| row));
        out.splice(table.first_row..end, merged);
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

/// The fragments under `root`'s [`DIR`]. A file or folder that isn't one is an error, except
/// `README.md` at the top; so is a fragment whose document isn't there.
pub fn gather(root: &Path) -> Result<Vec<Fragment>, String> {
    let dir = root.join(DIR);
    let mut found = Vec::new();
    if dir.is_dir() {
        walk(root, &dir, &mut Vec::new(), &mut found)?;
    }
    Ok(found)
}

fn walk(root: &Path, dir: &Path, parts: &mut Vec<String>, found: &mut Vec<Fragment>) -> Result<(), String> {
    let here = if parts.is_empty() { DIR.to_string() } else { format!("{DIR}/{}", parts.join("/")) };
    if parts.len() >= MAX_DEPTH {
        return Err(format!("{here}: deeper than {MAX_DEPTH} folders"));
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{here}: {e}"))? {
        let entry = entry.map_err(|e| format!("{here}: {e}"))?;
        let kind = entry.file_type().map_err(|e| format!("{here}: {e}"))?;
        entries.push((entry.file_name().to_string_lossy().into_owned(), entry.path(), kind));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, path, kind) in entries {
        let shown = format!("{here}/{name}");
        if kind.is_dir() {
            parts.push(name);
            walk(root, &path, parts, found)?;
            parts.pop();
            continue;
        }
        if parts.is_empty() && name == "README.md" {
            continue;
        }
        if !kind.is_file() {
            return Err(format!("{shown}: a fragment is a file"));
        }
        let Some(stem) = name.strip_suffix(".md").filter(|s| !s.is_empty()) else {
            return Err(format!("{shown}: a fragment is a `.md` file"));
        };
        let (table, doc) = match parts.split_last() {
            Some((table, doc)) if !doc.is_empty() => (table, doc),
            _ => return Err(format!("{shown}: a fragment is {DIR}/<document without .md>/<table>/<name>.md")),
        };
        if !TABLES.iter().any(|(folder, _)| folder == table) {
            let folders: Vec<&str> = TABLES.iter().map(|(f, _)| *f).collect();
            return Err(format!("{shown}: `{table}` isn't a table; the tables are {}", folders.join(", ")));
        }
        let doc = format!("{}.md", doc.join("/"));
        if !root.join(&doc).is_file() {
            return Err(format!("{shown}: there is no {doc}"));
        }
        if found.len() >= MAX_FRAGMENTS {
            return Err(format!("{DIR}: more than {MAX_FRAGMENTS} fragments"));
        }
        let size = std::fs::metadata(&path).map_err(|e| format!("{shown}: {e}"))?.len();
        if size > MAX_FRAGMENT {
            return Err(format!("{shown}: longer than {MAX_FRAGMENT} bytes"));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{shown}: {e}"))?;
        let lines = lines(&text).map_err(|e| format!("{shown} {e}"))?;
        found.push(Fragment { path, doc, table: table.clone(), name: stem.to_string(), lines });
    }
    Ok(())
}

/// Each document the fragments touch, with them joined in, in the order of their paths.
pub fn joined(root: &Path, fragments: &[Fragment]) -> Result<Vec<(String, String)>, String> {
    let docs: BTreeMap<&str, ()> = fragments.iter().map(|f| (f.doc.as_str(), ())).collect();
    let mut out = Vec::new();
    for doc in docs.into_keys() {
        let text = std::fs::read_to_string(root.join(doc)).map_err(|e| format!("{doc}: {e}"))?;
        out.push((doc.to_string(), join(doc, &text, fragments)?));
    }
    Ok(out)
}

/// What `cargo xtask ci` runs: the fragments join cleanly. Nothing is printed or changed.
pub fn check(root: &Path) -> Result<(), String> {
    joined(root, &gather(root)?).map(|_| ())
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let (write, only_check) = (args.contains(&"--write"), args.contains(&"--check"));
    let fragments = gather(root)?;
    let docs = joined(root, &fragments)?;
    let count = fragments.len();
    if only_check {
        eprintln!("docs-log: {count} fragment(s) join into {} document(s)", docs.len());
        return Ok(());
    }
    if !write {
        for (doc, text) in &docs {
            println!("==> {doc} <==");
            print!("{text}");
        }
        eprintln!("docs-log: {count} fragment(s) would be joined into {} document(s) (nothing changed; --write to join them)", docs.len());
        return Ok(());
    }
    // Every document is written beside itself first, so a failure leaves them all as they were;
    // then each takes its place by a rename, and its fragments go at once, so a run that stops
    // part way and is run again never joins a row twice.
    let mut written = Vec::new();
    for (doc, text) in &docs {
        let temp = root.join(format!("{doc}.docs-log.tmp"));
        if let Err(e) = std::fs::write(&temp, text) {
            for (_, t) in &written {
                let _ = std::fs::remove_file(t);
            }
            let _ = std::fs::remove_file(&temp);
            return Err(format!("{doc}: {e}"));
        }
        written.push((doc.as_str(), temp));
    }
    let top = root.join(DIR);
    for (doc, temp) in &written {
        std::fs::rename(temp, root.join(doc)).map_err(|e| format!("{doc}: {e}"))?;
        for fragment in fragments.iter().filter(|f| f.doc == *doc) {
            std::fs::remove_file(&fragment.path).map_err(|e| format!("{}: {e}", fragment.shown()))?;
            // The folders it leaves empty go too (a folder that isn't empty stays, and stops it).
            let mut folder = fragment.path.parent();
            while let Some(f) = folder.filter(|f| f.starts_with(&top) && *f != top) {
                if std::fs::remove_dir(f).is_err() {
                    break;
                }
                folder = f.parent();
            }
        }
    }
    eprintln!("docs-log: {count} fragment(s) joined into {} document(s) and removed", docs.len());
    Ok(())
}

#[cfg(test)]
#[path = "docs_log_tests.rs"]
mod tests;
