// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/**
 * What a day says to a screen reader (a cell of the month grid, a column of the week, a page of the
 * day view): the whole date, whether it is today, and how many events are on it.
 */
internal fun dayDescription(texts: Texts, day: LocalDate, today: Boolean, events: Int?, locale: Locale): String {
    val date = DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale).format(day)
    return listOfNotNull(date, texts.t("Today").takeIf { today }, eventCount(texts, events)).joinToString(", ")
}

/** How many events there are, or that they are still being read when [events] is null. */
private fun eventCount(texts: Texts, events: Int?): String = when (events) {
    null -> texts.t("Loading…")
    0 -> texts.t("No events")
    else -> texts.tn(events.toULong(), "{n} event", "{n} events")
}

/**
 * What the page on screen says when it comes (a swipe, Previous or Next, Today): the day view its day
 * as [dayDescription] has it, the week its days and how many events are in them, the others their title.
 * [events] is null while the page's own read is on its way: what is drawn meanwhile is the read before,
 * which may not hold the whole page, so no count is said from it.
 */
internal fun pageDescription(
    texts: Texts,
    state: CalendarUiState,
    events: List<CalendarEvent>?,
    weekStart: DayOfWeek,
    locale: Locale,
): String {
    val range = state.range(weekStart)
    val count = events?.count { e -> e.lastDay >= range.from && e.firstDay <= range.to }
    return when (state.mode) {
        CalendarMode.Day -> dayDescription(texts, state.anchor, state.anchor == state.today, count, locale)
        CalendarMode.Week -> rangeTitle(state.mode, state.anchor, locale, weekStart) + ", " + eventCount(texts, count)
        CalendarMode.Agenda, CalendarMode.Month -> rangeTitle(state.mode, state.anchor, locale, weekStart)
    }
}

/** When an event is, as a line: its times, or "All day", and a note when it goes on past the day. */
internal fun timeLine(texts: Texts, e: CalendarEvent, day: LocalDate, locale: Locale): String {
    if (e.allDay) return texts.t("All day")
    val fmt = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale)
    val from = if (e.firstDay.isBefore(day)) texts.t("Continues") else e.start.toLocalTime().format(fmt)
    val to = if (e.lastDay.isAfter(day)) texts.t("Continues") else e.end.toLocalTime().format(fmt)
    return "$from – $to"
}

/** What a block of the week says to a screen reader, whose words may be cut short on screen: its title, its time. */
internal fun blockDescription(texts: Texts, e: CalendarEvent, day: LocalDate, locale: Locale): String =
    listOfNotNull(
        e.title.ifBlank { texts.t("(No title)") },
        timeLine(texts, e, day, locale),
        texts.t("Cancelled").takeIf { e.cancelled },
    ).joinToString(", ")
