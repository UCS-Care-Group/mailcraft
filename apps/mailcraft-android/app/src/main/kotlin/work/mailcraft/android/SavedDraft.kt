// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.DayOfWeek
import java.time.LocalDate
import java.time.LocalTime
import java.time.format.DateTimeParseException

/**
 * An event draft as plain words, kept by the system while the app is away, so that what the owner was
 * writing is there again when the app comes back. Only this phone writes it and reads it back; even
 * so, anything that does not read back whole gives no draft rather than a wrong one.
 */
internal object SavedDraft {
    fun write(d: EventDraft): HashMap<String, String> {
        val out = HashMap<String, String>()
        d.id?.let { out["id"] = it.toString() }
        out["calendar"] = d.calendar.toString()
        out["title"] = d.title
        out["location"] = d.location
        out["description"] = d.description
        out["attendees"] = d.attendees
        out["optional"] = d.optional
        out["allDay"] = d.allDay.toString()
        out["startDate"] = d.startDate.toString()
        out["startTime"] = d.startTime.toString()
        out["endDate"] = d.endDate.toString()
        out["endTime"] = d.endTime.toString()
        d.reminder?.let { out["reminder"] = it.toString() }
        d.repeat.repeat?.let { out["repeat"] = it.code }
        out["interval"] = d.repeat.interval.toString()
        out["days"] = d.repeat.days.sorted().joinToString(",") { it.name }
        when (val e = d.repeat.end) {
            RepeatEnd.Never -> Unit
            is RepeatEnd.After -> out["endsAfter"] = e.count.toString()
            is RepeatEnd.On -> out["endsOn"] = e.day.toString()
        }
        out["kept"] = d.repeat.kept.toString()
        d.occurrence?.let { out["occurrence"] = it.toString() }
        out["repeatText"] = d.repeatText
        out["seriesShift"] = d.seriesShift.toString()
        d.loaded?.let {
            out["was.title"] = it.title
            out["was.location"] = it.location
            out["was.description"] = it.description
            out["was.attendees"] = it.attendees
            out["was.optional"] = it.optional
        }
        return out
    }

    fun read(m: Map<String, String>): EventDraft? = try {
        parse(m)
    } catch (_: NumberFormatException) {
        null
    } catch (_: DateTimeParseException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    }

    // A missing day or time is an empty text, which doesn't parse.
    private fun parse(m: Map<String, String>) =
        EventDraft(
            id = m["id"]?.toULong(),
            calendar = m["calendar"]?.toULong() ?: 0uL,
            title = m["title"].orEmpty(),
            location = m["location"].orEmpty(),
            description = m["description"].orEmpty(),
            attendees = m["attendees"].orEmpty(),
            optional = m["optional"].orEmpty(),
            allDay = m["allDay"] == "true",
            startDate = LocalDate.parse(m["startDate"].orEmpty()),
            startTime = LocalTime.parse(m["startTime"].orEmpty()),
            endDate = LocalDate.parse(m["endDate"].orEmpty()),
            endTime = LocalTime.parse(m["endTime"].orEmpty()),
            reminder = m["reminder"]?.toInt(),
            repeat = RepeatDraft(
                repeat = m["repeat"]?.let { code -> Repeat.entries.firstOrNull { it.code == code } },
                interval = m["interval"]?.toInt() ?: 1,
                days = m["days"].orEmpty().split(',').filter { it.isNotEmpty() }.map { DayOfWeek.valueOf(it) }.toSet(),
                end = m["endsAfter"]?.let { RepeatEnd.After(it.toInt()) }
                    ?: m["endsOn"]?.let { RepeatEnd.On(LocalDate.parse(it)) }
                    ?: RepeatEnd.Never,
                kept = m["kept"] == "true",
            ),
            occurrence = m["occurrence"]?.let { LocalDate.parse(it) },
            repeatText = m["repeatText"].orEmpty(),
            seriesShift = m["seriesShift"]?.toLong() ?: 0L,
            loaded = m["was.title"]?.let {
                LoadedText(
                    title = it,
                    location = m["was.location"].orEmpty(),
                    description = m["was.description"].orEmpty(),
                    attendees = m["was.attendees"].orEmpty(),
                    optional = m["was.optional"].orEmpty(),
                )
            },
        )
}
