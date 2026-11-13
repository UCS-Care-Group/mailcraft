// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MagicNumber", "MaxLineLength", "LongParameterList")

package work.mailcraft.android

import java.io.ByteArrayInputStream
import java.time.DayOfWeek
import java.time.LocalDate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CalendarMappingTest {
    @Test
    fun aRuleTheEditorCanShowIsShownAndOneItCannotIsKept() {
        val weekly = repeatOf(RuleFacts("weekly", 2u, listOf("mon", "wed"), null, 6u, null))
        assertEquals(RepeatDraft(Repeat.Weekly, 2, setOf(DayOfWeek.MONDAY, DayOfWeek.WEDNESDAY), RepeatEnd.After(6)), weekly)
        val until = repeatOf(RuleFacts("daily", 1u, emptyList(), null, null, "2026-12-31"))
        assertEquals(RepeatEnd.On(LocalDate.parse("2026-12-31")), until.end)
        assertFalse(until.kept)
        // The second Tuesday of the month, a rule with a day it can't name, an interval beyond reach, both ends.
        assertTrue(repeatOf(RuleFacts("monthly", 1u, listOf("tue"), 2, null, null)).kept)
        assertTrue(repeatOf(RuleFacts("weekly", 1u, listOf("xyz"), null, null, null)).kept)
        assertTrue(repeatOf(RuleFacts("daily", 1u, listOf("mon"), null, null, null)).kept)
        assertTrue(repeatOf(RuleFacts("weekly", 500u, emptyList(), null, null, null)).kept)
        assertTrue(repeatOf(RuleFacts("weekly", 1u, emptyList(), null, 3u, "2026-12-31")).kept)
        assertTrue(repeatOf(RuleFacts("fortnightly", 1u, emptyList(), null, null, null)).kept)
        assertTrue(repeatOf(RuleFacts("daily", 1u, emptyList(), null, null, "not a day")).kept)
    }

    @Test
    fun repeatsAndRemindersAreSaidInWords() {
        assertEquals("Every week", describeRepeat("weekly", 1u, EnglishTexts))
        assertEquals("Every 3 months", describeRepeat("monthly", 3u, EnglishTexts))
        assertEquals("Every day", describeRepeat("daily", 0u, EnglishTexts))
        assertEquals("", describeRepeat("nonsense", 1u, EnglishTexts))
        assertEquals("No reminder", reminderLabel(null, EnglishTexts))
        assertEquals("At the time of the event", reminderLabel(0, EnglishTexts))
        assertEquals("15 minutes before", reminderLabel(15, EnglishTexts))
        assertEquals("1 hour before", reminderLabel(60, EnglishTexts))
        assertEquals("2 days before", reminderLabel(2880, EnglishTexts))
        assertEquals("1 week before", reminderLabel(10080, EnglishTexts))
        assertEquals("90 minutes before", reminderLabel(90, EnglishTexts))
        assertTrue(reminderChoices.all { reminderLabel(it, EnglishTexts).isNotBlank() })
    }

    @Test
    fun aCalendarFileIsHandedOverAsANameAndBase64() {
        assertEquals("{\"name\":\"invite.ics\",\"data\":\"QUJD\"}", fileImportParams("invite.ics", "ABC".toByteArray()))
        assertEquals("a.ics", icsFileName("/storage/../a.ics"))
        assertEquals("b.ics", icsFileName("C:\\x\\b.ics"))
        assertEquals("calendar.ics", icsFileName("evil.exe"))
        assertEquals("calendar.ics", icsFileName(""))
        assertEquals("a.ics", icsFileName("a\u0000\n.ics"))
        assertTrue(icsFileName("x".repeat(1000) + ".ics").length <= 255)
    }

    @Test
    fun whatAnImportDidIsSaidInWords() {
        assertEquals("1 event added to the calendar.", importedNote("{\"format\":\"ics\",\"events\":1,\"tasks\":0}", EnglishTexts))
        assertEquals("3 events added to the calendar.", importedNote("{\"events\": 3}", EnglishTexts))
        assertEquals("The file holds no events.", importedNote("{\"events\":0,\"tasks\":0}", EnglishTexts))
        assertEquals("The file holds no events.", importedNote("", EnglishTexts))
        assertEquals(0, numberField("{\"events\":99999999999999}", "events"))
        assertEquals(7, numberField("{\"a\":1,\"tasks\":7}", "tasks"))
    }

    @Test
    fun calendarTypesAreKnownAndFilesAreReadToTheirCapOnly() {
        assertTrue(isCalendarType("text/calendar"))
        assertTrue(isCalendarType("Text/Calendar; charset=utf-8"))
        assertTrue(isCalendarType("application/ics"))
        assertFalse(isCalendarType("text/plain"))
        assertFalse(isCalendarType(null))
        assertEquals(3, readCapped(ByteArrayInputStream(ByteArray(3)), 3)?.size)
        assertNull(readCapped(ByteArrayInputStream(ByteArray(4)), 3))
        assertEquals(0, readCapped(ByteArrayInputStream(ByteArray(0)), 3)?.size)
        assertNull(readCapped(ByteArrayInputStream(ByteArray(ICS_MAX_BYTES + 1)), ICS_MAX_BYTES))
    }
}
