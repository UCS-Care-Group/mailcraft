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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val IconBox = 24.dp

/** Where the sheet is: its list of actions, or one of the two pickers. */
private enum class SheetPage { Actions, Move, Categories }

/** All the actions on one message: the sheet that a long press opens. */
@Composable
internal fun RowActionsSheet(
    context: MailListContext,
    row: InboxRow,
    onTriage: (Triage, InboxRow) -> Unit,
    onClose: () -> Unit,
) {
    var page by remember { mutableStateOf(SheetPage.Actions) }
    val texts = context.texts
    val choose = { triage: Triage ->
        onTriage(triage, row)
        onClose()
    }
    ModalBottomSheet(onDismissRequest = onClose) {
        Column(modifier = Modifier.verticalScroll(rememberScrollState()).padding(bottom = Gutter)) {
            Text(
                row.subject,
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp),
            )
            when (page) {
                SheetPage.Actions -> ActionItems(texts, row, { page = it }, choose)
                SheetPage.Move -> {
                    val folders = context.accounts.firstOrNull { it.id == row.account }?.folders.orEmpty()
                    folders.filter { it.id != row.folder }.forEach { folder ->
                        SheetItem(ListIcons.Move, folder.name) { choose(Triage.MoveTo(folder.id)) }
                    }
                }
                SheetPage.Categories -> context.categories.forEach { category ->
                    val on = category.name in row.categories
                    SheetItem(
                        icon = if (on) AppIcons.Check else ListIcons.Tag,
                        label = category.name,
                    ) { choose(Triage.Category(category.name, !on)) }
                }
            }
        }
    }
}

@Composable
private fun ActionItems(texts: Texts, row: InboxRow, goTo: (SheetPage) -> Unit, choose: (Triage) -> Unit) {
    SheetItem(ListIcons.Read, if (row.unread) texts.t("Mark as read") else texts.t("Mark as unread")) {
        choose(Triage.MarkRead(read = row.unread))
    }
    SheetItem(ListIcons.Flag, if (row.flagged) texts.t("Clear flag") else texts.t("Flag")) {
        choose(Triage.Flag(on = !row.flagged))
    }
    SheetItem(ListIcons.Archive, texts.t("Archive")) { choose(Triage.Archive) }
    SheetItem(ListIcons.Delete, texts.t("Delete")) { choose(Triage.Delete) }
    SheetItem(ListIcons.Junk, texts.t("Move to Junk")) { choose(Triage.Junk) }
    SheetItem(ListIcons.Move, texts.t("Move to…")) { goTo(SheetPage.Move) }
    SheetItem(ListIcons.Tag, texts.t("Categories…")) { goTo(SheetPage.Categories) }
}

@Composable
private fun SheetItem(icon: ImageVector, label: String, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = Gutter, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(Gutter),
    ) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(IconBox))
        Text(label, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}
