// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.core.app.NotificationManagerCompat
import uniffi.mailcraft_mobile.ReminderKind

/** What an alarm carries to [ReminderReceiver]: the reminder, and the words to show it with. */
internal data class ReminderPayload(
    val key: Long,
    val task: Boolean,
    val title: String,
    val location: String,
    val startsMillis: Long,
    val allDay: Boolean,
    val words: ReminderWords,
)

internal fun ReminderAlarm.payload(words: ReminderWords) = ReminderPayload(
    key = key,
    task = reminder.kind == ReminderKind.TASK,
    title = reminder.title,
    location = reminder.location,
    startsMillis = secondsToMillis(reminder.starts),
    allDay = reminder.allDay,
    words = words,
)

private const val EXTRA_KEY = "key"
private const val EXTRA_TASK = "task"
private const val EXTRA_TITLE = "title"
private const val EXTRA_LOCATION = "location"
private const val EXTRA_STARTS = "starts"
private const val EXTRA_ALL_DAY = "allDay"
private const val EXTRA_WORDS = "words"

/** The longest text read back from an alarm (the core cut what it gave to less). */
private const val LONGEST_EXTRA = 2_000

/** Puts [payload] in [intent]. (The system keeps an alarm's intent in memory only: nothing is written.) */
internal fun putPayload(intent: Intent, payload: ReminderPayload): Intent = intent
    .putExtra(EXTRA_KEY, payload.key)
    .putExtra(EXTRA_TASK, payload.task)
    .putExtra(EXTRA_TITLE, payload.title)
    .putExtra(EXTRA_LOCATION, payload.location)
    .putExtra(EXTRA_STARTS, payload.startsMillis)
    .putExtra(EXTRA_ALL_DAY, payload.allDay)
    .putExtra(EXTRA_WORDS, payload.words.toList().toTypedArray())

/** The payload in [intent]; null when it has no key. */
internal fun readPayload(intent: Intent): ReminderPayload? {
    if (!intent.hasExtra(EXTRA_KEY)) return null
    val words = intent.getStringArrayExtra(EXTRA_WORDS).orEmpty().map { it.orEmpty().take(LONGEST_EXTRA) }
    return ReminderPayload(
        key = intent.getLongExtra(EXTRA_KEY, 0L),
        task = intent.getBooleanExtra(EXTRA_TASK, false),
        title = intent.getStringExtra(EXTRA_TITLE).orEmpty().take(LONGEST_EXTRA),
        location = intent.getStringExtra(EXTRA_LOCATION).orEmpty().take(LONGEST_EXTRA),
        startsMillis = intent.getLongExtra(EXTRA_STARTS, 0L),
        allDay = intent.getBooleanExtra(EXTRA_ALL_DAY, false),
        words = ReminderWords.fromList(words),
    )
}

/** The key in [intent], for Dismiss (which carries nothing else). */
internal fun readKey(intent: Intent): Long? =
    if (intent.hasExtra(EXTRA_KEY)) intent.getLongExtra(EXTRA_KEY, 0L) else null

/**
 * An intent for [ReminderReceiver] about the reminder [key]: the action says what to do, the
 * address tells one reminder's intents from another's (the system compares them by it, not by
 * what they carry).
 */
internal fun reminderIntent(context: Context, action: String, key: Long): Intent =
    Intent(context, ReminderReceiver::class.java)
        .setAction(action)
        .setData(Uri.fromParts(REMINDER_SCHEME, key.toString(), null))
        .putExtra(EXTRA_KEY, key)

private const val REMINDER_SCHEME = "mailcraft-reminder"

/**
 * Whether the alarm may be exact: always before Android 12; from it, when the system lets the app
 * (`USE_EXACT_ALARM` from Android 13, as a calendar does; `SCHEDULE_EXACT_ALARM` on 12, which the
 * owner can take away). Otherwise it is set to come within the system's window
 * (`setAndAllowWhileIdle`), which may be some minutes late in Doze.
 */
internal fun exactAllowed(sdk: Int, canScheduleExact: () -> Boolean): Boolean =
    sdk < Build.VERSION_CODES.S || canScheduleExact()

/** [AlarmSink] over the system's AlarmManager. Exact alarms are for reminders only, never to check for mail. */
internal class AndroidAlarms(private val context: Context) : AlarmSink {
    private val manager: AlarmManager? = context.getSystemService(AlarmManager::class.java)

    override fun set(alarm: ReminderAlarm, words: ReminderWords) {
        val manager = manager ?: return
        val intent = putPayload(reminderIntent(context, ACTION_FIRE, alarm.key), alarm.payload(words))
        setAt(manager, alarm.atMillis, broadcast(alarm.key, intent, PendingIntent.FLAG_UPDATE_CURRENT) ?: return)
    }

    /** Sets [pending] for [atMillis], exact when the system lets the app and within its window when not. */
    fun setAt(manager: AlarmManager, atMillis: Long, pending: PendingIntent) {
        val exact = exactAllowed(Build.VERSION.SDK_INT) {
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && manager.canScheduleExactAlarms()
        }
        try {
            if (exact) {
                manager.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMillis, pending)
            } else {
                manager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMillis, pending)
            }
        } catch (_: SecurityException) {
            // The right to exact alarms went between the check and the call: within the window then.
            manager.setAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, atMillis, pending)
        }
    }

    override fun cancel(key: Long) {
        val pending = broadcast(key, reminderIntent(context, ACTION_FIRE, key), PendingIntent.FLAG_NO_CREATE) ?: return
        manager?.cancel(pending)
        pending.cancel()
    }

    override fun withdraw(key: Long) {
        NotificationManagerCompat.from(context).cancel(REMINDER_NOTIFICATION_TAG, notificationId(key.toULong()))
    }

    private fun broadcast(key: Long, intent: Intent, flags: Int): PendingIntent? =
        PendingIntent.getBroadcast(
            context,
            notificationId(key.toULong()),
            intent,
            flags or PendingIntent.FLAG_IMMUTABLE,
        )

    /** Snoozes [payload]: its notification goes, and comes back after [SNOOZE_MINUTES]. */
    fun snooze(payload: ReminderPayload, untilMillis: Long) {
        withdraw(payload.key)
        val manager = manager ?: return
        val intent = putPayload(reminderIntent(context, ACTION_FIRE, payload.key), payload)
        setAt(manager, untilMillis, broadcast(payload.key, intent, PendingIntent.FLAG_UPDATE_CURRENT) ?: return)
    }
}

internal const val ACTION_FIRE = "work.mailcraft.android.reminder.FIRE"
internal const val ACTION_DISMISS = "work.mailcraft.android.reminder.DISMISS"
internal const val ACTION_SNOOZE = "work.mailcraft.android.reminder.SNOOZE"
