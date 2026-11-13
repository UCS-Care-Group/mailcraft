// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TasksTest {
    private val today = "2026-10-14"

    private fun row(due: String?, done: Boolean = false) = TaskRow(1u, "Task", due, done, false)

    @Test
    fun aTaskIsLateWhenItsDayHasPassedAndItIsNotDone() {
        assertTrue(isOverdue(row("2026-10-13"), today))
        assertFalse(isOverdue(row("2026-10-14"), today))
        assertFalse(isOverdue(row("2026-10-13", done = true), today))
        assertFalse(isOverdue(row(null), today))
    }

    @Test
    fun dueDatesAreSaidForPeople() {
        assertEquals("Today", dueLabel(EnglishTexts, "2026-10-14", today))
        assertEquals("Tomorrow", dueLabel(EnglishTexts, "2026-10-15", today))
        assertEquals("Yesterday", dueLabel(EnglishTexts, "2026-10-13", today))
        assertTrue(dueLabel(EnglishTexts, "2026-12-24", today, Locale.US).contains("2026"))
        assertEquals("soon", dueLabel(EnglishTexts, "soon", today))
        assertEquals("2026-10-14", dueLabel(EnglishTexts, "2026-10-14", "not a day"))
    }

    @Test
    fun theQuickDatesAreTodayTomorrowAndTheNextMonday() {
        // 2026-10-14 is a Wednesday.
        assertEquals(listOf("2026-10-14", "2026-10-15", "2026-10-19"), quickDates(today).map { it.second })
        // A Sunday: tomorrow is a Monday, and "next Monday" is the one after it.
        assertEquals(listOf("2026-10-18", "2026-10-19", "2026-10-26"), quickDates("2026-10-18").map { it.second })
        assertTrue(quickDates("nonsense").isEmpty())
    }

    @Test
    fun theDatePickerAndTheCoreAgreeOnADay() {
        val millis = dateToMillis("2026-10-14")
        assertEquals("2026-10-14", millis?.let(::millisToDate))
        assertNull(dateToMillis(null))
        assertNull(dateToMillis("never"))
        assertNull(millisToDate(Long.MAX_VALUE))
    }

    @Test
    fun aGroupIsNamedForItsAccountAndWhichListWhenThereAreSeveral() {
        assertEquals("Ada", groupTitle(EnglishTexts, "Ada", 0, 1))
        assertEquals("Ada · List 2", groupTitle(EnglishTexts, "Ada", 1, 2))
    }

    @Test
    fun aNewTaskNeedsATitleAndSendsItsPlace() {
        assertFalse(newDraft(0u).canSave)
        val draft = newDraft(7u, "2026-10-20").copy(title = "  Water the plants ", notes = "Both \"shelves\"\n")
        assertTrue(draft.canSave)
        assertEquals(
            """{"account":7,"title":"Water the plants","notes":"Both \"shelves\"\n",""" +
                """"due":"2026-10-20","importance":"normal"}""",
            draft.params(),
        )
    }

    @Test
    fun anEditSendsOnlyWhatChanged() {
        val base = TaskDraft(5u, "Call Bo", "notes", "2026-10-14", false, 0u)
        val read = base.copy(base = base)
        assertTrue(read.unchanged)
        assertEquals("""{"id":5,"due":null}""", read.copy(due = null).params())
        assertEquals("""{"id":5,"importance":"high"}""", read.copy(important = true).params())
        assertEquals("""{"id":5,"title":"Call Bo today"}""", read.copy(title = "Call Bo today").params())
        assertFalse(read.copy(title = "x").unchanged)
    }

    @Test
    fun aNoteThatWasCutIsNeverWrittenBack() {
        val base = TaskDraft(5u, "Long", "n".repeat(MAX_NOTES), null, false, 0u, notesCut = true)
        val edited = base.copy(base = base).copy(notes = "changed", title = "Long one")
        assertEquals("""{"id":5,"title":"Long one"}""", edited.params())
    }

    @Test
    fun whatTheOwnerTypesIsEscapedForTheCore() {
        assertEquals("\"a\\\"b\\\\c\\n\\u0001\"", jsonString("a\"b\\c\n\u0001"))
        assertEquals("\"\\u2028\"", jsonString("\u2028"))
        assertEquals("\"é😀\"", jsonString("é😀"))
        // A title is cut at the limit.
        val params = newDraft(0u).copy(title = "x".repeat(MAX_TITLE + 100)).params()
        assertEquals(MAX_TITLE, params.count { it == 'x' })
    }

    @Test
    fun aListWithMoreThanItShowsOffersMore() {
        val rows = List(3) { row(null) }
        val group = TaskGroup(2u, "inbox", "Account", 5, 5, rows)
        assertFalse(group.complete)
        assertEquals("2/inbox", group.key)
        assertTrue(group.copy(rows = rows + rows.take(2)).complete)
    }
}
