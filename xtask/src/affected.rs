// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask affected`: what a pull request's CI run has to cover.
//!
//! The changed files are mapped to workspace packages (`cargo metadata --no-deps`), and the tests
//! that run are those packages' and every package's that depends on them, as a nextest filter
//! (`rdeps(=a) | rdeps(=b)`): never `cargo -p`, which would build with other features and miss the
//! cache. The whole suite runs instead when the change can reach everything (the lock file, the root
//! manifest, the toolchain, Cargo or nextest configuration, xtask, the workflows, a file outside
//! any package), when the pull request carries the `full-ci` label, when the diff can't be read,
//! and for anything that isn't a pull request (a push to main, the merge queue, a schedule, a run
//! by hand). The rules err towards running everything; main runs the whole suite on every merge.
//!
//! Two classes of pull request skip the Rust matrix's other legs or the matrix itself: a
//! translation (only `crates/i18n/po/` and documentation) runs on Linux alone, and a change to the
//! Android app's own files (Kotlin, resources, Gradle) runs no Rust tests, only the app's job.
//!
//! The logic may move to a shared action of the craft programs later; the outputs keep its names.

use std::collections::BTreeSet;
use std::path::Path;

/// The label that runs the whole suite on a pull request.
pub const FULL_CI_LABEL: &str = "full-ci";

/// The three systems of the full matrix, and Linux alone.
pub const ALL_OS: &str = r#"["ubuntu-24.04","macos-latest","windows-latest"]"#;
pub const LINUX_ONLY: &str = r#"["ubuntu-24.04"]"#;

/// Files and folders whose change can reach every package: the whole suite runs.
const GLOBAL: &[&str] =
    &["Cargo.lock", "Cargo.toml", "rust-toolchain", "rust-toolchain.toml", ".cargo/", ".config/", "xtask/", ".github/workflows/", ".github/scripts/"];

/// Test data one package keeps that another reads without depending on it: a change there selects
/// the reader too.
const ALSO_READ_BY: &[(&str, &str)] = &[("crates/smime/testdata/", "mailcraft-mime")];

/// Tests that read the source of the whole tree (a word nobody may write, a header nobody may
/// read): they run on every pull request whatever it changed, since a change in a package that
/// doesn't depend on theirs can still fail them. `xtask/src/affected_tests.rs` keeps the list
/// complete.
pub const TREE_WIDE: &[(&str, &str)] = &[
    ("crates/engine/tests/files_mark.rs", "binary_id(mailcraft-engine::files_mark)"),
    ("crates/model/src/headers_tests.rs", "(package(mailcraft-model) & test(/every_name_the_program_reads_stays_on_the_message$/))"),
    (
        "apps/mailcraft-cli/tests/graph_changes.rs",
        "(binary_id(mailcraft-cli::graph_changes) & test(/graphs_delete_past_the_retention_period_is_nowhere_in_the_code$/))",
    ),
];

/// The phone binding, which the `android` job checks for Android.
const MOBILE: &str = "mailcraft-mobile";

/// What the Android app's job (`android-app`) is built from, besides the app itself.
const ANDROID_APP_FROM: &[&str] = &["crates/mobile/", "crates/host/", "xtask/src/mobile", ".github/workflows/ci.yml"];

/// A workspace package: its name, its folder relative to the root (`/`-separated), the workspace
/// packages it depends on (every kind, for the tests) and those it builds with (normal and build,
/// for a check of the library).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub dir: String,
    pub deps: Vec<String>,
    pub build_deps: Vec<String>,
}

/// What one changed file is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// Documentation, fragments, `.github/` but the workflows: nothing the build or tests read.
    Doc,
    /// A translation catalogue (inside `mailcraft-i18n`).
    Translation,
    /// The Android app's own files: no workspace crate reads them.
    AndroidApp,
    /// Reaches every package.
    Global,
    /// Inside a workspace package (the index into the list).
    Package(usize),
    /// Outside every package: the whole suite runs.
    Outside,
}

/// What a run covers. `full` is the reason for running the whole suite, `None` on a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub full: Option<String>,
    /// The packages whose dependents' tests run, sorted.
    pub selected: Vec<String>,
    /// Why, one line a file or rule, for the log.
    pub why: Vec<String>,
    /// Run the Rust matrix and cargo-deny.
    pub rust: bool,
    /// The matrix on Linux alone (a translation).
    pub linux_only: bool,
    /// Run the Android app's job.
    pub android_app: bool,
    /// Run the check of the phone binding for Android.
    pub android_rust: bool,
}

impl Plan {
    /// The nextest filter, empty for the whole suite.
    pub fn filter(&self) -> String {
        if self.full.is_some() || !self.rust {
            return String::new();
        }
        let mut terms: Vec<String> = self.selected.iter().map(|p| format!("rdeps(={p})")).collect();
        terms.extend(TREE_WIDE.iter().map(|(_, t)| (*t).to_string()));
        terms.join(" | ")
    }

