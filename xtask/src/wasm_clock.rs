// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask wasm-clock`: no std clock in code the browser runs (#1337).
//!
//! On wasm32-unknown-unknown the standard library has no clock and no sleep: `Instant::now()`,
//! `SystemTime::now()`, `Instant::elapsed()`, `thread::sleep` and the waits with a time-out
//! (`recv_timeout`, `wait_timeout`, `park_timeout`) panic there, and a panic in the
//! browser build ends the program. `cargo xtask wasm` can't see it (it compiles), so this reads
//! the source of every workspace crate the browser app links, module by module from each crate's
//! root, and fails on such a call outside code that is compiled natively only: a module, an item,
//! a block or a statement under `#[cfg(not(target_arch = "wasm32"))]` (or `test`, `windows`,
//! `unix`, a `target_os`). Code the browser runs asks `mailcraft_time` instead (`now`,
//! `Stopwatch`), which asks the browser's clock there.
//!
//! A call the browser build compiles but can never reach (a type only the desktop app makes)
//! says so in a comment on its line or the line above: `// wasm-clock: <why>`.
//!
//! It reads the calls as written: a clock reached through an import under another name
//! (`use std::thread::sleep as nap`) or from inside a third-party crate is not found. The browser
//! has to be run for those.

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The calls that panic on wasm32-unknown-unknown.
pub const CALLS: &[&str] = &["Instant::now", "SystemTime::now", "thread::sleep", ".elapsed()", ".recv_timeout(", ".wait_timeout(", "park_timeout("];

/// A `mod name;` declared in a file.
#[derive(Debug, PartialEq, Eq)]
pub struct ModDecl {
    pub name: String,
    /// `#[path = "…"]`, as written.
    pub path: Option<String>,
    /// Compiled natively only.
    pub native: bool,
}

/// What one file holds: the calls the browser would run (line, call) and its modules.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub calls: Vec<(usize, &'static str)>,
    pub mods: Vec<ModDecl>,
}

/// Whether a `cfg(…)` predicate (the part inside the parentheses) holds on native targets only.
/// `test` counts as native: the browser build is never a test build. A predicate that can't be
/// read, or depends on something else (a feature), is not native.
pub fn is_native_cfg(pred: &str) -> bool {
    let p: String = pred.chars().filter(|c| !c.is_whitespace()).collect();
    parse_cfg(&p, 0).is_some_and(|(cfg, rest)| rest.is_empty() && on_wasm(&cfg) == Some(false))
}

/// A `cfg` predicate.
#[derive(Debug)]
enum Cfg {
    /// `name` or `name = "value"`.
    Key(String, Option<String>),
    All(Vec<Cfg>),
    Any(Vec<Cfg>),
    Not(Box<Cfg>),
}

/// Reads one predicate from the front of `p` (no white space); the rest after it.
fn parse_cfg(p: &str, depth: usize) -> Option<(Cfg, &str)> {
    if depth > 32 {
        return None;
    }
    let name_len = p.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(p.len());
    let (name, rest) = p.split_at(name_len);
    if name.is_empty() {
        return None;
    }
    if let Some(mut rest) = rest.strip_prefix('(') {
        let mut arms = Vec::new();
        while !rest.starts_with(')') {
            let (arm, after) = parse_cfg(rest, depth + 1)?;
            arms.push(arm);
            rest = after.strip_prefix(',').unwrap_or(after);
            if !after.starts_with(',') && !after.starts_with(')') {
                return None;
            }
        }
        let rest = rest.strip_prefix(')')?;
        let cfg = match name {
            "all" => Cfg::All(arms),
            "any" => Cfg::Any(arms),
            "not" if arms.len() == 1 => Cfg::Not(Box::new(arms.pop()?)),
            _ => return None,
        };
        return Some((cfg, rest));
    }
    if let Some(rest) = rest.strip_prefix("=\"") {
        let end = rest.find('"')?;
        let (value, after) = rest.split_at(end);
        return Some((Cfg::Key(name.to_string(), Some(value.to_string())), after.get(1..)?));
    }
    Some((Cfg::Key(name.to_string(), None), rest))
}

