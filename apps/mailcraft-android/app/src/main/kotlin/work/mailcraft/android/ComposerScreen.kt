// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class, ExperimentalMaterial3Api::class)
@file:Suppress("MaxLineLength", "MagicNumber", "TooManyFunctions")

package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.ComposeFormat
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.FromChoice
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.SuggestionRecord

/** What the lines of recipients ask. */
internal class LineActions(
    val onTyped: (RecipientLine, String) -> Unit,
    val onCommit: (RecipientLine) -> Unit,
    val onRemove: (RecipientLine, Int) -> Unit,
    val onPick: (RecipientLine, SuggestionRecord) -> Unit,
) {
    companion object {
        val None = LineActions({ _, _ -> }, {}, { _, _ -> }, { _, _ -> })
    }
}

/** What the files and pictures of the message ask: those read from the phone's picker, and one taken off. */
internal class FileActions(
    val onAttach: (String, ByteArray?) -> Unit,
    val onPicture: (ByteArray?) -> Unit,
    val onRemove: (UInt) -> Unit,
) {
    companion object {
        val None = FileActions({ _, _ -> }, {}, {})
    }
}

/** What the composer asks of the app. */
@Suppress("LongParameterList")
internal class ComposerActions(
    /** Leave the message: it is kept in Drafts when it has changes. */
    val onClose: () -> Unit,
    val onDiscard: () -> Unit,
    val onSend: () -> Unit,
    /** Keep the message in Drafts (the autosave, and when the app is left). */
    val onSave: () -> Unit,
    val onNoticeShown: () -> Unit,
    val onFrom: (ULong) -> Unit,
    val onSubject: (String) -> Unit,
    val onBody: (TextFieldValue) -> Unit,
    val onFormat: (ComposeFormat) -> Unit,
    val lines: LineActions = LineActions.None,
    val files: FileActions = FileActions.None,
    /** The owner has read what was left out of a share. */
    val onSkippedDismissed: () -> Unit = {},
) {
    companion object {
        val None = ComposerActions({}, {}, {}, {}, {}, {}, {}, {}, {})
    }
}

/** What opens the phone's pickers for a file and for a picture. */
internal class ComposerPickers(val file: () -> Unit, val picture: () -> Unit) {
    companion object {
        val None = ComposerPickers({}, {})
    }
}

private val Gutter = 16.dp

/** On a tablet the message is a column, not a page-wide field. */
private val MaxWidth = 720.dp
private val FieldHeight = 48.dp

/**
 * The composer: the account it is sent from (when there is more than one), the people it is for, the
 * subject, the bar of looks, the body, the pictures in it, its files, and the original it quotes. The core
 * holds the message; this draws it and passes what the owner does.
 */
@Composable
internal fun ComposerScreen(
    texts: Texts,
    ui: ComposeUi,
    actions: ComposerActions,
    pickers: ComposerPickers,
    modifier: Modifier = Modifier,
) {
    val view = ui.view
    val snackbar = remember { SnackbarHostState() }
    val notice = ui.notice
    LaunchedEffect(notice) {
        if (notice != null) {
            snackbar.showSnackbar(noticeText(texts, notice))
            actions.onNoticeShown()
        }
    }
    BackHandler(enabled = view != null) { actions.onClose() }
    Scaffold(
        modifier = modifier,
        topBar = { ComposerBar(texts, ui, actions, pickers) },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        if (view == null) {
            Centered(ui.failed ?: texts.t("Loading…"), Modifier.padding(padding))
        } else {
            Box(modifier = Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                ComposerBody(texts, ui, view, actions, pickers, Modifier.widthIn(max = MaxWidth))
            }
        }
    }
}

internal fun noticeText(texts: Texts, notice: ComposeNotice): String = when (notice) {
    is ComposeNotice.Said -> notice.why.ifBlank { texts.t("That did not work.") }
    ComposeNotice.Unreadable -> texts.t("That file could not be read.")
    ComposeNotice.NoRecipient -> texts.t("Add somebody to send this to.")
}

