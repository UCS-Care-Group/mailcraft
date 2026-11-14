// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawing
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import uniffi.mailcraft_mobile.Core

@Composable
internal fun InboxScreen(core: Core, state: InboxState) {
    when (state) {
        InboxState.Loading -> Centered(core.t("Loading…"))
        is InboxState.Failed -> Centered(state.message)
        is InboxState.Ready -> InboxList(state.model)
    }
}

@Composable
private fun InboxList(model: InboxModel) {
    Column(modifier = Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.safeDrawing)) {
        Text(
            text = model.title,
            style = MaterialTheme.typography.headlineSmall,
            modifier = Modifier.padding(start = 16.dp, top = 16.dp, end = 16.dp),
        )
        Text(
            text = model.count,
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
        )
        LazyColumn(contentPadding = PaddingValues(vertical = 8.dp)) {
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
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(row.sender, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(row.subject, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(
            text = row.preview,
            style = MaterialTheme.typography.bodySmall,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
        Text(formatWhen(row.whenSeconds), style = MaterialTheme.typography.labelSmall)
    }
}

/** A message's time as a date in the device's language and time zone. */
internal fun formatWhen(seconds: Long): String =
    DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM)
        .withZone(ZoneId.systemDefault())
        .format(Instant.ofEpochSecond(seconds.coerceIn(MIN_SECONDS, MAX_SECONDS)))

// The years 0001 to 9999: a time outside them is never a message's.
private const val MIN_SECONDS = -62_135_596_800L
private const val MAX_SECONDS = 253_402_300_799L
