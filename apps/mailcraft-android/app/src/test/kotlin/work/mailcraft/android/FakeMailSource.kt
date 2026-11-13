// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger

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
    var refreshOutcome = RefreshOutcome.NoAccount

    /** A change from outside: the rows are these now, at the next revision. */
    fun change(newRows: List<InboxRow>) {
        rows = newRows
        rev += 1uL
    }

    override suspend fun window(query: MailQuery, offset: Int, count: Int): RowsWindow? {
        windows += offset to count
        if (failWindows) return null
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

    override suspend fun categories() = listOf(CategoryItem("Green category", 0x2E8B57))

    override suspend fun refresh(hasServer: Boolean): RefreshOutcome {
        refreshes.incrementAndGet()
        return if (hasServer) RefreshOutcome.Requested else refreshOutcome
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
