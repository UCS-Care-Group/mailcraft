// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MailListModelTest {
    private fun window(rev: ULong, total: Int, offset: Int, n: Int) =
        RowsWindow(rev, total, offset, (offset until offset + n).map { FakeMailSource.row(it + 1) })

    @Test
    fun aSnapshotHoldsOnlyTheRowsItWasGiven() {
        val s = ListSnapshot(1uL, 200).with(window(1uL, 200, 50, PAGE_SIZE))!!
        assertNull(s.row(0))
        assertEquals("Subject 51", s.row(50)?.subject)
        assertEquals("Subject 100", s.row(99)?.subject)
        assertNull(s.row(100))
        assertNull(s.row(-1))
        assertNull(s.row(Int.MAX_VALUE))
    }

    @Test
    fun anOlderWindowIsDroppedAndANewerOneReplacesEverything() {
        val s = ListSnapshot.Empty.with(window(5uL, 100, 0, PAGE_SIZE))!!
        assertNull(s.with(window(4uL, 100, 50, PAGE_SIZE)))
        val newer = s.with(window(6uL, 90, 50, PAGE_SIZE))!!
        assertEquals(setOf(1), newer.heldPages)
        assertEquals(90, newer.total)
    }

    @Test
    fun aWindowNotOnAPageBoundaryIsNotKept() {
        val s = ListSnapshot.Empty.with(window(1uL, 100, 7, 10))!!
        assertTrue(s.heldPages.isEmpty())
    }

    @Test
    fun missingPagesAreThoseNotHeldAndNeverBeyondTheList() {
        val s = ListSnapshot.Empty.with(window(1uL, 120, 0, PAGE_SIZE))!!
        assertEquals(listOf(1, 2), s.missing(0, 1_000_000))
        assertEquals(emptyList<Int>(), s.missing(0, 10))
        assertEquals(emptyList<Int>(), ListSnapshot.Empty.missing(0, 10))
        assertEquals(listOf(0), ListSnapshot.pagesOf(-5, 3, 10))
    }

    @Test
    fun theQuerySurvivesBeingSavedAndJunkBecomesTheStart() {
        val q = MailQuery(7uL, ListFilter.Flagged, "social", true, "zebra", true)
        val saved = listOf<Any?>(7L, ListFilter.Flagged.ordinal, "social", true, "zebra", true)
        assertEquals(q, MailQuery.fromSaved(saved))
        assertEquals(MailQuery(0uL), MailQuery.fromSaved(listOf("x", 99, 5, "no", null)))
        assertEquals(MailQuery(0uL), MailQuery.fromSaved(emptyList()))
        assertEquals(0uL, MailQuery.fromSaved(listOf(-3L)).folder)
    }

    @Test
    fun theSwipesAreReadFromTheirCodesAndJunkIsTheDefault() {
        assertEquals(SwipeConfig(SwipeAction.Junk, SwipeAction.ToggleFlag), SwipeConfig.fromCodes("junk", "toggleFlag"))
        assertEquals(SwipeConfig(), SwipeConfig.fromCodes("nonsense", null))
        assertEquals(SwipeAction.Archive, SwipeConfig().start)
        assertEquals(SwipeAction.Delete, SwipeConfig().end)
        val prefs = MemorySwipePrefs()
        prefs.save(SwipeConfig(SwipeAction.ToggleRead, SwipeAction.Archive))
        assertEquals(SwipeAction.ToggleRead, prefs.load().start)
    }

    @Test
    fun aSwipeSaysWhatItWillDoToThisRow() {
        val unread = FakeMailSource.row(2)
        val read = FakeMailSource.row(1)
        assertEquals("Mark as read", swipeLabel(EnglishTexts, SwipeAction.ToggleRead, unread))
        assertEquals("Mark as unread", swipeLabel(EnglishTexts, SwipeAction.ToggleRead, read))
        assertEquals("Flag", swipeLabel(EnglishTexts, SwipeAction.ToggleFlag, read))
        assertEquals("Clear flag", swipeLabel(EnglishTexts, SwipeAction.ToggleFlag, read.copy(flagged = true)))
        assertEquals("Archive", swipeLabel(EnglishTexts, SwipeAction.Archive, read))
    }

    @Test
    fun noticesAreWordedAndOnlyDestructiveOnesOfferUndo() {
        assertEquals("1 message deleted", noticeText(EnglishTexts, Notice.Done(Triage.Delete, 1)))
        assertEquals("3 messages archived", noticeText(EnglishTexts, Notice.Done(Triage.Archive, 3)))
        assertTrue(Notice.Done(Triage.MoveTo(1uL), 1).undoable)
        assertEquals(false, Notice.Done(Triage.Flag(true), 1).undoable)
        assertEquals(false, Notice.NoAccount.undoable)
    }

    @Test
    fun rowsAreDescribedForAScreenReader() {
        val row = FakeMailSource.row(2, members = 3).copy(flagged = true, hasAttachments = true)
        assertEquals(
            "Unread, Flagged, Has attachments, 3 messages in the conversation",
            rowDescription(EnglishTexts, row),
        )
        assertEquals("", rowDescription(EnglishTexts, FakeMailSource.row(1)))
    }
}
