// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workspace tooling: `cargo xtask <command>`.
//!
//! Pure Rust (std + serde_json). `cargo` and `git` are invoked through `std::process::Command`.

mod affected;
mod assets;
mod docs_log;
mod doctests;
mod i18n;
mod i18n_kotlin;
mod ico;
mod icon;
mod layers;
mod mobile;
mod parity;
mod play_upload;
mod release_key;
mod release_notes;
mod spdx;
mod stats;
mod test_files;
mod version;
mod version_code;
mod wasm_clock;
mod workflows;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  affected [--event <name>] [--base <rev>] [--full-ci]
                  the tests a pull request's CI runs: the changed packages and what depends on them,
                  as a nextest filter, or the whole suite (and why); CI passes it to `ci` as XTASK_TEST_FILTER
  assets          check that every icon/image/font/asset is attributed in ATTRIBUTION.md
  spdx [--fix]    check that every Rust file starts with its SPDX copyright and licence lines
  test-files      tests live in files of their own: no new inline `#[cfg(test)] mod … { }` (CONTRIBUTING.md)
  layers          enforce the crate dependency layering (docs/architecture.md)
  workflows       no job of .github/workflows that holds signing material builds or runs what was built;
                  no `permissions: read-all`; curl only with `--proto =https` (docs/releasing.md)
  mobile          build, lint and test the Android app (needs the Android SDK, NDK, cargo-ndk); not part of `ci`
  wasm            cargo check --target wasm32-unknown-unknown for the wasm-safe crates
  wasm-clock      no std clock or sleep (they panic there) in code the browser build runs (#1337)
  deny            cargo deny check (advisories, licences, bans, sources; see deny.toml)
  ci              the cheap checks first (fmt --check, assets, spdx, test-files, workflows, layers, wasm-clock,
                  i18n --check, release-notes, docs-log), then clippy -D warnings, test, wasm, deny
                  (stops at first failure; XTASK_CI_ONLY=a,b runs only those steps, XTASK_CI_SKIP=a,b skips them)
  stats [--exact] count tests and lines per crate (--exact: ask the test harness via `-- --list`)
  i18n [--check] [--lenient]
                  gather the interface's English into crates/i18n/po/messages.pot and merge it into
                  each language's .po (coverage per language); --check changes nothing, fails if out of date
                  (only reports it with --lenient, and in CI's merge queue and on main)
  parity          recompute the Outlook feature-parity summary in docs/parity-checklist.md
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
  icon            render the app icon (assets/app-icon/source/mailcraft.svg) in every size into assets/app-icon
  version [...]   show or set the workspace version
  version-code [X.Y.Z[-rc.N]]
                  the Android version code of that version (default: the workspace's)
  release-notes [--write]
                  print the release notes with the fragments in packaging/release-notes.d/ joined in;
                  --write (before tagging) joins them into packaging/release-notes.md and removes them
  docs-log [--check | --write]
                  print each document with the table-row fragments in docs/log.d/ joined in; --check
                  only checks they join; --write (in the release's pull request) joins them and removes them
  release-key new <dir>
                  make the two release keys (in use, spare): private keys into <dir>, public keys printed
  release-key sign <file>
                  sign <file> into <file>.sig with the key in MAILCRAFT_RELEASE_KEY (base64 PKCS#8)
  play-upload <file.aab>
                  the AAB to Google Play's internal track as a draft, with the service account's JSON
                  key in PLAY_SERVICE_ACCOUNT_JSON (the release workflow; docs/releasing.md)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("affected") => metadata().and_then(|m| affected::run(&root(), &m, &rest)),
        Some("assets") => assets::run(&root()),
        Some("spdx") => spdx::run(&root(), rest.contains(&"--fix")),
        Some("test-files") => test_files::run(&root()),
        Some("layers") => cmd_layers(),
        Some("workflows") => workflows::run(&root()),
        Some("mobile") => mobile::run(&root()),
        Some("wasm") => cmd_wasm(),
        Some("wasm-clock") => metadata().and_then(|m| wasm_clock::run(&root(), &m)),
        Some("deny") => cmd_deny(),
        Some("ci") => cmd_ci(),
        Some("stats") => stats::run(&root(), rest.contains(&"--exact")),
        Some("parity") => parity::run(&root()),
        Some("i18n") => i18n::run(&root(), rest.contains(&"--check"), i18n::lenient(rest.contains(&"--lenient"), event().as_deref())),
        Some("ico") => ico::run(&rest),
        Some("icon") => icon::run(&root()),
        Some("version") => version::run(&root(), &rest),
        Some("version-code") => version_code::run(&root(), &rest),
        Some("release-notes") => release_notes::run(&root(), rest.contains(&"--write")),
        Some("docs-log") => docs_log::run(&root(), &rest),
        Some("release-key") => release_key::run(&rest),
        Some("play-upload") => play_upload::run(&rest),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The GitHub Actions event this runs for (`pull_request`, `merge_group`, `push`), when in CI.
fn event() -> Option<String> {
    std::env::var("GITHUB_EVENT_NAME").ok()
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

/// `cargo`, run from the workspace root. Every command xtask runs on the workspace passes
/// `--locked`, so what CI checks (and cargo-deny audits) is the committed `Cargo.lock`, not a fresh
/// resolution; a stale lock is an error, not something that is quietly rewritten.
///
/// What `cargo run` set for xtask itself (its package's name, version, folder and so on) is taken
/// out: a build script that reads one of them (ring's reads `CARGO_MANIFEST_DIR`) would otherwise
/// see it change and rebuild, with its dependents, at every step. Cargo's own settings
/// (`CARGO_HOME`, `CARGO_TARGET_DIR`, `CARGO_BUILD_*`, `CARGO_NET_*`, `RUSTFLAGS`…) are kept.
pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(set_by_cargo_run) {
            c.env_remove(key);
        }
    }
    c
}

/// Whether `key` is one of the variables `cargo run` sets for the program it runs (and a build
/// script for its crate), rather than a setting of the caller's.
pub fn set_by_cargo_run(key: &str) -> bool {
    key.starts_with("CARGO_PKG_")
        || matches!(
            key,
            "CARGO_MANIFEST_DIR"
                | "CARGO_MANIFEST_PATH"
                | "CARGO_MANIFEST_LINKS"
                | "CARGO_CRATE_NAME"
                | "CARGO_BIN_NAME"
                | "CARGO_PRIMARY_PACKAGE"
                | "CARGO_RUSTC_CURRENT_DIR"
                | "OUT_DIR"
        )
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--locked", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

fn cmd_layers() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    println!("Dependency layering (docs/architecture.md)\n");
    println!("{:<28} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| {
                let k = match d.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{k}", layers::short_name(&d.name))
            })
            .collect();
        println!("{:<28} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        Ok(())
    } else {
        println!("{} violation(s):", violations.len());
        for v in &violations {
            println!("  - {v}");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

/// Workspace packages that must build for wasm32: all L0–L5 crates, the egui shell and the
/// browser app.
fn wasm_set() -> Result<Vec<String>, String> {
    let crates = layers::from_metadata(&metadata()?)?;
    Ok(crates
        .into_iter()
        .filter(|c| match layers::classify(&c.name) {
            Some(layers::Class::Layer(l)) => l <= 5 || layers::short_name(&c.name) == "ui-egui",
            Some(layers::Class::Standalone) => true,
            _ => false,
        })
        .map(|c| c.name)
        // The browser app itself: its own code is wasm-only, and the release builds it.
        .chain(std::iter::once("mailcraft-web".to_string()))
        .collect())
}

fn cmd_wasm() -> Result<(), String> {
    let set = wasm_set()?;
    let mut results = Vec::new();
    for pkg in &set {
        let mut c = cargo();
        c.args(["check", "--locked", "--target", "wasm32-unknown-unknown", "-p", pkg]);
        let ok = run(c, &format!("cargo check --target wasm32-unknown-unknown -p {pkg}")).is_ok();
        results.push((pkg.clone(), ok));
    }
    println!("\nwasm32-unknown-unknown check:");
    for (p, ok) in &results {
        println!("  {:<6} {p}", if *ok { "ok" } else { "FAIL" });
    }
    let failed = results.iter().filter(|r| !r.1).count();
    if failed == 0 { Ok(()) } else { Err(format!("{failed} crate(s) failed the wasm check")) }
}

/// `cargo deny check` against deny.toml. cargo-deny is a separate install: without it the step
/// says how to get it and passes on a developer's machine, and fails where `CI` is set.
fn cmd_deny() -> Result<(), String> {
    let installed = cargo().args(["deny", "--version"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !installed {
        let hint = "cargo-deny is not installed: `cargo install cargo-deny --locked`";
        if std::env::var_os("CI").is_some() {
            return Err(hint.into());
        }
        eprintln!("deny: skipped, {hint}");
        return Ok(());
    }
    let mut c = cargo();
    c.args(["deny", "--locked", "check"]);
    run(c, "cargo deny --locked check")
}

/// The workspace's tests. With cargo-nextest installed (CI installs it) the test binaries run side
/// by side rather than one after another; without it, `cargo test` runs the lot.
///
/// `XTASK_TEST_FILTER` (CI sets it on a pull request from `cargo xtask affected`) runs only the
/// tests it selects, a nextest filter such as `rdeps(=mailcraft-model)`; empty or unset, all of
/// them. nextest doesn't run doc-tests, and the workspace has none: `cargo test --doc` runs only
/// when one appears (`doctests.rs`), so a new one is never silently skipped.
fn cmd_test() -> Result<(), String> {
    let nextest = cargo().args(["nextest", "--version"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !nextest {
        let mut c = cargo();
        c.args(["test", "--locked", "--workspace"]);
        return run(c, "cargo test --locked --workspace");
    }
    let filter = std::env::var("XTASK_TEST_FILTER").unwrap_or_default();
    let filter = filter.trim();
    let mut c = cargo();
    c.args(["nextest", "run", "--locked", "--workspace", "--no-fail-fast"]);
    if filter.is_empty() {
        eprintln!("test: every test (no XTASK_TEST_FILTER)");
        run(c, "cargo nextest run --locked --workspace --no-fail-fast")?;
    } else {
        eprintln!("test: the tests XTASK_TEST_FILTER selects: {filter}");
        c.args(["--no-tests=warn", "-E", filter]);
        run(c, &format!("cargo nextest run --locked --workspace --no-fail-fast --no-tests=warn -E '{filter}'"))?;
    }
    let doctests = doctests::find(&root());
    if doctests.is_empty() {
        eprintln!("test: no doc-tests in the workspace, `cargo test --doc` skipped");
        return Ok(());
    }
    eprintln!("test: doc-tests found ({}), running them", doctests.join(", "));
    let mut c = cargo();
    c.args(["test", "--locked", "--workspace", "--doc"]);
    run(c, "cargo test --locked --workspace --doc")
}

fn cmd_ci() -> Result<(), String> {
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    // The cheap checks first (seconds together, no build of the workspace), so that a stale
    // catalogue or a fragment that won't join fails at once and not after the tests. CI's `quick`
    // job runs just these (`XTASK_CI_ONLY`).
    let steps: Vec<Step> = vec![
        (
            "fmt",
            Box::new(|| {
                let mut c = cargo();
                c.args(["fmt", "--all", "--", "--check"]);
                run(c, "cargo fmt --all -- --check")
            }),
        ),
        ("assets", Box::new(|| assets::run(&root()))),
        ("spdx", Box::new(|| spdx::run(&root(), false))),
        ("test-files", Box::new(|| test_files::run(&root()))),
        ("workflows", Box::new(|| workflows::run(&root()))),
        ("layers", Box::new(cmd_layers)),
        ("wasm-clock", Box::new(|| metadata().and_then(|m| wasm_clock::run(&root(), &m)))),
        ("i18n", Box::new(|| i18n::run(&root(), true, i18n::lenient(false, event().as_deref())))),
        ("release-notes", Box::new(|| release_notes::check(&root()))),
        ("docs-log", Box::new(|| docs_log::check(&root()))),
        (
            "clippy",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"]);
                run(c, "cargo clippy --locked --workspace --all-targets -- -D warnings")
            }),
        ),
        ("test", Box::new(cmd_test)),
        ("wasm", Box::new(cmd_wasm)),
        ("deny", Box::new(cmd_deny)),
    ];
    // `XTASK_CI_SKIP=clippy,wasm`: steps another runner covers (CI runs them on Linux only).
    let skip = std::env::var("XTASK_CI_SKIP").unwrap_or_default();
    let skipped: Vec<&str> = skip.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    // `XTASK_CI_ONLY=fmt,spdx`: only those steps (CI's `quick` job).
    let only = std::env::var("XTASK_CI_ONLY").unwrap_or_default();
    let only: Vec<&str> = only.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if let Some(unknown) = only.iter().chain(&skipped).find(|n| !steps.iter().any(|(s, _)| s == *n)) {
        return Err(format!("ci: no step `{unknown}` (XTASK_CI_ONLY / XTASK_CI_SKIP)"));
    }
    let steps: Vec<Step> = steps.into_iter().filter(|(name, _)| !skipped.contains(name) && (only.is_empty() || only.contains(name))).collect();
    if !skipped.is_empty() {
        eprintln!("ci: skipping {} (XTASK_CI_SKIP)", skipped.join(", "));
    }
    if !only.is_empty() {
        eprintln!("ci: only {} (XTASK_CI_ONLY)", only.join(", "));
    }
    let mut done = Vec::new();
    for (name, f) in &steps {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI summary:");
            for d in &done {
                println!("  ok    {d}");
            }
            println!("  FAIL  {name}: {e}");
            for (n, _) in steps.iter().skip(done.len() + 1) {
                println!("  skip  {n}");
            }
            return Err(format!("ci failed at `{name}`"));
        }
        done.push(*name);
    }
    println!("\nCI summary: all {} steps passed ({})", done.len(), done.join(", "));
    Ok(())
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
