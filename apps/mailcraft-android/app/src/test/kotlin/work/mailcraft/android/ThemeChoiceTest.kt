// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Test

class ThemeChoiceTest {
    @Test
    fun theDefaultFollowsTheSystem() = assertEquals(ThemeChoice.System, ReadingPrefs().theme)

    @Test
    fun aStoredChoiceIsRestored() {
        ThemeChoice.entries.forEach { choice ->
            val read = readingPrefsFrom({ if (it == "theme") choice.code else null }) { _, default -> default }
            assertEquals(choice, read.theme)
        }
    }

    @Test
    fun anOlderVersionOrAnUnknownCodeReadsAsFollowSystem() {
        assertEquals(ThemeChoice.System, readingPrefsFrom { _, default -> default }.theme)
        assertEquals(ThemeChoice.System, readingPrefsFrom({ if (it == "theme") "sepia" else null }) { _, d -> d }.theme)
    }

    @Test
    fun theChoiceDecidesWhetherTheAppIsDark() {
        listOf(true, false).forEach { system ->
            assertEquals(system, ThemeChoice.System.dark(system))
            assertEquals(false, ThemeChoice.Light.dark(system))
            assertEquals(true, ThemeChoice.Dark.dark(system))
        }
    }
}
