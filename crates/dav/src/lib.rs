// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure DAV document helpers. These modules open no sockets, read no files, and hold no secrets.
//! The protocol layer: requests ([`request`]), typed answers ([`answer`]), addresses held to the
//! account's origin ([`href`]), statuses ([`status`]), and a strict in-process server for the tests
//! of the crates above (`standin`, behind the `stand-in` feature). Discovery, sync, account
//! settings and writes are not wired in here.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
pub use mailcraft_model::dav::{Collection, Resource};
pub mod answer;
pub mod document;
pub mod href;
pub mod http;
#[cfg(test)]
#[path = "property_tests.rs"]
mod property_tests;
pub mod request;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin;
pub mod status;
pub mod xml;
