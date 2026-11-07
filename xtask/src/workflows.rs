// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask workflows`: the shape of the GitHub workflows that keeps signing keys away from
//! anything compiled.
//!
//! A line-based reading of `.github/workflows/*.yml`, job by job (no YAML parser: the checks only
//! need to know which job a line belongs to). It fails when:
//!
//! - a job that holds signing material (a secret other than the few that aren't, `secrets:
//!   inherit`, `azure/login`, a keychain import, or the `release` environment) also runs cargo,
//!   rustup, rustc, trunk or cross, sets up a Rust toolchain, restores a cache, uses a local action
//!   (this check can't see into one; a reusable workflow under `.github/workflows/` is checked
//!   itself), runs a program built here (`target/…`, xtask, the MailCraft programs), calls a
//!   packaging script without the flags that keep it from building and from running what was
//!   built, or runs a script from inside `packaging/` that isn't one of the signing scripts;
//! - a workflow or job asks for `permissions: read-all` or `write-all`;
//! - `curl` runs without `--proto =https`.
//!
//! Comments are ignored, so a comment may say what a job doesn't do.

use std::path::Path;

/// Secrets that aren't signing material: the token every job has, and the Google desktop client,
/// which is compiled into the programs anyway (`.github/workflows/package.yml`).
const NOT_SIGNING: &[&str] = &["GITHUB_TOKEN", "MAILCRAFT_GOOGLE_CLIENT_ID", "MAILCRAFT_GOOGLE_CLIENT_SECRET"];

/// What gives a job signing material, besides a secret and the `release` environment.
const KEYED: &[(&str, &str)] = &[
    ("secrets: inherit", "inherits every secret"),
    ("azure/login", "signs in to Azure"),
    ("import-cert.sh", "imports a signing certificate into a keychain"),
    ("security import", "imports into a keychain"),
    ("create-keychain", "makes a keychain"),
    ("import-codesign-certs", "imports a signing certificate into a keychain"),
];

/// Commands that compile or set up a compiler.
const BUILD_WORDS: &[&str] = &["cargo", "rustup", "rustc", "trunk", "cross"];

/// What sets up Rust or runs a program built here.
const BUILT: &[&str] = &[
    "dtolnay/rust-toolchain",
    "actions-rs/",
    "rust-cache",
    "target/",
    "target\\",
    "xtask",
    "mailcraft-cli",
    "mailcraft.exe",
    "/release/mailcraft",
    "MacOS/MailCraft",
    "xvfb-run",
    "actions/cache",
];

/// The scripts a job with signing material may run from inside `packaging/` (`./<script>` with a
/// `working-directory:` there).
const SIGNING_SCRIPTS: &[&str] = &["sign-sums.sh", "handoff.sh", "import-cert.sh", "sign.ps1", "package.ps1", "package.sh"];

/// Every problem in the workflows under `root`, or `Ok` when there is none.
pub fn run(root: &Path) -> Result<(), String> {
    let dir = root.join(".github").join("workflows");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .collect();
    names.sort();
    let mut problems = Vec::new();
    for name in &names {
        let path = dir.join(name);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        problems.extend(check(&format!(".github/workflows/{name}"), &text));
    }
    if problems.is_empty() {
        println!("workflows: {} files, no job that signs builds or runs what was built", names.len());
        Ok(())
    } else {
        for p in &problems {
            eprintln!("{p}");
        }
        Err(format!("{} problem(s) in the workflows (docs/releasing.md)", problems.len()))
    }
}

/// A line of a workflow without its comment: its number (from 1), its indentation and its code.
struct Line<'a> {
    number: usize,
    indent: usize,
    code: &'a str,
}

/// The problems in one workflow file, each as `file:line: what`.
pub fn check(file: &str, text: &str) -> Vec<String> {
    let lines: Vec<Line> = text
        .lines()
        .enumerate()
        .map(|(i, raw)| {
            let code = strip_comment(raw);
            Line { number: i + 1, indent: raw.len() - raw.trim_start().len(), code: code.trim_end() }
        })
        .collect();
    let mut problems = Vec::new();

    for l in &lines {
        let code = l.code.trim();
        if let Some(value) = code.strip_prefix("permissions:") {
            let value = value.trim().trim_matches(|c| c == '\'' || c == '"');
            if value == "read-all" || value == "write-all" {
                problems.push(format!("{file}:{}: `permissions: {value}`: name the permissions the jobs need", l.number));
            }
        }
    }
    for (number, command) in commands(&lines) {
        if runs_curl_without_https_only(&command) {
            problems.push(format!("{file}:{number}: curl without `--proto =https` (and `--proto-redir =https`)"));
        }
    }

    let jobs_at = lines.iter().position(|l| l.indent == 0 && l.code.trim() == "jobs:");
    let Some(jobs_at) = jobs_at else {
        return problems;
    };
    let (top, rest) = lines.split_at(jobs_at);
    let rest = rest.get(1..).unwrap_or_default();
    let workflow_keyed = keyed(top);
    for (name, job) in jobs(rest) {
        let why = workflow_keyed.clone().or_else(|| keyed(job));
        let Some((at, why)) = why else {
            continue;
        };
        let mut found = Vec::new();
        for (number, command) in commands(job) {
            if let Some(what) = builds_or_runs(&command) {
                found.push((number, what));
            }
        }
        for step in steps(job) {
            found.extend(packaging_scripts(step));
        }
        found.sort();
        for (number, what) in found {
            problems.push(format!(
                "{file}:{number}: job `{name}` {why} (line {at}) and {what}: build in a job without signing material and hand the files on by digest"
            ));
        }
    }
    problems
}

