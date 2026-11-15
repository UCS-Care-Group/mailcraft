// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import java.util.TimeZone
import uniffi.mailcraft_mobile.HostException
import uniffi.mailcraft_mobile.NetworkKind
import uniffi.mailcraft_mobile.NotificationRecord
import uniffi.mailcraft_mobile.Platform
import uniffi.mailcraft_mobile.RedirectKind
import uniffi.mailcraft_mobile.SecretStore

/**
 * What the app gives the Rust core: the Keystore-sealed credential store, the sign-in page and its
 * redirect, files, the clock and time zone, the memory budget and the network. Every entry is
 * [guarded]: whatever the system throws is the binding's own [HostException], never an exception
 * through UniFFI. Showing and scheduling notifications are for the notification slice and say
 * [HostException.Unsupported] until then (the channels they will use exist already).
 */
@Suppress("TooManyFunctions") // One function per entry of the core's `Platform`, which is that long.
internal class AndroidPlatform(
    context: Context,
    private val handback: SignInHandback,
    foreground: () -> android.app.Activity?,
) : Platform {
    private val device = DeviceState(context)
    private val secrets: SecretStore = SecretFiles(
        java.io.File(context.noBackupFilesDir, "secrets"),
        AesGcmSealer { KeystoreKey().get() },
    )
    private val page = SignInPage(context, handback, foreground)
    private val files = OpenedFiles(context)

    override fun secrets(): SecretStore? = secrets

    override fun redirect(): RedirectKind = RedirectKind.CustomScheme(Redirect.SCHEME)

    override fun openSignInPage(url: String) = guarded { page.open(url) }

    override fun notify(notification: NotificationRecord) {
        throw HostException.Unsupported()
    }

    override fun scheduleReminder(at: Long, notification: NotificationRecord) {
        throw HostException.Unsupported()
    }

    override fun openFile(name: String, bytes: ByteArray) = guarded { files.open(name, bytes) }

    override fun now(): Long = System.currentTimeMillis() / MILLIS_PER_SECOND

    override fun utcOffsetMinutes(at: Long): Int = guarded { utcOffsetMinutes(TimeZone.getDefault(), at) }

    /** The IANA id (`Europe/Berlin`); the core uses it when it knows the name, else the offset above. */
    override fun timeZoneName(): String = guarded { TimeZone.getDefault().id }

    override fun memoryBudget(): ULong? = guarded { device.memoryBudget() }

    override fun network(): NetworkKind = guarded { device.network() }

    private companion object {
        const val MILLIS_PER_SECOND = 1000L
    }
}
