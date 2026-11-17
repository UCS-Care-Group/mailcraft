// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.work.BackoffPolicy
import androidx.work.NetworkType
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.SyncSummary

private class FakeTarget(
    private val fails: Boolean = false,
    private val saves: Boolean = true,
    private val gate: CompletableDeferred<Unit>? = null,
    private val started: CompletableDeferred<Unit>? = null,
) : SyncTarget {
    val budgets = mutableListOf<Long>()
    var saved = 0

    override suspend fun backgroundSync(budgetMs: Long): SyncSummary {
        budgets.add(budgetMs)
        started?.complete(Unit)
        gate?.await()
        if (fails) throw CoreException.Cancelled()
        return SyncSummary(ran = true, finished = true, newMail = 2u, errors = emptyList(), signInNeeded = false)
    }

    override suspend fun save(): Boolean {
        saved++
        return saves
    }
}

/** The system's periodic Send/Receive: one at a time, within its budget, and always saved. */
class BackgroundSyncTest {
    @Test
    fun aRoundRunsWithinItsBudgetAndTheMailboxIsSavedAfter() = runBlocking {
        val target = FakeTarget()
        assertEquals(RunOutcome.Done, runBackgroundSync(target, SYNC_BUDGET_MS, Mutex()))
        assertEquals(listOf(SYNC_BUDGET_MS), target.budgets)
        assertEquals(1, target.saved)
    }

    @Test
    fun theBudgetEndsWellBeforeTheSystemStopsAWorker() {
        // The system gives a worker ten minutes; the round must end, and the save be done, before.
        assertTrue(SYNC_BUDGET_MS in 1L..TimeUnit.MINUTES.toMillis(8))
    }

    @Test
    fun aRoundTheCoreRefusesIsTriedAgainAndWhatItAppliedIsStillSaved() = runBlocking {
        val target = FakeTarget(fails = true)
        assertEquals(RunOutcome.Retry, runBackgroundSync(target, SYNC_BUDGET_MS, Mutex()))
        assertEquals(1, target.saved)
    }

    @Test
    fun aSaveThatFailsHasTheRunTriedAgain() = runBlocking {
        assertEquals(RunOutcome.Retry, runBackgroundSync(FakeTarget(saves = false), SYNC_BUDGET_MS, Mutex()))
    }

    @Test
    fun noCoreIsSaidAndNothingIsRun() = runBlocking {
        assertEquals(RunOutcome.NoCore, runBackgroundSync(null, SYNC_BUDGET_MS, Mutex()))
    }

    @Test
    fun aSecondRunWhileOneIsUnderWayLeavesItToFinish() = runBlocking {
        val lock = Mutex()
        val gate = CompletableDeferred<Unit>()
        val started = CompletableDeferred<Unit>()
        val first = FakeTarget(gate = gate, started = started)
        val second = FakeTarget()
        val running = CoroutineScope(Dispatchers.Default).async { runBackgroundSync(first, SYNC_BUDGET_MS, lock) }
        // The first round has begun (and holds the lock) before the second is asked for.
        withTimeout(10_000) { started.await() }
        assertEquals(RunOutcome.Busy, runBackgroundSync(second, SYNC_BUDGET_MS, lock))
        assertTrue(second.budgets.isEmpty())
        assertEquals(0, second.saved)
        gate.complete(Unit)
        assertEquals(RunOutcome.Done, running.await())
        // The lock is given back: the next run runs.
        assertFalse(lock.isLocked)
        assertEquals(RunOutcome.Done, runBackgroundSync(second, SYNC_BUDGET_MS, lock))
    }

    @Test
    fun theRequestWaitsForANetworkAndABatteryThatIsNotLow() {
        val request = syncRequest()
        val spec = request.workSpec
        assertEquals(NetworkType.CONNECTED, spec.constraints.requiredNetworkType)
        assertTrue(spec.constraints.requiresBatteryNotLow())
        assertTrue(spec.isPeriodic)
        assertEquals(TimeUnit.MINUTES.toMillis(SYNC_EVERY_MINUTES), spec.intervalDuration)
        assertEquals(BackoffPolicy.EXPONENTIAL, spec.backoffPolicy)
        assertEquals(SyncWorker::class.java.name, spec.workerClassName)
    }
}