/// The steps of a job: the items under its `steps:`.
fn steps<'a, 'b>(job: &'b [Line<'a>]) -> Vec<&'b [Line<'a>]> {
    let Some(at) = job.iter().position(|l| l.code.trim() == "steps:") else {
        return Vec::new();
    };
    let below = job.get(at + 1..).unwrap_or_default();
    let Some(indent) = below.iter().find(|l| !l.code.trim().is_empty()).map(|l| l.indent) else {
        return Vec::new();
    };
    let end = below.iter().position(|l| !l.code.trim().is_empty() && l.indent < indent).unwrap_or(below.len());
    let below = below.get(..end).unwrap_or_default();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, l) in below.iter().enumerate() {
        if l.indent == indent
            && l.code.trim().starts_with('-')
            && let Some(s) = start.replace(i)
        {
            out.push(below.get(s..i).unwrap_or_default());
        }
    }
    if let Some(s) = start {
        out.push(below.get(s..).unwrap_or_default());
    }
    out
}

/// In a step that works inside `packaging/`, every `./<script>` that isn't a signing script.
fn packaging_scripts(step: &[Line]) -> Vec<(usize, String)> {
    let inside = step.iter().any(|l| {
        let code = l.code.trim();
        let code = code.strip_prefix("- ").unwrap_or(code);
        code.strip_prefix("working-directory:").is_some_and(|v| v.contains("packaging"))
    });
    if !inside {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (number, command) in commands(step) {
        let unquoted: String = command.chars().filter(|c| *c != '\'' && *c != '"').collect();
        for word in unquoted.split_whitespace().filter(|w| w.starts_with("./")) {
            let script = word.rsplit(['/', '\\']).next().unwrap_or_default();
            if !script.is_empty() && !SIGNING_SCRIPTS.contains(&script) {
                out.push((number, format!("runs `{word}` inside packaging/, which isn't one of the signing scripts")));
            }
        }
    }
    out
}

/// `raw` up to its comment (`#` at the start or after a space).
fn strip_comment(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    if trimmed.starts_with('#') {
        return "";
    }
    match raw.find(" #") {
        Some(i) => raw.get(..i).unwrap_or(raw),
        None => raw,
    }
}

/// The jobs under `jobs:`: each name with its lines.
fn jobs<'a, 'b>(lines: &'b [Line<'a>]) -> Vec<(&'a str, &'b [Line<'a>])> {
    let Some(indent) = lines.iter().find(|l| !l.code.trim().is_empty()).map(|l| l.indent) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut start: Option<(usize, &str)> = None;
    for (i, l) in lines.iter().enumerate() {
        let code = l.code.trim();
        if code.is_empty() {
            continue;
        }
        let header = l.indent == indent && code.ends_with(':');
        if l.indent < indent || header {
            if let Some((s, name)) = start.take() {
                out.push((name, lines.get(s..i).unwrap_or_default()));
            }
            if l.indent < indent {
                return out;
            }
            start = Some((i, code.trim_end_matches(':')));
        }
    }
    if let Some((s, name)) = start {
        out.push((name, lines.get(s..).unwrap_or_default()));
    }
    out
}

/// Whether these lines give a job signing material: where, and how.
fn keyed(lines: &[Line]) -> Option<(usize, String)> {
    for (i, l) in lines.iter().enumerate() {
        let code = l.code.trim();
        if let Some(secret) = signing_secret(code) {
            return Some((l.number, format!("has the secret {secret}")));
        }
        if let Some((_, why)) = KEYED.iter().find(|(needle, _)| code.contains(needle)) {
            return Some((l.number, (*why).to_string()));
        }
        if let Some(value) = code.strip_prefix("environment:").or_else(|| code.strip_prefix("- environment:")) {
            let mut named = value.contains("release");
            if value.trim().is_empty() {
                // `environment:` with `name: release` (and maybe `url:`) under it.
                named = lines
                    .iter()
                    .skip(i + 1)
                    .take_while(|n| n.code.trim().is_empty() || n.indent > l.indent)
                    .any(|n| n.code.trim().strip_prefix("name:").is_some_and(|v| v.contains("release")));
            }
            if named {
                return Some((l.number, "runs in the `release` environment".to_string()));
            }
        }
    }
    None
}

/// The first secret on the line that is signing material (any but [`NOT_SIGNING`]).
fn signing_secret(code: &str) -> Option<String> {
    let mut rest = code;
    while let Some(at) = rest.find("secrets") {
        let after = rest.get(at + "secrets".len()..).unwrap_or_default();
        let before = rest.get(..at).and_then(|b| b.chars().last());
        rest = after;
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if let Some(name) = after.strip_prefix('.') {
            let name: String = name.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            if !name.is_empty() && !NOT_SIGNING.contains(&name.as_str()) {
                return Some(name);
            }
        } else if after.starts_with('[') {
            return Some("secrets[…]".to_string());
        } else if after.trim_start().starts_with(')') || before == Some('(') {
            // `toJSON(secrets)` and the like: every secret at once.
            return Some("context, all of them".to_string());
        }
    }
    None
}

/// The lines as commands: a line ending in `\` is joined to the next, with the first one's number.
fn commands(lines: &[Line]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut open = false;
    for l in lines {
        let code = l.code.trim();
        if code.is_empty() {
            continue;
        }
        let (body, continues) = match code.strip_suffix('\\') {
            Some(body) => (body, true),
            None => (code, false),
        };
        match out.last_mut() {
            Some((_, joined)) if open => {
                joined.push(' ');
                joined.push_str(body);
            }
            _ => out.push((l.number, body.to_string())),
        }
        open = continues;
    }
    out
}

/// What in this command compiles or runs a built program, if anything.
fn builds_or_runs(command: &str) -> Option<String> {
    for word in BUILD_WORDS {
        if has_word(command, word) {
            return Some(format!("runs `{word}`"));
        }
    }
    if let Some(what) = BUILT.iter().find(|b| command.contains(*b)) {
        return Some(format!("sets up Rust, restores a cache or runs what was built (`{what}`)"));
    }
    if let Some(at) = command.find("uses: ./") {
        // A reusable workflow of this repository is checked itself; a local action is not.
        let target = command.get(at + "uses: ".len()..).unwrap_or_default();
        let checked_too = target.starts_with("./.github/workflows/") && (target.ends_with(".yml") || target.ends_with(".yaml"));
        if !checked_too {
            return Some("uses a local action, which this check can't read".to_string());
        }
    }
    if command.contains("package.ps1") && !(command.contains("-SkipBuild") && command.contains("-NoRun")) {
        return Some("calls package.ps1 without -SkipBuild -NoRun".to_string());
    }
    if command.contains("package.sh") && !(command.contains("--skip-build") && command.contains("--no-run")) {
        return Some("calls a package.sh without --skip-build --no-run".to_string());
    }
    None
}

/// Whether `word` stands on its own in `text` (not part of a longer name, a path or a variable).
fn has_word(text: &str, word: &str) -> bool {
    let mut from = 0;
    while let Some(at) = text.get(from..).and_then(|t| t.find(word)) {
        let start = from + at;
        let end = start + word.len();
        let before = text.get(..start).and_then(|t| t.chars().last());
        // `cargo.exe` is still cargo, and so is `/usr/bin/cargo`.
        let rest = text.get(end..).unwrap_or_default();
        let after = rest.strip_prefix(".exe").unwrap_or(rest).chars().next();
        let edge = |c: Option<char>, path: bool| {
            c.is_none_or(|c| (path && matches!(c, '/' | '\\')) || !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | '$')))
        };
        if edge(before, true) && edge(after, false) {
            return true;
        }
        from = end;
    }
    false
}

/// Whether the command runs curl (as a command, not a package name) without HTTPS-only.
fn runs_curl_without_https_only(command: &str) -> bool {
    let unquoted: String = command.chars().filter(|c| *c != '\'' && *c != '"').collect();
    let segments = unquoted.split(['|', ';', '&', '(', ')', '`', '{', '}']);
    for segment in segments {
        let mut words = segment.split_whitespace().skip_while(|w| {
            w.ends_with(':')
                || *w == "-"
                || w.contains('=') && !w.starts_with('-')
                || matches!(*w, "sudo" | "exec" | "time" | "if" | "then" | "do" | "else" | "!" | "$" | "command")
        });
        let Some(first) = words.next() else {
            continue;
        };
        if first == "curl" || first.ends_with("/curl") {
            let rest: Vec<&str> = words.collect();
            let https_only =
                rest.windows(2).any(|w| w.first() == Some(&"--proto") && w.get(1) == Some(&"=https")) || rest.contains(&"--proto==https");
            if !https_only {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "workflows_tests.rs"]
mod workflows_tests;
