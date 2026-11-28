// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val RowHeight = 56.dp
private val FabClearance = 96.dp
private val ListPane = 360.dp

/** What the People tab shows: the search, the people found so far, and the person opened. */
internal class PeopleData(
    val query: String,
    val state: PeopleState,
    val open: ULong? = null,
    val person: PersonState? = null,
    /** A line about the last thing the owner asked for that could not be done. */
    val notice: String? = null,
    /** The new-person form, while it is open. */
    val editor: PersonDraft? = null,
    /** A new person on its way to the core. */
    val busy: Boolean = false,
    /** Where a new person can go; null until the core has said. */
    val places: PersonPlaces? = null,
    /** Move to contacts folder, while it is open over the person's page. */
    val move: MoveState? = null,
)

/** What the People tab asks of the app. */
internal class PeopleActions(
    val onQuery: (String) -> Unit = {},
    val onMore: () -> Unit = {},
    val onOpen: (ULong) -> Unit = {},
    val onClose: () -> Unit = {},
    val onMail: (String) -> Unit = {},
    val onCall: (String) -> Unit = {},
    val form: FormActions = FormActions(),
    val move: MoveActions = MoveActions(),
)

/** What the new-person form asks of the app. */
internal class FormActions(
    val onNew: () -> Unit = {},
    val onDraft: (PersonDraft) -> Unit = {},
    val onSave: () -> Unit = {},
    val onCancel: () -> Unit = {},
)

/**
 * People, with a form to add a person (a person already there is read, not edited). A phone shows the
 * list, or the person opened in its place; a wide window (a tablet) shows both side by side.
 */
@Composable
internal fun PeopleScreen(texts: Texts, data: PeopleData, actions: PeopleActions, modifier: Modifier = Modifier) {
    BackHandler(enabled = data.editor != null || data.open != null) {
        when {
            data.editor != null -> actions.form.onCancel()
            data.move != null -> actions.move.onCancel()
            else -> actions.onClose()
        }
    }
    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        if (maxWidth >= TwoPaneWidth) {
            Row(modifier = Modifier.fillMaxSize()) {
                PeopleList(texts, data, actions, Modifier.width(ListPane))
                VerticalRule()
                PersonPane(texts, data, actions, showBack = false, Modifier.weight(1f))
            }
        } else if (data.editor != null) {
            PersonPane(texts, data, actions, showBack = true, Modifier.fillMaxSize())
        } else if (data.open != null) {
            PersonPane(texts, data, actions, showBack = true, Modifier.fillMaxSize())
        } else {
            PeopleList(texts, data, actions, Modifier.fillMaxSize())
        }
    }
}

@Composable
private fun PeopleList(texts: Texts, data: PeopleData, actions: PeopleActions, modifier: Modifier) {
    Box(modifier = modifier) {
        PeopleColumn(texts, data, actions)
        ExtendedFloatingActionButton(
            onClick = actions.form.onNew,
            icon = { Icon(AppIcons.Add, contentDescription = null) },
            text = { Text(texts.t("New person")) },
            modifier = Modifier.align(Alignment.BottomEnd).padding(Gutter),
        )
    }
}

@Composable
private fun PeopleColumn(texts: Texts, data: PeopleData, actions: PeopleActions) {
    Column(modifier = Modifier.fillMaxSize()) {
        OutlinedTextField(
            value = data.query,
            onValueChange = actions.onQuery,
            label = { Text(texts.t("Search people")) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp),
        )
        when (val state = data.state) {
            PeopleState.Loading -> Centered(texts.t("Loading…"))
            is PeopleState.Failed -> Centered(state.message)
            is PeopleState.Ready -> PeopleRows(texts, state.list, data.open, actions)
        }
    }
}

@Composable
private fun PeopleRows(texts: Texts, list: PeopleList, open: ULong?, actions: PeopleActions) {
    if (list.rows.isEmpty()) {
        Centered(texts.t("No people found."))
        return
    }
    val items = peopleItems(list.rows)
    LazyColumn(contentPadding = PaddingValues(bottom = FabClearance)) {
        items(items, key = { it.key }) { item ->
            when (item) {
                is PeopleItem.Heading -> Heading(item.letter)
                is PeopleItem.Person -> PersonRowView(item.row, item.row.id == open) { actions.onOpen(item.row.id) }
            }
        }
        if (!list.complete) {
            item(key = "more") { LaunchedEffect(list.rows.size) { actions.onMore() } }
        }
    }
}

