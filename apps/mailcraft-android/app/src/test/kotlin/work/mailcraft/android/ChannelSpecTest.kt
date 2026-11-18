// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ChannelSpecTest {
    @Test
    fun theNamesGoThroughTheTranslation() {
        val specs = channelSpecs { "<$it>" }
        assertEquals(listOf("<New mail>", "<Reminders>"), specs.map { it.name })
    }

    @Test
    fun idsAreStableAndDistinct() {
        val ids = channelSpecs { it }.map { it.id }
        assertEquals(listOf(CHANNEL_MAIL, CHANNEL_REMINDERS), ids)
        assertEquals(ids.size, ids.toSet().size)
        assertTrue(channelSpecs { it }.all { it.importance > 0 })
    }
}
