// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How long something has taken, on every platform the program runs on.
//!
//! `std::time::Instant::now()` panics on wasm32-unknown-unknown, where the standard library has no
//! clock (#1337): a budget or a "this was slow" measured in code the browser runs goes through a
//! [`Stopwatch`], which asks the system's monotonic clock natively and the browser's clock there.

use std::time::Duration;

/// Started when it is made; [`Stopwatch::so_far`] says how long ago that was.
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    #[cfg(not(target_arch = "wasm32"))]
    started: std::time::Instant,
    /// The browser's clock when it was made, in milliseconds since the epoch.
    #[cfg(target_arch = "wasm32")]
    started: f64,
}

impl Stopwatch {
    /// A stopwatch running from now.
    pub fn start() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self { started: std::time::Instant::now() }
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self { started: browser_ms() }
        }
    }

    /// How long it has been running. Never negative: a browser clock set back says zero.
    pub fn so_far(&self) -> Duration {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.started.elapsed()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let ms = browser_ms() - self.started;
            if ms.is_finite() && ms > 0.0 { Duration::try_from_secs_f64(ms / 1000.0).unwrap_or(Duration::MAX) } else { Duration::ZERO }
        }
    }
}

/// The browser's clock, in milliseconds since the epoch; 0 when it says something unusable.
#[cfg(target_arch = "wasm32")]
fn browser_ms() -> f64 {
    let ms = js_sys::Date::now();
    if ms.is_finite() && ms > 0.0 { ms } else { 0.0 }
}
