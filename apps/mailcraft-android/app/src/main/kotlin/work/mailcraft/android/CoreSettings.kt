// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context
import android.os.StatFs
import java.io.File
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.DeviceSettings
import uniffi.mailcraft_mobile.deviceSettingsJson
import uniffi.mailcraft_mobile.keepOfflineCodes
import uniffi.mailcraft_mobile.offlineEstimate
import uniffi.mailcraft_mobile.parseDeviceSettings

/** Where the phone's own switches are kept: one small text in the app's private storage. */
internal interface SwitchesStore {
    /** What was kept; what can't be read is every switch off. */
    fun load(): Switches

    fun save(switches: Switches)
}

/**
 * [SwitchesStore] in a private preferences file, as the Rust side's text ([DeviceSettings]): a
 * text that has gone wrong is read as every switch off, so what sends something is never turned
 * on by a fault. Nothing in it is a secret, and it is not part of any backup.
 */
internal class DeviceSwitches(context: Context) : SwitchesStore {
    private val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    override fun load(): Switches = runCatching {
        val settings = parseDeviceSettings(prefs.getString(KEY, null).orEmpty())
        Switches(settings.downloadPictures, settings.senderImages, settings.newMailOnly, settings.primaryOnly)
    }.getOrDefault(Switches())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    override fun save(switches: Switches) {
        runCatching {
            val settings = DeviceSettings(
                downloadPictures = switches.downloadPictures,
                senderImages = switches.senderImages,
                newMailOnly = switches.newMailOnly,
                primaryOnly = switches.primaryOnly,
            )
            val text = deviceSettingsJson(settings)
            prefs.edit().putString(KEY, text).apply()
        }
    }

    private companion object {
        const val FILE = "device_settings"
        const val KEY = "settings"
    }
}

/** [SwitchesStore] in memory, for tests and screenshots. */
internal class MemorySwitches(private var switches: Switches = Switches()) : SwitchesStore {
    override fun load(): Switches = switches

    override fun save(switches: Switches) {
        this.switches = switches
    }
}

/** The bytes free where [dir] is, or a negative number when the system can't say. */
internal fun freeBytesAt(dir: File): Long =
    try {
        StatFs(dir.path).availableBytes
    } catch (_: IllegalArgumentException) {
        -1L
    }

/** [SettingsApi] over the Rust core, the phone's storage and its private preferences. */
internal class CoreSettings(
    private val core: Core,
    private val store: SwitchesStore,
    private val dir: File,
    private val keep: suspend () -> Unit,
) : SettingsApi {
    override fun codes(): List<String> =
        runCatching { keepOfflineCodes() }.getOrNull()?.takeIf { it.isNotEmpty() } ?: OFFLINE_CODES

    override suspend fun keepOffline(account: ULong): String? = try {
        core.keepOffline(account)
    } catch (_: CoreException) {
        null
    }

    override suspend fun setKeepOffline(account: ULong, code: String): WindowChange? = try {
        val change = core.setKeepOffline(account, code)
        keep()
        WindowChange(change.code, change.shorter)
    } catch (_: CoreException) {
        null
    }

    override suspend fun thin(): Thinned? = try {
        val done = core.thinOffline(THIN_BATCH)
        if (done.thinned > 0u) keep()
        Thinned(done.thinned.toInt(), done.left.toInt())
    } catch (_: CoreException) {
        null
    }

    override fun estimate(code: String): Long = offlineEstimate(code).toLong().coerceAtLeast(0L)

    override fun freeBytes(): Long = freeBytesAt(dir)

    override suspend fun signature(account: ULong): SignatureState? = try {
        val found = core.accountSignature(account)
        SignatureState(saved = found.text, draft = found.text, editable = found.editable)
    } catch (_: CoreException) {
        null
    }

    override suspend fun setSignature(account: ULong, text: String): Boolean = try {
        core.setAccountSignature(account, text)
        keep()
        true
    } catch (_: CoreException) {
        false
    }

    override fun switches(): Switches = store.load()

    override fun saveSwitches(switches: Switches) = store.save(switches)

    private companion object {
        const val THIN_BATCH = 300u
    }
}
