// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

private val Indent = 16.dp
private val ItemGutter = 12.dp
private val HeaderGutter = 28.dp

/**
 * The ☰ drawer: each account with its folders (as the core's folder query lists them, with their
 * unread counts), and Settings pinned at the bottom whatever the length of the list.
 */
@Composable
internal fun AppDrawer(
    texts: Texts,
    accounts: List<AccountItem>,
    selectedFolder: ULong?,
    onFolder: (AccountItem, FolderItem) -> Unit,
    onSettings: () -> Unit,
) {
    ModalDrawerSheet {
        Column(modifier = Modifier.weight(1f).verticalScroll(rememberScrollState())) {
            accounts.forEach { account ->
                AccountHeader(account)
                account.folders.forEach { folder ->
                    FolderRow(texts, folder, selected = folder.id == selectedFolder) { onFolder(account, folder) }
                }
            }
        }
        HorizontalDivider()
        NavigationDrawerItem(
            label = { Text(texts.t("Settings")) },
            icon = { Icon(AppIcons.Settings, contentDescription = null) },
            selected = false,
            onClick = onSettings,
            modifier = Modifier.padding(NavigationDrawerItemDefaultsPadding),
        )
    }
}

private val NavigationDrawerItemDefaultsPadding = PaddingValues(horizontal = ItemGutter, vertical = 8.dp)

@Composable
private fun AccountHeader(account: AccountItem) {
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = HeaderGutter, vertical = 12.dp)) {
        Text(account.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(
            account.email,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun FolderRow(texts: Texts, folder: FolderItem, selected: Boolean, onClick: () -> Unit) {
    val unread = if (folder.unread > 0u) {
        texts.tn(folder.unread.toULong(), "{n} unread message", "{n} unread messages")
    } else {
        ""
    }
    NavigationDrawerItem(
        label = { Text(folder.name, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        icon = { Icon(AppIcons.Folder, contentDescription = null) },
        badge = {
            if (folder.unread > 0u) {
                Text(folder.unread.toString(), modifier = Modifier.semantics { contentDescription = unread })
            }
        },
        selected = selected,
        onClick = onClick,
        modifier = Modifier.padding(start = Indent * folder.depth, end = 0.dp).padding(horizontal = ItemGutter),
    )
}
