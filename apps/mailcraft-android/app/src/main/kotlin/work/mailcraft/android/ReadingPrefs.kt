// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context

/** What the owner chose about reading, kept on this phone only. */
internal data class ReadingPrefs(
    /** In a dark window a message is drawn on a light page, as its sender made it. On. */
    val lightMessages: Boolean = true,
    /** The header shows From, To and Cc in full; off, one line with the sender. */
    val headerOpen: Boolean = false,
)

/** [ReadingPrefs] in the app's private preferences. A store that cannot be read gives the defaults. */
internal class DeviceReadingPrefs(context: Context) {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    fun load(): ReadingPrefs = runCatching {
        ReadingPrefs(
            lightMessages = prefs?.getBoolean(LIGHT, true) ?: true,
            headerOpen = prefs?.getBoolean(HEADER, false) ?: false,
        )
    }.getOrDefault(ReadingPrefs())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    fun save(value: ReadingPrefs) {
        runCatching {
            prefs?.edit()?.putBoolean(LIGHT, value.lightMessages)?.putBoolean(HEADER, value.headerOpen)?.apply()
        }
    }

    private companion object {
        const val FILE = "reading"
        const val LIGHT = "light_messages"
        const val HEADER = "header_open"
    }
}
