// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.SyncAnswer

/** The holds on new accounts' first syncs: kept on the phone, said to the core whenever it starts. */
class FirstSyncHoldTest {
    private val kept = MemoryHolds()
    private val told = mutableListOf<Pair<ULong, Boolean>>()
    private var refuse: CoreException? = null
    private var refuseOnly: Set<ULong>? = null
    private val holds = FirstSyncHolds(kept) { account, held ->
        refuse?.takeIf { refuseOnly?.contains(account) ?: true }?.let { throw it }
        told.add(account to held)
    }

    @Test
    fun notNowHoldsTheAccountAndAYesLetsItGo() = runBlocking {
        holds.answered(7u, yes = false)
        holds.answered(8u, yes = false)
        assertEquals(setOf(7uL, 8uL), kept.load())
        holds.answered(7u, yes = true)
        assertEquals(setOf(8uL), holds.held())
        assertEquals(listOf(7uL to true, 8uL to true, 7uL to false), told)
    }

    @Test
    fun aCoreThatStartsIsToldEveryHoldAgain() = runBlocking {
        kept.save(setOf(7uL, 8uL))
        holds.reapply()
        assertEquals(setOf(7uL to true, 8uL to true), told.toSet())
        assertEquals(2, told.size)
    }

    @Test
    fun nothingHeldTellsTheCoreNothing() = runBlocking {
        assertNull(holds.reapply())
        holds.ownerRound { SyncAnswer(started = true, message = "") }
        assertTrue(told.isEmpty())
    }

    @Test
    fun aRoundTheOwnerAskedForForgetsEveryHold() = runBlocking {
        kept.save(setOf(7uL, 8uL))
        holds.ownerRound { SyncAnswer(started = true, message = "") }
        assertTrue(kept.load().isEmpty())
        holds.reapply()
        assertTrue(told.isEmpty())
    }

    @Test
    fun anAccountTheCoreNoLongerHasIsForgotten() = runBlocking {
        kept.save(setOf(7uL))
        refuse = CoreException.BadRequest("there is no such account")
        // (Not a failure: there is nothing left to hold.)
        assertNull(holds.reapply())
        assertTrue(kept.load().isEmpty())
    }

    @Test
    fun aFailureThatMayPassKeepsTheHoldForTheNextStart() = runBlocking {
        refuse = CoreException.Failed("busy")
        holds.answered(7u, yes = false)
        assertEquals(setOf(7uL), kept.load())
        refuse = null
        holds.reapply()
        assertEquals(listOf(7uL to true), told)
    }

    @Test
    fun aRoundThatDidNotStartKeepsEveryHold() = runBlocking {
        kept.save(setOf(7uL))
        val answer = holds.ownerRound { SyncAnswer(started = false, message = "No network") }
        assertFalse(answer.started)
        assertEquals(setOf(7uL), kept.load())
    }

    @Test
    fun aNotNowAnsweredWhileTheOwnersRoundIsAskedForOutlivesIt() = runBlocking {
        val asking = CompletableDeferred<Unit>()
        val begun = CompletableDeferred<Unit>()
        val round = launch {
            holds.ownerRound {
                asking.complete(Unit)
                begun.await()
                SyncAnswer(started = true, message = "")
            }
        }
        asking.await()
        val answer = launch { holds.answered(7u, yes = false) }
        yield()
        // The answer waits for the round to have begun (and the core to have let go of the rest)...
        assertTrue(told.isEmpty())
        begun.complete(Unit)
        round.join()
        answer.join()
        // ...so it is said after, and kept, as the core keeps it.
        assertEquals(listOf(7uL to true), told)
        assertEquals(setOf(7uL), kept.load())
    }

    @Test
    fun aHoldThatCannotBeSaidAgainIsReportedAfterTheRestAreTried() = runBlocking {
        kept.save(setOf(7uL, 8uL, 9uL))
        val busy = CoreException.Failed("busy")
        refuse = busy
        refuseOnly = setOf(8uL)
        assertSame(busy, holds.reapply())
        assertEquals(setOf(7uL to true, 9uL to true), told.toSet())
        assertEquals(setOf(7uL, 8uL, 9uL), kept.load())
    }

    @Test
    fun aRoundThatStartsByItselfWaitsForEveryHold() = runBlocking {
        kept.save(setOf(7uL))
        refuse = CoreException.Failed("busy")
        var rounds = 0
        val thrown = runCatching { afterHolds(holds) { rounds++ } }.exceptionOrNull()
        assertTrue(thrown is CoreException.Failed)
        assertEquals(0, rounds)
        refuse = null
        afterHolds(holds) { rounds++ }
        assertEquals(1, rounds)
        assertEquals(listOf(7uL to true), told)
    }

    @Test
    fun aRoundRunsWhenNothingIsHeldOrTheOnlyHoldIsGone() = runBlocking {
        var rounds = 0
        afterHolds(null) { rounds++ }
        afterHolds(holds) { rounds++ }
        kept.save(setOf(7uL))
        refuse = CoreException.BadRequest("there is no such account")
        afterHolds(holds) { rounds++ }
        assertEquals(3, rounds)
    }

    @Test
    fun removingTheDataForgetsEveryHoldAndAFailedRemovalNone() = runBlocking {
        kept.save(setOf(7uL, 8uL))
        assertNull(holds.removing<String> { null })
        assertEquals(setOf(7uL, 8uL), kept.load())
        assertEquals("removed", holds.removing { "removed" })
        assertTrue(kept.load().isEmpty())
        assertTrue(told.isEmpty())
    }
}
