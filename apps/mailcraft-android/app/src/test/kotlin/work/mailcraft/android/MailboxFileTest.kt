// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
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
}
