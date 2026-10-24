// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure DAV document helpers. These modules open no sockets, read no files, and hold no secrets.
//! Discovery, sync, account settings and writes are deliberately not wired into this change.
pub mod document;
pub mod xml;
