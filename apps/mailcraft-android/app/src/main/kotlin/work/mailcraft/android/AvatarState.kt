// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.dp

/** The sender's picture for a row, when it was found; null while there is none (the initials stay). */
internal class AvatarState(val picture: ByteArray?)

private val AvatarSize = 40.dp

/** The letters a sender's initials are made of: the first of up to two words, in capitals. */
internal fun initialsOf(label: String): String {
    val words = label.split(' ', '.', '@', '_', '-').filter { it.isNotBlank() }
    val letters = words.take(2).mapNotNull { word -> word.firstOrNull { it.isLetterOrDigit() } }
    return letters.joinToString("").uppercase().ifEmpty { "?" }
}

/**
 * A round avatar: the sender's picture on white (an icon with see-through parts stays readable in the
 * dark), else their initials.
 */
@Composable
internal fun SenderAvatar(row: InboxRow, state: AvatarState) {
    val decoded = rememberDecoded(state.picture)
    Box(
        modifier = Modifier
            .size(AvatarSize)
            .clip(CircleShape)
            .background(if (decoded != null) Color.White else MaterialTheme.colorScheme.primaryContainer)
            .clearAndSetSemantics {},
        contentAlignment = Alignment.Center,
    ) {
        if (decoded != null) {
            Image(
                bitmap = decoded.bitmap,
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.size(AvatarSize),
            )
        } else {
            Text(
                initialsOf(row.sender),
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onPrimaryContainer,
            )
        }
    }
}
