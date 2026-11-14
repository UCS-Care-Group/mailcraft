// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

private val Gutter = 16.dp
private val RowGap = 2.dp

@Composable
internal fun InboxScreen(texts: Texts, state: InboxState, modifier: Modifier = Modifier) {
    when (state) {
        InboxState.Loading -> Centered(texts.t("Loading…"), modifier)
        is InboxState.Failed -> Centered(state.message, modifier)
        is InboxState.Ready -> InboxList(state.model, modifier)
    }
}

@Composable
private fun InboxList(model: InboxModel, modifier: Modifier) {
    Column(modifier = modifier.fillMaxSize()) {
        Text(
            text = model.count,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp),
        )
        LazyColumn(contentPadding = PaddingValues(bottom = Gutter)) {
            items(model.rows, key = { it.id.toString() }) { row ->
                Row(row)
                HorizontalDivider()
            }
        }
    }
}

@Composable
private fun Row(row: InboxRow) {
    val weight = if (row.unread) FontWeight.Bold else FontWeight.Normal
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(RowGap),
    ) {
        Text(row.sender, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(row.subject, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(
            text = row.preview,
            style = MaterialTheme.typography.bodySmall,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        val whenText = formatWhen(row.whenSeconds, LocalMessageZone.current, LocalMessageLocale.current)
        Text(whenText, style = MaterialTheme.typography.labelSmall)
    }
}

/** The time zone message dates are shown in: the device's, unless a screenshot fixes one. */
internal val LocalMessageZone = compositionLocalOf { ZoneId.systemDefault() }

/** The language message dates are shown in: the device's, unless a screenshot fixes one. */
internal val LocalMessageLocale = compositionLocalOf { Locale.getDefault() }

/** A message's time as a date in the device's language and time zone (or the ones given). */
internal fun formatWhen(
    seconds: Long,
    zone: ZoneId = ZoneId.systemDefault(),
    locale: Locale = Locale.getDefault(),
): String =
    DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM)
        .withLocale(locale)
        .withZone(zone)
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
