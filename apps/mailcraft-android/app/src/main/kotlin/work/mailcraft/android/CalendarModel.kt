// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException
import java.time.temporal.ChronoUnit
import java.time.temporal.WeekFields
import java.util.Locale

/** The four ways of looking at the calendar. */
internal enum class CalendarMode { Agenda, Day, Week, Month }

/** Days from [from] to [to], both included. */
internal data class DateRange(val from: LocalDate, val to: LocalDate) {
    operator fun contains(day: LocalDate): Boolean = !day.isBefore(from) && !day.isAfter(to)

    val days: Long get() = ChronoUnit.DAYS.between(from, to) + 1
}

internal const val DAYS_IN_WEEK = 7
internal const val MONTH_WEEKS = 6
internal const val AGENDA_DAYS = 30L
internal const val MINUTES_IN_HOUR = 60
private const val MINUTES_IN_DAY = 1440
private const val MONTHS_IN_YEAR = 12
private const val PAGED_YEARS = 400

/** The first day the swiped views page from, and how many days they page through (400 years exactly). */
private val PAGE_ORIGIN: LocalDate = LocalDate.of(1900, 1, 1)
private const val PAGED_DAYS = 146_097

/** The first day of the week in [locale]. */
internal fun weekStartOf(locale: Locale): DayOfWeek = WeekFields.of(locale).firstDayOfWeek

/** What range of days a view needs, and how a step moves from one range to the next. */
internal object CalendarPaging {
    /** The days a view shows around [anchor]; never more than the core answers for at once. */
    fun range(mode: CalendarMode, anchor: LocalDate, weekStart: DayOfWeek): DateRange = when (mode) {
        CalendarMode.Agenda -> DateRange(anchor, anchor.plusDays(AGENDA_DAYS - 1))
        CalendarMode.Day -> DateRange(anchor, anchor)
        CalendarMode.Week -> weekStartOf(anchor, weekStart).let { DateRange(it, it.plusDays(DAYS_IN_WEEK - 1L)) }
        CalendarMode.Month -> {
            val first = weekStartOf(anchor.withDayOfMonth(1), weekStart)
            DateRange(first, first.plusDays((MONTH_WEEKS * DAYS_IN_WEEK - 1).toLong()))
        }
    }

    /**
     * The days read for a view: its own, and for the day and the week the page on either side too, so
     * that a swipe shows the next page's events while it is still being dragged in. Never more than
     * the core answers for at once (the month has its six weeks alone).
     */
    fun loadRange(mode: CalendarMode, anchor: LocalDate, weekStart: DayOfWeek): DateRange {
        val own = range(mode, anchor, weekStart)
        return when (mode) {
            CalendarMode.Day -> DateRange(own.from.minusDays(1), own.to.plusDays(1))
            CalendarMode.Week -> DateRange(own.from.minusWeeks(1), own.to.plusWeeks(1))
            CalendarMode.Agenda, CalendarMode.Month -> own
        }
    }

    /** The anchor [pages] pages away from [anchor], either way: pages of agenda, days, weeks, months. */
    fun shift(mode: CalendarMode, anchor: LocalDate, pages: Int): LocalDate = when (mode) {
        CalendarMode.Agenda -> anchor.plusDays(AGENDA_DAYS * pages)
        CalendarMode.Day -> anchor.plusDays(pages.toLong())
        CalendarMode.Week -> anchor.plusWeeks(pages.toLong())
        CalendarMode.Month -> anchor.plusMonths(pages.toLong())
    }

    /** How many pages the swiped views have: one for each day, week or month from 1900 to 2299. */
    fun pageCount(mode: CalendarMode): Int = when (mode) {
        CalendarMode.Agenda, CalendarMode.Day -> PAGED_DAYS
        CalendarMode.Week -> PAGED_DAYS / DAYS_IN_WEEK
        CalendarMode.Month -> PAGED_YEARS * MONTHS_IN_YEAR
    }

    /**
     * The page of a swiped view that shows [anchor]: the same day, week or month is always the same
     * page, so the pager turned back after the phone is turned lands where it was. Held to the pages
     * there are; the agenda counts in days like the day view.
     */
    fun pageOf(mode: CalendarMode, anchor: LocalDate, weekStart: DayOfWeek): Int {
        val page = when (mode) {
            CalendarMode.Agenda, CalendarMode.Day -> anchor.toEpochDay() - PAGE_ORIGIN.toEpochDay()
            CalendarMode.Week ->
                Math.floorDiv(
                    weekStartOf(anchor, weekStart).toEpochDay() - PAGE_ORIGIN.toEpochDay(),
                    DAYS_IN_WEEK.toLong(),
                )
            CalendarMode.Month ->
                (anchor.year - PAGE_ORIGIN.year).toLong() * MONTHS_IN_YEAR + anchor.monthValue - 1
        }
        return page.coerceIn(0L, pageCount(mode) - 1L).toInt()
    }

