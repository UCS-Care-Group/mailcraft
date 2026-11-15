// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context

/**
 * The one JNI entry of the app, implemented in `crates/mobile/src/android_jni.rs`.
 *
 * The Rust side asks the system's trust manager for every TLS connection, through
 * rustls-platform-verifier, which has to be given the app's [Context] before the first one.
 * It must be called before a `Core` is built; a failure throws and is never ignored.
 */
internal object Bridge {
    init {
        System.loadLibrary("mailcraft_mobile")
    }

    external fun initVerifier(context: Context)
}