@Composable
private fun ComposerBar(texts: Texts, ui: ComposeUi, actions: ComposerActions, pickers: ComposerPickers) {
    var menu by remember { mutableStateOf(false) }
    TopAppBar(
        title = {
            val title = ui.view?.let { composeTitle(texts, it.kind) } ?: texts.t("New message")
            Column {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (ui.saved) {
                    Text(texts.t("Saved to Drafts"), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        },
        navigationIcon = {
            IconButton(onClick = actions.onClose) { Icon(AppIcons.Close, contentDescription = texts.t("Close and keep as draft")) }
        },
        actions = {
            IconButton(onClick = pickers.file) { Icon(AppIcons.Attach, contentDescription = texts.t("Attach a file")) }
            FilledTonalButton(onClick = actions.onSend, enabled = ui.view != null && !ui.sending, modifier = Modifier.padding(horizontal = 4.dp)) {
                Text(texts.t("Send"))
            }
            Box {
                IconButton(onClick = { menu = true }) { Icon(AppIcons.More, contentDescription = texts.t("More")) }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    DropdownMenuItem(
                        text = { Text(texts.t("Discard")) },
                        onClick = {
                            menu = false
                            actions.onDiscard()
                        },
                    )
                }
            }
        },
    )
}

@Composable
private fun ComposerBody(
    texts: Texts,
    ui: ComposeUi,
    view: ComposeView,
    actions: ComposerActions,
    pickers: ComposerPickers,
    modifier: Modifier,
) {
    var more by remember { mutableStateOf(false) }
    Column(modifier = modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState())) {
        ui.skipped?.let { SkippedBanner(texts, it, actions.onSkippedDismissed) }
        if (ui.accounts.size > 1) {
            FromRow(texts, ui.accounts, view.account, actions.onFrom)
            HorizontalDivider()
        }
        RecipientsRow(texts, texts.t("To"), RecipientLine.TO, ui, actions, content = {
            TextButton(onClick = { more = !more }) { Text(if (more) texts.t("Hide Cc and Bcc") else texts.t("Cc and Bcc")) }
        })
        if (more || ui.people[RecipientLine.CC].orEmpty().isNotEmpty() || ui.people[RecipientLine.BCC].orEmpty().isNotEmpty()) {
            HorizontalDivider()
            RecipientsRow(texts, texts.t("Cc"), RecipientLine.CC, ui, actions)
            HorizontalDivider()
            RecipientsRow(texts, texts.t("Bcc"), RecipientLine.BCC, ui, actions)
        }
        HorizontalDivider()
        SubjectRow(texts, ui.subject, actions.onSubject)
        HorizontalDivider()
        if (view.html) FormatBar(texts, ui, view, actions, pickers)
        BodyField(texts, ui, view, actions)
        PicturesStrip(texts, view, ui)
        FilesList(texts, view, actions)
        QuoteBlock(texts, view)
    }
}

/** What a share left out, said in words and kept until it is dismissed: a snackbar would be gone before it was read. */
@Composable
private fun SkippedBanner(texts: Texts, text: String, onDismiss: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(MaterialTheme.colorScheme.errorContainer)
            .padding(start = Gutter, end = 4.dp, top = 4.dp, bottom = 4.dp)
            .semantics { liveRegion = LiveRegionMode.Polite },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onErrorContainer, modifier = Modifier.weight(1f))
        TextButton(onClick = onDismiss) { Text(texts.t("Dismiss")) }
    }
}

@Composable
private fun FromRow(texts: Texts, choices: List<FromChoice>, current: ULong, onFrom: (ULong) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val chosen = choices.firstOrNull { it.account == current }
    Box {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = FieldHeight)
                .clickable(onClickLabel = texts.t("Choose who this is sent from"), role = Role.Button) { open = true }
                .padding(horizontal = Gutter, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(texts.t("From"), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.widthIn(min = 48.dp))
            Text(chosen?.let { fromLabel(texts, it) }.orEmpty(), style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            fromOrder(choices, current).forEach { choice ->
                DropdownMenuItem(
                    text = { Text(fromLabel(texts, choice)) },
                    onClick = {
                        open = false
                        onFrom(choice.account)
                    },
                )
            }
        }
    }
}

