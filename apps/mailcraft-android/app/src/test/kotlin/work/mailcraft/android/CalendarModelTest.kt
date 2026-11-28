// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength", "LongParameterList")

package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
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
    fun theWeekStartsOnTheWeekStartAndStepsByWeeks() {
        // 4 November 2026 is a Wednesday.
        val monday = CalendarPaging.range(CalendarMode.Week, day("2026-11-04"), DayOfWeek.MONDAY)
        assertEquals(DateRange(day("2026-11-02"), day("2026-11-08")), monday)
        val sunday = CalendarPaging.range(CalendarMode.Week, day("2026-11-04"), DayOfWeek.SUNDAY)
        assertEquals(DateRange(day("2026-11-01"), day("2026-11-07")), sunday)
        val saturday = CalendarPaging.range(CalendarMode.Week, day("2026-11-07"), DayOfWeek.SATURDAY)
        assertEquals(day("2026-11-07"), saturday.from)
        assertEquals(day("2026-11-11"), CalendarPaging.shift(CalendarMode.Week, day("2026-11-04"), 1))
        assertEquals(CalendarPaging.weekOf(day("2026-11-04"), DayOfWeek.SUNDAY), (0L..6L).map { day("2026-11-01").plusDays(it) })
    }

    @Test
    fun aDayWeekOrMonthIsAlwaysTheSamePageAndNeighboursAreNeighbours() {
        for (mode in listOf(CalendarMode.Day, CalendarMode.Week, CalendarMode.Month)) {
            for (start in DayOfWeek.entries) {
                val here = day("2026-11-04")
                val page = CalendarPaging.pageOf(mode, here, start)
                assertEquals(page + 1, CalendarPaging.pageOf(mode, CalendarPaging.shift(mode, here, 1), start))
                assertEquals(page - 1, CalendarPaging.pageOf(mode, CalendarPaging.shift(mode, here, -1), start))
                assertEquals(page + 25, CalendarPaging.pageOf(mode, CalendarPaging.shift(mode, here, 25), start))
                // Any day of the same week or month is on the same page.
                val sameWeek = CalendarPaging.weekOf(here, start).map { CalendarPaging.pageOf(CalendarMode.Week, it, start) }
                assertTrue(sameWeek.all { it == sameWeek.first() })
            }
        }
        assertEquals(CalendarPaging.pageOf(CalendarMode.Month, day("2026-11-01"), DayOfWeek.MONDAY), CalendarPaging.pageOf(CalendarMode.Month, day("2026-11-30"), DayOfWeek.MONDAY))
    }

    @Test
    fun aDayHeldToThePagesHasRealNeighboursAtEitherEnd() {
        for (far in listOf(day("1800-01-01"), day("1900-01-01"), day("2299-12-31"), day("2500-06-15"))) {
            val held = CalendarPaging.heldToPages(far)
            val cases = listOf(CalendarMode.Day, CalendarMode.Week, CalendarMode.Month).flatMap { mode ->
                DayOfWeek.entries.flatMap { start -> (-1..1).map { Triple(mode, start, it) } }
            }
            for ((mode, start, k) in cases) {
                // What the pager draws on the page k away is the page it is on.
                val page = CalendarPaging.pageOf(mode, held, start)
                assertEquals(page + k, CalendarPaging.pageOf(mode, CalendarPaging.shift(mode, held, k), start))
            }
        }
        assertEquals(day("2026-11-04"), CalendarPaging.heldToPages(day("2026-11-04")))
    }

    @Test
    fun aSwipeLandsOnItsDayAndPastTheLastPageIsTurnedBack() {
        for (mode in listOf(CalendarMode.Day, CalendarMode.Week, CalendarMode.Month)) {
            val start = DayOfWeek.MONDAY
            val mid = day("2026-11-04")
            val page = CalendarPaging.pageOf(mode, mid, start)
            assertNull(settle(mode, mid, page, start))
            assertEquals(Landing(CalendarPaging.shift(mode, mid, 1), page + 1), settle(mode, mid, page + 1, start))
            assertEquals(Landing(CalendarPaging.shift(mode, mid, -1), page - 1), settle(mode, mid, page - 1, start))
            for (edge in listOf(day("1800-01-01"), day("2500-01-01"))) {
                val held = CalendarPaging.heldToPages(edge)
                val at = CalendarPaging.pageOf(mode, held, start)
                val outward = if (edge.year < 2000) -1 else 1
                // Past the edge: the same day, and the pager turned back to its page.
                val past = settle(mode, held, at + outward, start)
                assertEquals(Landing(held, at), past)
                // Back inward: the next day, week or month in.
                assertEquals(CalendarPaging.shift(mode, held, -outward), settle(mode, held, at - outward, start)?.day)
            }
        }
    }

    @Test
    fun pagesAreHeldToThePagesThereAre() {
        for (mode in CalendarMode.entries) {
            assertEquals(0, CalendarPaging.pageOf(mode, day("1800-01-01"), DayOfWeek.MONDAY))
            assertEquals(CalendarPaging.pageCount(mode) - 1, CalendarPaging.pageOf(mode, day("2500-01-01"), DayOfWeek.MONDAY))
            assertTrue(CalendarPaging.pageOf(mode, day("2026-11-04"), DayOfWeek.MONDAY) in 1 until CalendarPaging.pageCount(mode) - 1)
        }
    }

    @Test
    fun noRangeIsLongerThanTheCoreAnswersFor() {
        for (mode in CalendarMode.entries) {
            for (start in DayOfWeek.entries) {
                assertTrue(CalendarPaging.loadRange(mode, day("2026-12-31"), start).days <= 100)
                assertTrue(CalendarPaging.range(mode, day("2026-12-31"), start).days <= 100)
            }
        }
    }

    @Test
    fun stepsMoveByTheirOwnUnitAndCrossTheYear() {
        assertEquals(day("2027-01-19"), CalendarPaging.shift(CalendarMode.Agenda, day("2026-12-20"), 1))
        assertEquals(day("2026-11-20"), CalendarPaging.shift(CalendarMode.Agenda, day("2026-12-20"), -1))
        assertEquals(day("2027-01-01"), CalendarPaging.shift(CalendarMode.Day, day("2026-12-31"), 1))
        assertEquals(day("2026-02-28"), CalendarPaging.shift(CalendarMode.Month, day("2026-01-31"), 1))
        assertEquals(day("2025-12-15"), CalendarPaging.shift(CalendarMode.Month, day("2026-01-15"), -1))
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
    fun backToBackShortEventsGetSeparateColumnsWhileTheirBlocksOverlap() {
        val a = event(1, "2026-11-10T09:00", "2026-11-10T09:15")
        val b = event(2, "2026-11-10T09:15", "2026-11-10T09:30")
        val utc = ZoneId.of("UTC")
        for (min in listOf(30, 50)) {
            val byId = dayLayout(listOf(a, b), day("2026-11-10"), utc, min).placed.associateBy { it.event.id }
            assertEquals(2, byId.getValue(1uL).columns)
            assertEquals(setOf(0, 1), byId.values.map { it.column }.toSet())
        }
        // A block long enough to end where the next begins frees its column.
        val c = event(3, "2026-11-10T09:00", "2026-11-10T09:30")
        val d = event(4, "2026-11-10T09:30", "2026-11-10T10:00")
        val placed = dayLayout(listOf(c, d), day("2026-11-10"), utc, 30).placed
        assertEquals(listOf(0, 0), placed.map { it.column })
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

    @Test
    fun anEventThatEndsOnAnotherDayNamesThatDayToo() {
        val date = DateTimeFormatter.ofPattern("d MMM", Locale.US)
        val time = DateTimeFormatter.ofPattern("HH:mm", Locale.US)
        // A whole-day event's end is the moment after its last day.
        assertEquals("3 Nov – 5 Nov, All day", whenText(at("2026-11-03T00:00", "2026-11-06T00:00"), true, "All day", date, time))
        assertEquals("3 Nov – All day", whenText(at("2026-11-03T00:00", "2026-11-04T00:00"), true, "All day", date, time))
        assertEquals("3 Nov, 22:00 – 4 Nov, 02:00", whenText(at("2026-11-03T22:00", "2026-11-04T02:00"), false, "All day", date, time))
        assertEquals("3 Nov, 14:00 – 15:00", whenText(at("2026-11-03T14:00", "2026-11-03T15:00"), false, "All day", date, time))
        // An end at midnight is the same evening's.
        assertEquals("3 Nov, 22:00 – 00:00", whenText(at("2026-11-03T22:00", "2026-11-04T00:00"), false, "All day", date, time))
    }

    @Test
    fun oneTimeOfASeriesIsOnItsOwnDayAndAnyOtherEventOnItsOwn() {
        val series = detail().copy(repeat = RepeatDraft(Repeat.Weekly, 1, setOf(DayOfWeek.TUESDAY)))
        assertEquals(at("2026-11-17T14:00", "2026-11-17T15:00"), series.span(day("2026-11-17")))
        assertEquals(at("2026-11-03T14:00", "2026-11-03T15:00"), detail().span(day("2026-11-17")))
    }

    @Test
    fun aMonthCellSaysItsDateItsTodayAndItsEvents() {
        val en = java.util.Locale.ENGLISH
        val d = day("2026-11-03")
        assertEquals("Tuesday, November 3, 2026, No events", dayDescription(EnglishTexts, d, false, 0, en))
        assertEquals("Tuesday, November 3, 2026, Today, 1 event", dayDescription(EnglishTexts, d, true, 1, en))
        assertEquals("Tuesday, November 3, 2026, 5 events", dayDescription(EnglishTexts, d, false, 5, en))
    }

    @Test
    fun aPageSaysWhatItShowsWhenItComes() {
        val en = Locale.US
        val events = listOf(
            event(1, "2026-11-02T09:00", "2026-11-02T10:00"),
            event(2, "2026-11-04T00:00", "2026-11-05T00:00", allDay = true),
            event(3, "2026-11-09T09:00", "2026-11-09T10:00"),
        )
        val state = CalendarUiState(mode = CalendarMode.Week, anchor = day("2026-11-04"), today = day("2026-11-04"))
        assertEquals(
            "Nov 1 – Nov 7, 2026, 2 events",
            pageDescription(EnglishTexts, state, events, DayOfWeek.SUNDAY, en),
        )
        assertEquals(
            "Wednesday, November 4, 2026, Today, 1 event",
            pageDescription(EnglishTexts, state.copy(mode = CalendarMode.Day), events, DayOfWeek.SUNDAY, en),
        )
        assertEquals(
            "Nov 1 – Nov 7, 2026, Loading…",
            pageDescription(EnglishTexts, state, null, DayOfWeek.SUNDAY, en),
        )
        assertEquals("November 2026", pageDescription(EnglishTexts, state.copy(mode = CalendarMode.Month), events, DayOfWeek.SUNDAY, en))
    }

    @Test
    fun aWeekBlockSaysItsWholeTitleAndTime() {
        val e = event(1, "2026-11-04T09:00", "2026-11-04T10:30", title = "A long title that the column cuts short")
        assertEquals(
            "A long title that the column cuts short, 9:00 AM – 10:30 AM",
            blockDescription(EnglishTexts, e, day("2026-11-04"), Locale.US).replace('\u202F', ' '),
        )
        assertEquals("(No title), All day", blockDescription(EnglishTexts, event(2, "2026-11-04T00:00", "2026-11-05T00:00", allDay = true, title = ""), day("2026-11-04"), Locale.US))
    }

    @Test
    fun aWeeksTitleNamesTheYearOnceWhenItHasOne() {
        val d = day("2026-11-04")
        assertEquals("Nov 1 – Nov 7, 2026", rangeTitle(CalendarMode.Week, d, Locale.US, DayOfWeek.SUNDAY))
        assertEquals("02.11. – 08.11.2026", rangeTitle(CalendarMode.Week, d, Locale.GERMANY, DayOfWeek.MONDAY))
        assertEquals("11月2日 – 2026年11月8日", rangeTitle(CalendarMode.Week, d, Locale.CHINA, DayOfWeek.MONDAY))
        assertEquals("Dec 28, 2026 – Jan 3, 2027", rangeTitle(CalendarMode.Week, day("2026-12-30"), Locale.US, DayOfWeek.MONDAY))
        val pt = rangeTitle(CalendarMode.Week, d, Locale.forLanguageTag("pt-BR"), DayOfWeek.SUNDAY)
        assertFalse(pt, pt.substringBefore(" – ").contains("2026"))
        assertFalse(pt, pt.substringBefore(" – ").endsWith("de"))
    }

    @Test
    fun theWeekHoursFollowTheLocalesClock() {
        assertEquals("9 AM", LocalTime.of(9, 0).format(hourFormat(Locale.US)).replace('\u202F', ' '))
        assertEquals("09", LocalTime.of(9, 0).format(hourFormat(Locale.GERMANY)))
    }

    private fun at(from: String, to: String) = LocalDateTime.parse(from) to LocalDateTime.parse(to)
}