    /// The matrix's systems, as JSON.
    pub fn matrix(&self) -> &'static str {
        if self.linux_only { LINUX_ONLY } else { ALL_OS }
    }

    /// One line for the log and the job's outputs.
    pub fn reason(&self) -> String {
        match &self.full {
            Some(r) => format!("whole suite: {r}"),
            None if !self.rust && self.android_app => "the Android app alone: no Rust tests".into(),
            None if !self.rust => "documentation alone: no Rust tests".into(),
            None => format!(
                "tests of {} and what depends on {}{}",
                self.selected.join(", "),
                if self.selected.len() == 1 { "it" } else { "them" },
                if self.linux_only { ", on Linux alone (a translation)" } else { "" }
            ),
        }
    }
}

fn is_rust_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.ends_with(".rs") || name == "Cargo.toml" || name == "Cargo.lock"
}

fn classify(path: &str, packages: &[Package]) -> Kind {
    let in_github = path.starts_with(".github/");
    if GLOBAL.iter().any(|g| if g.ends_with('/') { path.starts_with(g) } else { path == *g }) {
        return Kind::Global;
    }
    if path.starts_with("docs/") || (path.ends_with(".md") && path != "ATTRIBUTION.md") || in_github {
        return Kind::Doc;
    }
    if path.starts_with("crates/i18n/po/") {
        return Kind::Translation;
    }
    if path.starts_with("apps/mailcraft-android/") {
        return if is_rust_file(path) { Kind::Outside } else { Kind::AndroidApp };
    }
    let best = packages
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.dir.is_empty() && path.strip_prefix(p.dir.as_str()).is_some_and(|rest| rest.starts_with('/')))
        .max_by_key(|(_, p)| p.dir.len());
    match best {
        Some((i, _)) => Kind::Package(i),
        None => Kind::Outside,
    }
}

/// The plan for a run: `event` is GitHub's event name, `label` whether the pull request has
/// `full-ci`, `changed` the files the pull request changes (`None` when the diff couldn't be read).
pub fn plan(event: &str, label: bool, changed: Option<&[String]>, packages: &[Package]) -> Plan {
    let full = |reason: String, android_app: bool| Plan {
        why: vec![reason.clone()],
        full: Some(reason),
        selected: Vec::new(),
        rust: true,
        linux_only: false,
        android_app,
        android_rust: true,
    };
    if event != "pull_request" {
        return full(format!("a {event} run"), true);
    }
    if label {
        return full(format!("the pull request has the `{FULL_CI_LABEL}` label"), true);
    }
    let Some(changed) = changed else {
        return full("the pull request's diff could not be read".into(), true);
    };
    if changed.is_empty() {
        return full("the pull request's diff is empty".into(), true);
    }
    let android_app = changed.iter().any(|f| f.starts_with("apps/mailcraft-android/") || ANDROID_APP_FROM.iter().any(|p| f.starts_with(p)));

    let mut selected = BTreeSet::new();
    let mut why = Vec::new();
    let mut docs_or_po = true;
    let mut any_app = false;
    for file in changed {
        let kind = classify(file, packages);
        match kind {
            Kind::Global => return full(format!("{file} can reach every package"), android_app),
            Kind::Outside => return full(format!("{file} is outside every package"), android_app),
            Kind::Doc => {}
            Kind::Translation => {
                if let Some(p) = packages.iter().find(|p| p.dir == "crates/i18n") {
                    why.push(format!("{file} -> {}", p.name));
                    selected.insert(p.name.clone());
                }
            }
            Kind::AndroidApp => {
                why.push(format!("{file} -> the Android app's job"));
                docs_or_po = false;
                any_app = true;
            }
            Kind::Package(i) => {
                docs_or_po = false;
                if let Some(p) = packages.get(i) {
                    why.push(format!("{file} -> {}", p.name));
                    selected.insert(p.name.clone());
                }
            }
        }
        for (prefix, reader) in ALSO_READ_BY {
            if file.starts_with(prefix) && packages.iter().any(|p| p.name == *reader) {
                why.push(format!("{file} -> {reader} (reads it)"));
                selected.insert((*reader).to_string());
            }
        }
    }
    if selected.is_empty() {
        // Documentation, the Android app, or both.
        if any_app {
            why.push("only the Android app's own files: its job, no Rust tests".into());
        } else {
            why.push("only documentation: no Rust tests".into());
        }
        return Plan { full: None, selected: Vec::new(), why, rust: false, linux_only: false, android_app: any_app, android_rust: any_app };
    }
    let mobile_reached = builds_with(MOBILE, packages).iter().any(|p| selected.contains(p));
    Plan {
        full: None,
        selected: selected.into_iter().collect(),
        why,
        rust: true,
        linux_only: docs_or_po,
        android_app,
        android_rust: android_app || mobile_reached,
    }
}

