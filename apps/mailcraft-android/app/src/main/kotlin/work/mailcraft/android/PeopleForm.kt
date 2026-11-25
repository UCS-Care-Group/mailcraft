// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.runtime.MutableState
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

/**
 * What the new-person form's buttons do. While a person is on its way to the core the form stays as
 * it is; when it is saved the form closes only if it still shows what was sent. [add] sends it
 * (`people.new`, see [addPerson]); [done] gets the new person's id, or null when the core refused it
 * (the form is then kept, so nothing typed is lost).
 */
internal fun formActions(
    scope: CoroutineScope,
    editor: MutableState<PersonDraft?>,
    busy: MutableState<Boolean>,
    add: suspend (PersonDraft) -> ULong?,
    done: (ULong?) -> Unit,
): FormActions = FormActions(
    onNew = { if (!busy.value) editor.value = PersonDraft() },
    onDraft = { if (!busy.value) editor.value = it },
    onCancel = { if (!busy.value) editor.value = null },
    onSave = {
        val draft = editor.value
        if (draft != null && draft.canSave && !busy.value) {
            busy.value = true
            scope.launch {
                try {
                    val made = add(draft)
                    if (made != null && editor.value === draft) editor.value = null
                    done(made)
                } finally {
                    busy.value = false
                }
            }
        }
    },
)
