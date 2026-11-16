// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.ScheduledReminder

/**
 * How many reminders the core is asked for: its own cap (`MAX_SCHEDULED`, the plan's 64, which
 * iPhone keeps too), far inside the 500 alarms Android lets an app hold.
 */
internal const val REMINDERS_ASKED = 64

/**
 * How many reminders already shown or dismissed are remembered, so that a later ask doesn't raise
 * them again; as many as the desktop remembers dismissals. Older ones are for events long past.
 */
internal const val REMINDERS_REMEMBERED = 400

/** How long Snooze puts a reminder off, as the desktop's own Snooze from a desktop alert. */
internal const val SNOOZE_MINUTES = 5L

private const val MILLIS_PER_SECOND = 1000L

/** What the app keeps of its reminders between runs: keys and times only, never a title. */
internal data class Remembered(
    /** The reminders the last ask gave, by key, with when each one's event starts (ms). */
    val scheduled: Map<Long, Long> = emptyMap(),
    /** Reminders shown or dismissed, oldest first, at most [REMINDERS_REMEMBERED]. */
    val handled: List<Long> = emptyList(),
    /** Reminders put off, by key, until when (ms). */
    val snoozed: Map<Long, Long> = emptyMap(),
)

/** What the core said: the next reminders not yet shown, and which of those shown are still to come. */
internal data class ReminderAnswer(val reminders: List<ScheduledReminder>, val kept: Set<Long> = emptySet())

/**
 * How long after its time a snooze that never came is kept: its alarm may still be on its way (a
 * snooze stays until it is shown or called off), but one lost to a restart is not kept for ever.
 */
private const val SNOOZE_KEPT_AFTER_MILLIS = 24L * 60L * 60L * 1000L

/** The snoozes still owed: shown when their alarm comes, until a day after their time. */
internal fun owedSnoozes(remembered: Remembered, nowMillis: Long): Map<Long, Long> =
    remembered.snoozed.filterValues { it > nowMillis.saturatingMinus(SNOOZE_KEPT_AFTER_MILLIS) }

private fun Long.saturatingMinus(other: Long): Long =
    if (this < Long.MIN_VALUE + other) Long.MIN_VALUE else this - other

/** The keys the core is asked to leave out: shown or dismissed, and not snoozed (a snooze is set again). */
internal fun skipped(remembered: Remembered, nowMillis: Long): List<Long> {
    val snoozed = owedSnoozes(remembered, nowMillis).keys
    return remembered.handled.filter { it !in snoozed }
}

/** One alarm to set: the reminder, and when the system is to raise it (ms). */
internal data class ReminderAlarm(val reminder: ScheduledReminder, val atMillis: Long) {
    val key: Long get() = reminder.key.toLong()
}

/** What to do with the system's alarms after an ask, and what to remember after it. */
internal data class ReminderPlan(
    val set: List<ReminderAlarm>,
    /** Alarms to call off: the reminder went from the schedule (its event moved, was deleted or started). */
    val cancel: List<Long>,
    /** Notifications showing to take away: their event moved or was deleted before it started. */
    val withdraw: List<Long>,
    val remembered: Remembered,
)

/** [seconds] since 1970 as milliseconds, held at the ends instead of wrapping round. */
internal fun secondsToMillis(seconds: Long): Long = when {
    seconds > Long.MAX_VALUE / MILLIS_PER_SECOND -> Long.MAX_VALUE
    seconds < Long.MIN_VALUE / MILLIS_PER_SECOND -> Long.MIN_VALUE
    else -> seconds * MILLIS_PER_SECOND
}

/**
 * Works out the alarms for [asked], the core's next reminders, against what was [remembered].
 * A key the core says is [ReminderAnswer.kept] (shown already and still to come) has not left the schedule.
 * A reminder already shown or dismissed is not raised again, unless it is snoozed (then it comes
 * at the snooze's end). One that left the schedule has its alarm called off; when its event had
 * not started yet (it moved or was deleted), a notification of it that shows is taken away too,
 * while one whose event started keeps a snooze running. With reminders [on] false, every alarm
 * is called off and nothing is set.
 */
internal fun planReminders(
    asked: ReminderAnswer,
    remembered: Remembered,
    nowMillis: Long,
    on: Boolean,
): ReminderPlan {
    val live = owedSnoozes(remembered, nowMillis)
    if (!on) {
        val all = (remembered.scheduled.keys + live.keys).distinct()
        return ReminderPlan(emptyList(), all, emptyList(), Remembered(handled = remembered.handled))
    }
    val current = asked.reminders.take(REMINDERS_ASKED).distinctBy { it.key }
    val keys = current.mapTo(HashSet()) { it.key.toLong() } + asked.kept
    val handled = remembered.handled.toHashSet()
    val removed = remembered.scheduled.filterKeys { it !in keys }
    val withdraw = removed.filterValues { it > nowMillis }.keys
    val snoozed = live.filterKeys { it !in withdraw }
    val cancel = removed.keys.filter { it !in snoozed }
    val set = current.mapNotNull { r ->
        val key = r.key.toLong()
        when {
            key in snoozed -> ReminderAlarm(r, snoozed.getValue(key))
            key in handled -> null
            else -> ReminderAlarm(r, secondsToMillis(r.at))
        }
    }
    val scheduled = remembered.scheduled.filterKeys { it in asked.kept } +
        current.associate { it.key.toLong() to secondsToMillis(it.starts) }
    return ReminderPlan(set, cancel, withdraw.toList(), Remembered(scheduled, remembered.handled, snoozed))
}

/** [remembered] with [key] shown or dismissed: no longer snoozed, and the oldest forgotten past the cap. */
internal fun handledOne(remembered: Remembered, key: Long): Remembered {
    val handled = (remembered.handled.filter { it != key } + key).takeLast(REMINDERS_REMEMBERED)
    return remembered.copy(handled = handled, snoozed = remembered.snoozed - key)
}

/**
 * An alarm of [key] came: whether it is shown (a snooze is owed, or the last schedule still has it
 * and it wasn't shown already: an alarm set again by a refresh that raced the first one shows
 * nothing), and [remembered] with it handled when it is.
 */
internal fun firing(remembered: Remembered, key: Long): Pair<Remembered, Boolean> {
    val known = key in remembered.snoozed || (key in remembered.scheduled && key !in remembered.handled)
    return (if (known) handledOne(remembered, key) else remembered) to known
}

/** [remembered] with [key] put off until [untilMillis]. */
internal fun snoozedOne(remembered: Remembered, key: Long, untilMillis: Long): Remembered =
    remembered.copy(snoozed = remembered.snoozed + (key to untilMillis))

/** What a reminder's notification says under its title: when, and where (or that it is a task). */
internal fun reminderText(payload: ReminderPayload, at: String): String {
    val first = when {
        payload.task -> payload.words.task
        payload.allDay -> payload.words.allDay
        else -> ""
    }
    return listOf(first, at, payload.location).filter { it.isNotBlank() }.joinToString(" · ")
}