/// `name` and every workspace package it builds with, through normal and build dependencies.
fn builds_with(name: &str, packages: &[Package]) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut stack = vec![name.to_string()];
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(p) = packages.iter().find(|p| p.name == n) {
            stack.extend(p.build_deps.iter().filter(|d| !seen.contains(*d)).cloned());
        }
    }
    seen
}

/// The workspace's packages from `cargo metadata --no-deps`.
pub fn packages(meta: &serde_json::Value, root: &Path) -> Result<Vec<Package>, String> {
    let pkgs = meta["packages"].as_array().ok_or("metadata: no packages array")?;
    let names: BTreeSet<&str> = pkgs.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut out = Vec::new();
    for p in pkgs {
        let name = p["name"].as_str().ok_or("package without name")?.to_string();
        let manifest = p["manifest_path"].as_str().ok_or("package without manifest_path")?;
        let dir = Path::new(manifest).parent().ok_or("manifest without folder")?;
        let dir = dir.strip_prefix(root).map_err(|_| format!("{name}: {} is outside the workspace", dir.display()))?;
        let dir = dir.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        let (mut deps, mut build_deps) = (Vec::new(), Vec::new());
        for d in p["dependencies"].as_array().into_iter().flatten() {
            let Some(dname) = d["name"].as_str() else { continue };
            if !names.contains(dname) {
                continue;
            }
            deps.push(dname.to_string());
            if d["kind"].as_str() != Some("dev") {
                build_deps.push(dname.to_string());
            }
        }
        out.push(Package { name, dir, deps, build_deps });
    }
    Ok(out)
}

/// The files changed between the merge base of `base` and `HEAD`, or `None` when git can't say. CI
/// passes `HEAD^1`, the first parent of a pull request's merge commit.
fn changed_files(root: &Path, base: &str) -> Option<Vec<String>> {
    if base.is_empty() {
        return None;
    }
    let out = std::process::Command::new("git")
        .current_dir(root)
        .args(["diff", "-z", "--name-only", "--no-renames", &format!("{base}...HEAD")])
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!("affected: git diff {base}...HEAD failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        return None;
    }
    Some(out.stdout.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

/// `cargo xtask affected [--event <name>] [--base <rev>] [--full-ci]`: prints the plan, and writes
/// it to `$GITHUB_OUTPUT` (`rust`, `matrix`, `android_app`, `android_rust`, `filter`, `reason`)
/// and `$GITHUB_STEP_SUMMARY` when they are set. Run locally with `--base origin/main`.
pub fn run(root: &Path, meta: &serde_json::Value, args: &[&str]) -> Result<(), String> {
    let mut event = "pull_request".to_string();
    let mut base = String::new();
    let mut label = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match *a {
            "--event" => event = it.next().ok_or("--event needs a value")?.to_string(),
            "--base" => base = it.next().ok_or("--base needs a value")?.to_string(),
            "--full-ci" => label = true,
            other => return Err(format!("affected: unknown argument `{other}`")),
        }
    }
    let packages = packages(meta, root)?;
    let changed = if event == "pull_request" && !label { changed_files(root, &base) } else { None };
    let plan = plan(&event, label, changed.as_deref(), &packages);
    let filter = plan.filter();
    let lines = [
        format!("rust={}", plan.rust),
        format!("matrix={}", plan.matrix()),
        format!("android_app={}", plan.android_app),
        format!("android_rust={}", plan.android_rust),
        format!("filter={filter}"),
        format!("reason={}", plan.reason().replace(['\n', '\r'], " ")),
    ];
    println!("affected: {}", plan.reason());
    for w in &plan.why {
        println!("  {w}");
    }
    for l in &lines {
        println!("{l}");
    }
    let append = |var: &str, text: &str| -> Result<(), String> {
        let Some(path) = std::env::var_os(var) else { return Ok(()) };
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&path).map_err(|e| format!("{var}: {e}"))?;
        f.write_all(text.as_bytes()).map_err(|e| format!("{var}: {e}"))
    };
    append("GITHUB_OUTPUT", &format!("{}\n", lines.join("\n")))?;
    let mut summary = format!("### Tests this run\n\n{}\n\n", plan.reason());
    if !filter.is_empty() {
        summary.push_str(&format!("nextest filter: `{filter}`\n\n"));
    }
    for w in plan.why.iter().take(50) {
        summary.push_str(&format!("- {w}\n"));
    }
    append("GITHUB_STEP_SUMMARY", &summary)
}

#[cfg(test)]
#[path = "affected_tests.rs"]
mod tests;
