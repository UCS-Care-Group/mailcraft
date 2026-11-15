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
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** The Rust core and what the app built on it, once both are ready. */
internal class Opened(val core: Core, val hub: AccountsHub, val mail: MailListController, val settings: SettingsController)

/** What starting the Rust core has come to: under way, the core, or why there is none. */
internal typealias Started = Phase<Opened>

/** Holds the one [Core] for the life of the process. */
class MailCraftApplication : Application() {
    /** Where the sign-in's redirect lands ([RedirectActivity]). */
    internal val handback by lazy { SignInHandback(PendingSignIn(File(noBackupFilesDir, "pending-signin"))) }
    private val foreground = ForegroundActivity()

    /** Outlives every window: the sign-in, the account work and the save in `onStop` run here. */
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** The one reader of the core's events (a window never reads them itself). */
    private val events = EventDispatcher()

    /** Opening the core: off the main thread, and nothing touches the core before it is ready. */
    private val startup = Startup<Opened>(scope)

    /** Where starting the core stands; [begin] it first. */
    internal val started: StateFlow<Started> get() = startup.state

    override fun onCreate() {
        super.onCreate()
        registerActivityLifecycleCallbacks(object : ActivityLifecycleCallbacks {
            override fun onActivityResumed(activity: Activity) {
                foreground.resumed(activity)
                // The owner may have turned notifications on or off in the system's settings meanwhile.
                followNotificationSwitch()
            }

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

    /** Starts the core, the first time a window asks; later calls change nothing. */
    internal fun begin() {
        startup.begin(::open, ::wire)
    }

    /**
     * Tells the core whether new mail is to raise notifications: the system's switch for the app and
     * for its mail channel, and the right to notify, are the owner's choice, and the core builds no
     * notification the owner turned off.
     */
    internal fun followNotificationSwitch() {
        val ready = startup.readyOrNull() ?: return
        runCatching { ready.core.setNotifyNewMail(mailNotificationsAllowed(applicationContext)) }
    }

    /**
     * The mailbox is written when no window is on screen: the system may end the process after it.
     * (A core still opening has nothing of the owner's to write yet.)
     */
    internal fun saveNow() {
        val ready = startup.readyOrNull() ?: return
        // (A failure is shown and tried again; the core says so itself too, with a SaveFailed event.)
        scope.launch { ready.hub.saveNow() }
    }

    // Setting up the system's certificate check and opening the core can each fail without
    // being a bug in the app (the library did not load, the platform refused), so a failure is
    // an answer for the screen to show and never a crash. Runs on a background thread.
    @Suppress("TooGenericExceptionCaught")
    private fun open(): Opened? =
        try {
            Bridge.initVerifier(applicationContext)
            // The core keeps the mailbox itself: opened from here (a phone that has one keeps it),
            // saved after a change, at `saveNow` and when it ends. Private and left out of every
            // backup (the manifest's rules), as the mail is private.
            val core = Core.open(
                AndroidPlatform(applicationContext, handback, foreground::get),
                File(noBackupFilesDir, "mailbox").path,
            )
            createChannels(applicationContext, channelSpecs(core::t))
            val hub = AccountsHub(
                api = CoreAccounts(core, handback),
                texts = CoreTexts(core),
                scope = scope,
                save = { core.saveNow() },
                copy = { text -> copyToClipboard(applicationContext, text) },
                sync = { core.syncNow() },
            )
            val settings = SettingsController(
                api = CoreSettings(
                    core = core,
                    store = DeviceSwitches(applicationContext),
                    dir = noBackupFilesDir,
                    keep = { hub.saveNow() },
                ),
                scope = scope,
            )
            Opened(core, hub, MailListController(CoreMailSource(core)), settings)
        } catch (_: CoreException) {
            null
        } catch (_: LinkageError) {
            null
        } catch (_: RuntimeException) {
            null
        }

    /** Wires the core that was opened to the rest of the app, on the main thread, before anything else sees it. */
    @Suppress("TooGenericExceptionCaught")
    private fun wire(opened: Opened): Boolean =
        try {
            // The address the sign-in's page comes back to goes to the core as it is.
            handback.sink = opened.core::handBackRedirect
            handback.interrupted = opened.hub.controller::signInInterrupted
            events.register(opened.hub::handle)
            events.register(MailNotifier(applicationContext, opened.core::t)::handle)
            // The words of what a background round says (a notification's "New mail") are the
            // window's language even when no window has opened (the owner's saved choice first).
            opened.core.setLanguage(LanguageChoice.inUse(this))
            opened.core.setNotifyNewMail(mailNotificationsAllowed(applicationContext))
            events.register { event -> opened.mail.handle(event, scope) }
            // A pull to refresh is the hub's Send/Receive: the list shows its spinner and its word.
            opened.mail.follow(opened.hub.syncing, opened.hub.syncNotice, opened.hub::syncNoticeShown, scope)
            events.launchIn(scope) { nextEvent(opened.core) }
            // Send/Receive while the app is closed: one periodic run, kept if it is there already.
            // (WorkManager not starting is no reason to refuse the core: the window still works.)
            runCatching { scheduleBackgroundSync(applicationContext) }
            true
        } catch (_: RuntimeException) {
            false
        }
}

/** The next event of the core's stream; null when the stream is over or the core can't say. */
internal suspend fun nextEvent(core: Core): uniffi.mailcraft_mobile.Event? =
    try {
        core.nextEvent()
    } catch (_: CoreException) {
        null
    }
