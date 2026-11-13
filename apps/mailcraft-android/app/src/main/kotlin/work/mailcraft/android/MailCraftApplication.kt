// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Application
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** What starting the Rust core came to: the core, or why there is none. */
internal sealed interface Started {
    class Ready(val core: Core) : Started

    data object Failed : Started
}

/** Holds the one [Core] for the life of the process. */
class MailCraftApplication : Application() {
    internal val started: Started by lazy { start() }

    // Setting up the system's certificate check and opening the core can each fail without
    // being a bug in the app (the library did not load, the platform refused), so a failure is
    // an answer for the screen to show and never a crash.
    @Suppress("TooGenericExceptionCaught")
    private fun start(): Started =
        try {
            Bridge.initVerifier(applicationContext)
            Started.Ready(Core(AndroidPlatform()))
        } catch (_: CoreException) {
            Started.Failed
        } catch (_: LinkageError) {
            Started.Failed
        } catch (_: RuntimeException) {
            Started.Failed
        }
}
