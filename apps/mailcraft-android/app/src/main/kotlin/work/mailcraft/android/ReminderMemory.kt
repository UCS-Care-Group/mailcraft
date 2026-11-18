// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context

/** Where [Remembered] is kept. [update] is one step: what it reads is what nobody changes before it writes. */
internal interface ReminderStore {
    fun <T> update(change: (Remembered) -> Pair<Remembered, T>): T
}

/** [ReminderStore] in memory, for tests. */
internal class MemoryReminderStore(var remembered: Remembered = Remembered()) : ReminderStore {
    @Synchronized
    override fun <T> update(change: (Remembered) -> Pair<Remembered, T>): T {
        val (next, result) = change(remembered)
        remembered = next
        return result
    }
}

/**
 * [ReminderStore] in a private preferences file, left out of backups as the app's data is: keys
 * and times only (no title, no place, nothing of an event). What can't be read is forgotten, which
 * at worst raises a reminder a second time.
 */
internal class PrefsReminderStore(context: Context) : ReminderStore {
    private val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    // (One lock for the process: the scheduler and the receiver's Dismiss and Snooze both write here.
    // `apply` updates what is read at once and writes in the background; the system waits for it
    // before it ends a receiver or a stopped app.)
    @SuppressLint("UseKtx")
    override fun <T> update(change: (Remembered) -> Pair<Remembered, T>): T = synchronized(LOCK) {
        val before = decodeRemembered(
            prefs.getString(SCHEDULED, null).orEmpty(),
            prefs.getString(HANDLED, null).orEmpty(),
            prefs.getString(SNOOZED, null).orEmpty(),
        )
        val (after, result) = change(before)
        prefs.edit()
            .putString(SCHEDULED, encodePairs(after.scheduled))
            .putString(HANDLED, after.handled.joinToString(","))
            .putString(SNOOZED, encodePairs(after.snoozed))
            .apply()
        result
    }

    private companion object {
        val LOCK = Any()
        const val FILE = "reminders"
        const val SCHEDULED = "scheduled"
        const val HANDLED = "handled"
        const val SNOOZED = "snoozed"
    }
}

/** The most entries read back of each list: what the app writes is never more. */
private const val MOST_READ = REMINDERS_REMEMBERED

/** The longest text read back; anything longer was not written by the app. */
private const val LONGEST_TEXT = 64 * 1024

internal fun encodePairs(pairs: Map<Long, Long>): String = pairs.entries.joinToString(",") { "${it.key}:${it.value}" }

private fun parts(text: String): List<String> =
    if (text.isEmpty() || text.length > LONGEST_TEXT) emptyList() else text.split(',').take(MOST_READ)

internal fun decodePairs(text: String): Map<Long, Long> =
    parts(text).mapNotNull { part ->
        val (key, value) = part.split(':').takeIf { it.size == 2 } ?: return@mapNotNull null
        val k = key.toLongOrNull() ?: return@mapNotNull null
        val v = value.toLongOrNull() ?: return@mapNotNull null
        k to v
    }.toMap()

internal fun decodeRemembered(scheduled: String, handled: String, snoozed: String): Remembered =
    Remembered(
        scheduled = decodePairs(scheduled),
        handled = parts(handled).mapNotNull { it.toLongOrNull() },
        snoozed = decodePairs(snoozed),
    )
