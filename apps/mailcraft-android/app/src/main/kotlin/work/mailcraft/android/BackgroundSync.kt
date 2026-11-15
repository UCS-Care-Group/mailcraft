// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.NetworkType
import androidx.work.PeriodicWorkRequest
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.sync.Mutex
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.SyncSummary

/** The one periodic Send/Receive the system runs while the app is closed (unique by this name). */
internal const val SYNC_WORK = "mailcraft-background-sync"

/** How often the system is asked to run it (its shortest period); it may run later (Doze, low battery, no network). */
internal const val SYNC_EVERY_MINUTES = 15L

/** A failed run is tried again after this, doubling each time (the system caps it at five hours). */
internal const val SYNC_BACKOFF_MINUTES = 5L

/**
 * The time a run gives the core: well inside the ten minutes the system gives a worker, so that the
 * round is called off at a safe place and the mailbox saved before the system stops the worker.
 */
internal const val SYNC_BUDGET_MS = 4L * 60L * 1000L

/**
 * The request: every [SYNC_EVERY_MINUTES], only with a network and a battery that isn't low (the
 * system's own constraints; the core says "not now" itself on a metered or data-saving network it
 * is told of). Nothing of Google Play Services: WorkManager runs on the system's JobScheduler.
 */
internal fun syncRequest(): PeriodicWorkRequest =
    PeriodicWorkRequestBuilder<SyncWorker>(SYNC_EVERY_MINUTES, TimeUnit.MINUTES)
        .setConstraints(
            Constraints.Builder()
                .setRequiredNetworkType(NetworkType.CONNECTED)
                .setRequiresBatteryNotLow(true)
                .build(),
        )
        .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, SYNC_BACKOFF_MINUTES, TimeUnit.MINUTES)
        .build()

/**
 * Asks the system to run the background Send/Receive. Unique by [SYNC_WORK] and kept when it is
 * there already, so calling it at every start never makes a second one nor moves the first.
 */
internal fun scheduleBackgroundSync(context: Context) {
    WorkManager.getInstance(context)
        .enqueueUniquePeriodicWork(SYNC_WORK, ExistingPeriodicWorkPolicy.KEEP, syncRequest())
}

/** What a background run needs of the core: one round within a budget, and a save. */
internal interface SyncTarget {
    /** One round to its end or until [budgetMs] runs out; throws [CoreException] when it can't. */
    suspend fun backgroundSync(budgetMs: Long): SyncSummary

    /** Writes the mailbox; true when it is on disk. */
    suspend fun save(): Boolean
}

/** What a background run came to, for the system's scheduler. */
internal enum class RunOutcome {
    /** The round ran (or had nothing to do) and the mailbox is saved. */
    Done,

    /** It failed in a way that may pass (the core said no, the save failed): try again later. */
    Retry,

    /** There is no core to run (the library didn't load, the mailbox couldn't be opened). */
    NoCore,

    /** Another run is under way in this process; this one leaves it to finish. */
    Busy,
}

/** One background run at a time in the process, whoever asks for it. */
private val running = Mutex()

/**
 * One background run: the round within [budgetMs], then a save whatever the round came to (what
 * it applied before failing is the owner's mail too). Never throws for what the core says.
 */
internal suspend fun runBackgroundSync(target: SyncTarget?, budgetMs: Long, lock: Mutex = running): RunOutcome {
    if (target == null) return RunOutcome.NoCore
    if (!lock.tryLock()) return RunOutcome.Busy
    return try {
        val synced = try {
            target.backgroundSync(budgetMs)
            true
        } catch (_: CoreException) {
            false
        }
        val saved = target.save()
        if (synced && saved) RunOutcome.Done else RunOutcome.Retry
    } finally {
        lock.unlock()
    }
}

/** [SyncTarget] over the app's one core and the hub that saves it. */
internal class CoreSyncTarget(private val core: Core, private val hub: AccountsHub) : SyncTarget {
    override suspend fun backgroundSync(budgetMs: Long): SyncSummary =
        core.backgroundSync(budgetMs.coerceAtLeast(0L).toULong())

    override suspend fun save(): Boolean = hub.saveNow()
}

/**
 * The system's worker. It never opens a core of its own: it asks the application for the one core
 * of the process (opening it, off the main thread, when no window has), so that two cores never
 * share the mailbox's folder, and the events the round raises (new mail, a sign-in to redo) reach
 * the same handlers a window's would.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = applicationContext as? MailCraftApplication ?: return Result.failure()
        return when (runBackgroundSync(app.syncTarget(), SYNC_BUDGET_MS)) {
            RunOutcome.Done, RunOutcome.Busy -> Result.success()
            RunOutcome.Retry -> Result.retry()
            // (A periodic run that fails is run again at the next period; nothing is lost.)
            RunOutcome.NoCore -> Result.failure()
        }
    }
}

/** The application's one core as a [SyncTarget], once it is open; null when it couldn't be. */
internal suspend fun MailCraftApplication.syncTarget(): SyncTarget? {
    begin()
    val phase = started.first { it !is Phase.Loading }
    val opened = (phase as? Phase.Ready)?.value ?: return null
    return CoreSyncTarget(opened.core, opened.hub)
}
