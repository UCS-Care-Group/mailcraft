// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Bakes the contributor credits into the binary as static Rust tables (`docs/contributors.md`,
//! craftrules standards/contributors.md), so nothing is read from disk or the network at run time
//! and the web build has them too.
//!
//! A missing or malformed file is not a reason to fail a build: the About window then shows no
//! contributors (or no testers) and the build says why in a warning. The tests read the repository's own files
//! with the same code (`credits_data.rs`), so a malformed file fails `cargo xtask ci`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[path = "src/credits_data.rs"]
mod credits_data;

use std::path::{Path, PathBuf};

fn read(path: &Path) -> Result<String, String> {
    println!("cargo::rerun-if-changed={}", path.display());
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The project's maintainers' address, from `funding.json` at the root of the repository (its
/// `entity.email`, #54): Report a Problem's Send by Email writes to it. Empty, with a warning,
/// when it can't be read; the window then says email isn't available.
fn maintainer() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../funding.json");
    let found = read(&path).and_then(|json| {
        let value: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))?;
        let email = value.get("entity").and_then(|e| e.get("email")).and_then(serde_json::Value::as_str).unwrap_or_default();
        let plain = email.len() <= 200 && email.matches('@').count() == 1 && email.chars().all(|c| c.is_ascii_alphanumeric() || "@.-_+".contains(c));
        if plain { Ok(email.to_string()) } else { Err(format!("{}: no plain entity.email", path.display())) }
    });
    found.unwrap_or_else(|e| {
        println!("cargo::warning=maintainer address: {e}; Report a Problem can't send by email");
        String::new()
    })
}

fn main() {
    println!("cargo::rustc-env=MAILCRAFT_MAINTAINER_EMAIL={}", maintainer());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contributors");
    // Without people.toml everyone is still credited, by username.
    let people = read(&dir.join("people.toml")).unwrap_or_else(|e| {
        println!("cargo::warning=contributor credits: {e}; contributors are shown by username only");
        String::new()
    });
    let credits = read(&dir.join("contributors.json")).and_then(|json| credits_data::parse(&json, &people));
    let credits = credits.unwrap_or_else(|e| {
        println!("cargo::warning=contributor credits: {e}; the About window shows no contributors");
        credits_data::Credits::default()
    });
    let testers = read(&dir.join("testers.toml")).and_then(|toml| credits_data::parse_testers(&toml));
    let testers = testers.unwrap_or_else(|e| {
        println!("cargo::warning=tester credits: {e}; the About window shows no testers");
        Vec::new()
    });
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    for (name, src) in [("credits.rs", credits_data::render(&credits)), ("testers.rs", credits_data::render_testers(&testers))] {
        let out = out_dir.join(name);
        if let Err(e) = std::fs::write(&out, src) {
            println!("cargo::warning=writing {}: {e}", out.display());
        }
    }
}
