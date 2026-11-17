// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Test

class LinkHitTest {
    @Test
    fun aTapIsTestedOnTheNearestCharacterAndTheOneBefore() {
        assertEquals(listOf(3, 2), hitCandidates(3, 10))
    }

    @Test
    fun theBoundaryAfterTheLastCharacterStillTestsTheLastCharacter() {
        assertEquals(listOf(9), hitCandidates(10, 10))
    }

    @Test
    fun theStartHasNoCharacterBefore() {
        assertEquals(listOf(0), hitCandidates(0, 5))
    }

    @Test
    fun noTextHasNoCandidates() {
        assertEquals(emptyList<Int>(), hitCandidates(0, 0))
    }
}
