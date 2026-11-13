// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.ListQuery
import uniffi.mailcraft_mobile.NetworkKind

/** What asking the core to check for mail came to. */
internal enum class RefreshOutcome {
    /** No account is kept in step with a server: there is nothing to check. */
    NoAccount,

    /** The core was asked. */
    Requested,

    /** The core refused or could not be asked. */
    Failed,
}

/**
 * What the mail list needs of the core, so that its logic runs without the native library. Every
 * call may take long and is made off the main thread; a failure is an answer (null, false), never a throw.
 */
internal interface MailSource {
    /** A window of the list [query] asks for, or null when it could not be read. */
    suspend fun window(query: MailQuery, offset: Int, count: Int): RowsWindow?

    /** The tabs of the Inbox split over [folder]; none when it has no split. */
    suspend fun splitTabs(folder: ULong): List<SplitTabItem>

    /** Runs a command by its id; whether it was done. */
    suspend fun run(command: String, params: String): Boolean

    suspend fun categories(): List<CategoryItem>

    /** Asks the core to check [hasServer] accounts for mail. */
    suspend fun refresh(hasServer: Boolean): RefreshOutcome

    /** Whether the device is online. */
    suspend fun offline(): Boolean
}

/** [MailSource] over the Rust core. */
internal class CoreMailSource(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
) : MailSource {
    override suspend fun window(query: MailQuery, offset: Int, count: Int): RowsWindow? = withContext(dispatcher) {
        try {
            val w = core.messageList(query.toCore(), offset.coerceAtLeast(0).toUInt(), count.coerceAtLeast(1).toUInt())
            RowsWindow(w.rev, w.total.toInt(), w.offset.toInt(), w.rows.map { it.toRow() })
        } catch (_: CoreException) {
            null
        }
    }

    override suspend fun splitTabs(folder: ULong): List<SplitTabItem> = withContext(dispatcher) {
        try {
            core.inboxSplit(folder).tabs.map { SplitTabItem(it.code, it.name, it.unread.toInt()) }
        } catch (_: CoreException) {
            emptyList()
        }
    }

    override suspend fun run(command: String, params: String): Boolean = withContext(dispatcher) {
        try {
            core.execute(command, params)
            true
        } catch (_: CoreException) {
            false
        }
    }

    override suspend fun categories(): List<CategoryItem> = withContext(dispatcher) {
        try {
            core.categories(0uL).categories.map { CategoryItem(it.name, it.color.toInt()) }
        } catch (_: CoreException) {
            emptyList()
        }
    }

    override suspend fun refresh(hasServer: Boolean): RefreshOutcome = when {
        !hasServer -> RefreshOutcome.NoAccount
        run("sync.request", "{}") -> RefreshOutcome.Requested
        else -> RefreshOutcome.Failed
    }

    override suspend fun offline(): Boolean = withContext(dispatcher) { core.network() == NetworkKind.Offline }
}

internal fun MailQuery.toCore() = ListQuery(
    folder = folder,
    filter = filter.code,
    split = split,
    conversations = conversations,
    search = search,
    everywhere = everywhere,
)

internal fun uniffi.mailcraft_mobile.MessageRow.toRow() = InboxRow(
    id = id,
    sender = senderLabel(fromName, fromAddress),
    subject = subject,
    preview = preview,
    unread = unread,
    whenSeconds = `when`,
    flagged = flagged,
    hasAttachments = hasAttachments,
    important = important,
    members = members.toInt(),
    memberIds = memberIds,
    group = group,
    categories = categories,
    account = account,
    folder = folder,
)
