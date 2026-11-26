// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private const val RGB_MASK = 0xFFFFFF

/** What to draw for message [id]: the open one, or (a swipe just made it so) the one read ahead; null: it failed. */
internal fun shownFor(ui: ReadingUi, id: ULong): ReadingUi? = when {
    ui.id == id && ui.opened != null -> ui
    ui.id == id && ui.load == LoadState.Failed -> null
    else -> ui.near[id]?.let { ReadingUi(id = id, load = LoadState.Ready, opened = it) }
}

/** A neighbour of the open message, drawn while a drag brings it in: its page as read ahead; its links not followed. */
@Composable
@Suppress("LongParameterList")
internal fun NeighbourPage(
    texts: Texts,
    row: InboxRow?,
    ui: ReadingUi,
    actions: ReadingActions,
    tools: ReadingTools,
    draft: Boolean,
    light: Boolean,
) {
    row ?: return
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val opened = ui.near[row.id]
        if (opened == null) {
            NotThere(texts, tools, false)
        } else {
            val shown = ReadingUi(id = row.id, load = LoadState.Ready, opened = opened)
            MessagePage(texts, row.id, shown, actions, tools, draft, light, maxWidth - Gutter * 2, {}, {}) {}
        }
    }
}

/** One message as a page: its bar, its header, banner, attachments and body, and the reply bar. */
@Composable
@Suppress("LongParameterList")
internal fun MessagePage(
    texts: Texts,
    id: ULong,
    ui: ReadingUi,
    actions: ReadingActions,
    tools: ReadingTools,
    draft: Boolean,
    light: Boolean,
    bodyWidth: androidx.compose.ui.unit.Dp,
    onLink: (LinkInfo) -> Unit,
    onLinkDetails: (LinkInfo) -> Unit,
    onMenu: () -> Unit,
) {
    val opened = ui.opened ?: return
    val colors = if (light) LightColors else MaterialTheme.colorScheme
    val env = remember(opened, ui.pictures, texts, colors) {
        BodyEnv(
            texts = texts,
            links = opened.links,
            pictures = ui.pictures,
            rule = colors.outlineVariant,
            onLink = onLink,
            onLinkDetails = onLinkDetails,
        )
    }
    val page = if (light) colors.surfaceContainerLowest else colors.surface
    // A dark page recolours a message written for a white one (#1966); one with its own dark design keeps its colours.
    val turn = (page.toArgb() and RGB_MASK).takeIf { appIsDark() && !light && !opened.view.darkDesign }
    val paper = Paper.of(page.toArgb() and RGB_MASK, colors.primary.toArgb() and RGB_MASK)
        .copy(ink = colors.onSurface.toArgb() and RGB_MASK, turn = turn)
    Column(modifier = Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        val title = opened.view.subject.ifBlank { shortName(opened.view.from) }
        ReadingBar(texts, tools, title, onMenu)
        MessageBody(
            texts, ui, opened, env, paper, bodyWidth, actions, tools, if (light) page else null,
            Modifier.weight(1f),
        )
        HorizontalDivider()
        ReplyBar(texts, draft) { actions.onCompose(startFor(it, id)) }
    }
}
