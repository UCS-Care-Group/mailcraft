// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Event

/**
 * The message list's logic, apart from any screen: which list is shown, which pages of it have been
 * read, what the commands on it do. It reads windows of rows through [MailSource] and never the
 * whole list: pages as the screen scrolls to them, and, when the core says the mailbox changed,
 * again only the pages on screen. Every function may be called from the main thread: the work is
 * the source's, off it. There is one for the app, made with the core, so that the list outlives a
 * window; the windows only read [ui].
 */
internal class MailListController(
    private val source: MailSource,
    private val clock: () -> Long = System::currentTimeMillis,
    /** Waits between tries of a page that could not be read. */
    private val pause: suspend (Long) -> Unit = { delay(it) },
) {
    private val state = MutableStateFlow(MailListUi.Start)

    val ui: StateFlow<MailListUi> get() = state

    /** Moves on when the list changes, so that an answer to an earlier question is dropped. */
    private val generation = AtomicInteger()
    private val notices = AtomicInteger()

    /** The list (by generation) a reload last brought rows for: a first page that fails then leaves them. */
    @Volatile
    private var freshFor = -1

    /** Moves on with each command, so that only the last one's completion offers an undo. */
    private val commands = AtomicInteger()

    /** Pulls to refresh under way. */
    private val refreshes = AtomicInteger()

    /** The rows last on screen, kept to read the same ones again after a change. */
    @Volatile
    private var visible = 0..0

    /** The pages being read now, by list and page, so that scrolling does not ask for one twice. */
    private val reading: MutableSet<Pair<Int, Int>> = java.util.Collections.synchronizedSet(HashSet())

    /** Whether the rows held are of an earlier query, its first page not yet in: nothing is read into them. */
    @Volatile
    private var switching = false

    /**
     * Shows the list [query] asks for: reads its first page and its tabs. In the same folder, the
     * rows of the list that was shown stay until that page arrives, so that a keystroke in the search
     * or a change of filter does not blank the screen; another folder's rows never stand under the
     * new folder's name, so the list says it is loading until its own page is in.
     */
    suspend fun show(query: MailQuery) {
        val mine = generation.incrementAndGet()
        val before = state.value
        val sameFolder = before.query?.folder == query.folder
        switching = true
        state.update {
            val keep = sameFolder && it.snapshot.total > 0
            it.copy(
                query = query,
                load = if (keep) it.load else LoadState.Loading,
                snapshot = if (sameFolder) it.snapshot else ListSnapshot.Empty,
                tabs = if (sameFolder) it.tabs else emptyList(),
            )
        }
        visible = 0..0
        val first = try {
            source.window(query, 0, PAGE_SIZE)
        } finally {
            // (Called off too: paging is never left waiting for a first page that will not come.)
            if (mine == generation.get()) switching = false
        }
        if (mine != generation.get()) return
        if (first == null) {
            // A reload of this list that brought its rows meanwhile is kept; else the list is unreadable.
            if (freshFor != mine) state.update { it.copy(load = LoadState.Failed, snapshot = ListSnapshot.Empty) }
            return
        }
        val snapshot = ListSnapshot.Empty.with(first) ?: ListSnapshot.Empty
        // A reload of this same list may have brought a newer revision while the first page was on
        // its way: that one stays.
        state.update {
            it.copy(load = LoadState.Ready, snapshot = if (it.snapshot.rev > first.rev) it.snapshot else snapshot)
        }
        if (query.search.isBlank()) loadTabs(mine, query.folder)
    }

    private suspend fun loadTabs(mine: Int, folder: ULong) {
        val tabs = source.splitTabs(folder)
        if (mine == generation.get()) state.update { it.copy(tabs = tabs) }
    }

    /** The rows [first] to [last] are on screen: reads the pages of them (and the next) that are missing. */
    suspend fun showRange(first: Int, last: Int) {
        visible = first..last
        val query = state.value.query ?: return
        val mine = generation.get()
        val ahead = last + PAGE_SIZE / 2
        val missing = if (switching) emptyList() else state.value.snapshot.missing(first, ahead)
        val wanted = missing.filter { reading.add(mine to it) }
        try {
            // A page that could not be read is tried again a few times, a while apart (and a pull to
            // refresh reads the pages on screen again).
            var left = wanted
            var tries = 0
            while (left.isNotEmpty() && mine == generation.get() && tries <= PAGE_RETRIES) {
                if (tries > 0) pause(RETRY_MILLIS * tries)
                val windows = source.pages(query, left)
                val got = windows.values.filterNotNull().takeIf { mine == generation.get() }.orEmpty()
                state.update { ui -> got.fold(ui) { acc, w -> acc.merged(w) } }
                left = windows.filterValues { it == null }.keys.toList()
                tries++
            }
        } finally {
            wanted.forEach { reading.remove(mine to it) }
        }
    }

    /**
     * The core says the mailbox is at revision [rev]. A list read at an older one is stale: the
     * pages on screen are read again, together, and the others are dropped to be read when they
     * are scrolled to. Nothing happens for a revision this list already has.
     */
    suspend fun revChanged(rev: ULong) {
        if (rev <= state.value.snapshot.rev) return
        reload()
    }

    /** Reads the pages on screen again, as the mailbox is now. */
    suspend fun reload() {
        val mine = generation.get()
        val query = state.value.query ?: return
        val total = state.value.snapshot.total
        val pages = ListSnapshot.pagesOf(visible.first, visible.last, maxOf(total, 1)).ifEmpty { listOf(0) }
        val windows = coroutineScope {
            pages.map { page -> async { source.window(query, page * PAGE_SIZE, PAGE_SIZE) } }.awaitAll()
        }.filterNotNull()
        if (mine != generation.get() || windows.isEmpty()) return
        val newest = windows.maxOf { it.rev }
        val fresh = windows.filter { it.rev == newest }
        val first = ListSnapshot(newest, fresh.first().total).with(fresh.first()) ?: return
        val snapshot = fresh.drop(1).fold(first) { acc, w -> acc.with(w) ?: acc }
        state.update { if (newest >= it.snapshot.rev) it.copy(load = LoadState.Ready, snapshot = snapshot) else it }
        freshFor = mine
        // The owner may have scrolled while the pages were read: what is on screen now is filled in.
        showRange(visible.first, visible.last)
        if (query.search.isBlank()) loadTabs(mine, query.folder)
    }

    /**
     * Pull to refresh: asks the core to send and receive. A round that starts needs no word (the
     * freshness line moves when it ends); one the core won't start says why, in its own words.
     */
    suspend fun refresh() {
        // The controller outlives the window whose pull started this: however it ends (the window's
        // work called off among them), the spinner stops once the last refresh under way is over.
        refreshes.incrementAndGet()
        state.update { it.copy(refreshing = true) }
        try {
            when (val outcome = source.refresh()) {
                RefreshOutcome.Started -> Unit
                is RefreshOutcome.Refused -> notify(ListNotice.Said(outcome.message))
                RefreshOutcome.Failed -> notify(ListNotice.Failed)
            }
            reload()
        } finally {
            refreshes.decrementAndGet()
            state.update { it.copy(refreshing = refreshes.get() > 0) }
        }
    }

    /** The categories mail can be given, with their colours. */
    suspend fun categories(): List<CategoryItem> = source.categories()

    /** A synchronisation finished: the list is as fresh as now. */
    fun updated() {
        val now = clock()
        state.update { it.copy(updatedAtMillis = now) }
    }

    /** Reads what the device says of the network. */
    suspend fun checkNetwork() {
        val offline = source.offline()
        state.update { it.copy(offline = offline) }
    }

    /** Makes [triage] to [rows]; a change that takes mail away says so, with the way to take it back. */
    suspend fun triage(triage: Triage, rows: List<InboxRow>) {
        if (rows.isEmpty()) return
        val mine = commands.incrementAndGet()
        val count = state.value.query?.let { source.focusedRun(triage, it, rows) }
        // Undo takes back the last change: once this one is made, an earlier offer would undo it instead.
        if (count != null) state.update { if (it.notice?.notice?.undoable == true) it.copy(notice = null) else it }
        reload()
        // A change the owner can see in the list needs no word; one that takes mail away offers its undo,
        // counting the messages it took, unless another command came after it.
        when {
            count == null -> notify(ListNotice.Failed)
            triage.destructive && mine == commands.get() -> notify(ListNotice.Done(triage, count))
            else -> Unit
        }
    }

    /** Takes back the last change. */
    suspend fun undo() {
        commands.incrementAndGet()
        if (!source.run("edit.undo", "{}")) notify(ListNotice.Failed)
        reload()
    }

    /** Puts the notice away once it was shown. */
    fun noticeShown(seq: Int) {
        state.update { if (it.notice?.seq == seq) it.copy(notice = null) else it }
    }

    private fun notify(notice: ListNotice) {
        val event = NoticeEvent(notices.incrementAndGet(), notice)
        state.update { it.copy(notice = event) }
    }

    private fun MailListUi.merged(window: RowsWindow): MailListUi {
        val next = snapshot.with(window) ?: return this
        return copy(snapshot = next, load = LoadState.Ready)
    }
}