@Composable
private fun RecipientsRow(
    texts: Texts,
    label: String,
    line: RecipientLine,
    ui: ComposeUi,
    actions: ComposerActions,
    content: @Composable () -> Unit = {},
) {
    val people = ui.people[line].orEmpty()
    Column(modifier = Modifier.fillMaxWidth()) {
        Row(modifier = Modifier.fillMaxWidth().heightIn(min = FieldHeight).padding(start = Gutter), verticalAlignment = Alignment.Top) {
            Text(
                label,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.widthIn(min = 48.dp).padding(top = 14.dp),
            )
            FlowRow(
                modifier = Modifier.weight(1f).padding(vertical = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
                itemVerticalAlignment = Alignment.CenterVertically,
            ) {
                people.forEachIndexed { index, who ->
                    PersonChip(texts, who) { actions.lines.onRemove(line, index) }
                }
                BasicTextField(
                    value = ui.typed[line].orEmpty(),
                    onValueChange = { actions.lines.onTyped(line, it) },
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email, imeAction = ImeAction.Next),
                    keyboardActions = KeyboardActions(onNext = { actions.lines.onCommit(line) }),
                    modifier = Modifier
                        .widthIn(min = 120.dp)
                        .heightIn(min = 36.dp)
                        .padding(vertical = 6.dp)
                        .onFocusChanged { if (!it.isFocused) actions.lines.onCommit(line) }
                        .semantics { contentDescription = label },
                )
            }
            content()
        }
        if (ui.suggestFor == line) {
            ui.suggestions.forEach { who -> SuggestionRow(who) { actions.lines.onPick(line, who) } }
        }
    }
}

@Composable
private fun PersonChip(texts: Texts, who: uniffi.mailcraft_mobile.RecipientRecord, onRemove: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    val shape = RoundedCornerShape(16.dp)
    Row(
        modifier = Modifier
            .clip(shape)
            .background(if (who.valid) colors.secondaryContainer else colors.errorContainer)
            .heightIn(min = 32.dp)
            .clickable(onClickLabel = texts.t("Remove"), role = Role.Button, onClick = onRemove)
            .padding(start = 12.dp, end = 8.dp)
            .semantics {
                contentDescription = recipientLabel(who) + if (who.valid) "" else ", " + texts.t("not an address that can be sent to")
            },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Text(
            recipientLabel(who),
            style = MaterialTheme.typography.bodyMedium,
            color = if (who.valid) colors.onSecondaryContainer else colors.onErrorContainer,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.widthIn(max = 220.dp),
        )
        Icon(AppIcons.Close, contentDescription = null, modifier = Modifier.size(16.dp))
    }
}

@Composable
private fun SuggestionRow(who: SuggestionRecord, onPick: () -> Unit) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = FieldHeight)
            .clickable(role = Role.Button, onClick = onPick)
            .padding(horizontal = Gutter, vertical = 6.dp),
    ) {
        if (who.name.isNotBlank()) Text(who.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(who.address, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

@Composable
private fun SubjectRow(texts: Texts, subject: String, onSubject: (String) -> Unit) {
    Row(modifier = Modifier.fillMaxWidth().heightIn(min = FieldHeight).padding(horizontal = Gutter), verticalAlignment = Alignment.CenterVertically) {
        BasicTextField(
            value = subject,
            onValueChange = onSubject,
            textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
            cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next),
            modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp).semantics { contentDescription = texts.t("Subject") },
            decorationBox = { inner ->
                if (subject.isEmpty()) Text(texts.t("Subject"), style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                inner()
            },
        )
    }
}

@Composable
private fun FormatBar(texts: Texts, ui: ComposeUi, view: ComposeView, actions: ComposerActions, pickers: ComposerPickers) {
    var linking by remember { mutableStateOf(false) }
    val look = lookAt(view, ui.body.selection.min)
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(0.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        LookButton(texts.t("Bold"), look.bold, { actions.onFormat(ComposeFormat.Bold) }) { Text("B", fontWeight = FontWeight.Bold) }
        LookButton(texts.t("Italic"), look.italic, { actions.onFormat(ComposeFormat.Italic) }) { Text("I", fontStyle = FontStyle.Italic) }
        LookButton(texts.t("Underline"), look.underline, { actions.onFormat(ComposeFormat.Underline) }) {
            Text("U", textDecoration = TextDecoration.Underline)
        }
        LookButton(texts.t("Bulleted list"), false, { actions.onFormat(ComposeFormat.Bullets) }) { Text("•") }
        LookButton(texts.t("Numbered list"), false, { actions.onFormat(ComposeFormat.Numbering) }) { Text("1.") }
        LookButton(texts.t("Link"), look.link, { linking = true }) { Text("↗") }
        LookButton(texts.t("Insert a picture"), false, pickers.picture) { Icon(AppIcons.File, contentDescription = null, modifier = Modifier.size(20.dp)) }
    }
    HorizontalDivider()
    if (linking) {
        LinkDialog(texts, onDismiss = { linking = false }) { url ->
            linking = false
            actions.onFormat(ComposeFormat.Link(url))
        }
    }
}

@Composable
private fun LookButton(description: String, on: Boolean, onClick: () -> Unit, content: @Composable () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Box(
        modifier = Modifier
            .size(48.dp)
            .clip(RoundedCornerShape(24.dp))
            .background(if (on) colors.secondaryContainer else colors.surface)
            .clickable(onClickLabel = description, role = Role.Button, onClick = onClick)
            .semantics { contentDescription = description },
        contentAlignment = Alignment.Center,
    ) { content() }
}

@Composable
private fun LinkDialog(texts: Texts, onDismiss: () -> Unit, onApply: (String?) -> Unit) {
    var url by remember { mutableStateOf("https://") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(texts.t("Link")) },
        text = {
            OutlinedTextField(
                value = url,
                onValueChange = { url = it },
                singleLine = true,
                label = { Text(texts.t("Address")) },
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
            )
        },
        confirmButton = { TextButton(onClick = { onApply(url.trim()) }) { Text(texts.t("Apply")) } },
        dismissButton = {
            Row {
                TextButton(onClick = { onApply(null) }) { Text(texts.t("Remove link")) }
                TextButton(onClick = onDismiss) { Text(texts.t("Cancel")) }
            }
        },
    )
}

@Composable
private fun BodyField(texts: Texts, ui: ComposeUi, view: ComposeView, actions: ComposerActions) {
    val colors = MaterialTheme.colorScheme
    val styled = styledBody(ui.body.text, view, colors.primary, colors.onSurfaceVariant)
    val value = TextFieldValue(styled, ui.body.selection, ui.body.composition)
    val marks = remember(view) { ListMarks(markersOf(view)) }
    BasicTextField(
        value = value,
        onValueChange = { actions.onBody(it.copy(annotatedString = androidx.compose.ui.text.AnnotatedString(it.text))) },
        textStyle = MaterialTheme.typography.bodyLarge.copy(color = colors.onSurface),
        cursorBrush = SolidColor(colors.primary),
        visualTransformation = marks,
        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Default),
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 200.dp)
            .padding(Gutter)
            .semantics { contentDescription = texts.t("Message") },
    )
}

