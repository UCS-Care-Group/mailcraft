// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.MessageView

private val Gutter = 16.dp
private val ButtonGap = 8.dp
private const val RGB_MASK = 0xFFFFFF

/** What the reading pane asks of the app. */
internal class ReadingActions(
    /** Message [id] is shown with a body this many dp wide in a dark or light window: read it so. */
    val onOpen: (ULong, Int, Boolean) -> Unit,
    /** Message [id] is no longer on screen: pictures still on their way are let go of. */
    val onClose: (ULong) -> Unit,
    val onAllow: (AllowScope) -> Unit,
    val onDownloadNow: () -> Unit,
    val onLink: (LinkInfo) -> LinkResult,
    val attachments: AttachmentActions = AttachmentActions.None,
    /** Start writing: a reply, a forward, a draft continued, or a message for a `mailto:` link. */
    val onCompose: (ComposeStart) -> Unit = {},
    /** The toolbar flagged or marked read the open message: the view follows. */
    val onTriaged: (id: ULong, unread: Boolean?, flagged: Boolean?) -> Unit = { _, _, _ -> },
) {
    companion object {
        val None = ReadingActions({ _, _, _ -> }, {}, {}, {}, { LinkResult.Ignored })
    }
}

/** A way of answering a message, or continuing a draft, which the composer takes. */
internal enum class Reply { Reply, ReplyAll, Forward, Edit }

/**
 * The reading pane: the message's header, the banner that offers its pictures, its attachments, and
 * its body drawn from the display blocks. Nothing in the message runs; links are followed only to the
 * browser, the dialer and the composer, and a long press says where one really goes first.
 */
@Composable
@Suppress("LongMethod")
internal fun ReadingPane(
    texts: Texts,
    id: ULong,
    ui: ReadingUi,
    actions: ReadingActions,
    modifier: Modifier = Modifier,
    attachments: AttachmentUi = AttachmentUi(),
    /** The message is a draft: it is continued rather than answered. */
    draft: Boolean = false,
    tools: ReadingTools = ReadingTools.None,
) {
    val dark = isSystemInDarkTheme()
    // In a dark window the body may sit on a light page; the choice for this message alone wins.
    var pageChoice by remember(id) { mutableStateOf<Boolean?>(null) }
    val light = dark && (pageChoice ?: tools.prefs.lightMessages)
    val snackbar = remember { SnackbarHostState() }
    var notice by remember { mutableStateOf<String?>(null) }
    var details by remember { mutableStateOf<LinkInfo?>(null) }
    LaunchedEffect(notice) {
        notice?.let {
            snackbar.showSnackbar(it)
            notice = null
        }
    }
    Box(modifier = modifier.fillMaxSize()) {
        BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
            val bodyWidth = maxWidth - Gutter * 2
            // (A light page asks the core for the message's light design, the one its sender made for it.)
            val designDark = dark && !light
            LaunchedEffect(id, bodyWidth.value.toInt(), designDark) {
                actions.onOpen(id, bodyWidth.value.toInt(), designDark)
            }
            DisposableEffect(id) { onDispose { actions.onClose(id) } }
            val opened = ui.opened
            when {
                ui.load == LoadState.Failed -> Centered(texts.t("The message could not be read."))
                opened == null || ui.id != id -> Centered(texts.t("Loading…"))
                else -> {
                    val colors = if (light) LightColors else MaterialTheme.colorScheme
                    val env = remember(opened, ui.pictures, texts, colors) {
                        BodyEnv(
                            texts = texts,
                            links = opened.links,
                            pictures = ui.pictures,
                            rule = colors.outlineVariant,
                            onLink = { link -> notice = followed(texts, actions, link) },
                            onLinkDetails = { details = it },
                        )
                    }
                    val page = if (light) colors.surfaceContainerLowest else colors.surface
                    val paper = Paper.of(page.toArgb() and RGB_MASK, colors.primary.toArgb() and RGB_MASK)
                        .copy(ink = colors.onSurface.toArgb() and RGB_MASK)
                    Column(modifier = Modifier.fillMaxSize()) {
                        tools.context?.let {
                            ReadingToolbar(texts, tools, it, opened.view, dark, light, actions.onTriaged) { on ->
                                pageChoice = on
                            }
                            HorizontalDivider()
                        }
                        MessageBody(
                            texts, ui, opened, env, paper, bodyWidth, actions, tools, if (light) page else null,
                            Modifier.weight(1f),
                        )
                        HorizontalDivider()
                        ReplyBar(texts, draft) { actions.onCompose(startFor(it, id)) }
                    }
                }
            }
        }
        SnackbarHost(snackbar, modifier = Modifier.align(Alignment.BottomCenter))
    }
    AttachmentLayers(texts, attachments, actions.attachments) { notice = it }
    details?.let { link ->
        LinkDetails(texts, link, onOpen = {
            notice = followed(texts, actions, link)
            details = null
        }) { details = null }
    }
}

private fun linkNotice(texts: Texts, result: LinkResult): String? = when (result) {
    LinkResult.NoApp -> texts.t("No app on this phone can open that link.")
    LinkResult.Opened, LinkResult.Ignored -> null
}

