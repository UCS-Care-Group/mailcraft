// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val Gap = 8.dp

/** What Settings shows of pictures: the owner's choices and what is kept in the cache. */
internal data class PicturesUi(
    val prefs: PicturePrefs = PicturePrefs(),
    val kept: Int = 0,
    val keptBytes: Long = 0,
)

/** The summary under the Pictures row of Settings. */
internal fun picturesSummary(texts: Texts, ui: PicturesUi): String =
    if (ui.prefs.all) texts.t("Downloaded automatically") else texts.t("Asked first")

/** The summary under the Sender images row. */
internal fun senderImagesSummary(texts: Texts, ui: PicturesUi): String =
    if (ui.prefs.senderImages) texts.t("On") else texts.t("Off")

@Composable
private fun SwitchRow(label: String, said: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .toggleable(value = on, role = Role.Switch, onValueChange = onChange)
            .padding(Gutter),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Gutter),
    ) {
        Column(modifier = Modifier.weight(1f)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            Text(said, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = on, onCheckedChange = null)
    }
    HorizontalDivider()
}

/**
 * Pictures: the switch for all mail (off), the senders and domains pictures are always downloaded
 * from, and what is kept of downloaded pictures, with Clear.
 */
@Composable
internal fun PicturesPage(
    texts: Texts,
    ui: PicturesUi,
    onPrefs: (PicturePrefs) -> Unit,
    onClear: () -> Unit,
) {
    SwitchRow(
        texts.t("Download pictures in mail automatically"),
        texts.t(
            "A picture on the web tells its sender that you opened the message, and when. Left off, pictures are downloaded only for the messages, senders and domains you say.",
        ),
        ui.prefs.all,
    ) { onPrefs(ui.prefs.copy(all = it)) }
    Text(
        texts.t("Never in Junk E-mail. With Data Saver on, a message's pictures wait until you ask for them."),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(Gutter),
    )
    Remembered(texts, ui.prefs, onPrefs)
    val kept = if (ui.kept == 0) {
        texts.t("No downloaded pictures are kept on this phone.")
    } else {
        texts.tn(
            ui.kept.toULong(),
            "{n} downloaded picture is kept on this phone ({size}).",
            "{n} downloaded pictures are kept on this phone ({size}).",
        ).replace("{size}", sizeLabel(ui.keptBytes))
    }
    Column(modifier = Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(Gap)) {
        Text(kept, style = MaterialTheme.typography.bodyLarge)
        Text(
            texts.t("Forget the pictures that are kept; they are downloaded again when you ask."),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedButton(onClick = onClear) { Text(texts.t("Clear")) }
    }
}

@Composable
private fun Remembered(texts: Texts, prefs: PicturePrefs, onPrefs: (PicturePrefs) -> Unit) {
    val senders = prefs.senders.sorted()
    val domains = prefs.domains.sorted()
    if (senders.isEmpty() && domains.isEmpty()) {
        Text(
            texts.t(
                "No senders are remembered. \"Always download pictures from\" above a message adds its sender or its sender's domain here.",
            ),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(Gutter),
        )
        return
    }
    Text(
        texts.t("Pictures are always downloaded from ({n}):").replace("{n}", (senders.size + domains.size).toString()),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(Gutter),
    )
    senders.forEach { who ->
        RememberedRow(texts, who) { onPrefs(prefs.copy(senders = prefs.senders - who)) }
    }
    domains.forEach { domain ->
        RememberedRow(texts, "@$domain") { onPrefs(prefs.copy(domains = prefs.domains - domain)) }
    }
}

@Composable
private fun RememberedRow(texts: Texts, name: String, onForget: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(name, modifier = Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        TextButton(onClick = onForget) { Text(texts.t("Ask first again")) }
    }
}

/** Sender images: off until the owner turns them on, and what the switch tells whom, in plain words. */
@Composable
internal fun SenderImagesPage(texts: Texts, ui: PicturesUi, onPrefs: (PicturePrefs) -> Unit) {
    SwitchRow(
        texts.t("Show sender images from Gravatar, favicon.im and Google"),
        texts.t(
            "Gravatar is asked for the sender's picture and told a hash of their address; else favicon.im, then Google, are asked for their website's icon and told the sender's domain. This tells those services whose pictures you look at. Only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for Junk E-mail, and a sender's picture only when your mail server confirms the mail is from them.",
        ),
        ui.prefs.senderImages,
    ) { onPrefs(ui.prefs.copy(senderImages = it)) }
    Text(
        texts.t("Only for the rows on screen. With Data Saver on, only pictures already kept on this phone are shown."),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(Gutter),
    )
}
