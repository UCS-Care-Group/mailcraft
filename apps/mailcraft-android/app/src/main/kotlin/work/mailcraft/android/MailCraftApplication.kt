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
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** What starting the Rust core came to: the core, or why there is none. */
internal sealed interface Started {
    class Ready(val core: Core, val hub: AccountsHub) : Started

    data object Failed : Started
}

/** Holds the one [Core] for the life of the process. */
class MailCraftApplication : Application() {
    /** Where the sign-in's redirect lands ([RedirectActivity]). */
    internal val handback by lazy { SignInHandback(PendingSignIn(File(noBackupFilesDir, "pending-signin"))) }
    private val foreground = ForegroundActivity()

    /** Outlives every window: the sign-in and the account work run here, not in a screen's scope. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** The one reader of the core's events (a window never reads them itself). */
    private val events = EventDispatcher()

    internal val started: Started by lazy { start() }

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(object : ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) = foreground.resumed(activity)

            override fun onActivityPaused(activity: Activity) = foreground.paused(activity)

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit

            override fun onActivityStarted(activity: Activity) = Unit

            override fun onActivityStopped(activity: Activity) = Unit

            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit

            override fun onActivityDestroyed(activity: Activity) = Unit
        })
        // Copies of attachments written for other programs to open are kept a day, not for good.
        runCatching { OpenedFiles(this).clearOld() }
    }

    // Setting up the system's certificate check and opening the core can each fail without
    // being a bug in the app (the library did not load, the platform refused), so a failure is
    // an answer for the screen to show and never a crash.
    @Suppress("TooGenericExceptionCaught")
    private fun start(): Started =
        try {
            Bridge.initVerifier(applicationContext)
            val core = Core(AndroidPlatform(applicationContext, handback, foreground::get))
            createChannels(applicationContext, channelSpecs(core::t))
            // The address the sign-in's page comes back to goes to the core as it is.
            handback.sink = core::handBackRedirect
            val store = MailboxFile(File(noBackupFilesDir, "mailbox"))
            val hub = AccountsHub(
                api = CoreAccounts(core, handback),
                texts = CoreTexts(core),
                scope = scope,
                restore = { store.restore(core) },
                keep = { store.keep(core) },
                copy = { text -> copyToClipboard(applicationContext, text) },
                savingOff = store.savingOff,
            )
            handback.interrupted = hub.controller::signInInterrupted
            events.register(hub::handle)
            events.launchIn(scope) { nextEvent(core) }
            Started.Ready(core, hub)
        } catch (_: CoreException) {
            Started.Failed
        } catch (_: LinkageError) {
            Started.Failed
        } catch (_: RuntimeException) {
            Started.Failed
        }
}

/** The next event of the core's stream; null when the stream is over or the core can't say. */
internal suspend fun nextEvent(core: Core): uniffi.mailcraft_mobile.Event? =
    try {
        core.nextEvent()
    } catch (_: CoreException) {
        null
    }
