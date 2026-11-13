// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog

private val Gutter = 16.dp
private val Gap = 8.dp
private val IconBox = 24.dp

/** About: the version, the licences of what is bundled, and the privacy statement. */
@Composable
internal fun AboutScreenPage(texts: Texts, version: String, actions: SettingsActions, startOnLicences: Boolean = false) {
    var licences by rememberSaveable { mutableStateOf(startOnLicences) }
    BackHandler(enabled = licences) { licences = false }
    if (licences) {
        LicencesPage(texts)
        return
    }
    Column(modifier = Modifier.padding(Gutter)) {
        Text("MailCraft", style = MaterialTheme.typography.headlineSmall)
        Text(
            texts.t("Version"),
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(top = Gutter),
        )
        Text(version, style = MaterialTheme.typography.bodyLarge)
        Text(
            texts.t("MailCraft is open source, under the MIT and Apache 2.0 licences."),
            style = MaterialTheme.typography.bodyMedium,
            modifier = Modifier.padding(top = Gutter),
        )
    }
    HorizontalDivider()
    LinkRow(texts.t("Licences"), texts.t("What is bundled in this app, and under which licence")) { licences = true }
    LinkRow(texts.t("Privacy"), texts.t("What MailCraft sends, and to whom")) { actions.onOpenLink(PRIVACY_URL) }
}

@Composable
private fun LinkRow(label: String, said: String, onClick: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().clickable(role = Role.Button, onClick = onClick).padding(Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            Text(said, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Icon(AppIcons.Next, contentDescription = null, modifier = Modifier.size(IconBox))
    }
    HorizontalDivider()
}

/** The licences of what is bundled. */
@Composable
internal fun LicencesPage(texts: Texts) {
    Hint(
        texts.t(
            "MailCraft carries the software below. The Rust libraries of its core are listed with their versions in the source repository.",
        ),
    )
    BUNDLED_LICENCES.forEach { item ->
        Column(modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = Gap)) {
            Text(item.name, style = MaterialTheme.typography.bodyLarge)
            Text(item.licence, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            Text(item.part, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        HorizontalDivider()
    }
}

/** The question before a sync that needs more room than the phone has. */
@Composable
internal fun SpaceCard(texts: Texts, ask: SpaceAsk, actions: SettingsActions) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh)) {
        Column(modifier = Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(Gutter)) {
            Text(texts.t("Not much room on this phone"), style = MaterialTheme.typography.titleMedium)
            Text(
                texts.t("This sync needs about {needed}. This phone has {free} free, and MailCraft keeps some room for the phone to go on working. Sync anyway?")
                    .replace("{needed}", sizeText(ask.warning.needed))
                    .replace("{free}", sizeText(ask.warning.free)),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                texts.t("The sync waits for your answer. A shorter window in Settings needs less room."),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = actions.onSpaceNo) { Text(texts.t("Not now")) }
                TextButton(onClick = actions.onSpaceYes) { Text(texts.t("Sync anyway")) }
            }
        }
    }
}

@Composable
internal fun SpaceDialog(texts: Texts, ask: SpaceAsk, actions: SettingsActions) {
    Dialog(onDismissRequest = actions.onSpaceNo) { SpaceCard(texts, ask, actions) }
}
