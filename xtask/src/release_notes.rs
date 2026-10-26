// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask release-notes [--write]`: the release's **Fixes** and **What's new** lines come
//! from fragments, one small file per change in `packaging/release-notes.d/fixes/` and
//! `packaging/release-notes.d/new/`, so pull requests don't all edit the same lines of
//! `packaging/release-notes.md` (#464).
//!
//! Without `--write` the notes are printed as they would be, and nothing changes; `cargo xtask ci`
//! checks the same join, so a fragment that would break the notes fails the pull request that adds it.
//! `--write`, before tagging, puts each fragment's lines at the top of its section, newest first,
//! and removes the fragments.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where the fragments are, under the workspace root.
pub const DIR: &str = "packaging/release-notes.d";
/// The notes the fragments go into.
pub const NOTES: &str = "packaging/release-notes.md";
/// The folders, and the heading of the section each one's lines go under.
pub const SECTIONS: [(&str, &str); 2] = [("new", "**What's new**"), ("fixes", "**Fixes**")];
/// The longest fragment read.
const MAX_FRAGMENT: u64 = 64 * 1024;

/// A fragment: where it is, when it was added (seconds; `u64::MAX` when not committed yet), and
/// its lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    pub path: PathBuf,
    pub added: u64,
    pub lines: Vec<String>,
}

/// The lines of a fragment's text, or why it can't go into the notes: every line that isn't blank
/// is one of the section's own (`- …`), and there is at least one.
pub fn lines(text: &str) -> Result<Vec<String>, String> {
    let lines: Vec<String> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).map(str::to_string).collect();
    if lines.is_empty() {
        return Err("is empty".into());
    }
    if let Some(bad) = lines.iter().find(|l| !l.starts_with("- ")) {
        return Err(format!("has a line that doesn't start with `- `: {bad:?}"));
    }
    Ok(lines)
}

/// `notes` with each section's fragments put at the top of its list, newest first (ties by name,
/// the last first). `fragments` holds one list per entry of [`SECTIONS`].
pub fn join(notes: &str, fragments: &[Vec<Fragment>]) -> Result<String, String> {
    let mut out: Vec<String> = notes.lines().map(str::to_string).collect();
    for ((_, heading), found) in SECTIONS.iter().zip(fragments) {
        if found.is_empty() {
            continue;
        }
        let at = out.iter().position(|l| l.trim_end() == *heading).ok_or_else(|| format!("{NOTES} has no {heading} heading"))?;
        let mut found = found.clone();
        found.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| b.path.cmp(&a.path)));
        let lines: Vec<String> = found.into_iter().flat_map(|f| f.lines).collect();
        out.splice(at + 1..at + 1, lines);
    }
    let mut text = out.join("\n");
    if notes.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// The fragments under `root`, one list per entry of [`SECTIONS`], with when git says each was
/// added. A file or folder that isn't one is an error, except `README.md` at the top.
pub fn gather(root: &Path) -> Result<Vec<Vec<Fragment>>, String> {
    let dir = root.join(DIR);
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir).map_err(|e| format!("{DIR}: {e}"))? {
            let name = entry.map_err(|e| format!("{DIR}: {e}"))?.file_name().to_string_lossy().into_owned();
            if name != "README.md" && !SECTIONS.iter().any(|(folder, _)| *folder == name) {
                return Err(format!("{DIR}/{name}: only `fixes/`, `new/` and README.md belong here"));
            }
        }
    }
    let mut all = Vec::new();
    for (folder, _) in SECTIONS {
        let mut found = Vec::new();
        let sub = dir.join(folder);
        if sub.is_dir() {
            for entry in std::fs::read_dir(&sub).map_err(|e| format!("{DIR}/{folder}: {e}"))? {
                let path = entry.map_err(|e| format!("{DIR}/{folder}: {e}"))?.path();
                let shown = format!("{DIR}/{folder}/{}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default());
                if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
                    return Err(format!("{shown}: a fragment is a `.md` file"));
                }
                let size = std::fs::metadata(&path).map_err(|e| format!("{shown}: {e}"))?.len();
                if size > MAX_FRAGMENT {
                    return Err(format!("{shown}: longer than {MAX_FRAGMENT} bytes"));
                }
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{shown}: {e}"))?;
                let lines = lines(&text).map_err(|e| format!("{shown} {e}"))?;
                found.push(Fragment { added: added(root, &path), path, lines });
            }
        }
        all.push(found);
    }
    Ok(all)
}

/// When the commit that added `path` was made, or `u64::MAX` when it isn't committed (or git
/// can't say): a fragment not in a commit yet is the newest.
fn added(root: &Path, path: &Path) -> u64 {
    Command::new("git")
        .current_dir(root)
        .args(["log", "--diff-filter=A", "--format=%ct", "-1", "--"])
        .arg(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(u64::MAX)
}

/// What `cargo xtask ci` runs: the fragments join cleanly. Nothing is printed or changed.
pub fn check(root: &Path) -> Result<(), String> {
    let notes = std::fs::read_to_string(root.join(NOTES)).map_err(|e| format!("{NOTES}: {e}"))?;
    join(&notes, &gather(root)?).map(|_| ())
}

pub fn run(root: &Path, write: bool) -> Result<(), String> {
    let fragments = gather(root)?;
    let path = root.join(NOTES);
    let notes = std::fs::read_to_string(&path).map_err(|e| format!("{NOTES}: {e}"))?;
    let joined = join(&notes, &fragments)?;
    let count: usize = fragments.iter().map(Vec::len).sum();
    if !write {
        print!("{joined}");
        eprintln!("release-notes: {count} fragment(s) would be joined (nothing changed; --write to join them)");
        return Ok(());
    }
    std::fs::write(&path, joined).map_err(|e| format!("{NOTES}: {e}"))?;
    for fragment in fragments.iter().flatten() {
        std::fs::remove_file(&fragment.path).map_err(|e| format!("{}: {e}", fragment.path.display()))?;
    }
    eprintln!("release-notes: {count} fragment(s) joined into {NOTES} and removed");
    Ok(())
}

#[cfg(test)]
#[path = "release_notes_tests.rs"]
mod tests;
