// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MailListControllerTest {
    private val inbox = MailQuery(1uL)

    private fun controller(source: FakeMailSource, clock: () -> Long = { 0L }) = MailListController(source, clock)

    @Test
    fun showingAListReadsItsFirstPageAndItsTabsOnly() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(500))
        source.tabs = listOf(SplitTabItem("primary", "Primary", 3))
        val c = controller(source)
        c.show(inbox)
        val ui = c.ui.value
        assertEquals(LoadState.Ready, ui.load)
        assertEquals(500, ui.snapshot.total)
        assertEquals(listOf(0 to PAGE_SIZE), source.windows)
        assertEquals("Subject 1", ui.snapshot.row(0)?.subject)
        assertNull(ui.snapshot.row(PAGE_SIZE))
        assertEquals(listOf("primary"), ui.tabs.map { it.code })
    }

    @Test
    fun scrollingReadsTheNextPagesAndNeverAPageTwice() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(500))
        val c = controller(source)
        c.show(inbox)
        c.showRange(40, 55)
        c.showRange(40, 55)
        c.showRange(110, 120)
        val pages = source.windows.map { it.first / PAGE_SIZE }
        assertEquals(pages.distinct(), pages)
        assertNotNull(c.ui.value.snapshot.row(120))
        assertNull(c.ui.value.snapshot.row(400))
    }

    @Test
    fun aNewRevisionReadsAgainOnlyThePagesOnScreen() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(500))
        val c = controller(source)
        c.show(inbox)
        c.showRange(0, 10)
        c.showRange(260, 270)
        source.windows.clear()
        source.change(FakeMailSource.rows(499))
        c.revChanged(source.rev)
        // Only the page of rows 260 to 270 is read (and none of the others), as one batch.
        assertEquals(listOf(5 * PAGE_SIZE), source.windows.map { it.first })
        assertEquals(source.rev, c.ui.value.snapshot.rev)
        assertEquals(499, c.ui.value.snapshot.total)
        assertEquals(setOf(5), c.ui.value.snapshot.heldPages)
    }

    @Test
    fun aRevisionTheListAlreadyHasReadsNothing() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(10))
        val c = controller(source)
        c.show(inbox)
        source.windows.clear()
        c.revChanged(source.rev)
        c.revChanged(0uL)
        assertTrue(source.windows.isEmpty())
    }

    @Test
    fun anUnreadableListIsAFailureAndNotAnEmptyOne() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3)).also { it.failWindows = true }
        val c = controller(source)
        c.show(inbox)
        assertEquals(LoadState.Failed, c.ui.value.load)
    }

    @Test
    fun aListThatFailsAfterItWasShownKeepsWhatItHad() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3))
        val c = controller(source)
        c.show(inbox)
        source.failWindows = true
        source.change(FakeMailSource.rows(2))
        c.revChanged(source.rev)
        assertEquals(LoadState.Ready, c.ui.value.load)
        assertEquals(3, c.ui.value.snapshot.total)
    }

    @Test
    fun changingTheQueryStartsTheListOver() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(120))
        val c = controller(source)
        c.show(inbox)
        c.showRange(100, 110)
        c.show(inbox.copy(filter = ListFilter.Unread))
        assertEquals(setOf(0), c.ui.value.snapshot.heldPages)
        assertEquals(ListFilter.Unread, c.ui.value.query?.filter)
    }

    @Test
    fun changingTheQueryLeavesTheListReadyWithTheNewRows() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(30))
        val c = controller(source)
        c.show(inbox)
        source.rows = FakeMailSource.rows(5)
        c.show(inbox.copy(search = "x"))
        assertEquals(LoadState.Ready, c.ui.value.load)
        assertEquals(5, c.ui.value.snapshot.total)
    }

    @Test
    fun swipingDeleteRunsTheCommandOnTheRowAndOffersUndo() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(5))
        val c = controller(source)
        c.show(inbox)
        c.triage(SwipeAction.Delete.triage(FakeMailSource.row(1)), listOf(FakeMailSource.row(1)))
        assertEquals("mail.delete" to """{"ids":[1]}""", source.commands.last())
        val notice = c.ui.value.notice?.notice
        assertTrue(notice is Notice.Done && notice.undoable)
        assertEquals(4, c.ui.value.snapshot.total)
        c.undo()
        assertEquals("edit.undo", source.commands.last().first)
        assertEquals(5, c.ui.value.snapshot.total)
    }

    @Test
    fun aChangeThatTakesNothingAwayAsksForNoUndo() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3))
        val c = controller(source)
        c.show(inbox)
        c.triage(SwipeAction.ToggleRead.triage(FakeMailSource.row(2)), listOf(FakeMailSource.row(2)))
        assertEquals("mail.markRead" to """{"ids":[2]}""", source.commands.last())
        assertNull(c.ui.value.notice)
        c.triage(SwipeAction.ToggleRead.triage(FakeMailSource.row(1)), listOf(FakeMailSource.row(1)))
        assertEquals("mail.markUnread", source.commands.last().first)
    }

    @Test
    fun theTriageCommandsAreTheEnginesOwn() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3))
        val c = controller(source)
        c.show(inbox)
        val row = FakeMailSource.row(1)
        c.triage(Triage.Flag(true), listOf(row))
        c.triage(Triage.Flag(false), listOf(row))
        c.triage(Triage.Junk, listOf(row))
        c.triage(Triage.MoveTo(9uL), listOf(row))
        c.triage(Triage.Category("Green \"one\"", true), listOf(row))
        assertEquals(
            listOf(
                "mail.flag" to """{"ids":[1]}""",
                "mail.clearFlag" to """{"ids":[1]}""",
                "mail.junk" to """{"ids":[1]}""",
                "mail.move" to """{"ids":[1],"folder":9}""",
                "mail.categorize" to """{"ids":[1],"category":"Green \"one\"","value":true}""",
            ),
            source.commands,
        )
    }

    @Test
    fun aConversationsRowNamesAllItsMessagesWhateverTheSessionsView() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3))
        val c = controller(source)
        c.show(inbox.copy(conversations = true))
        val row = FakeMailSource.row(2, members = 4)
        c.triage(Triage.Delete, listOf(row))
        assertEquals(listOf("mail.delete" to """{"ids":[200,201,202,203]}"""), source.commands)
    }

    @Test
    fun aCommandThatFailsIsSaidAndChangesNothingButTheNotice() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3)).also { it.failCommands = true }
        val c = controller(source)
        c.show(inbox)
        c.triage(Triage.Delete, listOf(FakeMailSource.row(1)))
        assertTrue(c.ui.value.notice?.notice is Notice.Failed)
        assertEquals(3, c.ui.value.snapshot.total)
    }

    @Test
    fun pullToRefreshWithNoAccountSaysSoAndWithOneAsksTheCore() = runBlocking {
        val source = FakeMailSource(FakeMailSource.rows(3))
        val c = controller(source)
        c.show(inbox)
        c.refresh()
        assertTrue(c.ui.value.notice?.notice is Notice.NoAccount)
        assertEquals(false, c.ui.value.refreshing)
        c.accountsHaveServer(true)
        c.refresh()
        assertTrue(c.ui.value.notice?.notice is Notice.NotYet)
        assertEquals(2, source.refreshes.get())
    }

    @Test
    fun aNoticeIsShownOnceAndTheSameNoticeTwiceIsTwo() = runBlocking {
        val c = controller(FakeMailSource(FakeMailSource.rows(1)))
        c.show(inbox)
        c.refresh()
        val first = c.ui.value.notice
        c.refresh()
        val second = c.ui.value.notice
        assertTrue(first != null && second != null && first.seq != second.seq)
        c.noticeShown(first!!.seq)
        assertNotNull(c.ui.value.notice)
        c.noticeShown(second!!.seq)
        assertNull(c.ui.value.notice)
    }

    @Test
    fun theNetworkAndTheClockGiveTheFreshnessState() = runBlocking {
        val source = FakeMailSource()
        var now = 1_000L
        val c = controller(source) { now }
        c.checkNetwork()
        assertEquals(false, c.ui.value.offline)
        source.isOffline = true
        c.checkNetwork()
        assertEquals(true, c.ui.value.offline)
        now = 5_000L
        c.updated()
        assertEquals(5_000L, c.ui.value.updatedAtMillis)
    }
}
