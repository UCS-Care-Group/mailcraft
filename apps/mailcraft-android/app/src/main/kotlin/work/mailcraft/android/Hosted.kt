// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableIntState
import androidx.compose.runtime.MutableState
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
import kotlinx.coroutines.Job
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
 * The state of the three tabs over [core], once [ready], read again whenever [reload] moves on (the
 * mailbox changed: a synchronisation brought contacts or tasks). Contacts are read a window at a time; a
 * change to a task is a command (`tasks.new`, `tasks.edit`, `tasks.complete`, `tasks.delete`) and
 * the lists are read again after it. Nothing is sent to a server from here.
 */
@Composable
internal fun rememberHosted(core: Core, texts: Texts, ready: Boolean, reload: Int = 0): Hosted {
    val context = LocalContext.current
    val tick = remember { mutableIntStateOf(0) }
    val people = rememberPeople(core, texts, ready, reload, context)
    val tasks = rememberTasks(core, texts, ready, reload, tick)
    val today by produceState<TodayState>(TodayState.Loading, ready, reload, tick.intValue) {
        if (ready) value = loadToday(core)
    }
    return Hosted(today, people.first, tasks.first, people.second, tasks.second)
}

@Composable
@Suppress("LongMethod")
private fun rememberPeople(
    core: Core,
    texts: Texts,
    ready: Boolean,
    reload: Int,
    context: Context,
): Pair<PeopleData, PeopleActions> {
    val scope = rememberCoroutineScope()
    var query by rememberSaveable { mutableStateOf("") }
    var open by rememberSaveable { mutableStateOf<Long?>(null) }
    var notice by remember { mutableStateOf<String?>(null) }
    var state by remember { mutableStateOf<PeopleState>(PeopleState.Loading) }
    // The new-person form is kept by the system while the app is away.
    val editorState = rememberSaveable(stateSaver = PersonDraft.Saver) { mutableStateOf<PersonDraft?>(null) }
    val busyState = remember { mutableStateOf(false) }
    var added by remember { mutableIntStateOf(0) }
    // Bumped on every reload and add: a page asked for before that is stale when it comes back.
    var generation by remember { mutableIntStateOf(0) }
    val person by produceState<PersonState?>(null, ready, reload, open) {
        value = open?.let { if (ready) loadPerson(core, it.toULong()) else PersonState.Loading }
    }
    LaunchedEffect(ready, reload, query, added) {
        generation += 1
        if (ready) state = loadPeople(core, query, 0)
    }
    val form = formActions(scope, core, editorState, busyState) { made ->
        if (made != null) {
            notice = null
            query = ""
            open = made.toLong()
            added += 1
        } else {
            notice = texts.t("That could not be saved.")
        }
    }
    val actions = PeopleActions(
        onQuery = { query = it.take(MAX_QUERY) },
        onMore = {
            val current = state as? PeopleState.Ready
            if (current != null && !current.list.complete) {
                val asked = query
                val offset = current.list.rows.size
                val asking = generation
                scope.launch {
                    val next = loadPeople(core, asked, offset)
                    val now = state as? PeopleState.Ready
                    // A window for a search the owner has since changed, or for a list that has moved on, is dropped.
                    val unchanged = generation == asking && query == asked && now?.list?.rows?.size == offset
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
        form = form,
        onMail = {
            val done = launch(context, Intent.ACTION_SENDTO, mailtoLink(it))
            notice = texts.t("No program can write to this address.").takeUnless { done }
        },
        onCall = {
            val done = launch(context, Intent.ACTION_DIAL, dialLink(it))
            notice = texts.t("No program can call this number.").takeUnless { done }
        },
    )
    val shown = open?.toULong()
    val card = if (shown != null) person ?: PersonState.Loading else null
    return PeopleData(query, state, shown, card, notice, editorState.value, busyState.value) to actions
}

private const val MAX_QUERY = 200

@Composable
private fun rememberTasks(
    core: Core,
    texts: Texts,
    ready: Boolean,
    reload: Int,
    tick: MutableIntState,
): Pair<TasksData, TasksActions> {
    val scope = rememberCoroutineScope()
    var showCompleted by rememberSaveable { mutableStateOf(false) }
    val editorState = remember { mutableStateOf<DraftState?>(null) }
    var editor by editorState
    var notice by remember { mutableStateOf<String?>(null) }
    var depth by remember { mutableStateOf<Map<String, UInt>>(emptyMap()) }
    // A change on its way to the core (a second tap waits for it), and the task being read into the editor.
    var busy by remember { mutableStateOf(false) }
    var load by remember { mutableStateOf<Job?>(null) }
    val state by produceState<TasksState>(TasksState.Loading, ready, reload, showCompleted, depth, tick.intValue) {
        if (ready) value = loadTasks(core, showCompleted, depth)
    }
    // Runs a command and reads the lists again; says so when the core refused it.
    fun act(command: String, params: String, done: () -> Unit = {}) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                if (runTaskCommand(core, command, params)) {
                    notice = null
                    done()
                    tick.intValue += 1
                } else {
                    notice = texts.t("That could not be saved.")
                }
            } finally {
                busy = false
            }
        }
    }
    // Shows [state] in the editor, dropping a task still being read into it.
    fun show(state: DraftState?) {
        load?.cancel()
        load = null
        editor = state
    }
    val editorActions = editorActions(editorState, { busy }, ::act) { show(null) }
    val actions = TasksActions(
        onToggle = { act("tasks.complete", "{\"id\":${it.id},\"value\":${!it.completed}}") },
        onOpen = { id ->
            show(DraftState.Loading)
            load = scope.launch { editor = loadDraft(core, id) }
        },
        onNew = { show(DraftState.Ready(newDraft(it))) },
        onShowCompleted = { showCompleted = it },
        onMore = { group ->
            val now = depth[group.key] ?: TASK_WINDOW
            depth = depth + (group.key to (now + TASK_WINDOW).coerceAtMost(MAX_DEPTH))
        },
        onSendLocal = { account -> act("tasks.sendLocal", "{\"account\":${account ?: "null"}}") },
        editor = editorActions,
    )
    return TasksData(state, editor, notice, busy) to actions
}

/**
 * What the editor's buttons do. While a change is on its way the draft it came from stays as it is;
 * when it is done the editor closes only if it still shows that draft, not one the owner opened
 * meanwhile.
 */
private fun editorActions(
    editor: MutableState<DraftState?>,
    busy: () -> Boolean,
    act: (String, String, () -> Unit) -> Unit,
    close: () -> Unit,
): EditorActions = EditorActions(
    onDraft = { if (!busy()) editor.value = DraftState.Ready(it) },
    onSave = {
        val shown = editor.value
        val draft = (shown as? DraftState.Ready)?.draft
        if (draft != null && draft.canSave) {
            act(if (draft.id == null) "tasks.new" else "tasks.edit", draft.params()) {
                if (editor.value === shown) editor.value = null
            }
        }
    },
    onDelete = {
        val shown = editor.value
        val id = (shown as? DraftState.Ready)?.draft?.id
        if (id != null) act("tasks.delete", "{\"id\":$id}") { if (editor.value === shown) editor.value = null }
    },
    onClose = close,
)
