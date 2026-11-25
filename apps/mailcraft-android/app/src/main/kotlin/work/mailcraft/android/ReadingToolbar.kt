// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.MessageView

/**
 * The actions on the open message, over the body: archive, delete, flag, mark unread, move, and a menu
 * for the rest. Each runs the list's triage on the message; one that takes it out of its folder goes
 * back to the list. [darkWindow] offers the light/dark page switch for this message alone.
 */
@Composable
@Suppress("LongMethod")
internal fun ReadingToolbar(
    texts: Texts,
    tools: ReadingTools,
    context: MailListContext,
    view: MessageView,
    darkWindow: Boolean,
    light: Boolean,
    onTriaged: (ULong, Boolean?, Boolean?) -> Unit,
    onLight: (Boolean) -> Unit,
) {
    var picker by remember(view.id) { mutableStateOf<SheetPage?>(null) }
    var menu by remember { mutableStateOf(false) }
    val row = rowOf(view)
    val leave = { triage: Triage ->
        tools.onTriage(triage, row)
        tools.onLeave()
    }
    Surface(tonalElevation = 2.dp) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
        ) {
            Tool(ListIcons.Archive, texts.t("Archive")) { leave(Triage.Archive) }
            Tool(ListIcons.Delete, texts.t("Delete")) { leave(Triage.Delete) }
            Tool(ListIcons.Flag, if (view.flagged) texts.t("Clear flag") else texts.t("Flag")) {
                tools.onTriage(Triage.Flag(on = !view.flagged), row)
                onTriaged(view.id, null, !view.flagged)
            }
            Tool(ListIcons.Read, if (view.unread) texts.t("Mark as read") else texts.t("Mark as unread")) {
                tools.onTriage(Triage.MarkRead(read = view.unread), row)
                onTriaged(view.id, !view.unread, null)
                if (!view.unread) tools.onLeave()
            }
            Tool(ListIcons.Move, texts.t("Move to…")) { picker = SheetPage.Move }
            Box {
                Tool(AppIcons.More, texts.t("More")) { menu = true }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    MenuItem(texts.t("Categories…")) {
                        menu = false
                        picker = SheetPage.Categories
                    }
                    MenuItem(texts.t("Move to Junk")) {
                        menu = false
                        leave(Triage.Junk)
                    }
                    if (darkWindow) {
                        val flip = if (light) {
                            texts.t("Show on a dark background")
                        } else {
                            texts.t("Show on a light background")
                        }
                        MenuItem(flip) {
                            menu = false
                            onLight(!light)
                        }
                    }
                }
            }
        }
    }
    picker?.let { page ->
        RowActionsSheet(
            context = context,
            row = row,
            onTriage = { triage, picked ->
                tools.onTriage(triage, picked)
                if (triage.destructive) tools.onLeave()
            },
            // The reading view opens the sheet only at a picker, never at its action list, so Reply
            // is not offered here; the reply bar below the message does that.
            onReply = { picker = null },
            onClose = { picker = null },
            start = page,
        )
    }
}

@Composable
private fun Tool(icon: ImageVector, label: String, onClick: () -> Unit) {
    IconButton(onClick = onClick) { Icon(icon, contentDescription = label) }
}

@Composable
private fun MenuItem(label: String, onClick: () -> Unit) {
    DropdownMenuItem(text = { Text(label) }, onClick = onClick)
}