@Composable
private fun PicturesStrip(texts: Texts, view: ComposeView, ui: ComposeUi) {
    if (view.pictures.isEmpty()) return
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)) {
        Text(texts.tn(view.pictures.size.toULong(), "{n} picture in the message", "{n} pictures in the message"), style = MaterialTheme.typography.labelLarge)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 4.dp)) {
            view.pictures.forEach { picture ->
                val decoded = rememberDecoded(ui.pictures[picture.id])
                Box(modifier = Modifier.size(72.dp).clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceVariant), contentAlignment = Alignment.Center) {
                    if (decoded != null) {
                        Image(decoded.bitmap, contentDescription = picture.alt.ifBlank { picture.name }, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
                    }
                }
            }
        }
    }
}

@Composable
private fun FilesList(texts: Texts, view: ComposeView, actions: ComposerActions) {
    if (view.files.isEmpty()) return
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp)) {
        Text(
            texts.tn(view.files.size.toULong(), "{n} attachment", "{n} attachments"),
            style = MaterialTheme.typography.labelLarge,
            modifier = Modifier.semantics { heading() },
        )
        view.files.forEach { file ->
            Row(
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Icon(AppIcons.File, contentDescription = null, modifier = Modifier.size(24.dp))
                Column(modifier = Modifier.weight(1f)) {
                    Text(file.name, style = MaterialTheme.typography.bodyLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(sizeLabel(file.size.toLong()), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                IconButton(onClick = { actions.files.onRemove(file.index) }) {
                    Icon(AppIcons.Close, contentDescription = texts.t("Remove {name}").replace("{name}", file.name))
                }
            }
        }
    }
}

/** The original a reply or forward quotes below what is written: a few lines, and all of it on a tap. */
@Composable
private fun QuoteBlock(texts: Texts, view: ComposeView) {
    if (view.quote.isBlank()) return
    var all by remember { mutableStateOf(false) }
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(horizontal = Gutter, vertical = 8.dp)
            .clip(RoundedCornerShape(8.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .clickable(onClickLabel = if (all) texts.t("Show less") else texts.t("Show the whole original"), role = Role.Button) { all = !all }
            .padding(12.dp),
    ) {
        Text(texts.t("Original message"), style = MaterialTheme.typography.labelLarge)
        Text(
            view.quote.trim(),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = if (all) Int.MAX_VALUE else QUOTE_LINES,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

private const val QUOTE_LINES = 4
