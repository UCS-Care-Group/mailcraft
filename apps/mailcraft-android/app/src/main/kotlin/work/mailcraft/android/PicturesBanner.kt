// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The words of the banner offering a message's pictures. */
internal fun bannerText(texts: Texts, ui: ReadingUi): String? = when {
    ui.offersPictures -> texts.t("Pictures not downloaded.")
    ui.access == PictureAccess.Allowed && ui.waiting ->
        texts.t("Data Saver is on, so the pictures wait until you ask for them.")
    else -> null
}

/**
 * One line: what is held back, Download at the right, and the lasting choices (this sender, this domain) behind
 * a menu, so the banner never pushes the message down the screen.
 */
@Composable
internal fun PicturesBanner(texts: Texts, ui: ReadingUi, actions: ReadingActions) {
    val words = bannerText(texts, ui) ?: return
    val lasting = if (ui.offersPictures) ui.choices.filter { it != AllowScope.Message } else emptyList()
    val download = if (ui.offersPictures) ({ actions.onAllow(AllowScope.Message) }) else actions.onDownloadNow
    Surface(
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        shape = MaterialTheme.shapes.medium,
        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
    ) {
        Row(modifier = Modifier.padding(start = Pad), verticalAlignment = Alignment.CenterVertically) {
            Text(words, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
            TextButton(onClick = download) { Text(texts.t("Download")) }
            if (lasting.isNotEmpty()) LastingChoices(texts, lasting, ui.facts, actions)
        }
    }
}

@Composable
private fun LastingChoices(texts: Texts, choices: List<AllowScope>, facts: MessageFacts?, actions: ReadingActions) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(AppIcons.More, contentDescription = texts.t("More actions")) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            choices.forEach { choice ->
                DropdownMenuItem(
                    text = { Text(choiceLabel(texts, choice, facts)) },
                    onClick = {
                        open = false
                        actions.onAllow(choice)
                    },
                )
            }
        }
    }
}

/** What each way of allowing pictures says on its button. */
internal fun choiceLabel(texts: Texts, choice: AllowScope, facts: MessageFacts?): String {
    val address = facts?.address?.let { PictureAddresses.sender(it) }.orEmpty()
    return when (choice) {
        AllowScope.Message -> texts.t("Download pictures")
        AllowScope.Sender ->
            texts.t("Always download pictures from {sender}").replace("{sender}", address.take(MAX_SHOWN))
        AllowScope.Domain -> texts.t("Always from @{domain}")
            .replace("{domain}", PictureAddresses.domain(address).orEmpty().take(MAX_SHOWN))
    }
}

private const val MAX_SHOWN = 60
private val Pad = 12.dp

