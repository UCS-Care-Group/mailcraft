// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft in the browser.
//!
//! Runs the same [`mailcraft_ui_egui::MailApp`] as the desktop app through eframe's web runner
//! (wgpu: WebGPU where available, WebGL2 otherwise). Build with `trunk build --release` from this
//! directory (output in `dist/web`).
//!
//! Differences from the desktop app: no IMAP or SMTP (a browser can't open sockets), but JMAP
//! accounts over `fetch` (`jmap.rs`), and no TCP control channel; the mailbox lives in the
//! browser's storage; Open, Import and Attach use the browser's file picker (bytes arrive through
//! `Services::inbox`); Save and the exports download.
//! URL flags: `?webgl` forces WebGL2; `?sample` starts from the sample mailbox; `?empty` from an
//! empty one; neither of those ever writes the mailbox kept in this browser. Without a flag the
//! mailbox kept in this browser opens, or the first time the sample (an empty mailbox when the page
//! names JMAP accounts of its own).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod jmap_rules;
#[cfg(test)]
mod jmap_rules_tests;
mod page;
#[cfg(test)]
mod page_tests;

#[cfg(target_arch = "wasm32")]
mod fetch;
#[cfg(target_arch = "wasm32")]
mod jmap;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    web::start();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("mailcraft-web only runs in the browser: build it with `trunk build --release` in apps/mailcraft-web");
}
