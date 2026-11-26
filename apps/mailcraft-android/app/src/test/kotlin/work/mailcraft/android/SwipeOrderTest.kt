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

    private fun rig(dir: File, wait: suspend (Long) -> Unit = {}): Pair<FakeReadSource, ReadingController> {
        val view = Corpus.plain.view.copy(id = 7uL, unread = true)
        val facts = MessageFacts("a@example.org", false, true, false)
        val source = FakeReadSource(OpenedMessage(view, facts, emptyMap()))
        val c = ReadingController(
            source, PictureCache(dir, SniffingCheck), MemoryPicturePrefs(PicturePrefs()),
            CoroutineScope(Dispatchers.Unconfined), Dispatchers.Unconfined, wait,
        )
        return source to c
    }

    @Test
    fun eachMarkReadChoiceWaitsAsLongAsItSaysForATapAndASwipe() {
        // (A swipe waits at least the minimum even when the choice is to mark at once.)
        val table = mapOf(
            MarkRead.WhenOpened to (0L to SWIPE_READ_MS),
            MarkRead.After2 to (2_000L to 2_000L),
            MarkRead.After5 to (5_000L to 5_000L),
            MarkRead.Never to (null to null),
        )
        for ((choice, want) in table) {
            assertEquals(choice.toString(), want.first, choice.delayMs(swiped = false))
            assertEquals(choice.toString(), want.second, choice.delayMs(swiped = true))
        }
    }

    @Test
    fun prefsFromAnOlderVersionMarkWhenOpened() {
        assertEquals(MarkRead.WhenOpened, readingPrefsFrom { _, default -> default }.markRead)
        val stored = readingPrefsFrom({ if (it == "mark_read") "5s" else null }) { _, d -> d }
        assertEquals(MarkRead.After5, stored.markRead)
        assertEquals(MarkRead.WhenOpened, readingPrefsFrom({ "garbage" }) { _, d -> d }.markRead)
    }

    @Test
    fun aTapMarksAtOnceANeverOpensWithoutMarkingAndAWaitMarksOnlyAfterIt() {
        val dir: File = Files.createTempDirectory("mark").toFile()
        try {
            val (tapped, c1) = rig(dir)
            c1.open(7uL, 360, dark = false, markAfterMs = MarkRead.WhenOpened.delayMs(swiped = false))
            assertEquals(listOf(7uL), tapped.marked)
            val (never, c2) = rig(dir)
            c2.open(7uL, 360, dark = false, markAfterMs = MarkRead.Never.delayMs(swiped = true))
            assertEquals(emptyList<ULong>(), never.marked)
            val gate = CompletableDeferred<Unit>()
            val (waited, c3) = rig(dir) { gate.await() }
            c3.open(7uL, 360, dark = false, markAfterMs = MarkRead.After2.delayMs(swiped = false))
            assertEquals(emptyList<ULong>(), waited.marked)
            gate.complete(Unit)
            assertEquals(listOf(7uL), waited.marked)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun neighboursAreReadAheadWithoutBeingOpenedAndNoMoreThanTwoAreKept() {
        val dir: File = Files.createTempDirectory("near").toFile()
        try {
            val (source, c) = rig(dir)
            c.open(7uL, 360, dark = false)
            c.preload(listOf(6uL, 8uL, 9uL), 360, dark = false)
            assertEquals(setOf(6uL, 8uL), c.ui.value.near.keys)
            // Read ahead is reading only: nothing is marked read.
            assertEquals(listOf(7uL), source.marked)
            // Moving on lets go of the far one and keeps the new neighbours.
            c.preload(listOf(8uL, 10uL), 360, dark = false)
            assertEquals(setOf(8uL, 10uL), c.ui.value.near.keys)
            // A different width means what was read ahead is not what would be drawn.
            c.preload(listOf(8uL), 400, dark = false)
            assertEquals(setOf(8uL), c.ui.value.near.keys)
            // Opening a neighbour uses what was read ahead: the source is not asked for it again.
            val asked = source.opened.size
            c.open(8uL, 400, dark = false)
            assertEquals(asked, source.opened.size)
            assertEquals(8uL, c.ui.value.id)
            assertEquals(LoadState.Ready, c.ui.value.load)
            assertEquals(emptyMap<ULong, OpenedMessage>(), c.ui.value.near)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun aReadStateSetByHandWhileWaitingIsNotUndoneByTheTimer() {
        val dir: File = Files.createTempDirectory("hand").toFile()
        try {
            val gate = CompletableDeferred<Unit>()
            val (source, c) = rig(dir) { gate.await() }
            c.open(7uL, 360, dark = false, markAfterMs = MarkRead.After5.delayMs(swiped = false))
            // The owner marks it read and then unread from the toolbar before the wait is over.
            c.triaged(7uL, unread = false)
            c.triaged(7uL, unread = true)
            gate.complete(Unit)
            assertEquals(emptyList<ULong>(), source.marked)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun goingOnByASwipeKeepsWhatWasReadAheadAndLeavingForTheListDropsIt() {
        val dir: File = Files.createTempDirectory("onward").toFile()
        try {
            val (_, c) = rig(dir)
            c.open(7uL, 360, dark = false)
            c.preload(listOf(6uL, 8uL), 360, dark = false)
            c.close(7uL, onward = true)
            assertEquals(setOf(6uL, 8uL), c.ui.value.near.keys)
            c.open(8uL, 360, dark = false)
            c.close(8uL)
            assertEquals(emptyMap<ULong, OpenedMessage>(), c.ui.value.near)
        } finally {
            dir.deleteRecursively()
        }
    }
}
