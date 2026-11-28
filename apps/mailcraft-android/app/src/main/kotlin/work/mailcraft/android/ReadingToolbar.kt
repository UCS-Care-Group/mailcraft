// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.paneTitle
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.MessageView

private val Touch = 48.dp
private val MenuGap = 16.dp
private val PanelMax = 360.dp
private const val PANEL_SHARE = 0.7f
private const val SCRIM_ALPHA = 0.4f
private const val SWIPE_CLOSE = 40f

internal const val SCRIM_TAG = "message-menu-scrim"

/** The bar over the message: back (on a phone), the subject on one line, and the button of the message menu. */
@Composable
internal fun ReadingBar(texts: Texts, tools: ReadingTools, title: String, onMore: (() -> Unit)?) {
    // (No list to act on, as in a shot of the message alone: no bar.)
    if (tools.context == null) return
    Column {
        Bar(texts, tools, title, onMore)
        HorizontalDivider()
    }
}

@Composable
private fun Bar(texts: Texts, tools: ReadingTools, title: String, onMore: (() -> Unit)?) {
    Surface(tonalElevation = 2.dp) {
        Row(
            modifier = Modifier.fillMaxWidth().heightIn(min = Touch).padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (tools.phone) {
                IconButton(onClick = tools.onLeave) { Icon(AppIcons.Back, contentDescription = texts.t("Back")) }
            }
            Text(
                title,
                modifier = Modifier.weight(1f).padding(horizontal = if (tools.phone) 4.dp else 12.dp),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (onMore != null) {
                IconButton(onClick = onMore) { Icon(AppIcons.More, contentDescription = texts.t("More")) }
            }
        }
    }
}

/** One action of the message menu: [group] is the small header it sits under. */
internal class MenuEntry(val group: String, val icon: ImageVector, val label: String, val onClick: () -> Unit)

/**
 * Every action the message has, grouped: sort (archive, delete, move, junk), mark (flag, read,
 * categories) and view. A triage that takes the message out of its folder goes back to the list;
 * [pick] opens a picker; the page switch is offered only in a dark window ([darkWindow]).
 */
@Suppress("LongParameterList")
internal fun menuEntries(
    texts: Texts,
    tools: ReadingTools,
    view: MessageView,
    darkWindow: Boolean,
    light: Boolean,
    onTriaged: (ULong, Boolean?, Boolean?) -> Unit,
    onLight: (Boolean) -> Unit,
    pick: (SheetPage) -> Unit,
): List<MenuEntry> {
    val row = rowOf(view)
    val leave = { triage: Triage ->
        tools.onTriage(triage, row)
        tools.onLeave()
    }
    val sort = texts.t("Sort")
    val mark = texts.t("Mark")
    val look = texts.t("View")
    return buildList {
        add(MenuEntry(sort, ListIcons.Archive, texts.t("Archive")) { leave(Triage.Archive) })
        add(MenuEntry(sort, ListIcons.Delete, texts.t("Delete")) { leave(Triage.Delete) })
        add(MenuEntry(sort, ListIcons.Move, texts.t("Move to…")) { pick(SheetPage.Move) })
        add(MenuEntry(sort, ListIcons.Junk, texts.t("Move to Junk")) { leave(Triage.Junk) })
        val flag = if (view.flagged) texts.t("Clear flag") else texts.t("Flag")
        add(
            MenuEntry(mark, ListIcons.Flag, flag) {
                tools.onTriage(Triage.Flag(on = !view.flagged), row)
                onTriaged(view.id, null, !view.flagged)
            },
        )
        val read = if (view.unread) texts.t("Mark as read") else texts.t("Mark as unread")
        add(
            MenuEntry(mark, ListIcons.Read, read) {
                tools.onTriage(Triage.MarkRead(read = view.unread), row)
                onTriaged(view.id, !view.unread, null)
                if (!view.unread) tools.onLeave()
            },
        )
        add(MenuEntry(mark, ListIcons.Tag, texts.t("Categories…")) { pick(SheetPage.Categories) })
        if (darkWindow) {
            val flip = if (light) texts.t("Show on a dark background") else texts.t("Show on a light background")
            add(MenuEntry(look, AppIcons.Settings, flip) { onLight(!light) })
        }
    }
}

/**
 * The message menu: a panel from the right edge, about 70% of the width (capped), over a dimmed
 * message. A tap outside it, a swipe of it to the right, or Back shuts it. A row that opens a picker
 * shuts the panel and opens the sheet.
 */
@Composable
@Suppress("LongParameterList", "LongMethod")
internal fun MessageMenu(
    texts: Texts,
    tools: ReadingTools,
    view: MessageView,
    darkWindow: Boolean,
    light: Boolean,
    open: Boolean,
    onClose: () -> Unit,
    onTriaged: (ULong, Boolean?, Boolean?) -> Unit,
    onLight: (Boolean) -> Unit,
) {
    val context = tools.context ?: return
    var picker by remember(view.id) { mutableStateOf<SheetPage?>(null) }
    val entries = menuEntries(texts, tools, view, darkWindow, light, onTriaged, onLight) { picker = it }
    BackHandler(enabled = open, onBack = onClose)
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val width: Dp = minOf(maxWidth * PANEL_SHARE, PanelMax)
        AnimatedVisibility(visible = open, enter = fadeIn(), exit = fadeOut()) {
            Box(
                modifier = Modifier.fillMaxSize().background(Color.Black.copy(alpha = SCRIM_ALPHA))
                    .testTag(SCRIM_TAG)
                    .semantics { contentDescription = texts.t("Close menu") }
                    .clickable(onClick = onClose),
            )
        }
        AnimatedVisibility(
            visible = open,
            modifier = Modifier.align(Alignment.CenterEnd),
            enter = slideInHorizontally { it },
            exit = slideOutHorizontally { it },
        ) {
            Surface(
                modifier = Modifier.width(width).fillMaxHeight()
                    .semantics { paneTitle = texts.t("Message menu") }
                    .pointerInput(Unit) {
                        var drag = 0f
                        detectHorizontalDragGestures(
                            onDragStart = { drag = 0f },
                            onDragEnd = { if (drag > SWIPE_CLOSE) onClose() },
                        ) { _, delta -> drag += delta }
                    },
                tonalElevation = 6.dp,
            ) {
                Column(modifier = Modifier.verticalScroll(rememberScrollState()).padding(vertical = 8.dp)) {
                    var group = ""
                    entries.forEach { entry ->
                        if (entry.group != group) {
                            group = entry.group
                            Text(
                                group,
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp)
                                    .semantics { heading() },
                                style = MaterialTheme.typography.labelLarge,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        }
                        MenuRow(entry) {
                            onClose()
                            entry.onClick()
                        }
                    }
                }
            }
        }
    }
    picker?.let { page ->
        RowActionsSheet(
            context = context,
            row = rowOf(view),
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
private fun MenuRow(entry: MenuEntry, onClick: () -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth().heightIn(min = Touch).clickable(onClick = onClick)
            .padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(MenuGap),
    ) {
        Icon(entry.icon, contentDescription = null)
        Text(entry.label, style = MaterialTheme.typography.bodyLarge)
    }
}
