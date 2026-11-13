// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.TaskRecord

/** One task on the list. */
internal data class TaskRow(
    val id: ULong,
    val title: String,
    /** `YYYY-MM-DD`, or null for a task with no due date. */
    val due: String?,
    val completed: Boolean,
    val important: Boolean,
)

/** The tasks of one list, under the account whose To Do keeps it (account 0 is this phone). */
internal data class TaskGroup(
    val account: ULong,
    val list: String,
    val title: String,
    val open: Int,
    val total: Int,
    val rows: List<TaskRow>,
) {
    /** The key the group is known by, to ask for more of it. */
    val key: String get() = groupKey(account, list)

    /** Everything on the list is in [rows]. */
    val complete: Boolean get() = rows.size >= total
}

internal fun groupKey(account: ULong, list: String): String = "$account/$list"

/** A place a task can be kept: an account whose To Do is kept in step, or this phone. */
internal data class TaskPlace(val account: ULong, val label: String)

/**
 * The tasks that exist only on this phone, and who they could go to. Nothing is sent until the
 * owner says yes; [answered] is that the owner was asked and said "Not now".
 */
internal data class LocalOffer(val count: Int, val accounts: List<TaskPlace>, val answered: Boolean)

internal data class TasksModel(
    /** The owner's today, `YYYY-MM-DD`. */
    val today: String,
    val groups: List<TaskGroup>,
    /** This phone first, then each account that keeps tasks. */
    val places: List<TaskPlace>,
    val offer: LocalOffer?,
    val showCompleted: Boolean,
)

internal sealed interface TasksState {
    data object Loading : TasksState

    class Ready(val model: TasksModel) : TasksState

    class Failed(val message: String) : TasksState
}

/** The most tasks of one list that are read at a time. */
internal const val TASK_WINDOW = 200u

/** The longest title and the longest note the editor lets through (the core cuts at these too). */
internal const val MAX_TITLE = 500
internal const val MAX_NOTES = 8_000

internal fun taskRow(record: TaskRecord): TaskRow =
    TaskRow(record.id, record.title, record.due, record.completed, record.important)

/** Whether a task with this due date is late: due before [today] and not done. */
internal fun isOverdue(row: TaskRow, today: String): Boolean = !row.completed && row.due != null && row.due < today

/** The title of a group: the account (or this phone), and which of its lists when it has several. */
internal fun groupTitle(texts: Texts, place: String, index: Int, lists: Int): String =
    if (lists > 1) "$place · " + texts.t("List {n}").replace("{n}", (index + 1).toString()) else place

private fun label(account: ULong, name: String, email: String): String =
    name.ifBlank { email }.ifBlank { account.toString() }

/** The most tasks of one list the phone holds in its list at once. */
internal const val MAX_DEPTH = 5_000u

private class TaskRead(val tasks: List<TaskRecord>, val total: UInt)

/** The first [want] tasks of a list, a window at a time. */
private suspend fun readTasks(core: Core, account: ULong, list: String, completed: Boolean, want: UInt): TaskRead {
    val out = ArrayList<TaskRecord>()
    var total = 0u
    var more = true
    while (more && out.size.toUInt() < want) {
        val window = core.tasks(account, list, completed, out.size.toUInt(), TASK_WINDOW)
        total = window.total
        out.addAll(window.tasks)
        more = window.tasks.isNotEmpty() && out.size.toUInt() < total
    }
    return TaskRead(out, total)
}

/** Reads the To Do lists and their tasks, grouped by account and list. */
@Suppress("LongMethod")
internal suspend fun loadTasks(
    core: Core,
    showCompleted: Boolean,
    /** How many tasks to read of a list, by [groupKey]; a list not named is read a window deep. */
    depth: Map<String, UInt> = emptyMap(),
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): TasksState = withContext(dispatcher) {
    try {
        val names = core.folders(0u).accounts.associate {
            it.id to label(it.id, it.displayName.ifBlank { it.name }, it.email)
        }
        val setup = core.taskSetup()
        val phone = TaskPlace(0u, core.t("This phone"))
        val accounts = setup.accounts.map { TaskPlace(it.id, names[it.id] ?: label(it.id, it.name, it.email)) }
        val lists = core.taskLists().lists
        val groups = ArrayList<TaskGroup>()
        for (list in lists) {
            val perAccount = lists.filter { it.account == list.account }
            val place = if (list.account == 0uL) phone.label else names[list.account].orEmpty()
            val want = (depth[groupKey(list.account, list.list)] ?: TASK_WINDOW).coerceIn(TASK_WINDOW, MAX_DEPTH)
            val window = readTasks(core, list.account, list.list, showCompleted, want)
            if (window.tasks.isEmpty()) continue
            groups.add(
                TaskGroup(
                    account = list.account,
                    list = list.list,
                    title = groupTitle(CoreTexts(core), place, perAccount.indexOf(list), perAccount.size),
                    open = list.open.toInt(),
                    total = window.total.toInt(),
                    rows = window.tasks.map(::taskRow),
                ),
            )
        }
        val waiting = setup.localWaiting.toInt()
        val offer = if (waiting > 0 && accounts.isNotEmpty()) LocalOffer(waiting, accounts, setup.answered) else null
        TasksState.Ready(TasksModel(core.today().date, groups, listOf(phone) + accounts, offer, showCompleted))
    } catch (_: CoreException) {
        TasksState.Failed(core.t("The tasks could not be read."))
    }
}

/** Runs one of the task commands; false when the core refused it (nothing changed then). */
internal suspend fun runTaskCommand(
    core: Core,
    command: String,
    params: String,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): Boolean = withContext(dispatcher) {
    try {
        core.execute(command, params)
        true
    } catch (_: CoreException) {
        false
    }
}
