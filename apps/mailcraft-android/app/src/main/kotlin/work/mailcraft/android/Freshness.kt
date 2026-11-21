// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

private const val MILLIS_PER_MINUTE = 60_000L
private const val MINUTES_PER_HOUR = 60L
private const val HOURS_PER_DAY = 24L

/**
 * The line over the list that says how fresh it is: "Offline" when the device is, else how long ago
 * mail was brought up to date. Nothing when it has not been since the app started: the line says
 * only what it knows. [now] and [updatedAt] are milliseconds since 1970.
 */
internal fun freshnessLine(texts: Texts, now: Long, updatedAt: Long?, offline: Boolean): String? {
    if (offline) return texts.t("Offline")
    if (updatedAt == null) return null
    val minutes = ((now - updatedAt) / MILLIS_PER_MINUTE).coerceAtLeast(0)
    return when {
        minutes < 1 -> texts.t("Updated just now")
        minutes < MINUTES_PER_HOUR -> texts.tn(minutes.toULong(), "Updated {n} minute ago", "Updated {n} minutes ago")
        minutes < MINUTES_PER_HOUR * HOURS_PER_DAY ->
            (minutes / MINUTES_PER_HOUR).let { texts.tn(it.toULong(), "Updated {n} hour ago", "Updated {n} hours ago") }
        else ->
            (minutes / (MINUTES_PER_HOUR * HOURS_PER_DAY)).let {
                texts.tn(it.toULong(), "Updated {n} day ago", "Updated {n} days ago")
            }
    }
}
