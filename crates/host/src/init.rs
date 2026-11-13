// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Start-up: the platform handed over once, and the memory budget worked out from it.

use std::fmt;
use std::sync::Arc;

use crate::Platform;

/// The program's hold on its platform, from [`init`].
pub struct Host {
    pub(crate) platform: Arc<dyn Platform>,
    memory_budget: u64,
}

impl Host {
    pub fn platform(&self) -> &dyn Platform {
        self.platform.as_ref()
    }

    /// The most bytes of mail content to keep in memory ([`memory_budget`]).
    pub fn memory_budget(&self) -> u64 {
        self.memory_budget
    }
}

/// Why [`init`] refused to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InitError {
    /// The platform says it can spare no memory at all.
    NoMemory,
}

impl fmt::Display for InitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InitError::NoMemory => f.write_str("the platform has no memory to spare for MailCraft"),
        }
    }
}

impl std::error::Error for InitError {}

/// The memory budget: the lower of what the device allows (`device`, when it says) and the
/// engine's own cap ([`mailcraft_engine::store::body_budget`]).
pub fn memory_budget(device: Option<u64>) -> u64 {
    let engine = engine_cap();
    device.map_or(engine, |d| d.min(engine))
}

/// The engine's own cap on mail content in memory.
#[cfg(not(target_arch = "wasm32"))]
fn engine_cap() -> u64 {
    mailcraft_engine::store::body_budget()
}

/// The browser has no mailbox file to keep bodies in, so the engine's fallback stands in.
#[cfg(target_arch = "wasm32")]
fn engine_cap() -> u64 {
    128 << 20
}

/// Start up on `platform`. Never panics: a platform that can't be used is an error, and the
/// caller says so.
pub fn init(platform: Arc<dyn Platform>) -> Result<Host, InitError> {
    let device = platform.memory_budget();
    if device == Some(0) {
        return Err(InitError::NoMemory);
    }
    Ok(Host { memory_budget: memory_budget(device), platform })
}
