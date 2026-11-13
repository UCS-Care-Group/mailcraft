// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * The message list's logic, apart from any screen: which list is shown, which pages of it have been
 * read, what the commands on it do. It reads windows of rows through [MailSource] and never the
 * whole list: pages as the screen scrolls to them, and, when the core says the mailbox changed,
 * again only the pages on screen. Every function may be called from the main thread: the work is
 * the source's, off it.
 */
internal class MailListController(
    private val source: MailSource,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val state = MutableStateFlow(MailListUi.Start)

    val ui: StateFlow<MailListUi> get() = state

    /** Moves on when the list changes, so that an answer to an earlier question is dropped. */
    private val generation = AtomicInteger()
    private val notices = AtomicInteger()

    /** The rows last on screen, kept to read the same ones again after a change. */
    @Volatile
    private var visible = 0..0

    /** The pages being read now, by list and page, so that scrolling does not ask for one twice. */
    private val reading: MutableSet<Pair<Int, Int>> = java.util.Collections.synchronizedSet(HashSet())

    @Volatile
    private var hasServer = false

    /** Whether any account is kept in step with a server: what a pull to refresh can do. */
    fun accountsHaveServer(value: Boolean) {
        hasServer = value
    }

    /** Shows the list [query] asks for: reads its first page and its tabs. */
    suspend fun show(query: MailQuery) {
        val mine = generation.incrementAndGet()
        val before = state.value
        val sameFolder = before.query?.folder == query.folder
        state.update {
            it.copy(
                query = query,
                load = LoadState.Loading,
                snapshot = ListSnapshot.Empty,
                tabs = if (sameFolder) it.tabs else emptyList(),
            )
        }
        visible = 0..0
        source.focus(query)
        val first = source.window(query, 0, PAGE_SIZE)
        if (mine != generation.get()) return
        if (first == null) {
            state.update { it.copy(load = LoadState.Failed) }
            return
        }
        val snapshot = ListSnapshot.Empty.with(first) ?: ListSnapshot.Empty
        state.update { it.copy(load = LoadState.Ready, snapshot = snapshot) }
        if (query.search.isBlank()) loadTabs(mine, query.folder)
    }

    private suspend fun loadTabs(mine: Int, folder: ULong) {
        val tabs = source.splitTabs(folder)
        if (mine == generation.get()) state.update { it.copy(tabs = tabs) }
    }

    /** The rows [first] to [last] are on screen: reads the pages of them (and the next) that are missing. */
    suspend fun showRange(first: Int, last: Int) {
        visible = first..last
        val mine = generation.get()
        val query = state.value.query ?: return
        val ahead = last + PAGE_SIZE / 2
        val wanted = state.value.snapshot.missing(first, ahead).filter { reading.add(mine to it) }
        if (wanted.isEmpty()) return
        try {
            val windows = coroutineScope {
                wanted.map { page -> async { source.window(query, page * PAGE_SIZE, PAGE_SIZE) } }.awaitAll()
            }
            if (mine != generation.get()) return
            windows.filterNotNull().forEach { w -> state.update { it.merged(w) } }
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
        if (query.search.isBlank()) loadTabs(mine, query.folder)
    }

    /** Pull to refresh: asks the core to check for mail, and says so when there is nothing to check. */
    suspend fun refresh() {
        state.update { it.copy(refreshing = true) }
        val outcome = source.refresh(hasServer)
        when (outcome) {
            RefreshOutcome.NoAccount -> notify(Notice.NoAccount)
            RefreshOutcome.Failed -> notify(Notice.Failed)
            RefreshOutcome.Requested -> notify(Notice.NotYet)
        }
        reload()
        state.update { it.copy(refreshing = false) }
    }

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
        val done = source.focusedRun(triage, rows)
        reload()
        // A change the owner can see in the list needs no word; one that takes mail away offers its undo.
        if (!done) notify(Notice.Failed) else if (triage.destructive) notify(Notice.Done(triage, rows.size))
    }

    /** Takes back the last change. */
    suspend fun undo() {
        if (!source.run("edit.undo", "{}")) notify(Notice.Failed)
        reload()
    }

    /** Puts the notice away once it was shown. */
    fun noticeShown(seq: Int) {
        state.update { if (it.notice?.seq == seq) it.copy(notice = null) else it }
    }

    private fun notify(notice: Notice) {
        val event = NoticeEvent(notices.incrementAndGet(), notice)
        state.update { it.copy(notice = event) }
    }

    private fun MailListUi.merged(window: RowsWindow): MailListUi {
        val next = snapshot.with(window) ?: return this
        return copy(snapshot = next, load = LoadState.Ready)
    }
}

/** Runs [triage] on [rows] as commands: a conversation's row is selected first, so that it takes all its messages. */
internal suspend fun MailSource.focusedRun(triage: Triage, rows: List<InboxRow>): Boolean {
    val whole = rows.size == 1 && rows.first().members > 1
    if (whole && !run("mail.select", jsonText(mapOf("id" to rows.first().id)))) return false
    val ids = rows.map { it.id }
    val withIds = if (whole) emptyMap() else mapOf("ids" to ids)
    val (command, params) = when (triage) {
        is Triage.MarkRead -> (if (triage.read) "mail.markRead" else "mail.markUnread") to withIds
        is Triage.Flag -> (if (triage.on) "mail.flag" else "mail.clearFlag") to withIds
        Triage.Archive -> "mail.archive" to withIds
        Triage.Delete -> "mail.delete" to withIds
        Triage.Junk -> "mail.junk" to withIds
        is Triage.MoveTo -> "mail.move" to (withIds + ("folder" to triage.folder))
        is Triage.Category -> "mail.categorize" to (withIds + mapOf("category" to triage.name, "value" to triage.on))
    }
    return run(command, jsonText(params))
}
