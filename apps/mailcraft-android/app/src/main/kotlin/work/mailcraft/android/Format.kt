// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

private const val MIN_SECONDS = -62_135_596_800L
private const val MAX_SECONDS = 253_402_300_799L
private const val KIB = 1024L

/** A message's time as a date and time in the device's language and time zone. */
internal fun formatDateTime(seconds: Long): String =
    DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT)
        .withZone(ZoneId.systemDefault())
        .format(Instant.ofEpochSecond(seconds.coerceIn(MIN_SECONDS, MAX_SECONDS)))

/** A number of bytes as an owner reads it: `340 KB`, `12.4 MB`, at least 1 KB. */
internal fun sizeLabel(bytes: Long): String {
    val kib = maxOf(bytes, KIB) / KIB
    return if (kib < KIB) "$kib KB" else "%.1f MB".format(java.util.Locale.ROOT, kib / KIB.toDouble())
}
