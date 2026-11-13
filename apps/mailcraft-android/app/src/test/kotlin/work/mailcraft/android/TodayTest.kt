// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TodayTest {
    private val today = "2026-10-14"

    private fun event(start: String, allDay: Boolean = false) = TodayEvent(1u, "Stand-up", start, allDay, "", false)

    @Test
    fun aTimedEventIsToldWithItsDayAndTime() {
        val said = eventWhen(EnglishTexts, event("2026-10-14T09:30:00"), today, Locale.US)
        assertTrue(said, said.startsWith("Today, "))
        assertTrue(said, said.contains("9:30"))
        assertTrue(eventWhen(EnglishTexts, event("2026-10-15T09:30:00"), today, Locale.US).startsWith("Tomorrow, "))
    }

    @Test
    fun anAllDayEventIsToldByItsDayAlone() {
        assertEquals("Today", eventWhen(EnglishTexts, event("2026-10-14", allDay = true), today, Locale.US))
    }

    @Test
    fun aStartThatCannotBeReadIsShownAsItIs() {
        assertEquals("sometime", eventWhen(EnglishTexts, event("sometime"), today, Locale.US))
    }

    @Test
    fun theHeadingIsTheDateInFullAndAnUnreadableDateIsItself() {
        assertTrue(todayHeading(today, Locale.US).contains("October"))
        assertEquals("never", todayHeading("never", Locale.US))
    }
}
