// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.ReminderKind
import uniffi.mailcraft_mobile.ScheduledReminder

private const val NOW = 1_800_000_000_000L
private const val HOUR = 3_600L

private fun reminder(key: Long, inHours: Long, title: String = "Standup", kind: ReminderKind = ReminderKind.EVENT) =
    ScheduledReminder(
        key = key.toULong(),
        kind = kind,
        at = NOW / 1000 + inHours * HOUR - HOUR / 4,
        starts = NOW / 1000 + inHours * HOUR,
        allDay = false,
        title = title,
        location = "",
    )

private class FakeAlarms : AlarmSink {
    val set = mutableListOf<ReminderAlarm>()
    val cancelled = mutableListOf<Long>()
    val withdrawn = mutableListOf<Long>()

    override fun set(alarm: ReminderAlarm, words: ReminderWords) {
        set.add(alarm)
    }

    override fun cancel(key: Long) {
        cancelled.add(key)
    }

    override fun withdraw(key: Long) {
        withdrawn.add(key)
    }
}

private val twoAsked = Asked(listOf(reminder(1, 2), reminder(2, 5)))

private val words = ReminderWords("Reminders", "Dismiss", "Snooze", "Task", "All day")

/** The reminders' alarms: set, replaced, called off and remembered as the core's schedule says. */
class RemindersTest {
    @Test
    fun eachReminderGetsAnAlarmAtItsTimeAndIsRememberedByKeyAndStart() {
        val plan = planReminders(twoAsked, Remembered(), NOW, on = true)
        assertEquals(listOf(1L, 2L), plan.set.map { it.key })
        assertEquals(NOW + 2 * HOUR * 1000 - HOUR / 4 * 1000, plan.set[0].atMillis)
        assertEquals(mapOf(1L to NOW + 2 * HOUR * 1000, 2L to NOW + 5 * HOUR * 1000), plan.remembered.scheduled)
        assertTrue(plan.cancel.isEmpty() && plan.withdraw.isEmpty())
    }

    @Test
    fun aMovedOrDeletedEventLosesItsAlarmAndItsNotification() {
        val before = planReminders(twoAsked, Remembered(), NOW, on = true).remembered
        // Event 1 moved (a new key) and event 2 was deleted, both before they started.
        val plan = planReminders(Asked(listOf(reminder(3, 3))), before, NOW, on = true)
        assertEquals(listOf(3L), plan.set.map { it.key })
        assertEquals(setOf(1L, 2L), plan.cancel.toSet())
        assertEquals(setOf(1L, 2L), plan.withdraw.toSet())
    }

    @Test
    fun anEventThatStartedKeepsItsNotificationAndItsSnooze() {
        val before = planReminders(Asked(listOf(reminder(1, 2))), Remembered(), NOW, on = true).remembered
        val snoozed = snoozedOne(handledOne(before, 1), 1, NOW + 3 * HOUR * 1000 + 60_000)
        // Three hours later the event has started and the core no longer lists it.
        val plan = planReminders(Asked(emptyList()), snoozed, NOW + 3 * HOUR * 1000, on = true)
        assertTrue(plan.withdraw.isEmpty())
        assertTrue(plan.cancel.isEmpty())
        assertEquals(setOf(1L), plan.remembered.snoozed.keys)
    }

    @Test
    fun aShownReminderIsNotRaisedAgainUnlessSnoozed() {
        val shown = handledOne(Remembered(), 1)
        assertTrue(planReminders(Asked(listOf(reminder(1, 2))), shown, NOW, on = true).set.isEmpty())
        val until = NOW + 5 * 60_000
        val plan = planReminders(Asked(listOf(reminder(1, 2))), snoozedOne(shown, 1, until), NOW, on = true)
        assertEquals(listOf(until), plan.set.map { it.atMillis })
        // A snooze whose time went is forgotten.
        val gone = snoozedOne(shown, 1, NOW - 1)
        assertTrue(planReminders(Asked(listOf(reminder(1, 2))), gone, NOW, on = true).set.isEmpty())
    }

    @Test
    fun theSwitchOffCallsOffEveryAlarmAndSetsNone() {
        val before = planReminders(twoAsked, Remembered(), NOW, on = true).remembered
        val plan = planReminders(Asked(listOf(reminder(1, 2))), snoozedOne(before, 9, NOW + 1), NOW, on = false)
        assertTrue(plan.set.isEmpty())
        assertEquals(setOf(1L, 2L, 9L), plan.cancel.toSet())
        assertTrue(plan.remembered.scheduled.isEmpty() && plan.remembered.snoozed.isEmpty())
    }

