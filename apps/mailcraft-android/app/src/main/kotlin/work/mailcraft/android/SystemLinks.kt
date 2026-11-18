// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ActivityNotFoundException
import android.content.Context
import android.annotation.SuppressLint
import android.content.Intent
import android.net.Uri
import android.provider.Settings

/** Opens the system's own settings for the notification channel [channelId] of this app. */
internal fun openChannelSettings(context: Context, channelId: String) {
    val intent = Intent(Settings.ACTION_CHANNEL_NOTIFICATION_SETTINGS)
        .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        .putExtra(Settings.EXTRA_CHANNEL_ID, channelId)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    try {
        context.startActivity(intent)
    } catch (_: ActivityNotFoundException) {
        // A phone with no such screen: the owner finds the channel in the system's app settings.
    }
}

/** Whether [url] is a web address that may be opened from a link of the app's own: https only. */
internal fun isWebLink(url: String): Boolean =
    url.startsWith("https://") && url.none { it.isWhitespace() || it.isISOControl() }

/**
 * Opens one of the project's own web addresses in the browser; anything but https is left.
 * (The KTX `toUri()` would be a dependency for one line.)
 */
@SuppressLint("UseKtx")
internal fun openWebLink(context: Context, url: String) {
    if (!isWebLink(url)) return
    val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    try {
        context.startActivity(intent)
    } catch (_: ActivityNotFoundException) {
        // No browser: nothing to open.
    }
}
