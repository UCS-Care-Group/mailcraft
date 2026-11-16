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
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val Gap = 8.dp
private val IconBox = 24.dp

/** The app's privacy notice on the project's site, which About links to. */
internal const val PRIVACY_URL = "https://mailcraft.work/privacy/app/"

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
    if (ui.thinning) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                texts.tn(ui.thinned.toULong(), "{n} older message taken back to its headers", "{n} older messages taken back to its headers"),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.weight(1f).padding(Gutter),
            )
            TextButton(onClick = actions.onThinStop) { Text(texts.t("Stop")) }
        }
    }
    if (ui.keepFailed) Hint(texts.t("That didn't go through. The window is as it was."))
    StorageSection(texts, ui, actions)
}

/** The phone's cap on the mail it keeps, all accounts together, with what is kept now. */
@Composable
private fun StorageSection(texts: Texts, ui: SettingsUi, actions: SettingsActions) {
    LaunchedEffect(Unit) { actions.onStorageShown() }
    val storage = ui.storage ?: return
    HorizontalDivider()
    Text(
        texts.t("Storage on this phone"),
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(horizontal = Gutter, vertical = Gap),
    )
    Hint(
        texts.t(
            "The most mail this phone keeps, all accounts together. Past it the oldest mail goes back to its headers, then off the phone. Nothing is deleted from the server.",
        ),
    )
    Text(
        storageUsedText(texts, storage),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(horizontal = Gutter),
    )
    storage.choicesMb.forEach { mb ->
        ChoiceRow(sizeText(mb * MEBIBYTE), selected = mb == storage.capMb) { actions.onStorageCap(mb) }
    }
    if (ui.storageFailed) Hint(texts.t("That didn't go through. The limit is as it was."))
}

private const val MEBIBYTE = 1024L * 1024L

/** Remove local data: what it takes off the phone, and the button that asks. */
@Composable
internal fun RemoveDataPage(texts: Texts, removal: Removal, actions: SettingsActions) {
    Hint(
        texts.t(
            "Removes the mail, calendars, contacts and tasks kept on this phone, your accounts, and every sign-in. Nothing is deleted from your mail provider's servers, and you can add your accounts again later.",
        ),
    )
    Row(modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter), horizontalArrangement = Arrangement.End) {
        Button(
            onClick = actions.onRemoveAsk,
            enabled = removal != Removal.Working,
            colors = ButtonDefaults.buttonColors(
                containerColor = MaterialTheme.colorScheme.error,
                contentColor = MaterialTheme.colorScheme.onError,
            ),
        ) { Text(texts.t("Remove local data")) }
    }
    if (removal == Removal.Done) Hint(texts.t("Everything kept on this phone was removed."))
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

/** Notifications: the switches, and the system's own settings for each channel. */
@Composable
internal fun NotificationsPage(texts: Texts, switches: Switches, actions: SettingsActions) {
    SwitchRow(
        texts.t("New mail only"),
        texts.t("A notification says only \"New mail\", never the sender or the subject, even when the lock screen shows everything."),
        switches.newMailOnly,
    ) { actions.onSwitches(switches.copy(newMailOnly = it)) }
    SwitchRow(
        texts.t("Primary mail only"),
        texts.t("Notify only for Primary mail in Gmail accounts. Microsoft accounts do not tell Focused from Other yet, so they notify for all mail."),
        switches.primaryOnly,
    ) { actions.onSwitches(switches.copy(primaryOnly = it)) }
    SwitchRow(
        texts.t("Reminders"),
        texts.t("Show a notification when a calendar reminder falls due. Off, none is shown; the reminders themselves stay in the calendar."),
        switches.reminders,
    ) { actions.onSwitches(switches.copy(reminders = it)) }
    Hint(
        texts.t(
            "Otherwise a notification shows the sender and the subject, never the message text or its pictures. What shows on the lock screen follows the phone's own privacy setting.",
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