    /**
     * [day] held a month inside the days the swiped views page through, so that every view's page of it
     * is a page there is and the pages beside it are its neighbours (a date typed or reached by paging
     * centuries away can't leave the pager and the title showing different days).
     */
    fun heldToPages(day: LocalDate): LocalDate {
        val first = PAGE_ORIGIN.plusMonths(1)
        val last = PAGE_ORIGIN.plusDays(PAGED_DAYS - 1L).minusMonths(1)
        return if (day.isBefore(first)) first else if (day.isAfter(last)) last else day
    }

    /** The first day of the week that holds [day]. */
    fun weekStartOf(day: LocalDate, weekStart: DayOfWeek): LocalDate =
        day.minusDays(Math.floorMod(day.dayOfWeek.value - weekStart.value, DAYS_IN_WEEK).toLong())

    /** The seven days of the week that holds [day], in the locale's order. */
    fun weekOf(day: LocalDate, weekStart: DayOfWeek): List<LocalDate> {
        val first = weekStartOf(day, weekStart)
        return List(DAYS_IN_WEEK) { first.plusDays(it.toLong()) }
    }

    /** Six weeks of seven days, so that the grid keeps its height from month to month. */
    fun monthGrid(anchor: LocalDate, weekStart: DayOfWeek): List<List<LocalDate>> {
        val start = weekStartOf(anchor.withDayOfMonth(1), weekStart)
        return List(MONTH_WEEKS) { week ->
            List(DAYS_IN_WEEK) { day -> start.plusDays((week * DAYS_IN_WEEK + day).toLong()) }
        }
    }

    /** The seven days of the week, in the locale's order, for the header of the month grid. */
    fun weekDays(weekStart: DayOfWeek): List<DayOfWeek> = List(DAYS_IN_WEEK) { weekStart.plus(it.toLong()) }
}

/** What the owner has answered to an invitation. */
internal enum class Answer(val code: String, val command: String) {
    None("none", ""),
    Accepted("accepted", "accept"),
    Tentative("tentative", "tentative"),
    Declined("declined", "decline"),
    ;

    companion object {
        fun fromCode(code: String): Answer = entries.firstOrNull { it.code == code } ?: None
    }
}

/** One event (or one time of a series) as the views draw it. Times are the owner's clock. */
internal data class CalendarEvent(
    val id: ULong,
    val calendar: ULong,
    val color: Int,
    val title: String,
    val location: String,
    val start: LocalDateTime,
    val end: LocalDateTime,
    val allDay: Boolean,
    val repeats: Boolean,
    val meeting: Boolean,
    val cancelled: Boolean,
    val invited: Boolean,
    val answer: Answer,
) {
    /** The last day it is on: an end at midnight is the day before's, and an end before the start is the start's. */
    val lastDay: LocalDate
        get() = if (end.isAfter(start)) end.minusNanos(1).toLocalDate() else start.toLocalDate()

    val firstDay: LocalDate get() = start.toLocalDate()

    /** It is on [day]. */
    fun isOn(day: LocalDate): Boolean = !day.isBefore(firstDay) && !day.isAfter(lastDay)

    /** It lasts more than one calendar day. */
    val spansDays: Boolean get() = lastDay.isAfter(firstDay)
}

/**
 * When an event is, in words: its day and times, or for a whole-day event its day and [allDayWord];
 * an event that ends on another day than it starts names that day too (a whole-day event's last day,
 * not the day after it).
 */
internal fun whenText(
    span: Pair<LocalDateTime, LocalDateTime>,
    allDay: Boolean,
    allDayWord: String,
    date: DateTimeFormatter,
    time: DateTimeFormatter,
): String {
    val (from, to) = span
    val first = from.toLocalDate()
    val last = if (to.isAfter(from)) to.minusNanos(1).toLocalDate() else first
    return when {
        allDay && last.isAfter(first) -> first.format(date) + " – " + last.format(date) + ", " + allDayWord
        allDay -> first.format(date) + " – " + allDayWord
        last.isAfter(first) -> first.format(date) + ", " + from.toLocalTime().format(time) + " – " +
            to.toLocalDate().format(date) + ", " + to.toLocalTime().format(time)
        else -> first.format(date) + ", " + from.toLocalTime().format(time) + " – " + to.toLocalTime().format(time)
    }
}

/** A time as the core writes it (`YYYY-MM-DDTHH:MM:SS`, or a date); null for what is neither. */
internal fun parseCoreTime(text: String): LocalDateTime? = try {
    if (text.length > DATE_LENGTH) LocalDateTime.parse(text) else LocalDate.parse(text).atStartOfDay()
} catch (_: DateTimeParseException) {
    null
}

private const val DATE_LENGTH = 10

/** A day as the core writes it; null for what is not one. */
internal fun parseCoreDate(text: String): LocalDate? = try {
    LocalDate.parse(text)
} catch (_: DateTimeParseException) {
    null
}

/** The events of [days] sorted for the views: whole-day ones first, then by start, then by title. */
internal val eventOrder: Comparator<CalendarEvent> =
    compareBy<CalendarEvent>({ !it.allDay }, { it.start }, { it.end }, { it.title }, { it.id })

/** A day of the agenda and what is on it. */
internal data class AgendaDay(val date: LocalDate, val events: List<CalendarEvent>)

