// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.TaskDetail

/**
 * What the editor holds. A draft of a task that exists ([id]) remembers [base], the task as it
 * was read, so that saving sends only what the owner changed: a note too long to be handed over
 * whole is never written back, and what the owner did not touch stays as the server has it.
 */
internal data class TaskDraft(
    val id: ULong?,
    val title: String,
    val notes: String,
    val due: String?,
    val important: Boolean,
    val account: ULong,
    val completed: Boolean = false,
    val notesCut: Boolean = false,
    val base: TaskDraft? = null,
) {
    val canSave: Boolean get() = title.isNotBlank()

    /** The parameters of `tasks.new` or `tasks.edit`, as JSON. */
    fun params(): String {
        val fields = ArrayList<String>()
        val before = base
        if (id != null) fields.add("\"id\":$id") else fields.add("\"account\":$account")
        val changed = { mine: Any?, theirs: Any? -> before == null || mine != theirs }
        if (changed(title, before?.title)) fields.add("\"title\":" + jsonString(title.trim().take(MAX_TITLE)))
        if (!notesCut && changed(notes, before?.notes)) fields.add("\"notes\":" + jsonString(notes.take(MAX_NOTES)))
        if (changed(due, before?.due)) fields.add("\"due\":" + (due?.let(::jsonString) ?: "null"))
        if (changed(important, before?.important)) {
            fields.add("\"importance\":" + jsonString(if (important) "high" else "normal"))
        }
        return fields.joinToString(prefix = "{", postfix = "}", separator = ",")
    }

    /** True when saving would change nothing. */
    val unchanged: Boolean
        get() = base != null && title == base.title && notes == base.notes && due == base.due &&
            important == base.important
}

/** [text] as a JSON string: quoted, with what JSON needs escaped. */
internal fun jsonString(text: String): String {
    val out = StringBuilder("\"")
    for (c in text) {
        when {
            c == '"' -> out.append("\\\"")
            c == '\\' -> out.append("\\\\")
            c == '\n' -> out.append("\\n")
            c == '\r' -> out.append("\\r")
            c == '\t' -> out.append("\\t")
            c.code < SPACE || c == LINE_SEPARATOR || c == PARAGRAPH_SEPARATOR ->
                out.append("\\u").append(c.code.toString(HEX).padStart(HEX_DIGITS, '0'))
            else -> out.append(c)
        }
    }
    return out.append('"').toString()
}

private const val SPACE = 0x20
private const val LINE_SEPARATOR = '\u2028'
private const val PARAGRAPH_SEPARATOR = '\u2029'
private const val HEX = 16
private const val HEX_DIGITS = 4

internal fun draftOf(detail: TaskDetail): TaskDraft {
    val task = detail.task
    val base = TaskDraft(
        id = task.id,
        title = task.title,
        notes = detail.notes,
        due = task.due,
        important = task.important,
        account = task.account,
        completed = task.completed,
        notesCut = detail.notesCut,
    )
    return base.copy(base = base)
}

internal fun newDraft(account: ULong, due: String? = null): TaskDraft =
    TaskDraft(id = null, title = "", notes = "", due = due, important = false, account = account)

internal sealed interface DraftState {
    data object Loading : DraftState

    class Ready(val draft: TaskDraft) : DraftState

    class Failed(val message: String) : DraftState
}

internal suspend fun loadDraft(
    core: Core,
    id: ULong,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): DraftState = withContext(dispatcher) {
    try {
        DraftState.Ready(draftOf(core.task(id)))
    } catch (_: CoreException) {
        DraftState.Failed(core.t("This task could not be read."))
    }
}

