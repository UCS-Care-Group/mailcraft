// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.format.DateTimeFormatter
import java.util.Locale

/** How a series repeats. */
internal enum class Repeat(val code: String) { Daily("daily"), Weekly("weekly"), Monthly("monthly"), Yearly("yearly") }

/** How a series ends. */
internal sealed interface RepeatEnd {
    data object Never : RepeatEnd

    data class After(val count: Int) : RepeatEnd

    data class On(val day: LocalDate) : RepeatEnd
}

/**
 * The repeat part of an event being edited. A rule this editor can't show (the second Tuesday of
 * a month, say, which a server may have made) is [kept]: it is told as text and left as it is, so
 * that saving never rewrites a series into another one.
 */
internal data class RepeatDraft(
    val repeat: Repeat? = null,
    val interval: Int = 1,
    val days: Set<DayOfWeek> = emptySet(),
    val end: RepeatEnd = RepeatEnd.Never,
    val kept: Boolean = false,
) {
    /** The `recurrence` of the core's `calendar.new`, or null for none. */
    fun json(): String? {
        val freq = repeat ?: return null
        val parts = arrayListOf(
            "\"freq\":${jsonString(freq.code)}",
            "\"interval\":${interval.coerceIn(1, MAX_INTERVAL)}",
        )
        if (freq == Repeat.Weekly && days.isNotEmpty()) {
            val codes = days.sorted().joinToString(",") { jsonString(it.name.take(DAY_CODE).lowercase(Locale.ROOT)) }
            parts.add("\"byDay\":[$codes]")
        }
        when (val e = end) {
            RepeatEnd.Never -> Unit
            is RepeatEnd.After -> parts.add("\"count\":${e.count.coerceIn(1, MAX_COUNT)}")
            is RepeatEnd.On -> parts.add("\"until\":${jsonString(e.day.toString())}")
        }
        return "{" + parts.joinToString(",") + "}"
    }

    companion object {
        const val MAX_INTERVAL = 99
        const val MAX_COUNT = 999
        private const val DAY_CODE = 3
    }
}

/** Where a new or whole-day event's times start, until the owner sets them. */
internal val DEFAULT_START: LocalTime = LocalTime.of(9, 0)
internal val DEFAULT_END: LocalTime = LocalTime.of(10, 0)

/** The minutes before the start that the reminder choices offer; null is no reminder. */
internal val reminderChoices: List<Int?> = listOf(null, 0, 5, 10, 15, 30, 60, 120, 1440, 2880, 10080)

/**
 * The words of an event as the editor was given them. The core hands over a long description, title or
 * list of people cut to size; a field the owner left as it was is not sent back, so that a save never
 * cuts what is kept.
 */
internal data class LoadedText(
    val title: String,
    val location: String,
    val description: String,
    val attendees: String,
    val optional: String,
    /** The people given are not all of them: the list can't be changed here, and is never sent back. */
    val peopleCut: Boolean = false,
)

