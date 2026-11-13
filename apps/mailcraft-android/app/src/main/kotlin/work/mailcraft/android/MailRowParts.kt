// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp

/** What a screen reader says of the row beyond its words: unread, flagged, attachments, conversation. */
internal fun rowDescription(texts: Texts, row: InboxRow): String = listOfNotNull(
    if (row.unread) texts.t("Unread") else null,
    if (row.flagged) texts.t("Flagged") else null,
    if (row.hasAttachments) texts.t("Has attachments") else null,
    if (row.members > 1) {
        texts.tn(row.members.toULong(), "{n} message in the conversation", "{n} messages in the conversation")
    } else {
        null
    },
).joinToString(", ")

/** The heading over the first row of a group (a day or a week), in the owner's language. */
@Composable
internal fun GroupHeader(texts: Texts, group: String) {
    Text(
        text = texts.t(group),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 6.dp),
    )
}

/** A row not read yet: a pale bar where the sender will be, in the height of a row. */
@Composable
internal fun RowPlaceholder() {
    Box(modifier = Modifier.fillMaxWidth().height(PlaceholderHeight).padding(horizontal = Gutter, vertical = 12.dp)) {
        Box(
            modifier = Modifier
                .fillMaxWidth(PLACEHOLDER_WIDTH)
                .height(BarHeight)
                .background(MaterialTheme.colorScheme.surfaceVariant),
        )
    }
}

private const val PLACEHOLDER_WIDTH = 0.6f
private val PlaceholderHeight = 72.dp
private val BarHeight = 16.dp
