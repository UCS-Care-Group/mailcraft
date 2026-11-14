// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context

/** One notification channel: a stable id, the importance, and the name the owner reads in the system's settings. */
internal data class ChannelSpec(val id: String, val name: String, val importance: Int)

internal const val CHANNEL_MAIL = "mail"
internal const val CHANNEL_REMINDERS = "reminders"

/** The channels, named through [t] (the interface language's own words for the English). */
internal fun channelSpecs(t: (String) -> String): List<ChannelSpec> = listOf(
    ChannelSpec(CHANNEL_MAIL, t("New mail"), NotificationManager.IMPORTANCE_DEFAULT),
    ChannelSpec(CHANNEL_REMINDERS, t("Reminders"), NotificationManager.IMPORTANCE_HIGH),
)

/**
 * Creates the channels, or renames them when the language changed (creating one that exists only
 * updates its name, so what the owner set for it is kept).
 */
internal fun createChannels(context: Context, specs: List<ChannelSpec>) {
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    specs.forEach { manager.createNotificationChannel(NotificationChannel(it.id, it.name, it.importance)) }
}