/** The [pages] of the list [query] asks for, read together: each page's window, or null when it couldn't be read. */
internal suspend fun MailSource.pages(query: MailQuery, pages: List<Int>): Map<Int, RowsWindow?> = coroutineScope {
    pages.map { page -> async { page to window(query, page * PAGE_SIZE, PAGE_SIZE) } }.awaitAll().toMap()
}

/** How many more times a page that could not be read is asked for, and how far apart (times the try). */
private const val PAGE_RETRIES = 2
private const val RETRY_MILLIS = 1_000L

/**
 * Takes the core's events that are the list's, in [scope]: a newer revision reads the pages on
 * screen again, a Send/Receive round that finished marks the list fresh. Every other event is left
 * to its own handler.
 */
internal fun MailListController.handle(event: Event, scope: CoroutineScope) {
    when (event) {
        is Event.RevChanged -> scope.launch { revChanged(event.rev) }
        is Event.SyncFinished -> if (event.finished) updated()
        else -> Unit
    }
}

/**
 * Runs [triage] on [rows] of the list [query] as commands, naming the messages: a conversation's row
 * stands for the messages it lists, which are the ones that command takes, whatever filter or search
 * made the list. A row that stands for more than the ids it carries has its whole list read first;
 * when that can't be had (the row is gone, or the core can't say), nothing is done. Gives how many
 * messages the command named, or null when it was not done.
 */
internal suspend fun MailSource.focusedRun(triage: Triage, query: MailQuery, rows: List<InboxRow>): Int? {
    val ids = rows.flatMap { row ->
        if (row.members > row.memberIds.size && row.members > 1) {
            members(query, row.id)?.takeIf { it.isNotEmpty() } ?: return null
        } else {
            row.memberIds.ifEmpty { listOf(row.id) }
        }
    }.distinct()
    val withIds = mapOf("ids" to ids)
    val (command, params) = when (triage) {
        is Triage.MarkRead -> (if (triage.read) "mail.markRead" else "mail.markUnread") to withIds
        is Triage.Flag -> (if (triage.on) "mail.flag" else "mail.clearFlag") to withIds
        Triage.Archive -> "mail.archive" to withIds
        Triage.Delete -> "mail.delete" to withIds
        Triage.Junk -> "mail.junk" to withIds
        is Triage.MoveTo -> "mail.move" to (withIds + ("folder" to triage.folder))
        is Triage.Category -> "mail.categorize" to (withIds + mapOf("category" to triage.name, "value" to triage.on))
    }
    return if (run(command, jsonText(params))) ids.size else null
}
