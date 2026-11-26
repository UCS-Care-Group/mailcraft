// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context
import android.content.SharedPreferences

/** What the owner chose about reading, kept on this phone only. */
internal data class ReadingPrefs(
    /** In a dark window a message is drawn on a light page, as its sender made it. On. */
    val lightMessages: Boolean = true,
    /** The header shows From, To and Cc in full; off, one line with the sender. */
    val headerOpen: Boolean = false,
    /** A message's files show as one row, "N attachments", that opens the list; off, the list is always open. On. */
    val foldAttachments: Boolean = true,
    /** When an opened message counts as read. */
    val markRead: MarkRead = MarkRead.WhenOpened,
    /** Light, dark, or the phone's setting. */
    val theme: ThemeChoice = ThemeChoice.System,
)

/** The reading prefs the window holds, and how it changes them (kept by the window: the theme is outside the shell). */
/** One field changed on the choices as stored now, so another window's newer save is kept. */
internal typealias ReadingEdit = (ReadingPrefs) -> ReadingPrefs

internal class ReadingChoice(val prefs: ReadingPrefs, val onChange: (ReadingEdit) -> Unit)

/** The app's theme: its own light or dark, or whatever the phone is set to. */
internal enum class ThemeChoice(val code: String) {
    System("system"),
    Light("light"),
    Dark("dark"),
    ;

    /** Whether the app is dark, given whether the phone is. */
    fun dark(system: Boolean): Boolean = when (this) {
        System -> system
        Light -> false
        Dark -> true
    }

    fun label(texts: Texts): String = when (this) {
        System -> texts.t("Follow system")
        Light -> texts.t("Light")
        Dark -> texts.t("Dark")
    }

    companion object {
        fun fromCode(code: String?): ThemeChoice = entries.firstOrNull { it.code == code } ?: System
    }
}

/** When a message that was opened is marked read. */
@Suppress("MagicNumber")
internal enum class MarkRead(val code: String, private val seconds: Int) {
    WhenOpened("opened", 0),
    After2("2s", 2),
    After5("5s", 5),
    Never("never", -1),
    ;

    /**
     * How long the message must stay open before it is marked, in milliseconds; null: never marked.
     * One swiped to waits at least [SWIPE_READ_MS], so that passing by quickly never marks.
     */
    fun delayMs(swiped: Boolean): Long? = when {
        seconds < 0 -> null
        else -> maxOf(seconds * MS_PER_SECOND, if (swiped) SWIPE_READ_MS else 0L)
    }

    fun label(texts: Texts): String = when (this) {
        WhenOpened -> texts.t("When opened")
        After2 -> texts.t("After 2 seconds")
        After5 -> texts.t("After 5 seconds")
        Never -> texts.t("Never (I mark them myself)")
    }

    companion object {
        private const val MS_PER_SECOND = 1000L

        fun fromCode(code: String?): MarkRead = entries.firstOrNull { it.code == code } ?: WhenOpened
    }
}

/** The prefs as [get] answers for a key with its default; a key an older version never wrote gives the default. */
internal fun readingPrefsFrom(
    text: (key: String) -> String? = { null },
    get: (key: String, default: Boolean) -> Boolean,
) = ReadingPrefs(
    lightMessages = get("light_messages", true),
    headerOpen = get("header_open", false),
    foldAttachments = get("fold_attachments", true),
    markRead = MarkRead.fromCode(text("mark_read")),
    theme = ThemeChoice.fromCode(text("theme")),
)

/** [ReadingPrefs] in the app's private preferences. A store that cannot be read gives the defaults. */
internal class DeviceReadingPrefs(context: Context) {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    fun load(): ReadingPrefs = runCatching {
        readingPrefsFrom({ prefs?.getString(it, null) }) { key, default -> prefs?.getBoolean(key, default) ?: default }
    }.getOrDefault(ReadingPrefs())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    fun save(value: ReadingPrefs) {
        runCatching {
            prefs?.edit()
                ?.putBoolean(LIGHT, value.lightMessages)
                ?.putBoolean(HEADER, value.headerOpen)
                ?.putBoolean(FOLD, value.foldAttachments)
                ?.putString(MARK, value.markRead.code)
                ?.putString(THEME, value.theme.code)
                ?.apply()
        }
    }

    /** Calls [onChange] whenever any window saves; returns the call that stops watching. */
    fun watch(onChange: () -> Unit): () -> Unit {
        // Held here: the preferences keep only a weak reference to a listener.
        val listener = SharedPreferences.OnSharedPreferenceChangeListener { _, _ -> onChange() }
        prefs?.registerOnSharedPreferenceChangeListener(listener)
        return { prefs?.unregisterOnSharedPreferenceChangeListener(listener) }
    }

    private companion object {
        const val FILE = "reading"
        const val LIGHT = "light_messages"
        const val HEADER = "header_open"
        const val FOLD = "fold_attachments"
        const val MARK = "mark_read"
        const val THEME = "theme"
    }
}
