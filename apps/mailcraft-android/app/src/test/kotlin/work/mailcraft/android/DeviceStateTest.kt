// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.TimeZone
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.NetworkKind

class DeviceStateTest {
    private val enabled = 3
    private val whitelisted = 2
    private val disabled = 1

    @Test
    fun noConnectionIsOffline() {
        assertEquals(NetworkKind.Offline, networkKind(false, true, enabled))
    }

    @Test
    fun dataSaverCountsOnAMeteredNetworkOnly() {
        assertEquals(NetworkKind.Online(metered = true, dataSaving = true), networkKind(true, true, enabled))
        assertEquals(NetworkKind.Online(metered = false, dataSaving = false), networkKind(true, false, enabled))
        assertEquals(NetworkKind.Online(metered = true, dataSaving = false), networkKind(true, true, whitelisted))
        assertEquals(NetworkKind.Online(metered = true, dataSaving = false), networkKind(true, true, disabled))
    }

    @Test
    fun memoryBudgetIsHalfTheMemoryClassAndAtMostASixteenthOfRam() {
        val gib = 1024L * 1024 * 1024
        assertEquals(128UL * 1024UL * 1024UL, memoryBudgetBytes(256, 8 * gib, false))
        assertEquals(64UL * 1024UL * 1024UL, memoryBudgetBytes(256, 8 * gib, true))
        assertEquals(64UL * 1024UL * 1024UL, memoryBudgetBytes(1024, gib, false))
        assertEquals(128UL * 1024UL * 1024UL, memoryBudgetBytes(256, 0, false))
    }

    @Test
    fun memoryBudgetIsNeverZeroAndNullWhenNothingIsSaid() {
        assertNull(memoryBudgetBytes(0, 1, false))
        assertNull(memoryBudgetBytes(-5, 1, false))
        assertEquals(8UL * 1024UL * 1024UL, memoryBudgetBytes(1, 1, true))
        assertNotNull(memoryBudgetBytes(Int.MAX_VALUE, Long.MAX_VALUE, false))
    }

    @Test
    fun theOffsetFollowsDaylightSaving() {
        val berlin = TimeZone.getTimeZone("Europe/Berlin")
        assertEquals(60, utcOffsetMinutes(berlin, 1_700_000_000))
        assertEquals(120, utcOffsetMinutes(berlin, 1_720_000_000))
        assertEquals(330, utcOffsetMinutes(TimeZone.getTimeZone("Asia/Kolkata"), 0))
        assertEquals(-300, utcOffsetMinutes(TimeZone.getTimeZone("America/New_York"), 1_700_000_000))
    }

    @Test
    fun aTimeOutsideTheCalendarNeverThrows() {
        val zone = TimeZone.getTimeZone("Europe/Berlin")
        assertTrue(utcOffsetMinutes(zone, Long.MAX_VALUE) in -1000..1000)
        assertTrue(utcOffsetMinutes(zone, Long.MIN_VALUE) in -1000..1000)
    }
}
