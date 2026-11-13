// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength", "LongParameterList")

package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private fun day(text: String): LocalDate = LocalDate.parse(text)

internal fun event(
    id: Int,
    start: String,
    end: String,
    allDay: Boolean = false,
    title: String = "Event $id",
    invited: Boolean = false,
    answer: Answer = Answer.None,
): CalendarEvent = CalendarEvent(
    id = id.toULong(),
    calendar = 1uL,
    color = 0xFF2E8B57.toInt(),
    title = title,
    location = "",
    start = LocalDateTime.parse(start),
    end = LocalDateTime.parse(end),
    allDay = allDay,
    repeats = false,
    meeting = invited,
    cancelled = false,
    invited = invited,
    answer = answer,
)

class CalendarPagingTest {
    @Test
    fun theAgendaShowsThirtyDaysFromTheAnchor() {
        val r = CalendarPaging.range(CalendarMode.Agenda, day("2026-12-20"), DayOfWeek.MONDAY)
        assertEquals(DateRange(day("2026-12-20"), day("2027-01-18")), r)
        assertEquals(30, r.days)
    }

    @Test
    fun theDayViewIsOneDay() {
        val r = CalendarPaging.range(CalendarMode.Day, day("2026-03-29"), DayOfWeek.MONDAY)
        assertEquals(1, r.days)
    }

    @Test
    fun theMonthGridStartsOnTheWeekStartAndHasSixWeeks() {
        // 1 February 2026 is a Sunday.
        val monday = CalendarPaging.range(CalendarMode.Month, day("2026-02-14"), DayOfWeek.MONDAY)
        assertEquals(day("2026-01-26"), monday.from)
        assertEquals(day("2026-03-08"), monday.to)
        val sunday = CalendarPaging.range(CalendarMode.Month, day("2026-02-14"), DayOfWeek.SUNDAY)
        assertEquals(day("2026-02-01"), sunday.from)
        assertEquals(42, sunday.days)
        val grid = CalendarPaging.monthGrid(day("2026-02-14"), DayOfWeek.MONDAY)
        assertEquals(6, grid.size)
        assertTrue(grid.all { it.size == 7 })
        assertEquals(DayOfWeek.MONDAY, grid[3][0].dayOfWeek)
        assertTrue(grid.flatten().zipWithNext().all { (a, b) -> a.plusDays(1) == b })
    }

    @Test
    fun noRangeIsLongerThanTheCoreAnswersFor() {
        for (mode in CalendarMode.entries) {
            for (start in DayOfWeek.entries) {
                assertTrue(CalendarPaging.range(mode, day("2026-12-31"), start).days <= 100)
            }
        }
    }

    @Test
    fun stepsMoveByTheirOwnUnitAndCrossTheYear() {
        assertEquals(day("2027-01-19"), CalendarPaging.step(CalendarMode.Agenda, day("2026-12-20"), 1))
        assertEquals(day("2026-11-20"), CalendarPaging.step(CalendarMode.Agenda, day("2026-12-20"), -1))
        assertEquals(day("2027-01-01"), CalendarPaging.step(CalendarMode.Day, day("2026-12-31"), 1))
        assertEquals(day("2026-02-28"), CalendarPaging.step(CalendarMode.Month, day("2026-01-31"), 1))
        assertEquals(day("2025-12-15"), CalendarPaging.step(CalendarMode.Month, day("2026-01-15"), -1))
    }

    @Test
    fun theWeekStartFollowsTheLocale() {
        assertEquals(DayOfWeek.SUNDAY, weekStartOf(Locale.US))
        assertEquals(DayOfWeek.MONDAY, weekStartOf(Locale.GERMANY))
        assertEquals(listOf(DayOfWeek.SUNDAY, DayOfWeek.MONDAY), CalendarPaging.weekDays(DayOfWeek.SUNDAY).take(2))
    }
}

class CalendarDayTest {
    private val berlin = ZoneId.of("Europe/Berlin")

    @Test
    fun aDayIsAsLongAsItsClocksMakeIt() {
        assertEquals(1440, dayMinutes(day("2026-06-01"), berlin))
        assertEquals(1380, dayMinutes(day("2026-03-29"), berlin))
        assertEquals(1500, dayMinutes(day("2026-10-25"), berlin))
        assertEquals(1440, dayMinutes(day("2026-03-29"), ZoneId.of("UTC")))
    }

    @Test
    fun theHourLinesSkipTheHourThatIsNotThereAndRepeatTheOneThatIs() {
        val spring = hourMarks(day("2026-03-29"), berlin)
        assertEquals(23, spring.size)
        assertFalse(spring.any { it.hour == 2 })
        assertEquals(HourMark(120, 3), spring[2])
        val autumn = hourMarks(day("2026-10-25"), berlin)
        assertEquals(25, autumn.size)
        assertEquals(2, autumn.count { it.hour == 2 })
        assertEquals(listOf(120, 180), autumn.filter { it.hour == 2 }.map { it.minute })
        assertEquals(24, hourMarks(day("2026-06-01"), berlin).size)
    }

    @Test
    fun anEventSitsWhereTheElapsedTimePutsIt() {
        // The clocks go from 02:00 to 03:00: 03:00 is two hours after midnight, not three.
        assertEquals(120, minuteOfDay(LocalDateTime.parse("2026-03-29T03:00"), day("2026-03-29"), berlin))
        assertEquals(660, minuteOfDay(LocalDateTime.parse("2026-03-29T12:00"), day("2026-03-29"), berlin))
        // A time inside the hour that isn't there is moved on by that hour (02:30 reads as 03:30), and stays in the day.
        assertEquals(150, minuteOfDay(LocalDateTime.parse("2026-03-29T02:30"), day("2026-03-29"), berlin))
        assertEquals(1380, minuteOfDay(LocalDateTime.parse("2026-03-30T00:00"), day("2026-03-29"), berlin))
        assertEquals(0, minuteOfDay(LocalDateTime.parse("2026-03-28T23:00"), day("2026-03-29"), berlin))
    }

