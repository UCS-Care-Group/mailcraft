// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableIntState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.core.net.toUri
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core

/** What the Today, People and Tasks tabs show, and what they do. */
internal class Hosted(
    val today: TodayState,
    val people: PeopleData,
    val tasks: TasksData,
    val peopleActions: PeopleActions,
    val tasksActions: TasksActions,
)

/** Opens [link] (a `mailto:` or `tel:` built by [mailtoLink] or [dialLink]) in the program that handles it. */
private fun launch(context: Context, action: String, link: String?): Boolean {
    if (link == null) return false
    return try {
        context.startActivity(Intent(action, link.toUri()).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        true
    } catch (_: ActivityNotFoundException) {
        false
    }
}

/**
 * The state of the three tabs over [core], once [ready]. Contacts are read a window at a time; a
 * change to a task is a command (`tasks.new`, `tasks.edit`, `tasks.complete`, `tasks.delete`) and
 * the lists are read again after it. Nothing is sent to a server from here.
 */
@Composable
internal fun rememberHosted(core: Core, texts: Texts, ready: Boolean): Hosted {
    val context = LocalContext.current
    val tick = remember { mutableIntStateOf(0) }
    val people = rememberPeople(core, texts, ready, context)
    val tasks = rememberTasks(core, texts, ready, tick)
    val today by produceState<TodayState>(TodayState.Loading, ready, tick.intValue) {
        if (ready) value = loadToday(core)
    }
    return Hosted(today, people.first, tasks.first, people.second, tasks.second)
}

@Composable
private fun rememberPeople(
    core: Core,
    texts: Texts,
    ready: Boolean,
    context: Context,
): Pair<PeopleData, PeopleActions> {
    val scope = rememberCoroutineScope()
    var query by rememberSaveable { mutableStateOf("") }
    var open by rememberSaveable { mutableStateOf<Long?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var state by remember { mutableStateOf<PeopleState>(PeopleState.Loading) }
    val person by produceState<PersonState?>(null, ready, open) {
        value = open?.let { if (ready) loadPerson(core, it.toULong()) else PersonState.Loading }
    }
    LaunchedEffect(ready, query) { if (ready) state = loadPeople(core, query, 0) }
    val actions = PeopleActions(
        onQuery = { query = it.take(MAX_QUERY) },
        onMore = {
            val current = state as? PeopleState.Ready
            if (current != null && !current.list.complete) {
                val asked = query
                val offset = current.list.rows.size
                scope.launch {
                    val next = loadPeople(core, asked, offset)
                    val now = state as? PeopleState.Ready
                    // A window for a search the owner has since changed, or for a list that has moved on, is dropped.
                    val unchanged = query == asked && now?.list?.rows?.size == offset
                    if (next is PeopleState.Ready && now != null && unchanged) {
                        state = PeopleState.Ready(now.list.append(next.list))
                    }
                }
            }
        },
        onOpen = {
            notice = null
            open = it.toLong()
        },
        onClose = { open = null },
        onMail = {
            val done = launch(context, Intent.ACTION_SENDTO, mailtoLink(it))
            notice = if (done) null else texts.t("No program can write to this address.")
        },
        onCall = {
            val done = launch(context, Intent.ACTION_DIAL, dialLink(it))
            notice = if (done) null else texts.t("No program can call this number.")
        },
    )
    val shown = open?.toULong()
    val card = if (shown != null) person ?: PersonState.Loading else null
    return PeopleData(query, state, shown, card, notice) to actions
}

private const val MAX_QUERY = 200

@Composable
private fun rememberTasks(
    core: Core,
    texts: Texts,
    ready: Boolean,
    tick: MutableIntState,
): Pair<TasksData, TasksActions> {
    val scope = rememberCoroutineScope()
    var showCompleted by rememberSaveable { mutableStateOf(false) }
    var editor by remember { mutableStateOf<DraftState?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var depth by remember { mutableStateOf<Map<String, UInt>>(emptyMap()) }
    val state by produceState<TasksState>(TasksState.Loading, ready, showCompleted, depth, tick.intValue) {
        if (ready) value = loadTasks(core, showCompleted, depth)
    }
    // Runs a command and reads the lists again; says so when the core refused it.
    fun act(command: String, params: String, done: () -> Unit = {}) {
        scope.launch {
            if (runTaskCommand(core, command, params)) {
                notice = null
                done()
                tick.intValue += 1
            } else {
                notice = texts.t("That could not be saved.")
            }
        }
    }
    val editorActions = EditorActions(
        onDraft = { editor = DraftState.Ready(it) },
        onSave = {
            val draft = (editor as? DraftState.Ready)?.draft
            if (draft != null && draft.canSave) {
                act(if (draft.id == null) "tasks.new" else "tasks.edit", draft.params()) { editor = null }
            }
        },
        onDelete = {
            val id = (editor as? DraftState.Ready)?.draft?.id
            if (id != null) act("tasks.delete", "{\"id\":$id}") { editor = null }
        },
        onClose = { editor = null },
    )
    val actions = TasksActions(
        onToggle = { act("tasks.complete", "{\"id\":${it.id},\"value\":${!it.completed}}") },
        onOpen = { id ->
            editor = DraftState.Loading
            scope.launch { editor = loadDraft(core, id) }
        },
        onNew = { editor = DraftState.Ready(newDraft(it)) },
        onShowCompleted = { showCompleted = it },
        onMore = { group ->
            val now = depth[group.key] ?: TASK_WINDOW
            depth = depth + (group.key to (now + TASK_WINDOW).coerceAtMost(MAX_DEPTH))
        },
        onSendLocal = { account -> act("tasks.sendLocal", "{\"account\":${account ?: "null"}}") },
        editor = editorActions,
    )
    return TasksData(state, editor, notice) to actions
}
