// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.CoreException

/** The holds on new accounts' first syncs: kept on the phone, said to the core whenever it starts. */
class FirstSyncHoldTest {
    private val kept = MemoryHolds()
    private val told = mutableListOf<Pair<ULong, Boolean>>()
    private var refuse: CoreException? = null
    private val holds = FirstSyncHolds(kept) { account, held ->
        refuse?.let { throw it }
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
        holds.reapply()
        holds.releaseAll()
        assertTrue(told.isEmpty())
    }

    @Test
    fun releasingAllForgetsEveryHold() = runBlocking {
        kept.save(setOf(7uL, 8uL))
        holds.releaseAll()
        assertTrue(kept.load().isEmpty())
        holds.reapply()
        assertTrue(told.isEmpty())
    }

    @Test
    fun anAccountTheCoreNoLongerHasIsForgotten() = runBlocking {
        kept.save(setOf(7uL))
        refuse = CoreException.BadRequest("there is no such account")
        holds.reapply()
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
}
