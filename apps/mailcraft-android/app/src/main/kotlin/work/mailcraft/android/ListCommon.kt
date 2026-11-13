// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

private val Gutter = 16.dp

/** A message's time as a date in the device's language and time zone. */
internal fun formatWhen(seconds: Long): String =
    DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM)
        .withZone(ZoneId.systemDefault())
        .format(Instant.ofEpochSecond(seconds.coerceIn(MIN_SECONDS, MAX_SECONDS)))

/** A line of text in the middle of the space it is given. */
@Composable
internal fun Centered(text: String, modifier: Modifier = Modifier) {
    Box(modifier = modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        Text(text, modifier = Modifier.padding(Gutter))
    }
}

// The years 0001 to 9999: a time outside them is never a message's.
private const val MIN_SECONDS = -62_135_596_800L
private const val MAX_SECONDS = 253_402_300_799L
