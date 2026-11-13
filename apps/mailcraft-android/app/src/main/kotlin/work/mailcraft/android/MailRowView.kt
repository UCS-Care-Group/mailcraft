// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalFoundationApi::class, ExperimentalMaterial3Api::class)

package work.mailcraft.android

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.CustomAccessibilityAction
import androidx.compose.ui.semantics.customActions
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import kotlin.math.roundToInt

private val Gutter = 16.dp
private val RowGap = 2.dp
private val SmallIcon = 16.dp
private val SwipeIcon = 24.dp
private val Gap8 = Arrangement.spacedBy(space = 8.dp)
private val Gap4 = Arrangement.spacedBy(space = 4.dp)

/** What a row of the list can be told to do, besides its two swipes. */
internal class RowActions(
    val onTriage: (Triage) -> Unit,
    val onMenu: () -> Unit,
)

/** A row that swipes: to the right does [SwipeConfig.start], to the left [SwipeConfig.end]. */
@Composable
internal fun SwipeableRow(
    texts: Texts,
    row: InboxRow,
    swipe: SwipeConfig,
    actions: RowActions,
    modifier: Modifier = Modifier,
) {
    val current by rememberUpdatedState(row)
    val config by rememberUpdatedState(swipe)
    val act by rememberUpdatedState(actions)
    val state = rememberSwipeToDismissBoxState()
    // The row always springs back; the list changes when the core has done what was asked.
    LaunchedEffect(state.currentValue) {
        when (state.currentValue) {
            SwipeToDismissBoxValue.StartToEnd -> act.onTriage(config.start.triage(current))
            SwipeToDismissBoxValue.EndToStart -> act.onTriage(config.end.triage(current))
            SwipeToDismissBoxValue.Settled -> Unit
        }
        if (state.currentValue != SwipeToDismissBoxValue.Settled) state.snapTo(SwipeToDismissBoxValue.Settled)
    }
    SwipeToDismissBox(
        state = state,
        modifier = modifier,
        backgroundContent = {
            when (state.dismissDirection) {
                SwipeToDismissBoxValue.StartToEnd -> SwipeBackdrop(texts, swipe.start, row, Alignment.CenterStart)
                SwipeToDismissBoxValue.EndToStart -> SwipeBackdrop(texts, swipe.end, row, Alignment.CenterEnd)
                SwipeToDismissBoxValue.Settled -> Unit
            }
        },
    ) {
        MailRowContent(texts, row, swipe, actions)
    }
}

/**
 * A row part-way through a swipe, drawn without a gesture: [fraction] of its width to the right
 * (negative: to the left), with the colour and the word of the action it would do behind it.
 */
@Composable
internal fun SwipeInProgress(texts: Texts, row: InboxRow, swipe: SwipeConfig, fraction: Float) {
    BoxWithConstraints(modifier = Modifier.fillMaxWidth()) {
        val widthPx = with(LocalDensity.current) { maxWidth.toPx() }
        Box(modifier = Modifier.matchParentSize()) {
            if (fraction >= 0f) {
                SwipeBackdrop(texts, swipe.start, row, Alignment.CenterStart)
            } else {
                SwipeBackdrop(texts, swipe.end, row, Alignment.CenterEnd)
            }
        }
        Box(modifier = Modifier.offset { IntOffset((widthPx * fraction).roundToInt(), 0) }) {
            MailRowContent(texts, row, swipe, RowActions({}, {}))
        }
    }
}

@Composable
private fun SwipeBackdrop(texts: Texts, action: SwipeAction, row: InboxRow, align: Alignment) {
    val colors = MaterialTheme.colorScheme
    val (fill, ink) = when (action) {
        SwipeAction.Delete, SwipeAction.Junk -> colors.errorContainer to colors.onErrorContainer
        SwipeAction.Archive -> colors.primaryContainer to colors.onPrimaryContainer
        else -> colors.secondaryContainer to colors.onSecondaryContainer
    }
    Box(modifier = Modifier.fillMaxSize().background(fill).padding(horizontal = Gutter), contentAlignment = align) {
        Column(horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(action.icon(), contentDescription = null, tint = ink, modifier = Modifier.size(SwipeIcon))
            Text(swipeLabel(texts, action, row), color = ink, style = MaterialTheme.typography.labelMedium)
        }
    }
}

/** What a swipe is called for this row: reading and flagging say what they will do to it. */
internal fun swipeLabel(texts: Texts, action: SwipeAction, row: InboxRow): String = when (action) {
    SwipeAction.ToggleRead -> if (row.unread) texts.t("Mark as read") else texts.t("Mark as unread")
    SwipeAction.ToggleFlag -> if (row.flagged) texts.t("Clear flag") else texts.t("Flag")
    else -> action.label(texts)
}

internal fun SwipeAction.icon(): ImageVector = when (this) {
    SwipeAction.Archive -> ListIcons.Archive
    SwipeAction.Delete -> ListIcons.Delete
    SwipeAction.ToggleRead -> ListIcons.Read
    SwipeAction.ToggleFlag -> ListIcons.Flag
    SwipeAction.Junk -> ListIcons.Junk
}

/** One message (or conversation) as a line of the list. */
@Composable
internal fun MailRowContent(texts: Texts, row: InboxRow, swipe: SwipeConfig, actions: RowActions) {
    val weight = if (row.unread) FontWeight.Bold else FontWeight.Normal
    val description = rowDescription(texts, row)
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.surface)
            .combinedClickable(onClick = {}, onLongClick = actions.onMenu)
            .semantics(mergeDescendants = true) {
                stateDescription = description
                customActions = listOf(swipe.start, swipe.end).distinct().map { action ->
                    CustomAccessibilityAction(swipeLabel(texts, action, row)) {
                        actions.onTriage(action.triage(row))
                        true
                    }
                } + CustomAccessibilityAction(texts.t("More actions")) {
                    actions.onMenu()
                    true
                }
            }
            .padding(horizontal = Gutter, vertical = 8.dp),
        verticalArrangement = Arrangement.spacedBy(RowGap),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Gap8) {
            Line(row.sender, weight, Modifier.weight(1f))
            if (row.members > 1) Text("${row.members}", style = MaterialTheme.typography.labelMedium)
            Text(formatWhen(row.whenSeconds), style = MaterialTheme.typography.labelSmall)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Gap4) {
            Line(row.subject, weight, Modifier.weight(1f))
            if (row.hasAttachments) MarkIcon(ListIcons.Attachment)
            if (row.flagged) MarkIcon(ListIcons.Flag, MaterialTheme.colorScheme.error)
        }
        Text(row.preview, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
        if (row.categories.isNotEmpty()) {
            Text(
                row.categories.joinToString(" · "),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.primary,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
}

@Composable
private fun Line(text: String, weight: FontWeight, modifier: Modifier) {
    Text(text, fontWeight = weight, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = modifier)
}

@Composable
private fun MarkIcon(icon: ImageVector, tint: Color = MaterialTheme.colorScheme.onSurfaceVariant) {
    Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(SmallIcon))
}
