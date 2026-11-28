// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import androidx.core.content.edit

/** What a button on a new-mail notification can do; the owner picks up to [NoticeButtons.MOST]. */
internal enum class NoticeButton(val code: String) {
    Delete("delete"),
    Archive("archive"),
    Reply("reply"),
    MarkRead("markRead"),
    Flag("flag"),
    Junk("junk"),
    ;

    /** Its name, through [t] (a window's texts, or the core's words in a notification). */
    fun label(t: (String) -> String): String = when (this) {
        Delete -> t("Delete")
        Archive -> t("Archive")
        Reply -> t("Reply")
        MarkRead -> t("Mark as read")
        Flag -> t("Flag")
        Junk -> t("Junk")
    }

    /** The change the button makes to the message, as the list's triage makes it; null: Reply, which opens the app. */
    fun triage(): Triage? = when (this) {
        Delete -> Triage.Delete
        Archive -> Triage.Archive
        Reply -> null
        MarkRead -> Triage.MarkRead(read = true)
        Flag -> Triage.Flag(on = true)
        Junk -> Triage.Junk
    }
}

/** The buttons a new-mail notification shows, in their order: Android shows no more than [MOST]. */
internal data class NoticeButtons(val chosen: List<NoticeButton> = DEFAULT) {
    /** How the choice is kept: the codes in order, comma-separated (empty: no buttons). */
    val codes: String get() = chosen.joinToString(",") { it.code }

    /** [button] taken off when it is chosen, else added last (when there is room). */
    fun toggled(button: NoticeButton): NoticeButtons = when {
        button in chosen -> NoticeButtons(chosen - button)
        chosen.size < MOST -> NoticeButtons(chosen + button)
        else -> this
    }

    companion object {
        const val MOST = 3
        val DEFAULT = listOf(NoticeButton.Delete, NoticeButton.Reply, NoticeButton.Archive)

        /**
         * The choice kept as [codes]: none kept is the default; unknown codes and repeats are left
         * out, and no more than [MOST] are read.
         */
        fun fromCodes(codes: String?): NoticeButtons {
            if (codes == null) return NoticeButtons()
            val chosen = codes.split(',')
                .mapNotNull { code -> NoticeButton.entries.firstOrNull { it.code == code.trim() } }
                .distinct()
                .take(MOST)
            return NoticeButtons(chosen)
        }
    }
}

/**
 * Where the owner's notification buttons are kept: on this device only (nothing is backed up). A
 * store that cannot be read gives the defaults.
 */
internal class DeviceNoticeButtons(context: Context) {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    fun load(): NoticeButtons =
        runCatching { NoticeButtons.fromCodes(prefs?.getString(KEY, null)) }.getOrDefault(NoticeButtons())

    fun save(buttons: NoticeButtons) {
        runCatching { prefs?.edit { putString(KEY, buttons.codes) } }
    }

    private companion object {
        const val FILE = "notices"
        const val KEY = "buttons"
    }
}
