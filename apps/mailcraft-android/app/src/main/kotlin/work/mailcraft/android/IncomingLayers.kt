// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable

/** The wait while shared files are read, and why a share or link was not taken in: both stay until over or closed. */
@Composable
internal fun IncomingLayers(texts: Texts, writing: ComposeHost) {
    if (writing.reading) {
        AlertDialog(
            onDismissRequest = {},
            confirmButton = {},
            title = { Text(texts.t("Reading what was shared")) },
            text = { CircularProgressIndicator() },
        )
    }
    writing.blocked?.let { why ->
        AlertDialog(
            onDismissRequest = writing::blockedShown,
            confirmButton = { TextButton(onClick = writing::blockedShown) { Text(texts.t("Close")) } },
            text = { Text(why) },
        )
    }
}
