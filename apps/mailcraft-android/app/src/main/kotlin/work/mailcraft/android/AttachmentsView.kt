// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "MagicNumber")

package work.mailcraft.android

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import uniffi.mailcraft_mobile.OpenVerdict

private val RowHeight = 56.dp

internal fun actionLabel(texts: Texts, action: AttachmentAction): String = when (action) {
    AttachmentAction.Open -> texts.t("Open")
    AttachmentAction.Save -> texts.t("Save to Files")
    AttachmentAction.Share -> texts.t("Share")
    AttachmentAction.AddToCalendar -> texts.t("Add to calendar")
}

/** The message's files, one row each: a tap does the first thing that can be done, the menu offers the rest. */
@Composable
internal fun AttachmentsList(texts: Texts, id: ULong, items: List<AttachmentItem>, actions: AttachmentActions) {
    if (items.isEmpty()) return
    Column(modifier = Modifier.fillMaxWidth().padding(top = 8.dp)) {
        Text(
            texts.tn(items.size.toULong(), "{n} attachment", "{n} attachments"),
            style = MaterialTheme.typography.labelLarge,
            modifier = Modifier.semantics { heading() },
        )
        items.forEach { item -> AttachmentRow(texts, id, item, actions) }
    }
}

@Composable
private fun AttachmentRow(texts: Texts, id: ULong, item: AttachmentItem, actions: AttachmentActions) {
    var menu by remember { mutableStateOf(false) }
    val primary = primaryOf(item)
    val offered = actionsOf(item)
    val note = when {
        item.verdict == OpenVerdict.BLOCKED || AttachmentAction.Open !in offered ->
            texts.t("Can't be opened here. Save it to keep it.")
        asksFirst(item.verdict) -> texts.t("Asks before it opens")
        else -> null
    }
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .clickable(onClickLabel = actionLabel(texts, primary), role = Role.Button) { actions.onAct(id, item, primary) },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(AppIcons.File, contentDescription = null, modifier = Modifier.size(24.dp))
        Column(modifier = Modifier.weight(1f).padding(vertical = 6.dp)) {
            Text(item.name, style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(
                listOfNotNull(sizeLabel(item.size), note).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Box {
            IconButton(onClick = { menu = true }) {
                Icon(AppIcons.More, contentDescription = texts.t("More actions for {name}").replace("{name}", item.name))
            }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                offered.forEach { action ->
                    DropdownMenuItem(
                        text = { Text(actionLabel(texts, action)) },
                        onClick = {
                            menu = false
                            actions.onAct(id, item, action)
                        },
                    )
                }
            }
        }
    }
}

/** The question, the picture, the place to save to and the notice: everything the attachments put in front of the owner. */
@Composable
internal fun AttachmentLayers(texts: Texts, ui: AttachmentUi, actions: AttachmentActions, say: (String) -> Unit) {
    ui.question?.let { q ->
        AlertDialog(
            onDismissRequest = actions.onDismiss,
            title = { Text(texts.t("Open this attachment?")) },
            text = { Text(question(texts, q.item)) },
            confirmButton = { TextButton(onClick = actions.onConfirm) { Text(texts.t("Open")) } },
            dismissButton = { TextButton(onClick = actions.onDismiss) { Text(texts.t("Cancel")) } },
        )
    }
    ui.preview?.let { PicturePreview(texts, it, actions) }
    ui.saving?.let { SaveLauncher(it, actions) }
    val notice = ui.notice
    if (notice != null) {
        val words = noticeText(texts, notice)
        LaunchedEffect(notice) {
            say(words)
            actions.onNoticeShown()
        }
    }
}

/** What the owner is asked before a file that can carry macros or scripts is opened, in the desktop's words. */
internal fun question(texts: Texts, item: AttachmentItem): String {
    val why = if (item.verdict == OpenVerdict.SCRIPTS) {
        texts.t("Pages like this open in the browser, where scripts in them run. Open it only if you know who sent it and expect it.")
    } else {
        texts.t("Files like this can carry macros, which run as programs do. Open it only if you know who sent it and expect it.")
    }
    return texts.t("Open {name}? {why}").replace("{name}", item.name).replace("{why}", why)
}

internal fun noticeText(texts: Texts, notice: AttachmentNotice): String = when (notice) {
    AttachmentNotice.NoApp -> texts.t("No app on this phone can open that file.")
    AttachmentNotice.Saved -> texts.t("Saved.")
    AttachmentNotice.SaveFailed -> texts.t("The file could not be saved there.")
    AttachmentNotice.ShareFailed -> texts.t("The file could not be shared.")
    is AttachmentNotice.Imported ->
        if (notice.events + notice.tasks == 0) {
            texts.t("Nothing new was found in that calendar file.")
        } else {
            texts.tn((notice.events + notice.tasks).toULong(), "{n} item added to your calendar", "{n} items added to your calendar")
        }
    is AttachmentNotice.Said -> notice.why.ifBlank { texts.t("That did not work.") }
}

@Composable
private fun SaveLauncher(request: SaveRequest, actions: AttachmentActions) {
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) {
        actions.onSaveTo(it?.toString())
    }
    LaunchedEffect(request) { picker.launch(request.name) }
}

/** A picture of a message, drawn inside the app: PNG, JPEG, GIF or WebP by their own bytes, and nothing else. */
@Composable
private fun PicturePreview(texts: Texts, preview: Preview, actions: AttachmentActions) {
    Dialog(onDismissRequest = actions.onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        PreviewContent(texts, preview, actions)
    }
}

/** The preview's screen: its name and the two things to do with it, over the picture. */
@Composable
internal fun PreviewContent(texts: Texts, preview: Preview, actions: AttachmentActions) {
    val decoded = rememberDecoded(preview.bytes)
    Surface(modifier = Modifier.fillMaxSize()) {
        Column(modifier = Modifier.fillMaxSize()) {
            Row(modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = actions.onDismiss) {
                    Icon(AppIcons.Close, contentDescription = texts.t("Close"))
                }
                Text(
                    preview.item.name,
                    style = MaterialTheme.typography.titleMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
            }
            Row(modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp), horizontalArrangement = Arrangement.End) {
                TextButton(onClick = { actions.onPreviewAct(AttachmentAction.Save) }) { Text(texts.t("Save to Files")) }
                TextButton(onClick = { actions.onPreviewAct(AttachmentAction.Share) }) { Text(texts.t("Share")) }
            }
            Box(modifier = Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                if (decoded == null) {
                    Text(texts.t("Loading…"))
                } else {
                    Image(
                        bitmap = decoded.bitmap,
                        contentDescription = preview.item.name,
                        contentScale = ContentScale.Fit,
                        modifier = Modifier.fillMaxSize().padding(8.dp),
                    )
                }
            }
        }
    }
}
