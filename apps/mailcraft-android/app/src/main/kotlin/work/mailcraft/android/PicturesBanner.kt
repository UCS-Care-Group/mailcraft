// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The words of the banner offering a message's pictures. */
internal fun bannerText(texts: Texts, ui: ReadingUi): String? = when {
    ui.offersPictures -> texts.t("Pictures not downloaded.")
    ui.access == PictureAccess.Allowed && ui.waiting ->
        texts.t("Data Saver is on, so the pictures wait until you ask for them.")
    else -> null
}

@Composable
internal fun PicturesBanner(texts: Texts, ui: ReadingUi, actions: ReadingActions) {
    val words = bannerText(texts, ui) ?: return
    val facts = ui.facts
    Surface(
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        shape = MaterialTheme.shapes.medium,
        modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
    ) {
        Column(modifier = Modifier.padding(Pad)) {
            Text(words, style = MaterialTheme.typography.bodyMedium)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(Gap)) {
                if (ui.offersPictures) {
                    ui.choices.forEach { choice ->
                        TextButton(onClick = { actions.onAllow(choice) }) { Text(choiceLabel(texts, choice, facts)) }
                    }
                } else {
                    TextButton(onClick = actions.onDownloadNow) { Text(texts.t("Download pictures")) }
                }
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
private val Gap = 8.dp