/** Follows [link]: a mail address opens the composer, filled in and never sent by itself; the rest goes to the core. */
private fun followed(texts: Texts, actions: ReadingActions, link: LinkInfo): String? {
    if (link.goes != LinkGoes.Composer) return linkNotice(texts, actions.onLink(link))
    actions.onCompose(ComposeStart.New(0uL, link.shown))
    return null
}

@Composable
private fun MessageBody(
    texts: Texts,
    ui: ReadingUi,
    opened: OpenedMessage,
    env: BodyEnv,
    paper: Paper,
    bodyWidth: androidx.compose.ui.unit.Dp,
    actions: ReadingActions,
    tools: ReadingTools,
    /** The light page the body is drawn on, in a dark window; null when it sits on the app's own. */
    lightPage: androidx.compose.ui.graphics.Color?,
    modifier: Modifier,
) {
    val view = opened.view
    val lines = remember(view.blocks) { linesOf(view.blocks) }
    val side = Modifier.padding(horizontal = Gutter)
    // The header, banner and attachments are the app's and keep its theme; only the body is the sender's page.
    val state = rememberLazyListState()
    val density = LocalDensity.current
    // A short message still sits on a light sheet down to the bar below: the last item takes what is left.
    val rest by remember(state, density) {
        derivedStateOf { with(density) { restBelow(state).toDp() } }
    }
    val bottom = if (lightPage != null) 0.dp else Gutter
    LazyColumn(
        modifier = modifier.fillMaxWidth(),
        state = state,
        contentPadding = PaddingValues(top = Gutter, bottom = bottom),
    ) {
        item(key = "header") {
            Box(side) {
                MessageHeader(texts, view, tools.prefs.headerOpen) { tools.onPrefs(tools.prefs.copy(headerOpen = it)) }
            }
        }
        item(key = "banner") { Box(side) { PicturesBanner(texts, ui, actions) } }
        if (opened.attachments.isNotEmpty()) {
            item(key = "attachments") {
                Box(side) { AttachmentsList(texts, view.id, opened.attachments, actions.attachments) }
            }
        }
        item(key = "gap") { Page(lightPage) { Box(modifier = Modifier.padding(top = Gutter)) } }
        items(lines.size, key = { "line-$it" }) { index ->
            Page(lightPage) { Box(side) { BodyLineView(lines[index], env, paper, bodyWidth) } }
        }
        if (lightPage != null) {
            item(key = END_KEY) { Page(lightPage) { Box(modifier = Modifier.height(maxOf(Gutter, rest))) } }
        }
    }
}

private const val END_KEY = "end"

/** The pixels under the items of [state] to the foot of the list, when all the other items are in view; else 0. */
private fun restBelow(state: LazyListState): Int {
    val info = state.layoutInfo
    val others = info.visibleItemsInfo.filter { it.key != END_KEY }
    if (others.size < info.totalItemsCount - 1) return 0
    val used = others.sumOf { it.size } + info.beforeContentPadding
    return (info.viewportSize.height - used).coerceAtLeast(0)
}

/** [content] on the light page when there is one: its colours are the light theme's, not the window's. */
@Composable
private fun Page(page: androidx.compose.ui.graphics.Color?, content: @Composable () -> Unit) {
    if (page == null) {
        content()
        return
    }
    MaterialTheme(colorScheme = LightColors) {
        CompositionLocalProvider(LocalContentColor provides LightColors.onSurface) {
            Box(modifier = Modifier.fillMaxWidth().background(page)) { content() }
        }
    }
}

/** What a button of the bar starts writing: an answer to message [id], or a draft continued. */
internal fun startFor(reply: Reply, id: ULong): ComposeStart = when (reply) {
    Reply.Reply -> ComposeStart.Reply(id)
    Reply.ReplyAll -> ComposeStart.ReplyAll(id)
    Reply.Forward -> ComposeStart.Forward(id)
    Reply.Edit -> ComposeStart.Draft(id)
}

@Composable
private fun ReplyBar(texts: Texts, draft: Boolean, onReply: (Reply) -> Unit) {
    Surface(tonalElevation = 2.dp) {
        FlowRow(
            modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp),
            horizontalArrangement = Arrangement.spacedBy(ButtonGap),
        ) {
            (if (draft) listOf(Reply.Edit) else Reply.entries.filter { it != Reply.Edit }).forEach { reply ->
                OutlinedButton(onClick = { onReply(reply) }) { Text(replyLabel(texts, reply)) }
            }
        }
    }
}

internal fun replyLabel(texts: Texts, reply: Reply): String = when (reply) {
    Reply.Reply -> texts.t("Reply")
    Reply.ReplyAll -> texts.t("Reply all")
    Reply.Forward -> texts.t("Forward")
    Reply.Edit -> texts.t("Edit draft")
}

@Composable
private fun LinkDetails(texts: Texts, link: LinkInfo, onOpen: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(texts.t("This link goes to")) },
        text = { Text(link.shown, style = MaterialTheme.typography.bodyLarge) },
        confirmButton = { TextButton(onClick = onOpen) { Text(texts.t("Open")) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(texts.t("Cancel")) } },
    )
}
