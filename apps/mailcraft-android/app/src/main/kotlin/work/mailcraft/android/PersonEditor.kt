// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp

private val Pad = 16.dp
private val Gap = 12.dp
private val Small = 8.dp
private const val MAX_LIST = 3_000

/** The form for a new person. It only collects: saving is `people.new`, run by the app. */
@Composable
internal fun PersonEditor(texts: Texts, draft: PersonDraft, busy: Boolean, actions: FormActions) {
    Column(
        modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(Pad),
        verticalArrangement = Arrangement.spacedBy(Gap),
    ) {
        Text(texts.t("New person"), style = MaterialTheme.typography.headlineSmall)
        Text(
            texts.t("Kept on this phone only: it is not sent to your account."),
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Field(texts.t("First name"), draft.first, MAX_PERSON_LINE) { actions.onDraft(draft.copy(first = it)) }
        Field(texts.t("Last name"), draft.last, MAX_PERSON_LINE) { actions.onDraft(draft.copy(last = it)) }
        Field(
            texts.t("Email addresses, one per line"),
            draft.emails,
            MAX_LIST,
            KeyboardType.Email,
            minLines = 2,
            error = when {
                draft.badEmail -> texts.t("One of these is not an email address.")
                draft.emailList.size > MAX_ENTRIES -> texts.t("At most 20 entries in each list.")
                else -> null
            },
        ) { actions.onDraft(draft.copy(emails = it)) }
        Field(texts.t("Phone numbers, one per line"), draft.phones, MAX_LIST, KeyboardType.Phone, minLines = 2,
            error = if (draft.phoneList.size > MAX_ENTRIES) texts.t("At most 20 entries in each list.") else null,
        ) {
            actions.onDraft(draft.copy(phones = it))
        }
        Field(texts.t("Company"), draft.company, MAX_PERSON_LINE) { actions.onDraft(draft.copy(company = it)) }
        Field(texts.t("Job title"), draft.title, MAX_PERSON_LINE) { actions.onDraft(draft.copy(title = it)) }
        Field(texts.t("Notes"), draft.notes, MAX_NOTES, minLines = 3) { actions.onDraft(draft.copy(notes = it)) }
        Row(horizontalArrangement = Arrangement.spacedBy(Small), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = actions.onSave, enabled = draft.canSave && !busy) { Text(texts.t("Save")) }
            TextButton(onClick = actions.onCancel) { Text(texts.t("Cancel")) }
        }
    }
}

@Composable
private fun Field(
    label: String,
    value: String,
    max: Int,
    keyboard: KeyboardType = KeyboardType.Text,
    minLines: Int = 1,
    error: String? = null,
    onChange: (String) -> Unit,
) {
    OutlinedTextField(
        value = value,
        onValueChange = { onChange(it.takeScalars(max)) },
        label = { Text(label) },
        isError = error != null,
        supportingText = error?.let { { Text(it) } },
        minLines = minLines,
        singleLine = minLines == 1,
        keyboardOptions = KeyboardOptions(keyboardType = keyboard),
        modifier = Modifier.fillMaxWidth(),
    )
}
