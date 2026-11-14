// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog

private val Gutter = 16.dp
private val Gap = 12.dp
private val Spinner = 28.dp

/** The question put to the owner before a permission is asked for on Microsoft's page. */
@Composable
internal fun PermissionQuestionCard(texts: Texts, state: QuestionState, account: String, actions: AccountActions) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh)) {
        Column(modifier = Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(Gap)) {
            Text(state.ask.permission.question(texts), style = MaterialTheme.typography.titleMedium)
            Text(account, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            when (state) {
                is QuestionState.Asking -> {
                    Text(state.ask.permission.explanation(texts), style = MaterialTheme.typography.bodyMedium)
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = actions.onNotNow) { Text(texts.t("Not now")) }
                        TextButton(onClick = actions.onAllow) { Text(texts.t("Continue")) }
                    }
                }
                is QuestionState.Waiting -> {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Gap)) {
                        CircularProgressIndicator(modifier = Modifier.size(Spinner))
                        Text(texts.t("Finish on Microsoft's page in your browser. MailCraft is waiting."), modifier = Modifier.weight(1f))
                    }
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = actions.onCancelGrant) { Text(texts.t("Cancel")) }
                    }
                }
                is QuestionState.Told -> {
                    Text(state.text, style = MaterialTheme.typography.bodyMedium)
                    Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                        TextButton(onClick = actions.onNotNow) { Text(texts.t("OK")) }
                    }
                }
            }
        }
    }
}

/** The question, in a window over the screen. */
@Composable
internal fun PermissionQuestionDialog(texts: Texts, state: QuestionState, account: String, actions: AccountActions) {
    Dialog(onDismissRequest = { if (state !is QuestionState.Waiting) actions.onNotNow() }) {
        // (Scrolls: with large text or a short window the buttons stay reachable.)
        Box(modifier = Modifier.verticalScroll(rememberScrollState())) { PermissionQuestionCard(texts, state, account, actions) }
    }
}

/** Asked before an account is removed: what goes and what stays. */
@Composable
internal fun RemoveAccountCard(texts: Texts, account: AccountItem, actions: AccountActions, failed: Boolean = false) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh)) {
        Column(modifier = Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(Gap)) {
            Text(texts.t("Remove this account?"), style = MaterialTheme.typography.titleMedium)
            Text(account.email, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                texts.t("Its mail on this phone and its sign-in are removed. Nothing is deleted from Microsoft's servers, and you can add the account again later."),
                style = MaterialTheme.typography.bodyMedium,
            )
            if (failed) {
                Text(
                    texts.t("The account could not be removed. It and its sign-in are still here. Try again."),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Row(modifier = Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = actions.onRemoveCancel) { Text(texts.t("Keep account")) }
                TextButton(onClick = actions.onRemove) { Text(texts.t("Remove")) }
            }
        }
    }
}

@Composable
internal fun RemoveAccountDialog(texts: Texts, account: AccountItem, actions: AccountActions, failed: Boolean = false) {
    Dialog(onDismissRequest = actions.onRemoveCancel) {
        Box(modifier = Modifier.verticalScroll(rememberScrollState())) { RemoveAccountCard(texts, account, actions, failed) }
    }
}
