// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import uniffi.mailcraft_mobile.NotifiedMessage

/** The broadcast of a new-mail notification's button that acts without opening the app. */
internal const val ACTION_NOTICE_BUTTON = "work.mailcraft.android.notice.BUTTON"

/** The window's action for a new-mail notification's Reply. */
internal const val ACTION_NOTICE_REPLY = "work.mailcraft.android.notice.REPLY"

/** The way into the window a notification's Reply takes: an alias of [MainActivity] that is not exported. */
internal const val NOTICE_REPLY_ALIAS = "work.mailcraft.android.NoticeReply"

private const val NOTICE_SCHEME = "mailcraft-notice"
private const val EXTRA_BUTTON = "button"
private const val EXTRA_MESSAGE = "message"
private const val EXTRA_FOLDER = "folder"

/** What a notification's button asks for: the button, and the message with the folder it was told of in. */
internal data class NoticeAsk(val button: NoticeButton, val message: ULong, val folder: ULong)

/**
 * What an intent from a notification's button asks for, from its parts, none of them trusted to be
 * there: a known button that fits the [action] (Reply opens the window, the others act without it),
 * and a message and its folder. Anything else is nothing.
 */
internal fun readNoticeAsk(action: String?, button: String?, message: Long?, folder: Long?): NoticeAsk? {
    val chosen = NoticeButton.entries.firstOrNull { it.code == button } ?: return null
    if (message == null || folder == null) return null
    val fits = when (action) {
        ACTION_NOTICE_BUTTON -> chosen.triage() != null
        ACTION_NOTICE_REPLY -> chosen == NoticeButton.Reply
        else -> false
    }
    return if (fits) NoticeAsk(chosen, message.toULong(), folder.toULong()) else null
}

/** [readNoticeAsk] of [intent]; an intent whose extras will not unparcel is nothing. */
internal fun noticeAskOf(intent: Intent): NoticeAsk? = try {
    readNoticeAsk(
        intent.action,
        intent.getStringExtra(EXTRA_BUTTON),
        if (intent.hasExtra(EXTRA_MESSAGE)) intent.getLongExtra(EXTRA_MESSAGE, 0L) else null,
        if (intent.hasExtra(EXTRA_FOLDER)) intent.getLongExtra(EXTRA_FOLDER, 0L) else null,
    )
} catch (_: RuntimeException) {
    null
}

/**
 * The button [button] of the notification about [about]: Reply opens the window (through
 * [NOTICE_REPLY_ALIAS]), the others go to [NoticeActionReceiver]. Both not exported, immutable, and
 * told apart by their address (the message and the button) and their request code.
 */
internal fun noticePendingIntent(context: Context, button: NoticeButton, about: NotifiedMessage): PendingIntent {
    val reply = button == NoticeButton.Reply
    val target = if (reply) {
        Intent().setClassName(context, NOTICE_REPLY_ALIAS).setAction(ACTION_NOTICE_REPLY)
    } else {
        Intent(context, NoticeActionReceiver::class.java).setAction(ACTION_NOTICE_BUTTON)
    }
    val intent = target
        .setData(Uri.fromParts(NOTICE_SCHEME, "${about.id}/${button.code}", null))
        .putExtra(EXTRA_BUTTON, button.code)
        .putExtra(EXTRA_MESSAGE, about.id.toLong())
        .putExtra(EXTRA_FOLDER, about.folder.toLong())
    val code = noticeRequestCode(about.id, button)
    val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    return if (reply) {
        PendingIntent.getActivity(context, code, intent, flags)
    } else {
        PendingIntent.getBroadcast(context, code, intent, flags)
    }
}

/** A request code of its own for each message and button. */
internal fun noticeRequestCode(message: ULong, button: NoticeButton): Int =
    notificationId(message) * NoticeButton.entries.size + button.ordinal

/** How long a notification's button waits for the core to open (the system gives a receiver little time). */
internal const val NOTICE_CORE_WAIT_MS = 8_000L

/** The longest a notification's button holds its broadcast open, whatever it waits for. */
internal const val NOTICE_BUDGET_MS = 20_000L

/** Takes away the new-mail notification about [message] (its Reply was taken up in the window). */
internal fun cancelMailNotice(context: Context, message: ULong) {
    NotificationManagerCompat.from(context).cancel(MAIL_NOTIFICATION_TAG, notificationId(message))
}

/** How long the word a button leaves in its notification's place stays before it goes by itself. */
internal const val NOTICE_CONFIRM_MS = 4_000L

/** What a notification's button came to. */
internal enum class NoticeOutcome { Done, Moved, Failed }

/** The word a notification's button leaves in its place: what it did, or why it did nothing; nothing of the message. */
internal fun noticeConfirmation(button: NoticeButton, outcome: NoticeOutcome, texts: Texts): String = when (outcome) {
    NoticeOutcome.Failed -> texts.t("That did not work.")
    NoticeOutcome.Moved -> texts.t("The message was moved or deleted already")
    NoticeOutcome.Done -> when (button) {
        NoticeButton.Delete -> texts.tn(1uL, "{n} message deleted", "{n} messages deleted")
        NoticeButton.Archive -> texts.tn(1uL, "{n} message archived", "{n} messages archived")
        NoticeButton.Junk -> texts.tn(1uL, "{n} message moved to Junk", "{n} messages moved to Junk")
        NoticeButton.MarkRead -> texts.t("Marked as read")
        NoticeButton.Flag -> texts.t("Flagged")
        // (Reply opens the window and leaves no word.)
        NoticeButton.Reply -> texts.t("Reply")
    }
}

/**
 * Puts [said] in the place of the notification about [message]: the same notification, so that the
 * shade stays open, quiet, saying nothing of the message, and gone by itself after
 * [NOTICE_CONFIRM_MS] (or swiped away).
 */
internal fun confirmMailNotice(context: Context, message: ULong, said: String) {
    val notification = NotificationCompat.Builder(context, CHANNEL_MAIL)
        .setSmallIcon(R.drawable.ic_notification)
        .setContentTitle(said)
        .setCategory(NotificationCompat.CATEGORY_STATUS)
        .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
        .setOnlyAlertOnce(true)
        .setSilent(true)
        .setTimeoutAfter(NOTICE_CONFIRM_MS)
        .setAutoCancel(true)
        .build()
    try {
        NotificationManagerCompat.from(context).notify(MAIL_NOTIFICATION_TAG, notificationId(message), notification)
    } catch (_: SecurityException) {
        // The right to notify was taken away meanwhile: nothing is shown.
    }
}

/**
 * The buttons of a new-mail notification that act without opening the app (Delete, Archive, Mark as
 * read, Flag, Junk). Not exported: only the app's own notifications reach it.
 */
class NoticeActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val ask = noticeAskOf(intent) ?: return
        val triage = ask.button.triage() ?: return
        val app = context.applicationContext as? MailCraftApplication ?: return
        val pending = goAsync()
        app.runNoticeButton(ask.button, triage, ask.message, ask.folder) { pending.finish() }
    }
}
