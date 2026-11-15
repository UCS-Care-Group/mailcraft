// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask mobile`: builds the Android app's Rust library and the app, lints it, runs its unit
//! tests and checks what ships in it. Not part of `cargo xtask ci`: it needs the Android SDK and NDK,
//! `cargo-ndk` and the Android Rust targets, which a developer's machine may not have; the
//! path-filtered `android` job of `.github/workflows/ci.yml` runs it.
//!
//! The steps, in one Gradle run (`apps/mailcraft-android`): `cargo ndk` builds `crates/mobile` for
//! arm64-v8a and x86_64, UniFFI's Kotlin bindings are generated from that library, then
//! `assembleDebug assembleRelease lint detekt detektDebug testDebugUnitTest verifyPaparazziDebug` (the
//! last compares the shell's screenshots with the ones committed in `app/src/test/snapshots`; after a
//! change to a screen, `./gradlew recordPaparazziDebug` draws them again). Then the dependency tree that ships
//! is read: every artifact's licence must be on the permissive allowlist
//! (`apps/mailcraft-android/licences-allowed.txt`), and no Google Play Services (`com.google.android.gms`)
//! or Firebase (`com.google.firebase`) artifact may be anywhere in any configuration.

use std::path::Path;
use std::process::Command;

const APP: &str = "apps/mailcraft-android";
const REPORT: &str = "app/build/reports/dependency-licences.tsv";

/// Group prefixes no artifact of the app may have, in any configuration.
const BANNED_GROUPS: &[&str] = &["com.google.android.gms", "com.google.firebase"];

/// What `licences-allowed.txt` says: the licences that may ship, and the artifacts whose POM names
/// none, each with the licence it was checked to have.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Allowlist {
    /// Lower-case words; a licence name is allowed when one of its words is on this list.
    pub words: Vec<String>,
    /// `group:name` to the licence it was checked to be under.
    pub unlisted: Vec<(String, String)>,
}

/// Parse the allowlist: `allow <word>` and `unlisted <group:name> <licence…>` lines; `#` starts a
/// comment; blank lines are skipped. A line that is neither is an error, so a typo is not a hole.
pub fn parse_allowlist(text: &str) -> Result<Allowlist, String> {
    let mut list = Allowlist::default();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, char::is_whitespace);
        match (parts.next(), parts.next(), parts.next()) {
            (Some("allow"), Some(word), None) => list.words.push(word.to_ascii_lowercase()),
            (Some("unlisted"), Some(artifact), Some(licence)) => list.unlisted.push((artifact.to_string(), licence.trim().to_string())),
            _ => {
                return Err(format!("licences-allowed.txt line {}: `{line}` is neither `allow <word>` nor `unlisted <group:name> <licence>`", n + 1));
            }
        }
    }
    Ok(list)
}

fn words(name: &str) -> impl Iterator<Item = String> + '_ {
    name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_lowercase)
}

/// Whether a licence name is on the list: one of its words is an allowed one (`Apache-2.0`,
/// `The Apache Software License, Version 2.0`, `MIT License`, `BSD-3-Clause`).
pub fn licence_allowed(name: &str, list: &Allowlist) -> bool {
    words(name).any(|w| list.words.contains(&w))
}

