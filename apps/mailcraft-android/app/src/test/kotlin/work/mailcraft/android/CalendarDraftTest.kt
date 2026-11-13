// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength")

package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private fun day(text: String): LocalDate = LocalDate.parse(text)

class EventDraftTest {
    private val base = EventDraft(
        title = "Planning",
        startDate = day("2026-11-02"),
        startTime = LocalTime.of(9, 0),
        endDate = day("2026-11-02"),
        endTime = LocalTime.of(10, 30),
    )

    @Test
    fun aNewEventIsSentAsTheCommandTakesIt() {
        assertEquals(
            "{\"title\":\"Planning\",\"allDay\":false,\"start\":\"2026-11-02T09:00\",\"end\":\"2026-11-02T10:30\"," +
                "\"location\":\"\",\"description\":\"\",\"attendees\":\"\",\"reminder\":null,\"recurrence\":null}",
            base.params(null),
        )
    }

    @Test
    fun anEventOnACalendarAndWithPeopleAndAReminderSaysSo() {
        val d = base.copy(calendar = 7uL, attendees = "a@example.org; b@example.org", reminder = 15, location = "Room \"4\"")
        val json = d.params(send = false)
        assertTrue(json.contains("\"calendar\":7"))
        assertTrue(json.contains("\"send\":false"))
        assertTrue(json.contains("\"reminder\":15"))
        assertTrue(json.contains("\"location\":\"Room \\\"4\\\"\""))
        assertTrue(json.contains("\"attendees\":\"a@example.org; b@example.org\""))
    }

    @Test
    fun aChangeNamesTheEventAndNeverMovesItsCalendar() {
        val json = base.copy(id = 12uL, calendar = 3uL).params(null)
        assertTrue(json.startsWith("{\"id\":12,"))
        assertFalse(json.contains("\"calendar\""))
    }

    @Test
    fun aWholeDayEventIsWrittenAsDaysWithTheLastDayIncluded() {
        val d = base.copy(allDay = true, endDate = day("2026-11-04"))
        val json = d.params(null)
        assertTrue(json.contains("\"start\":\"2026-11-02\",\"end\":\"2026-11-04\""))
        assertEquals(day("2026-11-05").atStartOfDay(), d.end)
        assertFalse(d.endBeforeStart)
    }

    @Test
    fun aSeriesIsWrittenAsItsRuleAndAKeptRuleIsLeftOut() {
        val weekly = RepeatDraft(Repeat.Weekly, 2, setOf(DayOfWeek.WEDNESDAY, DayOfWeek.MONDAY), RepeatEnd.After(6))
        assertEquals("{\"freq\":\"weekly\",\"interval\":2,\"byDay\":[\"mon\",\"wed\"],\"count\":6}", weekly.json())
        val until = RepeatDraft(Repeat.Daily, 1, emptySet(), RepeatEnd.On(day("2026-12-31")))
        assertEquals("{\"freq\":\"daily\",\"interval\":1,\"until\":\"2026-12-31\"}", until.json())
        assertNull(RepeatDraft().json())
        assertTrue(base.copy(repeat = weekly).params(null).contains("\"recurrence\":{\"freq\":\"weekly\""))
        assertFalse(base.copy(repeat = RepeatDraft(Repeat.Monthly, kept = true)).params(null).contains("recurrence"))
        // Numbers that are out of range are held to it.
        assertEquals("{\"freq\":\"daily\",\"interval\":99,\"count\":999}", RepeatDraft(Repeat.Daily, 5000, end = RepeatEnd.After(99999)).json())
        assertEquals("{\"freq\":\"daily\",\"interval\":1,\"count\":1}", RepeatDraft(Repeat.Daily, -4, end = RepeatEnd.After(0)).json())
    }

    @Test
    fun oneTimeOfASeriesIsChangedOnItsOwn() {
        val json = base.copy(id = 4uL, occurrence = day("2026-11-16"), repeat = RepeatDraft(Repeat.Weekly)).params(null)
        assertTrue(json.contains("\"scope\":\"occurrence\",\"occurrence\":\"2026-11-16\""))
        assertFalse(json.contains("recurrence"))
    }

    @Test
    fun theEndBeforeTheStartIsCaughtAndMovingTheStartKeepsTheLength() {
        assertTrue(base.copy(endTime = LocalTime.of(8, 0)).endBeforeStart)
        assertFalse(base.endBeforeStart)
        val moved = base.withStartTime(LocalTime.of(23, 30))
        assertEquals(day("2026-11-03"), moved.endDate)
        assertEquals(LocalTime.of(1, 0), moved.endTime)
        val later = base.withStartDate(day("2026-11-10"))
        assertEquals(day("2026-11-10"), later.endDate)
        assertEquals(LocalTime.of(10, 30), later.endTime)
    }

    @Test
    fun aNewEventStartsAtTheNextHalfHourOnTodayAndAtNineOnAnotherDay() {
        fun at(time: String) = EventDraft.blank(day("2026-11-02"), LocalDateTime.parse("2026-11-02T$time"), 1uL)
        assertEquals(LocalTime.of(10, 30), at("10:12").startTime)
        assertEquals(LocalTime.of(11, 0), at("10:30").startTime)
        assertEquals(LocalTime.of(11, 0), at("10:45").startTime)
        val late = at("23:50")
        assertEquals(LocalTime.of(0, 0), late.startTime)
        assertEquals(day("2026-11-03"), late.startDate)
        assertEquals(LocalTime.of(0, 30), late.endTime)
        val other = EventDraft.blank(day("2026-11-20"), LocalDateTime.parse("2026-11-02T10:12"), 1uL)
        assertEquals(LocalTime.of(9, 0), other.startTime)
        assertEquals(LocalTime.of(9, 30), other.endTime)
        assertEquals(1uL, other.calendar)
    }

    @Test
    fun peopleAreThoseWithAnAddress() {
        assertFalse(base.hasPeople)
        assertFalse(base.copy(attendees = "  ;  ").hasPeople)
        assertTrue(base.copy(attendees = "a@example.org").hasPeople)
    }
}

class JsonTextTest {
    @Test
    fun textIsQuotedAndWhatJsonCannotHoldIsEscaped() {
        assertEquals("\"a\\\"b\\\\c\\n\\r\\t\"", jsonString("a\"b\\c\n\r\t"))
        assertEquals("\"\\u0000\\u001f\\u2028\"", jsonString("\u0000\u001f\u2028"))
        assertEquals("\"日本語 😀\"", jsonString("日本語 😀"))
        assertEquals("\"\"", jsonString(""))
    }

    @Test
    fun answersAndDeletionsAreWrittenForTheirCommands() {
        assertEquals("{\"id\":5,\"response\":\"accept\",\"send\":true}", answerParams(5uL, Answer.Accepted, true, null))
        assertEquals(
            "{\"id\":5,\"response\":\"decline\",\"send\":false,\"scope\":\"occurrence\",\"occurrence\":\"2026-11-16\"}",
            answerParams(5uL, Answer.Declined, false, day("2026-11-16")),
        )
        assertEquals("{\"id\":9}", deleteParams(9uL, null, null))
        assertEquals("{\"id\":9,\"scope\":\"occurrence\",\"occurrence\":\"2026-11-16\",\"send\":true}", deleteParams(9uL, day("2026-11-16"), true))
    }
}
