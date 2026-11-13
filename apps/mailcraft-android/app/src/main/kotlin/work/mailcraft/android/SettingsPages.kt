// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val Gap = 8.dp
private val IconBox = 24.dp

/** The project's privacy statement, which About links to. */
internal const val PRIVACY_URL = "https://github.com/UCS-Care-Group/mailcraft/blob/main/PRIVACY.md"

@Composable
internal fun Hint(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(Gutter),
    )
}

@Composable
internal fun SwitchRow(label: String, said: String, on: Boolean, onChange: (Boolean) -> Unit) {
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

/** Mail to keep offline: a window for each account, and what it means. */
@Composable
internal fun MailOfflinePage(texts: Texts, ui: SettingsUi, accounts: List<AccountItem>, actions: SettingsActions) {
    Hint(
        texts.t(
            "How far back mail is kept in full on this phone, by the date it arrived. Older mail keeps its sender, subject and a preview, and is fetched again when you open it. A shorter window frees space; a longer one fetches more mail, and asks first when the phone is short of room.",
        ),
    )
    if (accounts.isEmpty()) Hint(texts.t("No account yet"))
    accounts.forEach { account ->
        val chosen = ui.keepOffline[account.id] ?: DEFAULT_OFFLINE
        Text(
            account.email,
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(horizontal = Gutter, vertical = Gap),
        )
        ui.codes.forEach { code ->
            ChoiceRow(offlineLabel(texts, code), selected = code == chosen) { actions.onKeepOffline(account, code) }
        }
    }
    if (ui.keepFailed) Hint(texts.t("That didn't go through. The window is as it was."))
}

@Composable
private fun ChoiceRow(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = Gutter, vertical = Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        if (selected) Icon(AppIcons.Check, contentDescription = null, modifier = Modifier.size(IconBox))
    }
    HorizontalDivider()
}

/** Notifications: the two switches, and the system's own settings for each channel. */
@Composable
internal fun NotificationsPage(texts: Texts, switches: Switches, actions: SettingsActions) {
    SwitchRow(
        texts.t("New mail only"),
        texts.t("Show notifications for new mail, not for calendar and task reminders."),
        switches.newMailOnly,
    ) { actions.onSwitches(switches.copy(newMailOnly = it)) }
    SwitchRow(
        texts.t("Primary or Focused only"),
        texts.t("Notify only for the mail that would alert: Primary for Gmail, Focused for Microsoft."),
        switches.primaryOnly,
    ) { actions.onSwitches(switches.copy(primaryOnly = it)) }
    Hint(
        texts.t(
            "A notification shows the sender and the subject, never the message text or its pictures. What shows on the lock screen follows the phone's own privacy setting.",
        ),
    )
    channelSpecs(texts::t).forEach { channel ->
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button) { actions.onChannelSettings(channel.id) }
                .padding(Gutter),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(channel.name, style = MaterialTheme.typography.bodyLarge)
                Text(
                    texts.t("Sound, vibration and lock screen, in the phone's settings"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Icon(AppIcons.Next, contentDescription = null, modifier = Modifier.size(IconBox))
        }
        HorizontalDivider()
    }
}

/** Pictures: the one switch for all mail, off until the owner turns it on. */
@Composable
internal fun PicturesSwitchPage(texts: Texts, switches: Switches, actions: SettingsActions) {
    SwitchRow(
        texts.t("Download pictures in mail automatically"),
        texts.t(
            "A picture on the web tells its sender that you opened the message, and when. Left off, pictures are downloaded only for the messages, senders and domains you say.",
        ),
        switches.downloadPictures,
    ) { actions.onSwitches(switches.copy(downloadPictures = it)) }
    Hint(texts.t("Never in Junk E-mail. With Data Saver on, a message's pictures wait until you ask for them."))
}

/** Sender images: off until the owner turns them on, and what the switch tells whom, in plain words. */
@Composable
internal fun SenderImagesSwitchPage(texts: Texts, switches: Switches, actions: SettingsActions) {
    SwitchRow(
        texts.t("Show sender images from Gravatar, favicon.im and Google"),
        texts.t(
            "Gravatar is asked for the sender's picture and told a hash of their address; else favicon.im, then Google, are asked for their website's icon and told the sender's domain. This tells those services whose pictures you look at. Only for the people you know (your contacts, and whoever you have written to) and the mail whose pictures you download, never for Junk E-mail, and a sender's picture only when your mail server confirms the mail is from them.",
        ),
        switches.senderImages,
    ) { actions.onSwitches(switches.copy(senderImages = it)) }
    Hint(texts.t("Only for the rows on screen. With Data Saver on, only pictures already kept on this phone are shown."))
}

/** Signatures: one for each account, as plain text, for new messages, replies and forwards. */
@Composable
internal fun SignaturesPage(texts: Texts, ui: SettingsUi, accounts: List<AccountItem>, actions: SettingsActions) {
    Hint(texts.t("Each account's signature goes at the end of its new messages, replies and forwards. Leave it empty for none."))
    if (accounts.isEmpty()) Hint(texts.t("No account yet"))
    accounts.forEach { account ->
        val state = ui.signatures[account.id] ?: SignatureState()
        Column(modifier = Modifier.padding(horizontal = Gutter, vertical = Gap), verticalArrangement = Arrangement.spacedBy(Gap)) {
            Text(account.email, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            if (state.editable) {
                OutlinedTextField(
                    value = state.draft,
                    onValueChange = { actions.onSignatureDraft(account, it) },
                    label = { Text(texts.t("Signature")) },
                    modifier = Modifier.fillMaxWidth(),
                    minLines = SIGNATURE_LINES,
                )
                if (state.failed) {
                    Text(texts.t("That didn't go through."), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodyMedium)
                }
                Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    Button(onClick = { actions.onSignatureSave(account) }, enabled = state.changed) { Text(texts.t("Save")) }
                }
            } else {
                Text(state.saved, style = MaterialTheme.typography.bodyMedium)
                Text(
                    texts.t("This signature has a picture or its own layout. Change it on the computer."),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        HorizontalDivider()
    }
}

private const val SIGNATURE_LINES = 3