    @Test
    fun aZoneWhoseMidnightIsMissingStillMakesADay() {
        val havana = ZoneId.of("America/Havana")
        val d = day("2026-03-08")
        assertTrue(dayMinutes(d, havana) in 1..1500)
        assertTrue(minuteOfDay(LocalDateTime.parse("2026-03-08T00:30"), d, havana) in 0..60)
        assertTrue(hourMarks(d, havana).isNotEmpty())
    }

    @Test
    fun overlappingEventsShareWidthAndOthersKeepIt() {
        val a = event(1, "2026-11-10T09:00", "2026-11-10T10:00")
        val b = event(2, "2026-11-10T09:30", "2026-11-10T10:30")
        val c = event(3, "2026-11-10T10:00", "2026-11-10T11:00")
        val d = event(4, "2026-11-10T13:00", "2026-11-10T14:00")
        val layout = dayLayout(listOf(d, c, b, a), day("2026-11-10"), ZoneId.of("UTC"))
        val byId = layout.placed.associateBy { it.event.id }
        assertEquals(0, byId.getValue(1uL).column)
        assertEquals(1, byId.getValue(2uL).column)
        assertEquals(0, byId.getValue(3uL).column)
        assertEquals(2, byId.getValue(3uL).columns)
        assertEquals(1, byId.getValue(4uL).columns)
        assertEquals(540, byId.getValue(1uL).startMinute)
        assertEquals(600, byId.getValue(1uL).endMinute)
    }

    @Test
    fun aShortEventIsStillAsTallAsALegibleBlockAndAnOvernightOneIsCutToTheDay() {
        val short = event(1, "2026-11-10T09:00", "2026-11-10T09:05")
        val night = event(2, "2026-11-10T22:00", "2026-11-11T02:00")
        val whole = event(3, "2026-11-10T00:00", "2026-11-11T00:00", allDay = true)
        val layout = dayLayout(listOf(short, night, whole), day("2026-11-10"), ZoneId.of("UTC"))
        assertEquals(listOf(3uL), layout.allDay.map { it.id })
        assertEquals(30, layout.placed.first { it.event.id == 1uL }.let { it.endMinute - it.startMinute })
        assertEquals(1440, layout.placed.first { it.event.id == 2uL }.endMinute)
        val next = dayLayout(listOf(night), day("2026-11-11"), ZoneId.of("UTC"))
        assertEquals(0, next.placed.single().startMinute)
        assertEquals(120, next.placed.single().endMinute)
    }
}

class CalendarEventTest {
    @Test
    fun theLastDayOfAnEventIsTheLastDayItIsOn() {
        assertEquals(day("2026-11-10"), event(1, "2026-11-10T00:00", "2026-11-11T00:00", allDay = true).lastDay)
        assertEquals(day("2026-11-12"), event(1, "2026-11-10T00:00", "2026-11-13T00:00", allDay = true).lastDay)
        assertEquals(day("2026-11-10"), event(1, "2026-11-10T22:00", "2026-11-11T00:00").lastDay)
        assertEquals(day("2026-11-11"), event(1, "2026-11-10T22:00", "2026-11-11T00:30").lastDay)
        // An end before the start (hostile) is the start's day.
        assertEquals(day("2026-11-10"), event(1, "2026-11-10T22:00", "2026-11-09T00:00").lastDay)
    }

    @Test
    fun theCoreSTimesAreReadAndWhatIsNotATimeIsRefused() {
        assertEquals(LocalDateTime.parse("2026-11-10T09:00:00"), parseCoreTime("2026-11-10T09:00:00"))
        assertEquals(LocalDateTime.parse("2026-11-10T00:00"), parseCoreTime("2026-11-10"))
        assertNull(parseCoreTime("soon"))
        assertNull(parseCoreTime(""))
        assertNull(parseCoreTime("2026-13-45T99:00:00"))
        assertNull(parseCoreDate("2026-02-30"))
    }

    @Test
    fun theAgendaListsDaysWithThingsOnAndTodayAndPutsWholeDaysFirst() {
        val range = DateRange(day("2026-11-09"), day("2026-11-15"))
        val timed = event(1, "2026-11-10T09:00", "2026-11-10T10:00", title = "B")
        val whole = event(2, "2026-11-10T00:00", "2026-11-11T00:00", allDay = true, title = "Z")
        val multi = event(3, "2026-11-12T00:00", "2026-11-15T00:00", allDay = true)
        val days = agendaDays(listOf(timed, whole, multi), range, day("2026-11-09"))
        assertEquals(
            listOf("2026-11-09", "2026-11-10", "2026-11-12", "2026-11-13", "2026-11-14").map(::day),
            days.map { it.date },
        )
        assertEquals(listOf(2uL, 1uL), days[1].events.map { it.id })
        assertTrue(days[0].events.isEmpty())
        assertEquals(listOf(3uL), days[3].events.map { it.id })
    }

    @Test
    fun answersAreReadFromTheirCodes() {
        assertEquals(Answer.Accepted, Answer.fromCode("accepted"))
        assertEquals(Answer.None, Answer.fromCode("whatever"))
        assertEquals("tentative", Answer.Tentative.command)
    }
}
