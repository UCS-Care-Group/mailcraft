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

    @Test
    fun aChangeSendsBackOnlyTheWordsTheOwnerChanged() {
        // The core hands over a long description cut short; a save that leaves it alone must not send it.
        val loaded = detail(people = true).copy(description = "Cut short by the core")
        val json = loaded.draft(null).copy(location = "Room 5").params(null)
        assertTrue(json.contains("\"location\":\"Room 5\""))
        assertFalse(json.contains("\"description\""))
        assertFalse(json.contains("\"title\""))
        assertFalse(json.contains("\"attendees\""))
        assertFalse(json.contains("\"optional\""))
        val changed = loaded.draft(null).copy(description = "New notes", title = " Renamed ").params(null)
        assertTrue(changed.contains("\"description\":\"New notes\""))
        assertTrue(changed.contains("\"title\":\"Renamed\""))
    }

    @Test
    fun optionalPeopleStayOptional() {
        val people = listOf(
            AttendeeItem("Ada Example", "ada@example.org", false, Answer.Accepted),
            AttendeeItem("Bo Example", "bo@example.org", true, Answer.None),
        )
        val draft = detail().copy(attendees = people).draft(null)
        assertEquals("ada@example.org", draft.attendees)
        assertEquals("bo@example.org", draft.optional)
        assertTrue(draft.hasPeople)
        // A change of the people sends both lists, each with its own people.
        val json = draft.copy(attendees = "ada@example.org; cy@example.org").params(false)
        assertTrue(json.contains("\"attendees\":\"ada@example.org; cy@example.org\",\"optional\":\"bo@example.org\""))
        // Optional people alone are people too.
        assertTrue(base.copy(optional = "bo@example.org").hasPeople)
        assertTrue(base.copy(optional = "bo@example.org").params(null).contains("\"optional\":\"bo@example.org\""))
    }

    @Test
    fun oneTimeOfASeriesShowsItsOwnDaysAndIsSentAsTheSeriesDays() {
        val series = detail().copy(
            start = LocalDateTime.parse("2026-11-01T14:00"),
            end = LocalDateTime.parse("2026-11-01T15:00"),
            repeat = RepeatDraft(Repeat.Weekly, days = setOf(DayOfWeek.SUNDAY)),
        )
        val draft = series.draft(day("2026-11-08"))
        assertEquals(day("2026-11-08"), draft.startDate)
        assertEquals(day("2026-11-08"), draft.endDate)
        // Moved a day later: the command is told the series' day a day later, and moves that time by as much.
        val json = draft.withStartDate(day("2026-11-09")).params(null)
        assertTrue(json.contains("\"start\":\"2026-11-02T14:00\",\"end\":\"2026-11-02T15:00\""))
        assertTrue(json.contains("\"occurrence\":\"2026-11-08\""))
        // Left as it was, no times are sent at all: the series stays where it is.
        assertFalse(draft.params(null).contains("\"start\""))
        // A whole-day series the same way.
        val days = series.copy(allDay = true, start = LocalDateTime.parse("2026-11-01T00:00"), end = LocalDateTime.parse("2026-11-02T00:00"))
        val whole = days.draft(day("2026-11-15"))
        assertEquals(day("2026-11-15"), whole.startDate)
        assertFalse(whole.params(null).contains("\"start\""))
        assertTrue(whole.withStartDate(day("2026-11-16")).params(null).contains("\"start\":\"2026-11-02\",\"end\":\"2026-11-02\""))
    }

    @Test
    fun aDraftKeptAsWordsReadsBackWholeAndJunkGivesNone() {
        val series = detail(people = true).copy(repeat = RepeatDraft(Repeat.Weekly, 2, setOf(DayOfWeek.MONDAY), RepeatEnd.On(day("2027-01-31"))))
        val draft = series.draft(null).copy(title = "Half written", optional = "bo@example.org", reminder = 30)
        assertEquals(draft, SavedDraft.read(SavedDraft.write(draft)))
        val counted = base.copy(repeat = RepeatDraft(Repeat.Daily, end = RepeatEnd.After(4)), occurrence = day("2026-11-05"), seriesShift = 3)
        assertEquals(counted, SavedDraft.read(SavedDraft.write(counted)))
        assertEquals(base, SavedDraft.read(SavedDraft.write(base)))
        assertNull(SavedDraft.read(emptyMap()))
        assertNull(SavedDraft.read(SavedDraft.write(base) + ("startDate" to "not a day")))
        assertNull(SavedDraft.read(SavedDraft.write(base) + ("id" to "-1")))
        assertNull(SavedDraft.read(SavedDraft.write(base) + ("days" to "FUNDAY")))
    }

    @Test
    fun aListOfPeopleTheEditorWasGivenOnlyPartOfIsNeverSentBack() {
        val cut = detail(people = true).copy(peopleCut = true)
        val draft = cut.draft(null)
        assertFalse(draft.copy(title = "Renamed").params(null).contains("attendees"))
        // Not even when the field was changed: the core would take it as the whole list.
        assertFalse(draft.copy(attendees = "ada@example.org; new@example.org").params(null).contains("attendees"))
        val whole = detail(people = true).draft(null)
        assertTrue(whole.copy(attendees = "ada@example.org; new@example.org").params(null).contains("\"attendees\""))
        assertEquals(draft, SavedDraft.read(SavedDraft.write(draft)))
    }

    @Test
    fun aTitleOnlyEditKeepsTheLoadedTimesWhateverSecondsTheyHave() {
        val precise = detail().copy(start = LocalDateTime.parse("2026-11-03T14:00:30"), end = LocalDateTime.parse("2026-11-03T15:00:45"))
        val draft = precise.draft(null)
        assertFalse(draft.copy(title = "Renamed").params(null).contains("\"start\""))
        assertFalse(draft.copy(title = "Renamed").params(null).contains("\"end\""))
        // A time the owner changed is sent.
        assertTrue(draft.withStartTime(java.time.LocalTime.of(9, 0)).params(null).contains("\"start\":\"2026-11-03T09:00\""))
        // So is a switch to a whole day.
        assertTrue(draft.copy(allDay = true).params(null).contains("\"start\":\"2026-11-03\""))
        assertEquals(draft, SavedDraft.read(SavedDraft.write(draft)))
    }

    @Test
    fun oneTimeOfASeriesAcrossAClockChangeShowsItsOwnClockAndMovesFromTheSeriesStart() {
        val series = detail().copy(
            start = LocalDateTime.parse("2026-03-01T10:00"),
            end = LocalDateTime.parse("2026-03-01T11:00"),
            repeat = RepeatDraft(Repeat.Weekly, days = setOf(DayOfWeek.SUNDAY)),
        )
        val row = LocalDateTime.parse("2026-04-05T11:00") to LocalDateTime.parse("2026-04-05T12:00")
        val draft = series.draft(day("2026-04-05"), row)
        assertEquals(java.time.LocalTime.of(11, 0), draft.startTime)
        assertEquals(row, series.span(day("2026-04-05"), row))
        // An hour later: the series' own 10:00 moves by that hour, not by the clock change as well.
        val json = draft.withStartTime(java.time.LocalTime.of(12, 0)).params(null)
        assertTrue(json.contains("\"start\":\"2026-03-01T11:00\""))
        assertTrue(json.contains("\"end\":\"2026-03-01T12:00\""))
        assertEquals(draft, SavedDraft.read(SavedDraft.write(draft)))
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
