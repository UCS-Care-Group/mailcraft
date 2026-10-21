// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Workspace tooling: `cargo xtask <command>`.
//!
//! Pure Rust (std + serde_json). `cargo` and `git` are invoked through `std::process::Command`.

mod assets;
mod i18n;
mod ico;
mod icon;
mod layers;
mod parity;
mod release_key;
mod spdx;
mod stats;
mod version;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  assets          check that every icon/image/font/asset is attributed in ATTRIBUTION.md
  spdx [--fix]    check that every Rust file starts with its SPDX copyright and licence lines
  layers          enforce the crate dependency layering (docs/architecture.md)
  wasm            cargo check --target wasm32-unknown-unknown for the wasm-safe crates
  deny            cargo deny check (advisories, licences, bans, sources; see deny.toml)
  ci              fmt --check, clippy -D warnings, test, assets, spdx, layers, wasm, deny, i18n --check
                  (stops at first failure)
  stats [--exact] count tests and lines per crate (--exact: ask the test harness via `-- --list`)
  i18n [--check]  gather the interface's English into crates/i18n/po/messages.pot and merge it into
                  each language's .po (coverage per language); --check changes nothing, fails if out of date
  parity          recompute the Outlook feature-parity summary in docs/parity-checklist.md
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
  icon            draw the app icon (original artwork, in code) into assets/app-icon
  version [...]   show or set the workspace version
  release-key new <dir>
                  make the two release keys (in use, spare): private keys into <dir>, public keys printed
  release-key sign <file>
                  sign <file> into <file>.sig with the key in MAILCRAFT_RELEASE_KEY (base64 PKCS#8)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("assets") => assets::run(&root()),
        Some("spdx") => spdx::run(&root(), rest.contains(&"--fix")),
        Some("layers") => cmd_layers(),
        Some("wasm") => cmd_wasm(),
        Some("deny") => cmd_deny(),
        Some("ci") => cmd_ci(),
        Some("stats") => stats::run(&root(), rest.contains(&"--exact")),
        Some("parity") => parity::run(&root()),
        Some("i18n") => i18n::run(&root(), rest.contains(&"--check")),
        Some("ico") => ico::run(&rest),
        Some("icon") => icon::run(&root()),
        Some("version") => version::run(&root(), &rest),
        Some("release-key") => release_key::run(&rest),
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

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask has a parent dir").to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--format-version", "1", "--no-deps"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
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

/// Workspace packages that must build for wasm32: all L0–L5 crates plus
/// the egui shell.
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
        .collect())
}

fn cmd_wasm() -> Result<(), String> {
    let set = wasm_set()?;
    let mut results = Vec::new();
    for pkg in &set {
        let mut c = cargo();
        c.args(["check", "--target", "wasm32-unknown-unknown", "-p", pkg]);
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
    c.args(["deny", "check"]);
    run(c, "cargo deny check")
}

fn cmd_ci() -> Result<(), String> {
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    let steps: Vec<Step> = vec![
        (
            "fmt",
            Box::new(|| {
                let mut c = cargo();
                c.args(["fmt", "--all", "--", "--check"]);
                run(c, "cargo fmt --all -- --check")
            }),
        ),
        (
            "clippy",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
                run(c, "cargo clippy --workspace --all-targets -- -D warnings")
            }),
        ),
        (
            "test",
            Box::new(|| {
                let mut c = cargo();
                c.args(["test", "--workspace"]);
                run(c, "cargo test --workspace")
            }),
        ),
        ("assets", Box::new(|| assets::run(&root()))),
        ("spdx", Box::new(|| spdx::run(&root(), false))),
        ("layers", Box::new(cmd_layers)),
        ("wasm", Box::new(cmd_wasm)),
        ("deny", Box::new(cmd_deny)),
        ("i18n", Box::new(|| i18n::run(&root(), true))),
    ];
    // `XTASK_CI_SKIP=clippy,wasm`: steps another runner covers (CI runs them on Linux only).
    let skip = std::env::var("XTASK_CI_SKIP").unwrap_or_default();
    let skipped: Vec<&str> = skip.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    let steps: Vec<Step> = steps.into_iter().filter(|(name, _)| !skipped.contains(name)).collect();
    if !skipped.is_empty() {
        eprintln!("ci: skipping {} (XTASK_CI_SKIP)", skipped.join(", "));
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