/** An event being made or changed, by the owner's clock; a whole-day event's [endDate] is its last day. */
internal data class EventDraft(
    val id: ULong? = null,
    val calendar: ULong = 0uL,
    val title: String = "",
    val location: String = "",
    val description: String = "",
    /** The people whose coming is asked for, and [optional] those who may come. */
    val attendees: String = "",
    val optional: String = "",
    val allDay: Boolean = false,
    val startDate: LocalDate,
    val startTime: LocalTime = DEFAULT_START,
    val endDate: LocalDate = startDate,
    val endTime: LocalTime = DEFAULT_END,
    val reminder: Int? = null,
    val repeat: RepeatDraft = RepeatDraft(),
    /** Set when only one time of a series is being changed: the day it is on. */
    val occurrence: LocalDate? = null,
    /** The series' rule, in words, for an event whose rule is [RepeatDraft.kept]. */
    val repeatText: String = "",
    /**
     * For one time of a series: how many days after the series' first time it is. The draft shows that
     * time's own days; the command takes the series' days, and is given them back.
     */
    val seriesShift: Long = 0,
    /** What the editor was given, for a change; null for a new event. */
    val loaded: LoadedText? = null,
) {
    val isNew: Boolean get() = id == null

    val start: LocalDateTime get() = if (allDay) startDate.atStartOfDay() else startDate.atTime(startTime)

    /** The end as a moment: the end of the last day for a whole-day event. */
    val end: LocalDateTime get() = if (allDay) endDate.plusDays(1).atStartOfDay() else endDate.atTime(endTime)

    /** Something is wrong that the owner must put right before saving: the end is before the start. */
    val endBeforeStart: Boolean get() = end.isBefore(start)

    /** Whether the meeting names people (so that a server would write to them). */
    val hasPeople: Boolean get() = attendees.any { it == '@' } || optional.any { it == '@' }

    /** Moves the start to [date], keeping the length of the event. */
    fun withStartDate(date: LocalDate): EventDraft {
        val shift = java.time.temporal.ChronoUnit.DAYS.between(startDate, date)
        return copy(startDate = date, endDate = endDate.plusDays(shift))
    }

    /** Moves the start time to [time], keeping the length of the event on the same day or after it. */
    fun withStartTime(time: LocalTime): EventDraft {
        val length = java.time.Duration.between(start, end).coerceAtLeast(java.time.Duration.ZERO)
        val moved = startDate.atTime(time).plus(length)
        return copy(startTime = time, endDate = moved.toLocalDate(), endTime = moved.toLocalTime())
    }

    /**
     * The params of `calendar.new` / `calendar.edit`. [send] is what the owner said about writing to the
     * people invited (null when nobody is, or when the event is on this phone only).
     */
    fun params(send: Boolean?): String {
        val parts = ArrayList<String>()
        id?.let { parts.add("\"id\":$it") }
        // A change sends back only the words the owner changed: what the core handed over may be cut.
        val was = loaded?.takeIf { !isNew }
        if (was == null || title != was.title) parts.add("\"title\":${jsonString(title.trim())}")
        parts.add("\"allDay\":$allDay")
        parts.addAll(timesJson())
        parts.addAll(wordsJson(was))
        parts.add("\"reminder\":${reminder?.toString() ?: "null"}")
        if (occurrence != null) {
            parts.add("\"scope\":\"occurrence\"")
            parts.add("\"occurrence\":${jsonString(occurrence.toString())}")
        } else if (!repeat.kept) {
            parts.add("\"recurrence\":${repeat.json() ?: "null"}")
        }
        if (isNew && calendar != 0uL) parts.add("\"calendar\":$calendar")
        send?.let { parts.add("\"send\":$it") }
        return "{" + parts.joinToString(",") + "}"
    }

    /** The start and end; one time of a series is written as the series' days, which the command moves it by. */
    private fun timesJson(): List<String> {
        if (allDay) {
            return listOf(
                "\"start\":${jsonString(startDate.minusDays(seriesShift).toString())}",
                "\"end\":${jsonString(endDate.minusDays(seriesShift).toString())}",
            )
        }
        val fmt = DateTimeFormatter.ISO_LOCAL_DATE_TIME
        val from = start.minusDays(seriesShift).withSecond(0).withNano(0)
        val to = end.minusDays(seriesShift).withSecond(0).withNano(0)
        return listOf(
            "\"start\":${jsonString(from.format(fmt).take(DATETIME_LENGTH))}",
            "\"end\":${jsonString(to.format(fmt).take(DATETIME_LENGTH))}",
        )
    }

    /** The location, notes and people: for a change ([was] not null), only those the owner changed. */
    private fun wordsJson(was: LoadedText?): List<String> {
        val parts = ArrayList<String>()
        if (was == null || location != was.location) parts.add("\"location\":${jsonString(location.trim())}")
        if (was == null || description != was.description) parts.add("\"description\":${jsonString(description)}")
        // The core rebuilds the list from both fields together, so a change of either sends both; a list
        // the editor was given only part of is never sent, as it would drop whoever was not given.
        val keepPeople = was != null && (was.peopleCut || (attendees == was.attendees && optional == was.optional))
        if (!keepPeople) {
            parts.add("\"attendees\":${jsonString(attendees.trim())}")
            if (was != null || optional.isNotBlank()) parts.add("\"optional\":${jsonString(optional.trim())}")
        }
        return parts
    }

    companion object {
        private const val DATETIME_LENGTH = 16

        /** A new event on [day], at the next half hour after [now] when [day] is today. */
        fun blank(day: LocalDate, now: LocalDateTime, calendar: ULong): EventDraft {
            val start = if (day == now.toLocalDate()) nextHalfHour(now) else day.atTime(DEFAULT_START)
            val end = start.plusMinutes(MIN_BLOCK_MINUTES.toLong())
            return EventDraft(
                calendar = calendar,
                startDate = start.toLocalDate(),
                startTime = start.toLocalTime(),
                endDate = end.toLocalDate(),
                endTime = end.toLocalTime(),
            )
        }

        private fun nextHalfHour(now: LocalDateTime): LocalDateTime {
            val rounded = now.withSecond(0).withNano(0)
            val over = rounded.minute % MIN_BLOCK_MINUTES
            return rounded.minusMinutes(over.toLong()).plusMinutes(MIN_BLOCK_MINUTES.toLong())
        }
    }
}

/** What the owner is asked before a command that makes a server write to people. */
internal sealed interface Ask {
    /** Saving a meeting with people invited on a server's calendar: write to them, or keep it here. */
    data class Invite(val draft: EventDraft) : Ask

    /** An answer to an invitation: tell the organiser now, or only keep it. */
    data class Reply(val event: CalendarEvent, val answer: Answer) : Ask

    /** Deleting: all of a series or one time of it; and for a meeting organised here, telling the people or not. */
    data class Delete(val detail: EventDetailModel, val occurrence: LocalDate?) : Ask
}

/** `text` as a JSON string, quoted, with what JSON can't hold escaped. */
internal fun jsonString(text: String): String {
    val out = StringBuilder(text.length + 2)
    out.append('"')
    for (c in text) {
        when {
            c == '"' -> out.append("\\\"")
            c == '\\' -> out.append("\\\\")
            c == '\n' -> out.append("\\n")
            c == '\r' -> out.append("\\r")
            c == '\t' -> out.append("\\t")
            c < ' ' || c == ' ' || c == ' ' -> out.append("\\u").append(String.format(Locale.ROOT, "%04x", c.code))
            else -> out.append(c)
        }
    }
    return out.append('"').toString()
}

/** The params of `calendar.respond` for [event]; [occurrence] is the day when only one time of a series is answered. */
internal fun answerParams(event: ULong, answer: Answer, send: Boolean, occurrence: LocalDate?): String {
    val scope = occurrenceScope(occurrence)
    return "{\"id\":$event,\"response\":${jsonString(answer.command)},\"send\":$send$scope}"
}

/** The params of `calendar.delete`. */
internal fun deleteParams(event: ULong, occurrence: LocalDate?, send: Boolean?): String {
    val scope = occurrenceScope(occurrence)
    val sent = send?.let { ",\"send\":$it" }.orEmpty()
    return "{\"id\":$event$scope$sent}"
}

/** The `scope` and `occurrence` params for one time of a series, or nothing. */
private fun occurrenceScope(occurrence: LocalDate?): String =
    if (occurrence == null) "" else ",\"scope\":\"occurrence\",\"occurrence\":${jsonString(occurrence.toString())}"
