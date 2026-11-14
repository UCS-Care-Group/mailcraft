// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.Activity
import android.app.Application
import android.os.Bundle
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** What starting the Rust core came to: the core, or why there is none. */
internal sealed interface Started {
    class Ready(val core: Core) : Started

    data object Failed : Started
}

/** Holds the one [Core] for the life of the process. */
class MailCraftApplication : Application() {
    /** Where the sign-in's redirect lands ([RedirectActivity]). */
    internal val handback = SignInHandback()
    private val foreground = ForegroundActivity()

    /** Outlives every window (the save in `onStop` runs here; the core's own calls are asynchronous). */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    internal val started: Started by lazy { start() }

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(object : ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) = foreground.resumed(activity)

            override fun onActivityPaused(activity: Activity) = foreground.paused(activity)

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit

            override fun onActivityStarted(activity: Activity) = Unit

            override fun onActivityStopped(activity: Activity) = saveNow()

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

            override fun onActivityDestroyed(activity: Activity) = Unit
        })
        // Copies of attachments written for other programs to open are kept a day, not for good.
        runCatching { OpenedFiles(this).clearOld() }
    }

    /** The mailbox is written when no window is on screen: the system may end the process after it. */
    internal fun saveNow() {
        val ready = (started as? Started.Ready) ?: return
        scope.launch {
            try {
                ready.core.saveNow()
            } catch (_: CoreException) {
                // The core says so itself (a SaveFailed event) and tries again.
            }
        }
    }

    // Setting up the system's certificate check and opening the core can each fail without
    // being a bug in the app (the library did not load, the platform refused), so a failure is
    // an answer for the screen to show and never a crash.
    @Suppress("TooGenericExceptionCaught")
    private fun start(): Started =
        try {
            Bridge.initVerifier(applicationContext)
            Started.Ready(Core.open(
                AndroidPlatform(applicationContext, handback, foreground::get),
                // Private and left out of every backup (the manifest's rules), as the mail is private.
                File(noBackupFilesDir, "mailbox").path,
            ).also {
                createChannels(applicationContext, channelSpecs(it::t))
            })
        } catch (_: CoreException) {
            Started.Failed
        } catch (_: LinkageError) {
            Started.Failed
        } catch (_: RuntimeException) {
            Started.Failed
        }
}
