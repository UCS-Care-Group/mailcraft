// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FreshnessJsonTest {
    private val minute = 60_000L

    @Test
    fun theLineSaysOfflineBeforeAnythingElse() {
        assertEquals("Offline", freshnessLine(EnglishTexts, 10 * minute, 0L, offline = true))
        assertEquals("Offline", freshnessLine(EnglishTexts, 0L, null, offline = true))
    }

    @Test
    fun theLineSaysNothingItDoesNotKnow() {
        assertNull(freshnessLine(EnglishTexts, 10 * minute, null, offline = false))
    }

    @Test
    fun theLineCountsMinutesHoursAndDays() {
        val now = 100 * 24 * 60 * minute
        fun at(ago: Long) = freshnessLine(EnglishTexts, now, now - ago, offline = false)
        assertEquals("Updated just now", at(30_000L))
        assertEquals("Updated 1 minute ago", at(minute))
        assertEquals("Updated 5 minutes ago", at(5 * minute))
        assertEquals("Updated 1 hour ago", at(60 * minute))
        assertEquals("Updated 3 hours ago", at(3 * 60 * minute))
        assertEquals("Updated 2 days ago", at(2 * 24 * 60 * minute))
    }

    @Test
    fun aClockThatWentBackwardsIsJustNow() {
        assertEquals("Updated just now", freshnessLine(EnglishTexts, 0L, 5 * minute, offline = false))
    }

    @Test
    fun parametersAreWrittenAsJsonWithTheirCharactersEscaped() {
        assertEquals("""{"ids":[1,2],"folder":3}""", jsonText(mapOf("ids" to listOf(1uL, 2uL), "folder" to 3uL)))
        assertEquals(""""a\"b\\c\nd\u0001"""", jsonText("a\"b\\c\nd\u0001"))
        assertEquals("""{"value":true,"x":null}""", jsonText(mapOf("value" to true, "x" to null)))
        assertEquals("[]", jsonText(emptyList<Int>()))
    }
}