/// What a predicate is on wasm32-unknown-unknown in the browser build: `None` when it depends
/// on something else.
fn on_wasm(cfg: &Cfg) -> Option<bool> {
    match cfg {
        Cfg::Key(name, None) => match name.as_str() {
            "test" | "windows" | "unix" => Some(false),
            _ => None,
        },
        Cfg::Key(name, Some(value)) => match name.as_str() {
            "target_arch" => Some(value == "wasm32"),
            "target_os" => Some(value == "unknown"),
            "target_family" => Some(value == "wasm"),
            _ => None,
        },
        Cfg::All(arms) => {
            let said: Vec<Option<bool>> = arms.iter().map(on_wasm).collect();
            if said.contains(&Some(false)) {
                Some(false)
            } else if said.iter().all(|s| *s == Some(true)) {
                Some(true)
            } else {
                None
            }
        }
        Cfg::Any(arms) => {
            let said: Vec<Option<bool>> = arms.iter().map(on_wasm).collect();
            if said.contains(&Some(true)) {
                Some(true)
            } else if said.iter().all(|s| *s == Some(false)) {
                Some(false)
            } else {
                None
            }
        }
        Cfg::Not(inner) => on_wasm(inner).map(|b| !b),
    }
}

/// `text` with comments and the insides of string and character literals turned to spaces (line
/// breaks kept, byte offsets unchanged), so braces and words in them don't count.
pub fn code_only(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = b.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in out.iter_mut().take(to).skip(from) {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    let mut i = 0;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        let next = b.get(i + 1).copied().unwrap_or(0);
        let ident_before = i > 0 && b.get(i - 1).is_some_and(|p| p.is_ascii_alphanumeric() || *p == b'_');
        if c == b'/' && next == b'/' {
            let end = b.iter().skip(i).position(|&x| x == b'\n').map_or(b.len(), |n| i + n);
            blank(&mut out, i, end);
            i = end;
        } else if c == b'/' && next == b'*' {
            let (mut depth, mut j) = (1usize, i + 2);
            while j < b.len() && depth > 0 {
                match (b.get(j), b.get(j + 1)) {
                    (Some(b'/'), Some(b'*')) => (depth, j) = (depth + 1, j + 2),
                    (Some(b'*'), Some(b'/')) => (depth, j) = (depth - 1, j + 2),
                    _ => j += 1,
                }
            }
            blank(&mut out, i, j.min(b.len()));
            i = j;
        } else if !ident_before && (c == b'r' || (c == b'b' && next == b'r')) && {
            let k = if c == b'b' { i + 2 } else { i + 1 };
            let hashes = b.iter().skip(k).take_while(|&&x| x == b'#').count();
            b.get(k + hashes) == Some(&b'"')
        } {
            let k = if c == b'b' { i + 2 } else { i + 1 };
            let hashes = b.iter().skip(k).take_while(|&&x| x == b'#').count();
            let open = k + hashes + 1;
            let mut close: Vec<u8> = vec![b'"'];
            close.extend(std::iter::repeat_n(b'#', hashes));
            let end = (open..b.len()).find(|&j| b.get(j..j + close.len()) == Some(&close[..])).map_or(b.len(), |j| j + close.len());
            blank(&mut out, open, end.saturating_sub(close.len()));
            i = end;
        } else if c == b'"' {
            let mut j = i + 1;
            while j < b.len() {
                match b.get(j) {
                    Some(b'\\') => j += 2,
                    Some(b'"') => break,
                    _ => j += 1,
                }
            }
            blank(&mut out, i + 1, j.min(b.len()));
            i = j + 1;
        } else if c == b'\'' {
            // A character literal ('x', '\n', '\u{…}', 'é'), or else a lifetime, left as it is.
            let end = if next == b'\\' {
                (i + 3..b.len().min(i + 13)).find(|&j| b.get(j) == Some(&b'\'')).map(|j| j + 1)
            } else {
                let width = text.get(i + 1..).and_then(|s| s.chars().next()).map_or(1, char::len_utf8);
                (b.get(i + 1 + width) == Some(&b'\'')).then_some(i + 2 + width)
            };
            match end {
                Some(end) => {
                    blank(&mut out, i + 1, end - 1);
                    i = end;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads one file. `native`: the whole file is compiled natively only.
pub fn scan(text: &str, native: bool) -> Scan {
    let code = code_only(text);
    let b = code.as_bytes();
    let mut out = Scan::default();
    // Open delimiters, each with whether what is inside is native only.
    let mut stack: Vec<bool> = Vec::new();
    // The attributes before the next item or statement: native only, `#[path]`, and the depth
    // they were written at.
    let mut pending: Option<(bool, Option<String>, usize)> = None;
    let mut file_native = native;
    let mut line = 1usize;
    let mut i = 0;
    while i < b.len() {
        let c = b.get(i).copied().unwrap_or(0);
        let here_native = file_native || stack.last().copied().unwrap_or(false) || pending.as_ref().is_some_and(|p| p.0 && p.2 == stack.len());
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c == b'#' && matches!(b.get(i + 1), Some(b'[') | Some(b'!')) {
            let inner = b.get(i + 1) == Some(&b'!');
            let open = if inner { i + 2 } else { i + 1 };
            if b.get(open) == Some(&b'[') {
                let mut depth = 0usize;
                let mut j = open;
                while j < b.len() {
                    match b.get(j) {
                        Some(b'[') => depth += 1,
                        Some(b']') => {
                            depth = depth.saturating_sub(1);
                            if depth == 0 {
                                break;
                            }
                        }
                        Some(b'\n') => line += 1,
                        _ => {}
                    }
                    j += 1;
                }
                // The attribute as written (string literals and all), without `#[` and `]`.
                let attr = text.get(open + 1..j.min(b.len())).unwrap_or("").trim();
                let native_attr = attr.strip_prefix("cfg(").and_then(|r| r.strip_suffix(')')).is_some_and(is_native_cfg);
                if inner {
                    file_native |= native_attr;
                } else {
                    let path = attr
                        .strip_prefix("path")
                        .map(str::trim_start)
                        .and_then(|r| r.strip_prefix('='))
                        .map(|r| r.trim().trim_matches('"').to_string());
                    let (was_native, was_path) = match pending.take() {
                        Some((n, p, d)) if d == stack.len() => (n, p),
                        _ => (false, None),
                    };
                    pending = Some((was_native || native_attr, path.or(was_path), stack.len()));
                }
                i = j + 1;
                continue;
            }
        }
        match c {
            b'{' | b'(' | b'[' => {
                stack.push(here_native);
                if c == b'{' && pending.as_ref().is_some_and(|p| p.2 + 1 == stack.len()) {
                    pending = None;
                }
            }
            b'}' | b')' | b']' => {
                stack.pop();
                if pending.as_ref().is_some_and(|p| p.2 > stack.len()) {
                    pending = None;
                }
            }
            b';' | b',' if pending.as_ref().is_some_and(|p| p.2 == stack.len()) => pending = None,
            _ => {}
        }
        let word_start = i == 0 || b.get(i - 1).is_some_and(|p| !(p.is_ascii_alphanumeric() || *p == b'_'));
        if word_start && code.get(i..).is_some_and(|r| r.starts_with("mod ")) {
            let rest = code.get(i + 4..).unwrap_or("");
            let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            let after = rest.trim_start().get(name.len()..).unwrap_or("").trim_start();
            if !name.is_empty() && after.starts_with(';') {
                let path = pending.as_ref().filter(|p| p.2 == stack.len()).and_then(|p| p.1.clone());
                out.mods.push(ModDecl { name, path, native: here_native });
            }
        }
        if !here_native
            && let Some(call) = CALLS.iter().find(|call| {
                code.get(i..).is_some_and(|r| r.starts_with(**call))
                    && (call.starts_with('.') || word_start || b.get(i.wrapping_sub(1)) == Some(&b':'))
            })
        {
            out.calls.push((line, call));
            i += call.len();
            continue;
        }
        i += 1;
    }
    // A call the browser build compiles but never reaches says why, on its line or the one above.
    let lines: Vec<&str> = text.lines().collect();
    let allowed = |n: usize| {
        [n.checked_sub(1), n.checked_sub(2)]
            .into_iter()
            .flatten()
            .filter_map(|k| lines.get(k))
            .any(|l| l.split_once("// wasm-clock:").is_some_and(|(_, why)| !why.trim().is_empty()))
    };
    out.calls.retain(|(n, _)| !allowed(*n));
    out
}

/// Where `mod name;` in `file` is, with `#[path]` if it has one.
fn module_file(file: &Path, decl: &ModDecl) -> Option<PathBuf> {
    let dir = file.parent()?;
    if let Some(p) = &decl.path {
        return Some(dir.join(p));
    }
    let stem = file.file_stem()?.to_str()?;
    let base = if matches!(stem, "lib" | "main" | "mod") { dir.to_path_buf() } else { dir.join(stem) };
    [base.join(format!("{}.rs", decl.name)), base.join(&decl.name).join("mod.rs")].into_iter().find(|p| p.is_file())
}

/// The calls the browser would run, from crate root `root_file` down (`path:line: call`).
pub fn scan_crate(root: &Path, root_file: &Path) -> Result<Vec<String>, String> {
    let mut found = Vec::new();
    let mut todo = vec![root_file.to_path_buf()];
    let mut seen = BTreeSet::new();
    while let Some(file) = todo.pop() {
        if !seen.insert(file.clone()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let scanned = scan(&text, false);
        let shown = file.strip_prefix(root).unwrap_or(&file).display().to_string();
        found.extend(scanned.calls.iter().map(|(line, call)| format!("{shown}:{line}: {call}")));
        for decl in scanned.mods.iter().filter(|d| !d.native) {
            let child = module_file(&file, decl).ok_or_else(|| format!("{shown}: can't find the file of `mod {};`", decl.name))?;
            todo.push(child);
        }
    }
    Ok(found)
}

/// The workspace packages the browser app links (itself included), with each one's root file.
pub fn browser_crates(meta: &Value) -> Result<Vec<(String, PathBuf)>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let find = |name: &str| pkgs.iter().find(|p| p["name"].as_str() == Some(name));
    let mut out = Vec::new();
    let mut todo = vec!["mailcraft-web".to_string()];
    let mut seen = BTreeSet::new();
    while let Some(name) = todo.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(pkg) = find(&name) else { continue };
        let targets = pkg["targets"].as_array().ok_or_else(|| format!("{name}: no targets"))?;
        let kind = |t: &&Value, k: &str| t["kind"].as_array().is_some_and(|ks| ks.iter().any(|x| x.as_str() == Some(k)));
        let root = targets.iter().find(|t| kind(t, "lib")).or_else(|| targets.iter().find(|t| kind(t, "bin"))).and_then(|t| t["src_path"].as_str());
        let root = root.ok_or_else(|| format!("{name}: no lib or bin target"))?;
        out.push((name.clone(), PathBuf::from(root)));
        for d in pkg["dependencies"].as_array().into_iter().flatten() {
            let normal = d["kind"].is_null() || d["kind"].as_str() == Some("normal");
            let for_browser =
                d["target"].as_str().and_then(|t| t.strip_prefix("cfg(")).and_then(|t| t.strip_suffix(')')).is_none_or(|t| !is_native_cfg(t));
            if let (true, true, Some(dep)) = (normal, for_browser, d["name"].as_str()) {
                todo.push(dep.to_string());
            }
        }
    }
    out.sort();
    Ok(out)
}

pub fn run(root: &Path, meta: &Value) -> Result<(), String> {
    let crates = browser_crates(meta)?;
    let mut found = Vec::new();
    for (_, file) in &crates {
        found.extend(scan_crate(root, file)?);
    }
    if found.is_empty() {
        println!("wasm-clock: no std clock or sleep in the {} crates the browser build links", crates.len());
        return Ok(());
    }
    Err(format!(
        "{} std clock or sleep call(s) the browser build would run, and panic on (#1337): use `mailcraft_time::now`/`Stopwatch`, or put the call under `#[cfg(not(target_arch = \"wasm32\"))]`:\n  {}",
        found.len(),
        found.join("\n  ")
    ))
}

#[cfg(test)]
#[path = "wasm_clock_tests.rs"]
mod tests;
