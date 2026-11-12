// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `cargo xtask live`: the desktop app's JMAP code against a real mail server, on this computer
//! and nowhere else (`docs/live-tests.md`).
//!
//! Not part of `cargo xtask ci`, and nothing here runs unless it is asked for: the tests it runs
//! are `#[ignore]`d (`apps/mailcraft/src/live_tests.rs`). It needs Docker.
//!
//! What it does: starts two throwaway containers on a network of their own. One is the server
//! image: a mail server that speaks JMAP over HTTP on one port, published on a loopback port, with
//! one mailbox (`ada@example.org`) whose password is made up for this run. The other catches every
//! message that server sends out (it is the server's smart host), so that mail goes nowhere, and
//! can be looked at over the catcher's own loopback port. Then it runs the ignored tests whose
//! names start with `live_`, one at a time, with where the server is and the password in the
//! environment of that one `cargo test`; and removes the containers and the network, whatever came
//! of the tests. The mailbox is made by the server's own first start, holds only what the tests
//! put in it, and goes with the container. No real mailbox is ever touched, and the password is
//! never printed.
//!
//! `--keep` leaves the containers running, for a look at them with a browser; the next run
//! removes them first. `MAILCRAFT_LIVE_IMAGE` names another server image, `MAILCRAFT_LIVE_ENV`
//! what that image is started with (its start recipe), `MAILCRAFT_LIVE_SERVER_PORT` the port it
//! speaks HTTP on inside its container, `MAILCRAFT_LIVE_PORT` another loopback port for it, and
//! `MAILCRAFT_LIVE_CATCHER_IMAGE` and `MAILCRAFT_LIVE_CATCHER_PORT` likewise for the catcher.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::process::Command;

const CONTAINER: &str = "mailcraft-live-jmap";
const CATCHER: &str = "mailcraft-live-catcher";
const NETWORK: &str = "mailcraft-live";
/// The server image a run starts when none is named. It is the mail server the author of this
/// harness works on: commercial software under its own licence, free to pull and to run for a few
/// mailboxes, not open source (`docs/live-tests.md` says so too). Nothing of it is in the
/// repository or in a build, and CI never pulls it.
const DEFAULT_IMAGE: &str = "nextxch/server:1.3.3";
/// What the default image is started with: the environment that makes its one mailbox on first
/// start, has it speak HTTP on port 80 and send everything through the catcher, in the clear, on
/// the network of the two. Another image has its own, in `MAILCRAFT_LIVE_ENV`.
const DEFAULT_RECIPE: &str = "ASPNETCORE_URLS=http://0.0.0.0:80 Bootstrap__DefaultDomain=example.org Bootstrap__AdminEmail={email} \
     Bootstrap__AdminPassword={password} Smtp__OutboundSmartHost={catcher} Smtp__OutboundSmartPort=1025 \
     Smtp__OutboundSmartHostTlsMode=None Smtp__OutboundWorkerEnabled=true Acme__Enabled=false";
const DEFAULT_SERVER_PORT: &str = "80";
const DEFAULT_PORT: &str = "58180";
const DEFAULT_CATCHER_IMAGE: &str = "axllent/mailpit:latest";
const DEFAULT_CATCHER_PORT: &str = "58125";
const EMAIL: &str = "ada@example.org";

/// A password for this run: letters and digits from the standard library's own random hashing
/// keys, with what a server's password rules ask for at the end.
fn made_password() -> String {
    let mut out = String::new();
    for n in 0..3u64 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(n);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out.push_str("tA!");
    out
}

fn docker(args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("docker").args(args).output().map_err(|e| format!("docker could not be run ({e}): `cargo xtask live` needs Docker"))
}

fn remove() {
    let _ = docker(&["rm", "-f", CONTAINER, CATCHER]);
    let _ = docker(&["network", "rm", NETWORK]);
}

/// A port from the environment, or the usual one.
fn port_of(name: &str, usual: &str) -> Result<String, String> {
    let port = std::env::var(name).unwrap_or_else(|_| usual.into());
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{name} is not a port number"));
    }
    Ok(port)
}

