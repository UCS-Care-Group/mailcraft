// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.ScheduledReminder

/** The words a reminder's notification uses, in the interface language, put with each alarm when it is set. */
internal data class ReminderWords(
    /** What a locked screen that hides private content shows instead of the title. */
    val hidden: String,
    val dismiss: String,
    val snooze: String,
    val task: String,
    val allDay: String,
) {
    fun toList() = listOf(hidden, dismiss, snooze, task, allDay)

    companion object {
        /** The words in English, the catalogues' own keys. */
        val ENGLISH = ReminderWords("Reminders", "Dismiss", "Snooze", "Task", "All day")

        /** The words in the interface language. */
        fun from(t: (String) -> String) =
            ReminderWords(t("Reminders"), t("Dismiss"), t("Snooze"), t("Task"), t("All day"))

        /** The words in [given] order, English for each one missing or empty. */
        fun fromList(given: List<String>): ReminderWords {
            val words = ENGLISH.toList()
                .mapIndexed { i, english -> given.getOrNull(i)?.takeIf { it.isNotEmpty() } ?: english }
                .iterator()
            return ReminderWords(words.next(), words.next(), words.next(), words.next(), words.next())
        }
    }
}

/** The system's alarms as the scheduler uses them (Android's AlarmManager in the app; a list in tests). */
internal interface AlarmSink {
    fun set(alarm: ReminderAlarm, words: ReminderWords)

    fun cancel(key: Long)

    /** Takes away the notification of [key] when it shows. */
    fun withdraw(key: Long)
}

/**
 * Keeps the system's alarms in step with the core's next reminders: asked at every open, when the
 * core says its schedule changed (a sync or an edit moved, added or deleted one), when the
 * Reminders switch is changed, and after the phone restarts or its clock or zone changes (the
 * alarms are kept by the instant they are due, and a whole-day event's instant moves with the
 * zone). One refresh at a time.
 */
internal class ReminderScheduler(
    private val alarms: AlarmSink,
    private val store: ReminderStore,
    private val nowMillis: () -> Long,
    /** The owner's Reminders switch. */
    private val on: () -> Boolean,
) {
    private val lock = Mutex()

    /**
     * Asks [ask] for the schedule (null when the core couldn't say: nothing is changed then) and
     * sets, replaces and calls off the alarms.
     */
    suspend fun refresh(words: ReminderWords, ask: suspend () -> List<ScheduledReminder>?) = lock.withLock {
        val switchedOn = on()
        val schedule = if (switchedOn) ask() ?: return@withLock else emptyList()
        val plan = store.update { remembered ->
            val plan = planReminders(schedule, remembered, nowMillis(), switchedOn)
            plan.remembered to plan
        }
        plan.cancel.forEach(alarms::cancel)
        plan.withdraw.forEach(alarms::withdraw)
        plan.set.forEach { alarms.set(it, words) }
    }
}

/** The core's next reminders; null when it can't say. */
internal suspend fun askSchedule(core: Core): List<ScheduledReminder>? =
    try {
        core.reminderSchedule(REMINDERS_ASKED.toUInt()).reminders
    } catch (_: CoreException) {
        null
    }
