// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DateTimeException
import java.time.DayOfWeek
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException
import java.time.format.FormatStyle
import java.util.Locale

internal fun parseDay(date: String): LocalDate? = try {
    LocalDate.parse(date)
} catch (_: DateTimeParseException) {
    null
}

/** A due date for people: Yesterday, Today, Tomorrow, else the date as the language writes it. */
internal fun dueLabel(
    texts: Texts,
    due: String,
    today: String,
    locale: Locale = Locale.getDefault(),
): String {
    val day = parseDay(due) ?: return due
    val now = parseDay(today)
    return when {
        now == null -> due
        day == now -> texts.t("Today")
        day == now.plusDays(1) -> texts.t("Tomorrow")
        day == now.minusDays(1) -> texts.t("Yesterday")
        else -> DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM).withLocale(locale).format(day)
    }
}

/** The quick choices of a due date: today, tomorrow, and the Monday after. */
internal fun quickDates(today: String): List<Pair<String, String>> {
    val now = parseDay(today) ?: return emptyList()
    val tomorrow = now.plusDays(1)
    // Never tomorrow again: from a Sunday the next Monday is the one after the Monday that follows.
    val ahead = (DayOfWeek.MONDAY.value - tomorrow.dayOfWeek.value + DAYS_A_WEEK) % DAYS_A_WEEK
    val toMonday = if (ahead == 0) DAYS_A_WEEK else ahead
    val monday = tomorrow.plusDays(toMonday.toLong())
    return listOf("today" to now.toString(), "tomorrow" to now.plusDays(1).toString(), "monday" to monday.toString())
}

private const val DAYS_A_WEEK = 7

/** The core's calendar ends with the year 9999. */
private const val MAX_YEAR = 9_999

/** The date picker names a day by the milliseconds of its midnight in UTC. */
internal fun dateToMillis(date: String?): Long? {
    val day = date?.let(::parseDay) ?: return null
    return day.atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli()
}

/** The day a date picker's milliseconds name, `YYYY-MM-DD`, or null for a value no calendar has. */
internal fun millisToDate(millis: Long): String? = try {
    Instant.ofEpochMilli(millis).atZone(ZoneOffset.UTC).toLocalDate().takeIf { it.year in 1..MAX_YEAR }?.toString()
} catch (_: DateTimeException) {
    null
}

