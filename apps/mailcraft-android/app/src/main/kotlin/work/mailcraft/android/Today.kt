// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.LocalDateTime
import java.time.format.DateTimeFormatter
import java.time.format.DateTimeParseException
import java.time.format.FormatStyle
import java.util.Locale
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.EventRecord
import uniffi.mailcraft_mobile.TodayRecord

/** An upcoming event on the Today card. */
internal data class TodayEvent(
    val id: ULong,
    val title: String,
    /** `YYYY-MM-DDTHH:MM:SS` in the owner's zone, or `YYYY-MM-DD` for a day-long event. */
    val start: String,
    val allDay: Boolean,
    val location: String,
    val meeting: Boolean,
)

/** What Today shows: the next events, the tasks due, and how many unread messages wait. */
internal data class TodayModel(
    /** The owner's today, `YYYY-MM-DD`. */
    val date: String,
    val events: List<TodayEvent>,
    val tasks: List<TaskRow>,
    val unread: Int,
)

internal sealed interface TodayState {
    data object Loading : TodayState

    class Ready(val model: TodayModel) : TodayState

    class Failed(val message: String) : TodayState
}

/** The heading of Today: the date in full, as the language writes it. */
internal fun todayHeading(date: String, locale: Locale = Locale.getDefault()): String = try {
    DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL).withLocale(locale).format(java.time.LocalDate.parse(date))
} catch (_: DateTimeParseException) {
    date
}

/** How many events and tasks a card lists before it stops. */
internal const val CARD_LIMIT = 5

internal fun todayEvent(record: EventRecord): TodayEvent = TodayEvent(
    id = record.event,
    title = record.title,
    start = record.start,
    allDay = record.allDay,
    location = record.location,
    meeting = record.meeting,
)

internal fun todayModel(record: TodayRecord): TodayModel = TodayModel(
    date = record.date,
    events = record.nextEvents.take(CARD_LIMIT).map(::todayEvent),
    tasks = record.tasks.map(::taskRow),
    unread = record.unread.toInt(),
)

/**
 * When an event starts, for people: the day (Today, Tomorrow, or the date) and the time, or only
 * the day for one that lasts all day. A start that cannot be read is shown as it is.
 */
internal fun eventWhen(
    texts: Texts,
    event: TodayEvent,
    today: String,
    locale: Locale = Locale.getDefault(),
): String {
    val at = try {
        LocalDateTime.parse(if (event.allDay) event.start + "T00:00:00" else event.start)
    } catch (_: DateTimeParseException) {
        return event.start
    }
    val day = dueLabel(texts, at.toLocalDate().toString(), today, locale)
    if (event.allDay) return day
    val time = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).withLocale(locale).format(at)
    return "$day, $time"
}

internal suspend fun loadToday(
    core: Core,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): TodayState = withContext(dispatcher) {
    try {
        TodayState.Ready(todayModel(core.today()))
    } catch (_: CoreException) {
        TodayState.Failed(core.t("Today could not be read."))
    }
}