/// The environment the server image is started with: the entries of `recipe` (`NAME=value`, with
/// white space between them), where `{email}` stands for the mailbox to make, `{password}` for
/// its password and `{catcher}` for the host name of the mail catcher.
fn environment(recipe: &str, password: &str) -> Result<Vec<String>, String> {
    let mut entries = Vec::new();
    for entry in recipe.split_whitespace() {
        if !entry.split_once('=').is_some_and(|(name, _)| !name.is_empty()) {
            return Err("MAILCRAFT_LIVE_ENV holds an entry that is not NAME=value".into());
        }
        entries.push(entry.replace("{email}", EMAIL).replace("{password}", password).replace("{catcher}", CATCHER));
    }
    Ok(entries)
}

/// Run docker with `args`, and say what it said when it failed (never the password).
fn start(args: &[&str], what: &str, password: &str) -> Result<(), String> {
    let done = docker(args)?;
    if done.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&done.stderr).replace(password, "[password]");
    Err(format!("{what} could not be started: {}", said.trim()))
}

/// The network and the two containers, started.
fn start_all(image: &str, catcher_image: &str, publish: &str, catcher_publish: &str, entries: &[String], password: &str) -> Result<(), String> {
    start(&["network", "create", NETWORK], "the test network", password)?;
    // The catcher: mail comes in over the network of the two, and is read over a loopback port.
    start(&["run", "-d", "--name", CATCHER, "--network", NETWORK, "-p", catcher_publish, catcher_image], "the mail catcher", password)?;
    let mut args = vec!["run", "-d", "--name", CONTAINER, "--network", NETWORK, "-p", publish];
    for entry in entries {
        args.extend(["-e", entry.as_str()]);
    }
    args.push(image);
    start(&args, "the test server", password)
}

pub fn run(rest: &[&str]) -> Result<(), String> {
    let keep = rest.contains(&"--keep");
    let image = std::env::var("MAILCRAFT_LIVE_IMAGE").unwrap_or_else(|_| DEFAULT_IMAGE.into());
    let recipe = std::env::var("MAILCRAFT_LIVE_ENV").unwrap_or_else(|_| DEFAULT_RECIPE.into());
    let catcher_image = std::env::var("MAILCRAFT_LIVE_CATCHER_IMAGE").unwrap_or_else(|_| DEFAULT_CATCHER_IMAGE.into());
    let server_port = port_of("MAILCRAFT_LIVE_SERVER_PORT", DEFAULT_SERVER_PORT)?;
    let port = port_of("MAILCRAFT_LIVE_PORT", DEFAULT_PORT)?;
    let catcher_port = port_of("MAILCRAFT_LIVE_CATCHER_PORT", DEFAULT_CATCHER_PORT)?;
    let password = made_password();
    let entries = environment(&recipe, &password)?;
    // Loopback only: the two are reachable from this computer and from nowhere else.
    let publish = format!("127.0.0.1:{port}:{server_port}");
    let catcher_publish = format!("127.0.0.1:{catcher_port}:8025");
    remove();
    if let Err(e) = start_all(&image, &catcher_image, &publish, &catcher_publish, &entries, &password) {
        remove();
        return Err(e);
    }
    println!("live: the test server is starting on 127.0.0.1:{port} ({image}), its mail caught on 127.0.0.1:{catcher_port}");
    let mut tests = crate::cargo();
    tests
        .args(["test", "--locked", "-p", "mailcraft", "--bin", "mailcraft", "live_", "--", "--ignored", "--test-threads=1", "--nocapture"])
        .env("MAILCRAFT_LIVE_SESSION", format!("http://127.0.0.1:{port}/.well-known/jmap"))
        .env("MAILCRAFT_LIVE_EMAIL", EMAIL)
        .env("MAILCRAFT_LIVE_SECRET", &password);
    let outcome = tests.status().map_err(|e| format!("cargo test could not be run: {e}"));
    if keep {
        println!("live: the containers are left running (`{CONTAINER}`, `{CATCHER}`); the next `cargo xtask live` removes them");
    } else {
        remove();
    }
    match outcome {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err("the live tests failed".into()),
        Err(e) => Err(e),
    }
}