/// The artifacts of the report (`group:name:version<TAB>licence;licence`) that may not ship: a
/// banned group, or no licence on the list. An artifact offered under several licences (JNA is
/// Apache-2.0 or LGPL) passes when any one is allowed. Returns the problems, empty when all is well.
pub fn check_report(report: &str, list: &Allowlist) -> Vec<String> {
    let mut problems = Vec::new();
    for line in report.lines().filter(|l| !l.trim().is_empty()) {
        let (coords, licences) = line.split_once('\t').unwrap_or((line, ""));
        let mut parts = coords.splitn(3, ':');
        let (group, name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let artifact = format!("{group}:{name}");
        if banned_group(group) {
            problems.push(format!("{coords}: Google Play Services and Firebase may not ship"));
            continue;
        }
        let names: Vec<&str> = licences.split(';').map(str::trim).filter(|l| !l.is_empty()).collect();
        if names.is_empty() {
            if !list.unlisted.iter().any(|(a, _)| *a == artifact) {
                problems.push(format!("{coords}: its POM names no licence (check it, then add `unlisted {artifact} <licence>`)"));
            }
        } else if !names.iter().any(|n| licence_allowed(n, list)) {
            problems.push(format!("{coords}: licence `{}` is not on the permissive allowlist", names.join("; ")));
        }
    }
    problems
}

/// Whether a group is banned: exactly, or as a subgroup (`com.google.firebase.tools`).
fn banned_group(group: &str) -> bool {
    BANNED_GROUPS.iter().any(|b| group == *b || group.starts_with(&format!("{b}.")))
}

/// The banned groups (or their subgroups) named anywhere in the text of `gradlew :app:dependencies`
/// (every configuration, the build's own tooling included).
pub fn banned_in_tree(tree: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in tree.lines() {
        for token in line.split_whitespace() {
            let coord = token.trim_start_matches(|c: char| !c.is_alphanumeric());
            let Some((group, _)) = coord.split_once(':') else { continue };
            if banned_group(group) && !found.iter().any(|f| f == coord) {
                found.push(coord.to_string());
            }
        }
    }
    found
}

fn gradle(root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(root.join(APP).join("gradlew"));
    c.current_dir(root.join(APP)).arg("--console=plain").arg("--no-daemon").args(args);
    c
}

fn check_tools() -> Result<(), String> {
    let ndk = Command::new("cargo").args(["ndk", "--version"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !ndk {
        return Err("cargo-ndk is not installed: `cargo install cargo-ndk --locked`".into());
    }
    let targets = Command::new("rustup").args(["target", "list", "--installed"]).output().map_err(|e| format!("rustup: {e}"))?;
    let installed = String::from_utf8_lossy(&targets.stdout);
    for t in ["aarch64-linux-android", "x86_64-linux-android"] {
        if !installed.lines().any(|l| l.trim() == t) {
            return Err(format!("the Rust target {t} is missing: `rustup target add {t}`"));
        }
    }
    if std::env::var_os("ANDROID_HOME").is_none()
        && std::env::var_os("ANDROID_SDK_ROOT").is_none()
        && !crate::root().join(APP).join("local.properties").exists()
    {
        return Err("the Android SDK is not found: set ANDROID_HOME (platform 36, build-tools, NDK 28.2.13676358)".into());
    }
    Ok(())
}

pub fn run(root: &Path) -> Result<(), String> {
    check_tools()?;
    let build =
        ["assembleDebug", "assembleRelease", "lint", "detekt", "detektDebug", "testDebugUnitTest", "verifyPaparazziDebug", ":app:dependencyLicences"];
    crate::run(gradle(root, &build), "gradlew assembleDebug assembleRelease lint detekt detektDebug testDebugUnitTest verifyPaparazziDebug")?;

    let list =
        parse_allowlist(&std::fs::read_to_string(root.join(APP).join("licences-allowed.txt")).map_err(|e| format!("licences-allowed.txt: {e}"))?)?;
    let report = std::fs::read_to_string(root.join(APP).join(REPORT)).map_err(|e| format!("{REPORT}: {e}"))?;
    let mut problems = check_report(&report, &list);

    let tree = gradle(root, &[":app:dependencies"]).output().map_err(|e| format!("gradlew dependencies: {e}"))?;
    if !tree.status.success() {
        return Err(format!("gradlew :app:dependencies failed:\n{}", String::from_utf8_lossy(&tree.stderr)));
    }
    for coord in banned_in_tree(&String::from_utf8_lossy(&tree.stdout)) {
        problems.push(format!("{coord}: Google Play Services and Firebase may not be in any configuration"));
    }

    if problems.is_empty() {
        println!(
            "mobile: built, linted and tested; {} artifacts ship, all under allowed licences, no Play Services or Firebase",
            report.lines().count()
        );
        Ok(())
    } else {
        Err(format!("{} dependency problem(s):\n  {}", problems.len(), problems.join("\n  ")))
    }
}

#[cfg(test)]
#[path = "mobile_tests.rs"]
mod tests;
