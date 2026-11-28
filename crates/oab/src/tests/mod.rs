// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tests: the specifications' own samples, round trips through a test encoder and writer,
//! and hostile input.

mod caps;
pub(crate) mod encoder;
mod hostile;
mod packed;
mod roundtrip;
mod spec_samples;
