// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Tests the reading pane's logic against invented messages. */
class ReadingControllerTest {
    private fun message(): OpenedMessage {
        val view = Corpus.plain.view.copy(id = 7uL, unread = true)
        return OpenedMessage(view, emptyMap())
    }

    // Unconfined: the fake source never suspends for real, so every launch runs to its end where it is made.
    private fun rig(opened: OpenedMessage? = message()): Pair<FakeReadSource, ReadingController> {
        val source = FakeReadSource(opened)
        return source to ReadingController(source, CoroutineScope(Dispatchers.Unconfined))
    }

    @Test
    fun aMessageOpensAndIsMarkedReadOnce() {
        val (source, c) = rig()
        c.open(7uL, 360, dark = false)
        assertEquals(LoadState.Ready, c.ui.value.load)
        assertEquals(listOf(Triple(7uL, 360, false)), source.opened)
        assertEquals(listOf(7uL), source.marked)
        c.open(7uL, 400, dark = true)
        assertEquals(listOf(7uL), source.marked)
    }

    @Test
    fun aMessageThatCannotBeReadIsFailedAndClosingForgetsIt() {
        val (_, c) = rig(null)
        c.open(7uL, 360, dark = false)
        assertEquals(LoadState.Failed, c.ui.value.load)
        c.close()
        assertNull(c.ui.value.id)
        assertNull(c.ui.value.opened)
    }
}
