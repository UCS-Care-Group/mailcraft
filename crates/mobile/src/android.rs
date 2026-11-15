// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Android: the TLS verifier asks the system's trust manager through the JVM, and
//! `rustls-platform-verifier` **panics** when it is used before it was set up. So the core
//! refuses to start until the set-up is done, and says so as an error.
//!
//! The set-up itself needs a `JavaVM` and the app's `Context`, which only a JNI entry gets, and
//! reaching one takes `unsafe` (an exported `Java_…` function or `JNI_OnLoad`): UniFFI can pass
//! neither. This crate forbids `unsafe`, so the entry lives apart, in the Android bridge, which
//! calls `rustls_platform_verifier::android::init_with_env` (the app's `Context` comes from
//! Kotlin, with the library's own Kotlin component in the app's build) and then
//! [`verifier_is_ready`], before it builds a `Core`.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::CoreError;

static READY: AtomicBool = AtomicBool::new(false);

/// For the Android bridge: the platform verifier's set-up has returned `Ok`. Nothing here checks
/// it: a bridge that says so without having done it gets the verifier's panic at the first TLS
/// connection.
pub fn verifier_is_ready() {
    READY.store(true, Ordering::Release);
}

/// Refuse to go on before the verifier is set up. Only Android has the need: elsewhere the
/// verifier asks the system without any set-up.
pub(crate) fn check() -> Result<(), CoreError> {
    if !cfg!(target_os = "android") || READY.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(CoreError::Init { reason: "the system's certificate check isn't set up".to_string() })
    }
}
