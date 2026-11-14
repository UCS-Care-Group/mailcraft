// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one JNI entry of the Android app: `Bridge.initVerifier(context)` sets up
//! `rustls-platform-verifier` with the app's `Context`, so that the system's trust manager is
//! asked for every TLS connection, and then [`crate::android::verifier_is_ready`] says so.
//!
//! The `jni` crate's `EnvUnowned::with_env` does the unsafe part (it catches a panic and turns an
//! exception into an error), so nothing here is `unsafe`. The app calls this before it builds a
//! `Core`; a failure is the exception that `Bridge.initVerifier` throws and is never a crash.

use jni::objects::JObject;
use jni::{EnvUnowned, jni_mangle};

use crate::android;

/// `work.mailcraft.android.Bridge.initVerifier(context: Context)`.
#[jni_mangle("work.mailcraft.android.Bridge")]
pub fn init_verifier<'local>(mut env: EnvUnowned<'local>, _bridge: JObject<'local>, context: JObject<'local>) {
    let done = env.with_env(|env| -> jni::errors::Result<()> {
        rustls_platform_verifier::android::init_with_env(env, context)?;
        android::verifier_is_ready();
        Ok(())
    });
    // (A failure leaves the verifier unready: the core then refuses to start, as an error.)
    let _ = done.resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
