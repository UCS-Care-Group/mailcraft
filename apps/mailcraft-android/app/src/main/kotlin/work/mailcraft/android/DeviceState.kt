// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.ActivityManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.util.TimeZone
import uniffi.mailcraft_mobile.NetworkKind

/**
 * What the network is like, from three facts. Data Saver only holds back a metered connection, so
 * it counts only on one; an app the owner let past Data Saver is not held back either (the system
 * reports that as another status than "enabled").
 */
internal fun networkKind(connected: Boolean, metered: Boolean, restrictBackgroundStatus: Int): NetworkKind =
    if (!connected) {
        NetworkKind.Offline
    } else {
        NetworkKind.Online(
            metered = metered,
            dataSaving = metered &&
                restrictBackgroundStatus == ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED,
        )
    }

/**
 * The most bytes of mail content to keep in memory. The system's per-app memory class is the figure
 * it publishes for what an app may use; the program takes half of it (the screen needs the rest),
 * and half of that again when the device says it is short of memory, and never more than a sixteenth
 * of the device's RAM. A device that says nothing gives null; the answer is never zero, which would
 * say the app can run in no memory at all.
 */
internal fun memoryBudgetBytes(memoryClassMiB: Int, totalMemBytes: Long, lowMemory: Boolean): ULong? {
    if (memoryClassMiB <= 0) return null
    var budget = memoryClassMiB.toLong() * MIB / 2
    if (lowMemory) budget /= 2
    if (totalMemBytes > 0) budget = minOf(budget, totalMemBytes / RAM_SHARE)
    return budget.coerceAtLeast(MIN_BUDGET).toULong()
}

/** Minutes east of UTC of [zone] at [seconds] since 1970, daylight saving included. */
internal fun utcOffsetMinutes(zone: TimeZone, seconds: Long): Int {
    val millis = seconds.coerceIn(-MAX_SECONDS, MAX_SECONDS) * MILLIS_PER_SECOND
    return zone.getOffset(millis) / MILLIS_PER_MINUTE
}

private const val MIB = 1024L * 1024L
private const val RAM_SHARE = 16L
private const val MIN_BUDGET = 8L * MIB
private const val MILLIS_PER_SECOND = 1000L
private const val MILLIS_PER_MINUTE = 60_000

// About the year 9999 either way, so the multiplication cannot overflow.
private const val MAX_SECONDS = 253_402_300_799L

/** The facts the system gives of the device and the network; each call asks afresh. */
internal class DeviceState(private val context: Context) {
    fun network(): NetworkKind {
        val manager = context.getSystemService(ConnectivityManager::class.java) ?: return NetworkKind.Unknown
        val capabilities = manager.activeNetwork?.let { manager.getNetworkCapabilities(it) }
        val connected = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        val metered = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true
        return networkKind(connected, metered, manager.restrictBackgroundStatus)
    }

    fun memoryBudget(): ULong? {
        val manager = context.getSystemService(ActivityManager::class.java) ?: return null
        val info = ActivityManager.MemoryInfo()
        manager.getMemoryInfo(info)
        return memoryBudgetBytes(manager.memoryClass, info.totalMem, info.lowMemory)
    }
}