    @Test
    fun theScheduleIsHeldToItsCapAndTheRememberedToTheirs() {
        val many = (1L..200L).map { reminder(it, it) }
        assertEquals(REMINDERS_ASKED, planReminders(Asked(many), Remembered(), NOW, on = true).set.size)
        val remembered = (1L..1_000L).fold(Remembered()) { r, key -> handledOne(r, key) }
        assertEquals(REMINDERS_REMEMBERED, remembered.handled.size)
        assertEquals(1_000L, remembered.handled.last())
    }

    @Test
    fun timesFarOutAreHeldNotWrapped() {
        assertEquals(Long.MAX_VALUE, secondsToMillis(Long.MAX_VALUE))
        assertEquals(Long.MIN_VALUE, secondsToMillis(Long.MIN_VALUE))
        assertEquals(5_000L, secondsToMillis(5))
    }

    @Test
    fun whatIsKeptReadsBackAndJunkIsForgotten() {
        val kept = Remembered(mapOf(1L to 2L, -3L to 4L), listOf(5L, 6L), mapOf(7L to 8L))
        val handled = kept.handled.joinToString(",")
        val back = decodeRemembered(encodePairs(kept.scheduled), handled, encodePairs(kept.snoozed))
        assertEquals(kept, back)
        assertEquals(Remembered(), decodeRemembered("x:1,2,:", "a,,b", "1:2:3"))
        val many = (1..5_000).joinToString(",")
        assertEquals(REMINDERS_REMEMBERED, decodeRemembered("", many, "").handled.size)
    }

    @Test
    fun theSchedulerAppliesThePlanAndLeavesAllAsItWasWhenTheCoreCannotSay() = runBlocking {
        val alarms = FakeAlarms()
        val store = MemoryReminderStore()
        var on = true
        val scheduler = ReminderScheduler(alarms, store, { NOW }) { on }
        assertTrue(scheduler.refresh(words) { Asked(listOf(reminder(1, 2))) })
        assertEquals(listOf(1L), alarms.set.map { it.key })
        assertEquals(false, scheduler.refresh(words) { null })
        assertEquals(mapOf(1L to NOW + 2 * HOUR * 1000), store.remembered.scheduled)
        on = false
        scheduler.refresh(words) { error("not asked with the switch off") }
        assertEquals(listOf(1L), alarms.cancelled)
    }

    @Test
    fun theTextSaysWhenWhereAndWhetherItIsATask() {
        val payload = ReminderPayload(1, false, "Standup", "Room 4", 0, false, words)
        assertEquals("10:00 · Room 4", reminderText(payload, "10:00"))
        assertEquals("Task · 10:00", reminderText(payload.copy(task = true, location = ""), "10:00"))
        assertEquals("All day · 12 Oct", reminderText(payload.copy(allDay = true, location = ""), "12 Oct"))
    }

    @Test
    fun missingWordsFallBackToEnglish() {
        assertEquals(words, ReminderWords.fromList(emptyList()))
        assertEquals("Später", ReminderWords.fromList(listOf("", "", "Später")).snooze)
        assertEquals(words, ReminderWords.fromList(words.toList() + "extra"))
    }

    @Test
    fun aShownReminderStillToComeIsNotTakenForOneThatWent() {
        val before = planReminders(twoAsked, Remembered(), NOW, on = true).remembered
        val shown = handledOne(before, 1)
        assertEquals(listOf(1L), skipped(shown, NOW))
        // The core leaves 1 out, as asked, and says it is still to come.
        val plan = planReminders(Asked(listOf(reminder(2, 5)), kept = setOf(1L)), shown, NOW, on = true)
        assertTrue(plan.cancel.isEmpty() && plan.withdraw.isEmpty())
        assertEquals(setOf(1L, 2L), plan.remembered.scheduled.keys)
        // A snoozed one is asked for again, to be set at the snooze's end.
        assertTrue(skipped(snoozedOne(shown, 1, NOW + 60_000), NOW).isEmpty())
    }

    @Test
    fun anAlarmOfAReminderNoLongerScheduledShowsNothing() {
        val kept = planReminders(Asked(listOf(reminder(1, 2))), Remembered(), NOW, on = true).remembered
        val (after, shown) = firing(kept, 1)
        assertTrue(shown)
        assertEquals(listOf(1L), after.handled)
        assertEquals(kept to false, firing(kept, 99))
        assertTrue(firing(snoozedOne(Remembered(), 7, NOW), 7).second)
    }
}
