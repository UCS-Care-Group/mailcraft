// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.mailcraft_mobile.CoreException

class MailboxFileTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private val unreadable = CoreException.Failed("not a mailbox")

    private fun kept(dir: File, text: String = "original"): File =
        File(dir, "mailbox.mailcraft").also { it.writeText(text) }

    @Test
    fun anUnreadableMailboxIsPutAsideAndSavingGoesOn() = runBlocking {
        val dir = tmp.newFolder("a")
        val file = kept(dir)
        val store = MailboxFile(dir)
        store.restore { throw unreadable }
        assertFalse(file.exists())
        assertTrue(dir.listFiles().orEmpty().any { it.name.startsWith("mailbox.mailcraft.unreadable-") })
        assertFalse(store.savingOff.value)
        var saved = 0
        store.keep { saved++ }
        assertEquals(1, saved)
    }

    @Test
    fun anUnreadableMailboxThatCannotBePutAsideIsNeverWrittenOver() = runBlocking {
        val dir = tmp.newFolder("b")
        val file = kept(dir)
        val store = MailboxFile(dir, setAside = { _, _ -> false })
        store.restore { throw unreadable }
        assertTrue(store.savingOff.value)
        var saved = 0
        store.keep { saved++ }
        assertEquals(0, saved)
        assertEquals("original", file.readText())
    }

    @Test
    fun aMailboxThatOpensIsSavedAsAlways() = runBlocking {
        val dir = tmp.newFolder("c")
        kept(dir)
        val store = MailboxFile(dir)
        store.restore { }
        var saved = 0
        store.keep { saved++ }
        assertEquals(1, saved)
        assertFalse(store.savingOff.value)
    }

    private fun scopeOf() =
        kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Default + kotlinx.coroutines.SupervisorJob())

    @Test
    fun aFailedSaveIsShownTriedAgainAndClearedWhenItWorks() = runBlocking {
        val dir = tmp.newFolder("d")
        val scope = scopeOf()
        val store = MailboxFile(dir, scope = scope, retryAfter = listOf(1L), pause = { kotlinx.coroutines.delay(it) })
        val tries = java.util.concurrent.atomic.AtomicInteger()
        store.keep {
            if (tries.incrementAndGet() < 3) throw CoreException.Failed("disk full")
        }
        assertTrue(store.saveFailed.value || tries.get() >= 3)
        kotlinx.coroutines.withTimeout(5_000) { store.saveFailed.first { !it && tries.get() >= 3 } }
        assertEquals(3, tries.get())
        assertFalse(store.saveFailed.value)
        scope.cancel()
    }

    @Test
    fun withoutAScopeAFailedSaveWaitsForRetry() = runBlocking {
        val dir = tmp.newFolder("e")
        val store = MailboxFile(dir)
        var fail = true
        var tries = 0
        store.keep {
            tries++
            if (fail) throw CoreException.Failed("disk full")
        }
        assertTrue(store.saveFailed.value)
        fail = false
        store.keepAgain()
        assertEquals(2, tries)
        assertFalse(store.saveFailed.value)
    }

    @Test
    fun aFolderThatCannotBeMadeIsAFailedSave() = runBlocking {
        val blocker = tmp.newFile("f")
        val store = MailboxFile(File(blocker, "sub"))
        var saved = 0
        store.keep { saved++ }
        assertEquals(0, saved)
        assertTrue(store.saveFailed.value)
    }

    @Test
    fun savesNeverOverlap() = runBlocking {
        val dir = tmp.newFolder("g")
        val store = MailboxFile(dir)
        val inside = java.util.concurrent.atomic.AtomicInteger()
        val worst = java.util.concurrent.atomic.AtomicInteger()
        val jobs = List(8) {
            kotlinx.coroutines.GlobalScope.launch(kotlinx.coroutines.Dispatchers.Default) {
                store.keep {
                    worst.updateAndGet { m -> maxOf(m, inside.incrementAndGet()) }
                    kotlinx.coroutines.delay(5)
                    inside.decrementAndGet()
                }
            }
        }
        jobs.forEach { it.join() }
        assertEquals(1, worst.get())
    }
}
