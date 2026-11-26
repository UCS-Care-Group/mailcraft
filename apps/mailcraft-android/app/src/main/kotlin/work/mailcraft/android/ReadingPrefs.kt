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
    /** A message's files show as one row, "N attachments", that opens the list; off, the list is always open. On. */
    val foldAttachments: Boolean = true,
)

/** The prefs as [get] answers for a key with its default; a key an older version never wrote gives the default. */
internal fun readingPrefsFrom(get: (key: String, default: Boolean) -> Boolean) = ReadingPrefs(
    lightMessages = get("light_messages", true),
    headerOpen = get("header_open", false),
    foldAttachments = get("fold_attachments", true),
)

/** [ReadingPrefs] in the app's private preferences. A store that cannot be read gives the defaults. */
internal class DeviceReadingPrefs(context: Context) {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    fun load(): ReadingPrefs = runCatching {
        readingPrefsFrom { key, default -> prefs?.getBoolean(key, default) ?: default }
    }.getOrDefault(ReadingPrefs())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    fun save(value: ReadingPrefs) {
        runCatching {
            prefs?.edit()
                ?.putBoolean(LIGHT, value.lightMessages)
                ?.putBoolean(HEADER, value.headerOpen)
                ?.putBoolean(FOLD, value.foldAttachments)
                ?.apply()
        }
    }

    private companion object {
        const val FILE = "reading"
        const val LIGHT = "light_messages"
        const val HEADER = "header_open"
        const val FOLD = "fold_attachments"
    }
}
