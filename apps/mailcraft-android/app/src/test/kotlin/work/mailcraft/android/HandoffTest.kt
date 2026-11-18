// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** A dispatcher that runs nothing until the test says which read finishes. */
private class Held : CoroutineDispatcher() {
    val tasks = ArrayList<Runnable>()

    override fun dispatch(context: CoroutineContext, block: Runnable) {
        tasks.add(block)
    }
}

class HandoffTest {
    private val io = Held()
    private val remembered = ArrayList<Boolean>()
    private fun handoff(pending: Boolean = false) =
        Handoff(CoroutineScope(Dispatchers.Unconfined), io, pending) { remembered.add(it) }

    private fun got(name: String) = IcsRead.Got(IcsFile(name, ByteArray(1)))

    @Test
    fun aFileIsReadOffTheWindowsThreadAndKeptUntilTakenIn() {
        val h = handoff()
        val file = got("a.ics")
        h.take { file }
        // Nothing is read until the reading thread runs it.
        assertNull(h.file.value)
        assertTrue(h.unfinished)
        io.tasks.removeAt(0).run()
        assertSame(file, h.file.value)
        h.handled()
        assertNull(h.file.value)
        assertFalse(h.unfinished)
        assertEquals(listOf(true, false), remembered)
    }

    @Test
    fun anEarlierReadThatComesBackLaterDoesNotReplaceALaterFile() {
        val h = handoff()
        val first = got("first.ics")
        val second = got("second.ics")
        h.take { first }
        h.take { second }
        io.tasks.removeAt(1).run()
        io.tasks.removeAt(0).run()
        assertSame(second, h.file.value)
    }

    @Test
    fun anIntentThatIsNoCalendarFileLeavesAWaitingFileAndOtherwiseNothingWaits() {
        val h = handoff()
        val file = got("a.ics")
        h.take { file }
        io.tasks.removeAt(0).run()
        h.take { IcsRead.NotOurs }
        io.tasks.removeAt(0).run()
        assertSame(file, h.file.value)
        assertTrue(h.unfinished)
        val plain = handoff()
        plain.take { IcsRead.NotOurs }
        io.tasks.removeAt(0).run()
        assertNull(plain.file.value)
        assertFalse(plain.unfinished)
    }

    @Test
    fun aFileNeverTakenInByAnEarlierWindowIsReadAgainOnce() {
        val h = handoff(pending = true)
        assertTrue(h.readAgain)
        h.take { got("a.ics") }
        assertFalse(h.readAgain)
        assertFalse(handoff(pending = false).readAgain)
    }

    @Test
    fun aReadThatThrowsIsAFileThatCouldNotBeReadAndNothingIsLeftWaiting() {
        val h = handoff()
        h.take { throw IllegalStateException("the provider went away") }
        io.tasks.removeAt(0).run()
        assertSame(IcsRead.Unreadable, h.file.value)
        h.handled()
        assertFalse(h.unfinished)
        assertEquals(listOf(true, false), remembered)
    }
}
