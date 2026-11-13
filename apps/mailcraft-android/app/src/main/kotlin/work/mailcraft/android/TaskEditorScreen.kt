// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp

private val Gutter = 16.dp
private val RowHeight = 56.dp
private val Small = 8.dp
private val Medium = 12.dp

@Composable
internal fun EditorPane(texts: Texts, data: TasksData, actions: EditorActions, showBack: Boolean, modifier: Modifier) {
    val model = (data.state as? TasksState.Ready)?.model
    Column(modifier = modifier) {
        if (showBack) {
            IconButton(onClick = actions.onClose) { Icon(AppIcons.Back, contentDescription = texts.t("Back")) }
        }
        when (val editor = data.editor) {
            null -> Centered(texts.t("Select a task"))
            DraftState.Loading -> Centered(texts.t("Loading…"))
            is DraftState.Failed -> Centered(editor.message)
            is DraftState.Ready -> TaskEditor(texts, editor.draft, model, actions)
        }
    }
}

@Composable
private fun TaskEditor(texts: Texts, draft: TaskDraft, model: TasksModel?, actions: EditorActions) {
    val today = model?.today.orEmpty()
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(Gutter),
        verticalArrangement = Arrangement.spacedBy(Medium),
    ) {
        OutlinedTextField(
            value = draft.title,
            onValueChange = { actions.onDraft(draft.copy(title = it.take(MAX_TITLE))) },
            label = { Text(texts.t("Title")) },
            modifier = Modifier.fillMaxWidth(),
        )
        NotesField(texts, draft, actions.onDraft)
        DueChoices(texts, draft.due, today) { actions.onDraft(draft.copy(due = it)) }
        SwitchRow(texts.t("Important"), draft.important) { actions.onDraft(draft.copy(important = it)) }
        PlaceChoice(texts, draft, model, actions.onDraft)
        EditorButtons(texts, draft, actions)
    }
}

@Composable
private fun NotesField(texts: Texts, draft: TaskDraft, onDraft: (TaskDraft) -> Unit) {
    if (draft.notesCut) {
        Text(texts.t("The notes are too long to edit here."), color = MaterialTheme.colorScheme.onSurfaceVariant)
        return
    }
    OutlinedTextField(
        value = draft.notes,
        onValueChange = { onDraft(draft.copy(notes = it.take(MAX_NOTES))) },
        label = { Text(texts.t("Notes")) },
        minLines = 3,
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun DueChoices(texts: Texts, due: String?, today: String, onDue: (String?) -> Unit) {
    var picking by remember { mutableStateOf(false) }
    Text(texts.t("Due"), style = MaterialTheme.typography.titleSmall, modifier = Modifier.semantics { heading() })
    val quick = quickDates(today)
    FlowRow(horizontalArrangement = Arrangement.spacedBy(Small)) {
        quick.forEach { (key, date) ->
            val name = when (key) {
                "today" -> texts.t("Today")
                "tomorrow" -> texts.t("Tomorrow")
                else -> texts.t("Next Monday")
            }
            FilterChip(selected = due == date, onClick = { onDue(date) }, label = { Text(name) })
        }
        FilterChip(selected = due == null, onClick = { onDue(null) }, label = { Text(texts.t("No date")) })
        val custom = due != null && quick.none { it.second == due }
        FilterChip(
            selected = custom,
            onClick = { picking = true },
            label = { Text(due?.takeIf { custom }?.let { dueLabel(texts, it, today) } ?: texts.t("Pick a date")) },
        )
    }
    if (picking) DatePickChoice(texts, due) { picked ->
        picking = false
        if (picked != null) onDue(picked)
    }
}

@Composable
private fun DatePickChoice(texts: Texts, due: String?, onDone: (String?) -> Unit) {
    val state = rememberDatePickerState(initialSelectedDateMillis = dateToMillis(due))
    DatePickerDialog(
        onDismissRequest = { onDone(null) },
        confirmButton = {
            TextButton(onClick = { onDone(state.selectedDateMillis?.let(::millisToDate)) }) { Text(texts.t("OK")) }
        },
        dismissButton = { TextButton(onClick = { onDone(null) }) { Text(texts.t("Cancel")) } },
    ) { DatePicker(state = state) }
}

@Composable
private fun SwitchRow(label: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = RowHeight)
            .selectable(selected = on, role = Role.Switch) { onChange(!on) },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = on, onCheckedChange = null)
    }
}

@Composable
private fun PlaceChoice(texts: Texts, draft: TaskDraft, model: TasksModel?, onDraft: (TaskDraft) -> Unit) {
    val places = model?.places.orEmpty()
    Text(texts.t("Kept in"), style = MaterialTheme.typography.titleSmall, modifier = Modifier.semantics { heading() })
    if (draft.id != null) {
        // A task stays where it is: the account of a task is not changed.
        Text(places.firstOrNull { it.account == draft.account }?.label ?: texts.t("This phone"))
        return
    }
    places.forEach { place ->
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = RowHeight)
                .selectable(selected = draft.account == place.account, role = Role.RadioButton) {
                    onDraft(draft.copy(account = place.account))
                },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RadioButton(selected = draft.account == place.account, onClick = null)
            Text(place.label, modifier = Modifier.padding(start = 12.dp))
        }
    }
}

@Composable
private fun EditorButtons(texts: Texts, draft: TaskDraft, actions: EditorActions) {
    var confirming by remember { mutableStateOf(false) }
    Row(horizontalArrangement = Arrangement.spacedBy(Small), verticalAlignment = Alignment.CenterVertically) {
        Button(onClick = actions.onSave, enabled = draft.canSave && !draft.unchanged) { Text(texts.t("Save")) }
        TextButton(onClick = actions.onClose) { Text(texts.t("Cancel")) }
        if (draft.id != null) {
            TextButton(onClick = { confirming = true }) {
                Text(texts.t("Delete"), color = MaterialTheme.colorScheme.error)
            }
        }
    }
    if (confirming) {
        AlertDialog(
            onDismissRequest = { confirming = false },
            title = { Text(texts.t("Delete this task?")) },
            confirmButton = {
                TextButton(
                    onClick = {
                        confirming = false
                        actions.onDelete()
                    },
                ) { Text(texts.t("Delete")) }
            },
            dismissButton = { TextButton(onClick = { confirming = false }) { Text(texts.t("Cancel")) } },
        )
    }
}
