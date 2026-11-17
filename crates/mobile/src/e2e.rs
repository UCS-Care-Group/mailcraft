// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The end-to-end run's one seam (MB3.15), in the `e2e` build alone: every call of the process
//! to Microsoft Graph and to the sign-in's token endpoint goes to the Graph stand-in
//! (`cargo run -p mailcraft-net --features stand-in --example standin -- --port 8780 --e2e`) on
//! the device's own loopback address, which `adb reverse` joins to the computer's. The sign-in
//! page is named at an address that is never found, for the instrumented test to play. Nothing
//! reaches Microsoft or any real account; a release build never has this module (the Android
//! build refuses to make one with `mailcraft.e2e` set).

use crate::CoreError;

/// Where the stand-in listens, on the device as on the computer (the port `adb reverse` joins).
pub(crate) const STAND_IN: &str = "http://127.0.0.1:8780";

/// Send the process's sign-ins and Graph to [`STAND_IN`]; before the core opens.
pub(crate) fn route() -> Result<(), CoreError> {
    if mailcraft_net::graph::standin::route_this_process(STAND_IN) {
        Ok(())
    } else {
        Err(CoreError::Init { reason: "the end-to-end build could not be pointed at the stand-in".into() })
    }
}
