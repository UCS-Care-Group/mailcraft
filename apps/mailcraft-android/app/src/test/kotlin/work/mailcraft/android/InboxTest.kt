// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class InboxTest {
    @Test
    fun senderIsTheNameWhenThereIsOne() {
        assertEquals("Ada", senderLabel("Ada", "ada@example.org"))
    }

    @Test
    fun senderIsTheAddressWhenThereIsNoName() {
        assertEquals("ada@example.org", senderLabel("  ", "ada@example.org"))
    }

    @Test
    fun aTimeOutsideTheCalendarIsClampedAndNeverThrows() {
        assertTrue(formatWhen(Long.MAX_VALUE).isNotEmpty())
        assertTrue(formatWhen(Long.MIN_VALUE).isNotEmpty())
    }
}
