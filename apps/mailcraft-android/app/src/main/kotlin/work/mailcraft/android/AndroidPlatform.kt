// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.TimeZone
import uniffi.mailcraft_mobile.HostException
import uniffi.mailcraft_mobile.NetworkKind
import uniffi.mailcraft_mobile.NotificationRecord
import uniffi.mailcraft_mobile.Platform
import uniffi.mailcraft_mobile.RedirectKind
import uniffi.mailcraft_mobile.SecretStore

/**
 * What the app gives the Rust core. Only the clock is real in this skeleton: there is no
 * sign-in, no notification and no credential store yet, and each of those says so as
 * [HostException.Unsupported] (which the core turns into its own words), never as a crash.
 */
internal class AndroidPlatform : Platform {
    override fun secrets(): SecretStore? = null

    override fun redirect(): RedirectKind = RedirectKind.CustomScheme(SCHEME)

    override fun openSignInPage(url: String) {
        throw HostException.Unsupported()
    }

    override fun notify(notification: NotificationRecord) {
        throw HostException.Unsupported()
    }

    override fun scheduleReminder(at: Long, notification: NotificationRecord) {
        throw HostException.Unsupported()
    }

    override fun openFile(name: String, bytes: ByteArray) {
        throw HostException.Unsupported()
    }

    override fun now(): Long = System.currentTimeMillis() / MILLIS_PER_SECOND

    override fun utcOffsetMinutes(at: Long): Int {
        val millis = at.coerceIn(-MAX_SECONDS, MAX_SECONDS) * MILLIS_PER_SECOND
        return TimeZone.getDefault().getOffset(millis) / MILLIS_PER_MINUTE
    }

    override fun memoryBudget(): ULong? = null

    override fun network(): NetworkKind = NetworkKind.Unknown

    private companion object {
        const val SCHEME = "work.mailcraft.android"
        const val MILLIS_PER_SECOND = 1000L
        const val MILLIS_PER_MINUTE = 60_000

        // About the year 9999 either way, so the multiplication cannot overflow.
        const val MAX_SECONDS = 253_402_300_799L
    }
}
