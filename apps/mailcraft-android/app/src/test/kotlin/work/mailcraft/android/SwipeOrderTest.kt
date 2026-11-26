// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The messages a swipe moves between, and when a message swiped to counts as read; invented rows only. */
class SwipeOrderTest {
    private fun list(total: Int, rows: List<InboxRow> = (1..total).map { FakeMailSource.row(it) }) =
        checkNotNull(ListSnapshot.Empty.with(RowsWindow(1uL, total, 0, rows)))

    @Test
    fun nextIsTheRowBelowAndPreviousTheRowAbove() {
        val n = SwipeOrder().around(list(5), 3uL)
        assertEquals(2, n.at)
        assertEquals(2uL, n.previous?.id)
        assertEquals(4uL, n.next?.id)
    }

    @Test
    fun theEndsOfTheListHaveNoNeighbourThere() {
        assertNull(SwipeOrder().around(list(5), 1uL).previous)
        assertNull(SwipeOrder().around(list(5), 5uL).next)
    }

    @Test
    fun aFilteredListIsWalkedInItsOwnOrder() {
        // (Say the Unread filter left these three, in this order.)
        val rows = listOf(9, 4, 7).map { FakeMailSource.row(it) }
        val n = SwipeOrder().around(list(3, rows), 4uL)
        assertEquals(9uL, n.previous?.id)
        assertEquals(7uL, n.next?.id)
    }

    @Test
    fun aConversationIsOneStopAndItsOtherMessagesAreFoundToo() {
        val rows = listOf(
            FakeMailSource.row(1),
            FakeMailSource.row(2).copy(members = 3, memberIds = listOf(2uL, 20uL, 21uL)),
            FakeMailSource.row(3),
        )
        val n = SwipeOrder().around(list(3, rows), 20uL)
        assertEquals(1uL, n.previous?.id)
        assertEquals(3uL, n.next?.id)
    }

    @Test
    fun aMessageThatLeftTheListKeepsItsPlace() {
        val order = SwipeOrder()
        order.around(list(4), 2uL)
        // Reading it marked it read and the Unread filter dropped it: row 3 is now where it was.
        val after = list(3, listOf(1, 3, 4).map { FakeMailSource.row(it) })
        val n = order.around(after, 2uL)
        assertEquals(1uL, n.previous?.id)
        assertEquals(3uL, n.next?.id)
    }

    @Test
    fun aRowNotReadYetGivesNoNeighbourRatherThanAWrongOne() {
        val n = SwipeOrder().around(list(200, (1..50).map { FakeMailSource.row(it) }), 50uL)
        assertEquals(49uL, n.previous?.id)
        assertNull(n.next)
    }

    @Test
    fun aMessageSwipedToIsMarkedReadOnlyWhenItStaysOpen() {
        val dir: File = Files.createTempDirectory("swipe").toFile()
        try {
            val view = Corpus.plain.view.copy(id = 7uL, unread = true)
            val facts = MessageFacts("a@example.org", false, true, false)
            val source = FakeReadSource(OpenedMessage(view, facts, emptyMap()))
            val gate = CompletableDeferred<Unit>()
            val c = ReadingController(
                source, PictureCache(dir, SniffingCheck), MemoryPicturePrefs(PicturePrefs()),
                CoroutineScope(Dispatchers.Unconfined), Dispatchers.Unconfined,
            ) { gate.await() }
            c.open(7uL, 360, dark = false, markAfterMs = SWIPE_READ_MS)
            assertEquals(emptyList<ULong>(), source.marked)
            // Passed by: the message is closed before the wait is over.
            c.close(7uL)
            gate.complete(Unit)
            assertEquals(emptyList<ULong>(), source.marked)
            // Stayed: marked once the wait is over.
            c.open(7uL, 360, dark = false, markAfterMs = SWIPE_READ_MS)
            assertEquals(listOf(7uL), source.marked)
        } finally {
            dir.deleteRecursively()
        }
    }
}