/**
 * The days of [range] that have something on, in order; an event of several days is on each of
 * them. Days with nothing on are left out, except [today] when it is in the range (the agenda
 * says "nothing on" there).
 */
internal fun agendaDays(events: List<CalendarEvent>, range: DateRange, today: LocalDate): List<AgendaDay> {
    val out = ArrayList<AgendaDay>()
    var day = range.from
    while (!day.isAfter(range.to)) {
        val here = events.filter { it.isOn(day) }.sortedWith(eventOrder)
        if (here.isNotEmpty() || day == today) out.add(AgendaDay(day, here))
        day = day.plusDays(1)
    }
    return out
}

/** An hour line of the day view: [minute] counts from the start of the day, [hour] is what the clock reads. */
internal data class HourMark(val minute: Int, val hour: Int)

/** An event placed in the day view: its column among [columns] side by side, and its minutes of the day. */
internal data class Placed(
    val event: CalendarEvent,
    val column: Int,
    val columns: Int,
    val startMinute: Int,
    val endMinute: Int,
)

/** The day view's content: whole-day events above, timed ones placed, the hour lines. */
internal data class DayLayout(
    val allDay: List<CalendarEvent>,
    val placed: List<Placed>,
    val totalMinutes: Int,
    val hours: List<HourMark>,
)

/** A shown block is at least this long, so that a short event can be read and touched. */
internal const val MIN_BLOCK_MINUTES = 30

/** How long [date] is by the clocks of [zone] (23, 24 or 25 hours where the clocks change), in minutes. */
internal fun dayMinutes(date: LocalDate, zone: ZoneId): Int {
    val start = date.atStartOfDay(zone)
    val end = date.plusDays(1).atStartOfDay(zone)
    val minutes = ChronoUnit.MINUTES.between(start, end)
    return if (minutes in 1..MINUTES_IN_DAY * 2L) minutes.toInt() else MINUTES_IN_DAY
}

/** The minutes that have gone of [date] when the clock of [zone] reads [time], held to the day. */
internal fun minuteOfDay(time: LocalDateTime, date: LocalDate, zone: ZoneId): Int {
    val total = dayMinutes(date, zone)
    val gone = ChronoUnit.MINUTES.between(date.atStartOfDay(zone), time.atZone(zone))
    return gone.coerceIn(0, total.toLong()).toInt()
}

/** The full-hour lines of [date], by elapsed time: the hour the clocks skip has none, the one they repeat has two. */
internal fun hourMarks(date: LocalDate, zone: ZoneId): List<HourMark> {
    val start = date.atStartOfDay(zone)
    val end = date.plusDays(1).atStartOfDay(zone)
    var t = start
    if (t.minute != 0 || t.second != 0 || t.nano != 0) t = t.truncatedTo(ChronoUnit.HOURS).plusHours(1)
    val marks = ArrayList<HourMark>()
    while (t.isBefore(end)) {
        marks.add(HourMark(ChronoUnit.MINUTES.between(start, t).toInt(), t.hour))
        t = t.plusHours(1)
    }
    return marks
}

/** The day view of [date]: [events] that are on it, placed side by side where they overlap. */
internal fun dayLayout(events: List<CalendarEvent>, date: LocalDate, zone: ZoneId): DayLayout {
    val here = events.filter { it.isOn(date) }.sortedWith(eventOrder)
    val (whole, timed) = here.partition { it.allDay }
    val total = dayMinutes(date, zone)
    val spans = timed.map { e ->
        val from = if (e.firstDay.isBefore(date)) 0 else minuteOfDay(e.start, date, zone)
        val to = if (e.lastDay.isAfter(date)) total else minuteOfDay(e.end, date, zone)
        Triple(e, from, maxOf(to, minOf(from + MIN_BLOCK_MINUTES, total)))
    }
    return DayLayout(whole, placeColumns(spans), total, hourMarks(date, zone))
}

/** Gives each of [spans] (
    event,
    start,
    end,
) a column; events that overlap, directly or through others, share a width. */
internal fun placeColumns(spans: List<Triple<CalendarEvent, Int, Int>>): List<Placed> {
    val out = ArrayList<Placed>()
    val cluster = ArrayList<Triple<CalendarEvent, Int, Int>>()
    var clusterEnd = Int.MIN_VALUE

    fun flush() {
        val ends = ArrayList<Int>()
        val columnOf = cluster.map { (_, from, to) ->
            val free = ends.indexOfFirst { it <= from }
            if (free >= 0) {
                ends[free] = to
                free
            } else {
                ends.add(to)
                ends.size - 1
            }
        }
        cluster.forEachIndexed { i, (e, from, to) -> out.add(Placed(e, columnOf[i], ends.size, from, to)) }
        cluster.clear()
    }
    for (span in spans.sortedWith(compareBy({ it.second }, { it.third }))) {
        if (cluster.isNotEmpty() && span.second >= clusterEnd) {
            flush()
            clusterEnd = Int.MIN_VALUE
        }
        cluster.add(span)
        clusterEnd = maxOf(clusterEnd, span.third)
    }
    if (cluster.isNotEmpty()) flush()
    return out
}
