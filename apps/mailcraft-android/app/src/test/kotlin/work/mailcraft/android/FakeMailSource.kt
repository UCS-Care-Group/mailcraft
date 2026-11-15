// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CompletableDeferred

/** A mail source over a list of invented rows that records what it is asked, for the tests. */
internal class FakeMailSource(var rows: List<InboxRow> = emptyList()) : MailSource {
    var rev = 1uL
    val windows = mutableListOf<Pair<Int, Int>>()
    val commands = mutableListOf<Pair<String, String>>()
    var tabs: List<SplitTabItem> = emptyList()
    var failWindows = false
    var failCommands = false
    var isOffline = false
    val refreshes = AtomicInteger()
    var refreshOutcome: RefreshOutcome = RefreshOutcome.Refused("There is no account to send and receive.")

    /** When set, a window is answered only once it is completed: a read still on its way. */
    var gate: CompletableDeferred<Unit>? = null

    /** When set, only the window asked for in this place (0 the first) waits for [gate]. */
    var waitCall: Int? = null

    /** When set, the window asked for in this place fails. */
    var failCall: Int? = null
    private var calls = 0

    /** A change from outside: the rows are these now, at the next revision. */
    fun change(newRows: List<InboxRow>) {
        rows = newRows
        rev += 1uL
    }

    override suspend fun window(query: MailQuery, offset: Int, count: Int): RowsWindow? {
        windows += offset to count
        val call = calls++
        val waiting = waitCall
        if (waiting == null || waiting == call) gate?.await()
        if (failWindows || failCall == call) return null
        return RowsWindow(rev, rows.size, offset, rows.drop(offset).take(count))
    }

    override suspend fun splitTabs(folder: ULong) = tabs

    override suspend fun run(command: String, params: String): Boolean {
        commands += command to params
        if (failCommands) return false
        if (command == "mail.delete" || command == "mail.archive") change(rows.drop(1))
        if (command == "edit.undo") change(listOf(row(0)) + rows)
        return true
    }

    /** The whole lists of messages rows stand for, by row; a row not here is gone. */
    var memberLists: Map<ULong, List<ULong>> = emptyMap()
    val memberAsks = mutableListOf<ULong>()

    override suspend fun members(query: MailQuery, id: ULong): List<ULong> {
        memberAsks += id
        return memberLists[id].orEmpty()
    }

    override suspend fun categories() = listOf(CategoryItem("Green category", 0x2E8B57))

    override suspend fun refresh(): RefreshOutcome {
        refreshes.incrementAndGet()
        return refreshOutcome
    }

    override suspend fun offline() = isOffline

    companion object {
        fun row(n: Int, members: Int = 1) = InboxRow(
            id = n.toULong(),
            sender = "Sender $n",
            subject = "Subject $n",
            preview = "Preview $n",
            unread = n % 2 == 0,
            whenSeconds = 1_730_000_000L - n,
            members = members,
            memberIds = if (members > 1) (0 until members).map { (n * 100 + it).toULong() } else emptyList(),
        )

        fun rows(count: Int) = (1..count).map { row(it) }
    }
}