@Composable
private fun Heading(letter: String) {
    Text(
        text = letter,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp).semantics { heading() },
    )
}

@Composable
private fun PersonRowView(row: PersonRow, selected: Boolean, onClick: () -> Unit) {
    val background = if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .background(background)
            .clickable(role = Role.Button, onClick = onClick)
            .padding(horizontal = Gutter, vertical = 8.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Text(row.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (row.detail.isNotBlank()) {
            Text(
                row.detail,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
    }
    HorizontalDivider()
}

@Composable
private fun PersonPane(texts: Texts, data: PeopleData, actions: PeopleActions, showBack: Boolean, modifier: Modifier) {
    Column(modifier = modifier) {
        if (showBack) {
            val back = when {
                data.editor != null -> actions.form.onCancel
                data.move != null -> actions.move.onCancel
                else -> actions.onClose
            }
            IconButton(onClick = back) { Icon(AppIcons.Back, contentDescription = texts.t("Back")) }
        }
        data.notice?.let {
            Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = Gutter))
        }
        val editor = data.editor
        if (editor != null) {
            PersonEditor(texts, editor, data.places, data.busy, actions.form)
            return@Column
        }
        val move = data.move
        if (move != null) {
            MovePage(texts, move, actions.move)
            return@Column
        }
        when (val person = data.person) {
            null -> if (data.open == null) Centered(texts.t("Select a person"))
            PersonState.Loading -> Centered(texts.t("Loading…"))
            is PersonState.Failed -> Centered(person.message)
            is PersonState.Ready -> PersonPage(texts, person.card, actions)
        }
    }
}

@Composable
private fun PersonPage(texts: Texts, card: PersonCard, actions: PeopleActions) {
    Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = Gutter)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                card.name,
                style = MaterialTheme.typography.headlineSmall,
                modifier = Modifier.weight(1f).padding(horizontal = Gutter),
            )
            // A folder the account may only read keeps its people where they are.
            if (!card.readOnly) PersonMenu(texts, actions.move)
        }
        if (card.subtitle.isNotBlank()) {
            Text(
                card.subtitle,
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = Gutter),
            )
        }
        KeptIn(texts, card, Gutter)
        val mark = texts.t("On this phone only")
        val emails = LocalOnlyText(mark, localOnlyWhy(texts, LocalOnlyKind.Email))
        val phones = LocalOnlyText(mark, localOnlyWhy(texts, LocalOnlyKind.Phone))
        val addresses = LocalOnlyText(mark, localOnlyWhy(texts, LocalOnlyKind.Address))
        LinesSection(texts.t("Email"), card.emails, emails, texts.t("Write"), actions.onMail) { mailtoLink(it) != null }
        LinesSection(texts.t("Phone"), card.phones, phones, texts.t("Call"), actions.onCall) { dialLink(it) != null }
        LinesSection(texts.t("Address"), card.addresses, addresses, null, {}) { false }
    }
}

@Composable
private fun LinesSection(
    title: String,
    lines: List<ContactLine>,
    local: LocalOnlyText,
    action: String?,
    onAct: (String) -> Unit,
    canAct: (String) -> Boolean,
) {
    if (lines.isEmpty()) return
    Text(
        title,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(start = Gutter, end = Gutter, top = Gutter).semantics { heading() },
    )
    lines.forEach { line ->
        val acts = action != null && canAct(line.value)
        val act = if (acts) {
            Modifier.clickable(onClickLabel = action, role = Role.Button) { onAct(line.value) }
        } else {
            Modifier
        }
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = RowHeight)
                .then(act)
                .padding(horizontal = Gutter, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(line.value, style = MaterialTheme.typography.bodyLarge)
                if (line.label.isNotBlank()) {
                    Text(
                        line.label,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (line.localOnly) {
                    Text(
                        local.mark,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.tertiary,
                    )
                }
            }
            if (acts) {
                val style = MaterialTheme.typography.labelLarge
                Text(action.orEmpty(), color = MaterialTheme.colorScheme.primary, style = style)
            }
        }
        HorizontalDivider()
    }
    if (lines.any { it.localOnly }) {
        Text(
            local.why,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(start = Gutter, end = Gutter, top = 4.dp),
        )
    }
}

/** A thin line between two panes. */
@Composable
internal fun VerticalRule() {
    Box(modifier = Modifier.fillMaxHeight().width(1.dp).background(MaterialTheme.colorScheme.outlineVariant))
}

/** From this width a tab shows its list and the page opened side by side. */
internal val TwoPaneWidth = 600.dp
