// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog

private val Gutter = 16.dp
private val Gap = 12.dp
private val Spinner = 28.dp

/** Add Account: Microsoft first. Its three faces are the form, the wait, and what to do when an administrator has to approve. */
@Composable
internal fun AddAccountScreen(texts: Texts, state: AddAccountState, actions: AccountActions, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(texts.t("Add account"), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    IconButton(onClick = onBack) { Icon(AppIcons.Back, contentDescription = texts.t("Back")) }
                },
            )
        },
    ) { padding ->
        Column(
            modifier = Modifier.padding(padding).verticalScroll(rememberScrollState()).padding(Gutter),
            verticalArrangement = Arrangement.spacedBy(Gap),
        ) {
            when (state) {
                is AddAccountState.Form -> AddAccountForm(texts, state, actions)
                is AddAccountState.Waiting -> SignInWaiting(texts, actions.onCancelSignIn)
                is AddAccountState.Approval -> ApprovalHelp(texts, state, actions)
                is AddAccountState.Done -> Unit
            }
        }
    }
}

@Composable
private fun AddAccountForm(texts: Texts, state: AddAccountState.Form, actions: AccountActions) {
    Text(texts.t("Microsoft account"), style = MaterialTheme.typography.titleMedium)
    Text(
        texts.t("Outlook.com, Hotmail, and Microsoft 365 work or school accounts. You sign in on Microsoft's own page in your browser, so MailCraft never sees your password."),
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    OutlinedTextField(
        value = state.email,
        onValueChange = actions.onEmail,
        modifier = Modifier.fillMaxWidth(),
        label = { Text(texts.t("Email address (optional)")) },
        supportingText = { Text(texts.t("Only used to fill in Microsoft's page for you.")) },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email),
    )
    state.notice?.let { NoticeCard(it.text(texts)) }
    Button(onClick = actions.onSignIn, modifier = Modifier.fillMaxWidth()) {
        Text(texts.t("Sign in with Microsoft"))
    }
}

@Composable
private fun NoticeCard(text: String) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer)) {
        Text(
            text,
            modifier = Modifier.padding(Gutter),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onErrorContainer,
        )
    }
}

@Composable
private fun SignInWaiting(texts: Texts, onCancel: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Gap)) {
        CircularProgressIndicator(modifier = Modifier.size(Spinner))
        Text(texts.t("Finish signing in on Microsoft's page in your browser. MailCraft is waiting."), modifier = Modifier.weight(1f))
    }
    OutlinedButton(onClick = onCancel, modifier = Modifier.fillMaxWidth()) { Text(texts.t("Cancel")) }
}

/** "Need admin approval": what is going on, what to do next, and the message to hand to an administrator. */
@Composable
private fun ApprovalHelp(texts: Texts, state: AddAccountState.Approval, actions: AccountActions) {
    Text(texts.t("Your organisation needs to approve MailCraft"), style = MaterialTheme.typography.titleMedium)
    Text(
        texts.t("Many organisations let only an administrator approve a new program. Nothing is wrong with your account or your phone."),
        style = MaterialTheme.typography.bodyMedium,
    )
    Text(texts.t("What to do next"), style = MaterialTheme.typography.titleSmall)
    Text(texts.t("1. Copy the message below and send it to your organisation's administrator."), style = MaterialTheme.typography.bodyMedium)
    Text(texts.t("2. When they have approved MailCraft, come back here and sign in again."), style = MaterialTheme.typography.bodyMedium)
    val message = state.link?.let { adminMessage(texts, it) }
    if (message != null) {
        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainer)) {
            Text(message, modifier = Modifier.padding(Gutter), style = MaterialTheme.typography.bodySmall)
        }
        Button(onClick = { actions.onCopyMessage(message) }, modifier = Modifier.fillMaxWidth()) {
            Text(texts.t("Copy message for an administrator"))
        }
        if (state.copied) {
            Text(
                texts.t("Copied. Paste it into mail or chat for your administrator."),
                color = MaterialTheme.colorScheme.primary,
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    } else {
        NoticeCard(texts.t("The message for your administrator could not be made."))
    }
    OutlinedButton(onClick = actions.onSignIn, modifier = Modifier.fillMaxWidth()) { Text(texts.t("Sign in again")) }
}

/** What the Mail tab shows when there is no account yet. */
@Composable
internal fun NoAccount(texts: Texts, onAdd: () -> Unit, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier.fillMaxWidth().padding(Gutter),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Gap, Alignment.CenterVertically),
    ) {
        Text(texts.t("No account yet"), style = MaterialTheme.typography.titleMedium)
        Text(
            texts.t("Add a Microsoft account to read your mail here."),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Button(onClick = onAdd) { Text(texts.t("Add account")) }
    }
}

/** Settings › Accounts: each account with where its permissions stand, and Add account. */
@Composable
internal fun AccountsPage(texts: Texts, accounts: List<AccountItem>, ui: AccountsUi, actions: AccountActions, onAdd: () -> Unit) {
    LaunchedEffect(accounts) { actions.onShown(accounts) }
    if (ui.savingOff) {
        Box(modifier = Modifier.padding(horizontal = Gutter, vertical = Gap)) {
            NoticeCard(texts.t("MailCraft couldn't set aside the saved mailbox it couldn't read, so changes aren't saved this time. Free up space and restart the app."))
        }
    }
    accounts.forEach { account ->
        Column(modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = Gap)) {
            Text(account.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                account.email,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            PermissionKind.entries.forEach { kind ->
                PermissionRow(texts, account, kind, ui.statuses[StatusKey(account.id, kind)] ?: PermissionStatus.Checking, actions) {
                    // Add Account opens with the address as its hint.
                    actions.onSignInAgain(account)
                    onAdd()
                }
            }
            TextButton(onClick = { actions.onRemoveAsk(account) }) { Text(texts.t("Remove account")) }
        }
        HorizontalDivider()
    }
    TextButton(onClick = onAdd, modifier = Modifier.padding(horizontal = Gutter / 2)) { Text(texts.t("Add account")) }
}

@Composable
private fun PermissionRow(
    texts: Texts,
    account: AccountItem,
    kind: PermissionKind,
    status: PermissionStatus,
    actions: AccountActions,
    onSignInAgain: () -> Unit,
) {
    Row(modifier = Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(modifier = Modifier.weight(1f)) {
            Text(kind.label(texts), style = MaterialTheme.typography.bodyLarge)
            Text(statusText(texts, status), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        when (status) {
            PermissionStatus.NotAllowed -> TextButton(onClick = { actions.onAsk(account, kind) }) { Text(texts.t("Allow")) }
            PermissionStatus.SignInNeeded -> TextButton(onClick = onSignInAgain) { Text(texts.t("Sign in again")) }
            else -> Unit
        }
    }
}
