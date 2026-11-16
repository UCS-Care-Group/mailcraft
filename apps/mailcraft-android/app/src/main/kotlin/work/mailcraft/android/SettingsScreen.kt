// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)

package work.mailcraft.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val IconBox = 24.dp

/** Settings: its sections, each a page of its own. */
@Composable
internal fun SettingsScreen(
    texts: Texts,
    page: SettingsPage?,
    data: ShellData,
    actions: ShellActions,
    onOpen: (SettingsPage) -> Unit,
    onBack: () -> Unit,
    onAddAccount: () -> Unit,
) {
    // The pages that hold something for each account read what is kept for them.
    LaunchedEffect(data.accounts) { actions.settings.onShown(data.accounts) }
    // About's Licences screen is a step inside About: the toolbar's Back and the system's both leave it first.
    var licences by rememberSaveable(page) { mutableStateOf(false) }
    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        text = page?.label(texts) ?: texts.t("Settings"),
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    val inLicences = page == SettingsPage.About && licences
                    IconButton(onClick = { if (inLicences) licences = false else onBack() }) {
                        Icon(AppIcons.Back, contentDescription = texts.t("Back"))
                    }
                },
            )
        },
    ) { padding ->
        Column(modifier = Modifier.padding(padding).verticalScroll(rememberScrollState())) {
            SettingsBody(texts, page, data, actions, onOpen, onAddAccount, licences) { licences = it }
        }
    }
}

@Composable
@Suppress("LongParameterList")
private fun SettingsBody(
    texts: Texts,
    page: SettingsPage?,
    data: ShellData,
    actions: ShellActions,
    onOpen: (SettingsPage) -> Unit,
    onAddAccount: () -> Unit,
    licences: Boolean,
    onLicences: (Boolean) -> Unit,
) {
    when (page) {
        null -> SettingsRows(texts, data, onOpen)
        SettingsPage.Accounts ->
            AccountsPage(texts, data.accounts, data.accountsUi, actions.accounts, onAddAccount)
        SettingsPage.Language -> LanguagePage(texts, data.chosenLanguage, actions.onLanguage)
        SettingsPage.SwipeActions -> SwipePage(texts, data.swipe, actions.onSwipe)
        SettingsPage.RemoveData -> RemoveDataPage(texts, data.settingsUi.removal, actions.settings)
        SettingsPage.About -> AboutScreenPage(texts, data.version, actions.settings, licences, onLicences)
        SettingsPage.MailOffline -> MailOfflinePage(texts, data.settingsUi, data.accounts, actions.settings)
        SettingsPage.Notifications -> NotificationsPage(texts, data.settingsUi.switches, actions.settings)
        SettingsPage.Pictures -> PicturesPage(texts, data.pictures, actions.onPictures, actions.onClearPictures)
        SettingsPage.SenderImages -> SenderImagesPage(texts, data.pictures, actions.onPictures)
        SettingsPage.Signatures -> SignaturesPage(texts, data.settingsUi, data.accounts, actions.settings)
    }
}

@Composable
private fun SettingsRows(texts: Texts, data: ShellData, onOpen: (SettingsPage) -> Unit) {
    SettingsPage.entries.forEach { entry ->
        val summary = when (entry) {
            SettingsPage.Accounts -> accountsSummary(texts, data.accounts.size)
            SettingsPage.Language -> languageName(texts, data.chosenLanguage)
            SettingsPage.SwipeActions -> "${data.swipe.start.label(texts)} · ${data.swipe.end.label(texts)}"
            SettingsPage.About -> data.version
            SettingsPage.RemoveData -> removeDataSummary(texts)
            SettingsPage.MailOffline -> offlineSummary(texts, data.settingsUi, data.accounts)
            SettingsPage.Notifications -> notificationsSummary(texts, data.settingsUi.switches)
            SettingsPage.Pictures -> picturesSummary(texts, data.pictures)
            SettingsPage.SenderImages -> senderImagesSummary(texts, data.pictures)
            SettingsPage.Signatures -> signaturesSummary(texts, data.settingsUi, data.accounts)
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(role = Role.Button) { onOpen(entry) }
                .padding(horizontal = Gutter, vertical = Gutter),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(entry.label(texts), style = MaterialTheme.typography.bodyLarge)
                Text(
                    summary,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Icon(AppIcons.Next, contentDescription = null, modifier = Modifier.size(IconBox))
        }
        HorizontalDivider()
    }
}

/** What the Accounts row says: how many accounts there are. */
private fun accountsSummary(texts: Texts, count: Int): String =
    if (count == 0) texts.t("No account yet") else texts.tn(count.toULong(), "{n} account", "{n} accounts")

/** What the language row says: the language chosen in its own name, or that the system's is used. */
private fun languageName(texts: Texts, chosen: String): String =
    LANGUAGES.firstOrNull { it.tag == chosen }?.nativeName ?: texts.t("System default")

@Composable
private fun LanguagePage(texts: Texts, chosen: String, onLanguage: (String) -> Unit) {
    ChoiceRow(texts.t("System default"), selected = chosen.isEmpty()) { onLanguage("") }
    LANGUAGES.forEach { language ->
        ChoiceRow(language.nativeName, selected = language.tag == chosen) { onLanguage(language.tag) }
    }
}

@Composable
private fun SwipePage(texts: Texts, config: SwipeConfig, onSwipe: (SwipeConfig) -> Unit) {
    // A row's swipe from its start runs `start`: rightwards in a left-to-right language, leftwards
    // in a right-to-left one, so the headings name the way the finger goes.
    val ltr = LocalLayoutDirection.current == LayoutDirection.Ltr
    val right = texts.t("Swipe to the right")
    val left = texts.t("Swipe to the left")
    Text(
        if (ltr) right else left,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(Gutter),
    )
    SwipeAction.entries.forEach { action ->
        ChoiceRow(action.label(texts), selected = action == config.start) { onSwipe(config.copy(start = action)) }
    }
    Text(
        if (ltr) left else right,
        style = MaterialTheme.typography.labelLarge,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(Gutter),
    )
    SwipeAction.entries.forEach { action ->
        ChoiceRow(action.label(texts), selected = action == config.end) { onSwipe(config.copy(end = action)) }
    }
}

@Composable
private fun ChoiceRow(name: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .selectable(selected = selected, role = Role.RadioButton, onClick = onClick)
            .padding(horizontal = Gutter, vertical = Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(name, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        if (selected) Icon(AppIcons.Check, contentDescription = null, modifier = Modifier.size(IconBox))
    }
    HorizontalDivider()
}
