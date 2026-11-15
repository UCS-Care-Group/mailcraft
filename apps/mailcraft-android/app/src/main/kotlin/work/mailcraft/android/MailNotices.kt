// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.Manifest
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.platform.LocalContext
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import androidx.core.graphics.createBitmap
import androidx.core.graphics.drawable.IconCompat
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.NotificationRecord
import uniffi.mailcraft_mobile.NotificationTopic

/** The tag every new-mail notification carries, so that its number never meets another kind's. */
internal const val MAIL_NOTIFICATION_TAG = "mail"

/** The system's number for the core's notification [key]: both halves folded in, so nearby keys stay apart. */
internal fun notificationId(key: ULong): Int = (key xor (key shr Int.SIZE_BITS)).toInt()

/**
 * Whether to ask for the right to notify now: only on Android 13 and later (before, it is given at
 * install), only once there is an account to bring mail (never at a first start, before the owner
 * has signed in and the question means anything), only when it isn't given already, and only once
 * ever: a no is the owner's answer, and the system's settings are where it is changed.
 */
internal fun shouldAskForNotifications(sdk: Int, hasAccount: Boolean, askedBefore: Boolean, granted: Boolean): Boolean =
    sdk >= Build.VERSION_CODES.TIRAMISU && hasAccount && !askedBefore && !granted

/** Whether the app may post a new-mail notification: the right given, notifications on, the channel not off. */
internal fun mailNotificationsAllowed(context: Context): Boolean {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
        PackageManager.PERMISSION_GRANTED
    ) {
        return false
    }
    val manager = NotificationManagerCompat.from(context)
    if (!manager.areNotificationsEnabled()) return false
    val channel = manager.getNotificationChannel(CHANNEL_MAIL)
    return channel == null || channel.importance != NotificationManager.IMPORTANCE_NONE
}

/** Remembers that the owner was asked for the right to notify, so that it is asked once. */
internal class NotificationQuestion(context: Context) {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    val askedBefore: Boolean get() = prefs.getBoolean(ASKED, false)

    fun markAsked() {
        prefs.edit { putBoolean(ASKED, true) }
    }

    private companion object {
        const val PREFS = "notices"
        const val ASKED = "askedForNotifications"
    }
}

/**
 * Asks for the right to notify once [hasAccount] is true (the first account signed in), when
 * [shouldAskForNotifications] says so; [answered] hears the owner's answer.
 */
@Composable
internal fun AskForNotificationsOnce(hasAccount: Boolean, answered: () -> Unit) {
    val context = LocalContext.current
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { answered() }
    LaunchedEffect(hasAccount) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return@LaunchedEffect
        val question = NotificationQuestion(context)
        val granted = ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        if (shouldAskForNotifications(Build.VERSION.SDK_INT, hasAccount, question.askedBefore, granted)) {
            question.markAsked()
            ask.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }
}

/**
 * Shows the core's new-mail notifications. What one says is the core's own (a sender and subject
 * when it gives them, made plain and short there, else a count), and the core sends none when the
 * owner turned new-mail notifications off (`Core.setNotifyNewMail`, which follows the system's
 * switch for the app and for the channel). The notification is private: where the system hides
 * private content on the lock screen (its sensitive-content setting), only "New mail" shows there
 * and what the message says waits until the phone is unlocked; where the owner lets the lock
 * screen show all of it, the sender and subject show there too. Tapping one opens the app.
 */
internal class MailNotifier(private val context: Context, private val t: (String) -> String) {
    fun handle(event: Event) {
        if (event is Event.Notification && event.notification.topic == NotificationTopic.MAIL) show(event.notification)
    }

    private fun show(record: NotificationRecord) {
        // Checked again here: the owner may have turned them off since the core was told.
        if (!mailNotificationsAllowed(context)) return
        val icon = IconCompat.createWithBitmap(envelope())
        val open = PendingIntent.getActivity(
            context,
            0,
            Intent.makeMainActivity(ComponentName(context, MainActivity::class.java)),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val hidden = NotificationCompat.Builder(context, CHANNEL_MAIL)
            .setSmallIcon(icon)
            .setContentTitle(t("New mail"))
            .build()
        val notification = NotificationCompat.Builder(context, CHANNEL_MAIL)
            .setSmallIcon(icon)
            .setContentTitle(record.title)
            .setContentText(record.body)
            .setCategory(NotificationCompat.CATEGORY_EMAIL)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(hidden)
            .setContentIntent(open)
            .setAutoCancel(true)
            .build()
        try {
            NotificationManagerCompat.from(context)
                .notify(MAIL_NOTIFICATION_TAG, notificationId(record.key), notification)
        } catch (_: SecurityException) {
            // The right was taken away between the check and the post: nothing is shown.
        }
    }
}

/** The status-bar icon, an envelope drawn here in white on nothing (the system tints it). */
internal fun envelope(): Bitmap {
    val size = ICON_PX
    val bitmap = createBitmap(size, size)
    val canvas = Canvas(bitmap)
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.STROKE
        strokeWidth = size * STROKE
        strokeJoin = Paint.Join.ROUND
        strokeCap = Paint.Cap.ROUND
    }
    val left = size * MARGIN
    val right = size - left
    val top = size * TOP
    val bottom = size - top
    canvas.drawRoundRect(left, top, right, bottom, size * CORNER, size * CORNER, paint)
    canvas.drawLine(left, top, size / 2f, size * FOLD, paint)
    canvas.drawLine(size / 2f, size * FOLD, right, top, paint)
    return bitmap
}

private const val ICON_PX = 96
private const val STROKE = 0.08f
private const val MARGIN = 0.12f
private const val TOP = 0.24f
private const val CORNER = 0.08f
private const val FOLD = 0.55f
