//! MailCraft in the browser.
//!
//! Runs the same [`mailcraft_ui_egui::MailApp`] as the desktop app through eframe's web runner
//! (wgpu: WebGPU where available, WebGL2 otherwise). Build with `trunk build --release` from this
//! directory (output in `dist/web`).
//!
//! Differences from the desktop app: no IMAP or SMTP connections (a browser can't open sockets), no
//! Microsoft Graph runner yet and no TCP control channel; the mailbox lives in the browser's storage;
//! Open, Import and Attach use the browser's file picker (bytes arrive through `Services::inbox`);
//! Save and the exports download. An account kept in step with a JMAP server (RFC 8620, 8621) is
//! read over HTTPS, through the browser's `fetch` (`jmap_services.rs`).
//! URL flags: `?webgl` forces WebGL2; `?sample` starts from the sample mailbox; `?empty` from an
//! empty one. Without a flag the mailbox kept in this browser opens, or the sample the first time.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

// HTTP through the browser's `fetch`, for JMAP; the runner of a JMAP account's Send/Receive and the
// credentials it is given (built everywhere, so that its tests run natively).
mod credentials;
#[cfg(target_arch = "wasm32")]
mod fetch;
mod jmap_services;
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
