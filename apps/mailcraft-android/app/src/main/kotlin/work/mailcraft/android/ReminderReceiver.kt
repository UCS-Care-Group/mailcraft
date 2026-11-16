// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.text.format.DateUtils
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.graphics.drawable.IconCompat
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters

/** The one-off work that sets the reminders' alarms again (unique by this name). */
internal const val REMINDERS_WORK = "mailcraft-reminders"

/**
 * The system's broadcasts after which the alarms are set again: a restart forgets every alarm, an
 * update of the app may, and a change of the clock or the zone moves when a whole-day event's
 * reminder is due. A change to the right to exact alarms (Android 12 and later) sets them exact,
 * or within the window, as the right now says.
 */
internal val RESCHEDULE_ACTIONS = setOf(
    Intent.ACTION_BOOT_COMPLETED,
    Intent.ACTION_MY_PACKAGE_REPLACED,
    Intent.ACTION_TIME_CHANGED,
    Intent.ACTION_TIMEZONE_CHANGED,
    "android.app.action.SCHEDULE_EXACT_ALARM_PERMISSION_STATE_CHANGED",
)

/**
 * The reminders' alarms, Dismiss and Snooze, and the system's broadcasts that call for the alarms
 * to be set again. Not exported: only the system and the app's own alarms reach it.
 */
class ReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        when (action) {
            ACTION_FIRE -> readPayload(intent)?.let { fire(context, it) }
            ACTION_DISMISS -> readKey(intent)?.let { dismiss(context, it) }
            ACTION_SNOOZE -> readPayload(intent)?.let { snooze(context, it) }
            in RESCHEDULE_ACTIONS -> enqueueReminderRefresh(context)
        }
    }

    private fun fire(context: Context, payload: ReminderPayload) {
        PrefsReminderStore(context).update { handledOne(it, payload.key) to Unit }
        // The owner's Reminders switch, the right to notify and the channel, read as it comes.
        if (!DeviceSwitches(context).load().reminders) return
        if (!mailNotificationsAllowed(context, CHANNEL_REMINDERS)) return
        showReminder(context, payload)
    }

    private fun dismiss(context: Context, key: Long) {
        PrefsReminderStore(context).update { handledOne(it, key) to Unit }
        AndroidAlarms(context).withdraw(key)
    }

    private fun snooze(context: Context, payload: ReminderPayload) {
        val until = System.currentTimeMillis() + SNOOZE_MINUTES * DateUtils.MINUTE_IN_MILLIS
        PrefsReminderStore(context).update { snoozedOne(it, payload.key, until) to Unit }
        AndroidAlarms(context).snooze(payload, until)
    }
}

/**
 * Shows a reminder: the title and when, with Dismiss and Snooze. Private: a locked screen that
 * hides private content shows only "Reminders", never the title or the place, as new mail shows
 * only "New mail".
 */
internal fun showReminder(context: Context, payload: ReminderPayload) {
    val at = DateUtils.formatDateTime(
        context,
        payload.startsMillis,
        if (payload.allDay) DateUtils.FORMAT_SHOW_DATE else DateUtils.FORMAT_SHOW_TIME or DateUtils.FORMAT_SHOW_DATE,
    )
    val icon = IconCompat.createWithBitmap(envelope())
    val id = notificationId(payload.key.toULong())
    fun action(intent: Intent) =
        PendingIntent.getBroadcast(
            context,
            id,
            intent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
    val open = PendingIntent.getActivity(
        context,
        0,
        Intent.makeMainActivity(ComponentName(context, MainActivity::class.java)),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
    val snoozeIntent = putPayload(reminderIntent(context, ACTION_SNOOZE, payload.key), payload)
    val hidden = NotificationCompat.Builder(context, CHANNEL_REMINDERS)
        .setSmallIcon(icon)
        .setContentTitle(payload.words.hidden)
        .build()
    val notification = NotificationCompat.Builder(context, CHANNEL_REMINDERS)
        .setSmallIcon(icon)
        .setContentTitle(payload.title)
        .setContentText(reminderText(payload, at))
        .setCategory(if (payload.task) NotificationCompat.CATEGORY_REMINDER else NotificationCompat.CATEGORY_EVENT)
        .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
        .setPublicVersion(hidden)
        .setContentIntent(open)
        .setAutoCancel(true)
        .addAction(0, payload.words.dismiss, action(reminderIntent(context, ACTION_DISMISS, payload.key)))
        .addAction(0, payload.words.snooze, action(snoozeIntent))
        .build()
    try {
        NotificationManagerCompat.from(context).notify(REMINDER_NOTIFICATION_TAG, id, notification)
    } catch (_: SecurityException) {
        // The right to notify was taken away between the check and the post: nothing is shown.
    }
}

/** Asks the system to set the reminders' alarms again soon, in the app's process (one such run at a time). */
internal fun enqueueReminderRefresh(context: Context) {
    runCatching {
        WorkManager.getInstance(context).enqueueUniqueWork(
            REMINDERS_WORK,
            ExistingWorkPolicy.REPLACE,
            OneTimeWorkRequestBuilder<ReminderWorker>().build(),
        )
    }
}

/** Sets the reminders' alarms from the application's one core (opening it when no window has). */
class ReminderWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        val app = applicationContext as? MailCraftApplication ?: return Result.failure()
        return if (app.refreshRemindersNow()) Result.success() else Result.failure()
    }
}
