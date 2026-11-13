// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.selection.selectable
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val RowHeight = 56.dp
private val ListPane = 380.dp
private val Small = 8.dp

/** What the Tasks tab shows: the lists, the task being edited (if any), and a note on a failure. */
internal class TasksData(
    val state: TasksState,
    val editor: DraftState? = null,
    val notice: String? = null,
)

/** What the Tasks tab asks of the app. */
@Suppress("LongParameterList")
internal class TasksActions(
    val onToggle: (TaskRow) -> Unit = {},
    val onOpen: (ULong) -> Unit = {},
    val onNew: (ULong) -> Unit = {},
    val onShowCompleted: (Boolean) -> Unit = {},
    /** Read the next window of one list. */
    val onMore: (TaskGroup) -> Unit = {},
    /** The owner's answer to "send the tasks that are only here to this account?"; null is "Not now". */
    val onSendLocal: (ULong?) -> Unit = {},
    val editor: EditorActions = EditorActions(),
)

/** What the editor asks of the app. */
internal class EditorActions(
    val onDraft: (TaskDraft) -> Unit = {},
    val onSave: () -> Unit = {},
    val onDelete: () -> Unit = {},
    val onClose: () -> Unit = {},
)

/**
 * Tasks: the lists, grouped by account, and an editor. A phone shows the lists, or the editor in
 * their place; a wide window shows both.
 */
@Composable
internal fun TasksScreen(texts: Texts, data: TasksData, actions: TasksActions, modifier: Modifier = Modifier) {
    BackHandler(enabled = data.editor != null) { actions.editor.onClose() }
    BoxWithConstraints(modifier = modifier.fillMaxSize()) {
        val wide = maxWidth >= TwoPaneWidth
        when {
            wide -> Row(modifier = Modifier.fillMaxSize()) {
                TaskLists(texts, data, actions, Modifier.width(ListPane))
                VerticalRule()
                EditorPane(texts, data, actions.editor, showBack = false, Modifier.weight(1f))
            }
            data.editor != null -> EditorPane(texts, data, actions.editor, showBack = true, Modifier.fillMaxSize())
            else -> TaskLists(texts, data, actions, Modifier.fillMaxSize())
        }
    }
}

@Composable
private fun TaskLists(texts: Texts, data: TasksData, actions: TasksActions, modifier: Modifier) {
    Box(modifier = modifier) {
        when (val state = data.state) {
            TasksState.Loading -> Centered(texts.t("Loading…"))
            is TasksState.Failed -> Centered(state.message)
            is TasksState.Ready -> {
                TaskGroups(texts, state.model, data.notice, actions)
                ExtendedFloatingActionButton(
                    onClick = { actions.onNew(state.model.places.firstOrNull()?.account ?: 0u) },
                    icon = { Icon(AppIcons.Add, contentDescription = null) },
                    text = { Text(texts.t("New task")) },
                    modifier = Modifier.align(Alignment.BottomEnd).padding(Gutter),
                )
            }
        }
    }
}

@Composable
private fun TaskGroups(texts: Texts, model: TasksModel, notice: String?, actions: TasksActions) {
    LazyColumn(contentPadding = PaddingValues(bottom = FabClearance)) {
        if (notice != null) item(key = "notice") { Notice(notice) }
        model.offer?.let { offer -> item(key = "offer") { OfferCard(texts, offer, actions.onSendLocal) } }
        item(key = "completed") { ShowCompleted(texts, model.showCompleted, actions.onShowCompleted) }
        if (model.groups.isEmpty()) {
            item(key = "empty") { Centered(texts.t("No tasks.")) }
        }
        model.groups.forEach { group ->
            item(key = "g" + group.account + "/" + group.list) { GroupHeader(texts, group) }
            items(group.rows, key = { "t" + it.id }) { row ->
                TaskRowView(texts, row, model.today, actions)
            }
            if (!group.complete) {
                item(key = "m" + group.key) {
                    TextButton(onClick = { actions.onMore(group) }, modifier = Modifier.padding(horizontal = Gutter)) {
                        Text(texts.t("Show more tasks"))
                    }
                }
            }
        }
    }
}

private val FabClearance = 96.dp

@Composable
private fun Notice(text: String) {
    Text(
        text,
        color = MaterialTheme.colorScheme.error,
        modifier = Modifier.fillMaxWidth().padding(Gutter),
    )
}

@Composable
private fun ShowCompleted(texts: Texts, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .selectable(selected = on, role = Role.Switch) { onChange(!on) }
            .padding(horizontal = Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(texts.t("Show completed tasks"), modifier = Modifier.weight(1f))
        Switch(checked = on, onCheckedChange = null)
    }
}

@Composable
private fun GroupHeader(texts: Texts, group: TaskGroup) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp).semantics { heading() },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            group.title,
            style = MaterialTheme.typography.titleSmall,
            color = MaterialTheme.colorScheme.primary,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        Text(
            texts.tn(group.open.toULong(), "{n} open", "{n} open"),
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun TaskRowView(texts: Texts, row: TaskRow, today: String, actions: TasksActions) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .clickable(onClickLabel = texts.t("Edit task"), role = Role.Button) { actions.onOpen(row.id) }
            .padding(end = Gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Checkbox(checked = row.completed, onCheckedChange = { actions.onToggle(row) })
        Column(modifier = Modifier.weight(1f).padding(vertical = 8.dp)) {
            Text(row.title, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
            row.due?.let { due ->
                val late = isOverdue(row, today)
                Text(
                    (if (late) texts.t("Overdue") + ", " else "") + dueLabel(texts, due, today),
                    style = MaterialTheme.typography.bodySmall,
                    color = if (late) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        if (row.important) Text(texts.t("Important"), style = MaterialTheme.typography.labelSmall)
    }
    HorizontalDivider()
}

@Composable
private fun OfferCard(texts: Texts, offer: LocalOffer, onAnswer: (ULong?) -> Unit) {
    Card(modifier = Modifier.fillMaxWidth().padding(Gutter)) {
        Column(modifier = Modifier.padding(Gutter), verticalArrangement = Arrangement.spacedBy(Small)) {
            val count = offer.count.toULong()
            Text(
                if (offer.answered) {
                    texts.tn(count, "{n} task is only on this phone.", "{n} tasks are only on this phone.")
                } else {
                    texts.tn(
                        count,
                        "{n} task is only on this phone. Send it to To Do at the next sync?",
                        "{n} tasks are only on this phone. Send them to To Do at the next sync?",
                    )
                },
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                texts.t("Nothing is sent until you say so, and then it goes at the next sync."),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            offer.accounts.forEach { place ->
                TextButton(onClick = { onAnswer(place.account) }) {
                    Text(texts.t("Send to {account}").replace("{account}", place.label), maxLines = 2)
                }
            }
            if (!offer.answered) TextButton(onClick = { onAnswer(null) }) { Text(texts.t("Not now")) }
        }
    }
}

