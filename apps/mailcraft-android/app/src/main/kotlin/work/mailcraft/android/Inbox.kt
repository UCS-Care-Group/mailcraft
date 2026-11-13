// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** One line of the message list, as the screen draws it. */
internal data class InboxRow(
    val id: ULong,
    val sender: String,
    val subject: String,
    val preview: String,
    val unread: Boolean,
    val whenSeconds: Long,
)

/** A folder's messages as the screen shows them: its name and count in the owner's language, and its rows. */
internal data class InboxModel(val title: String, val count: String, val rows: List<InboxRow>)

internal sealed interface InboxState {
    data object Loading : InboxState

    class Ready(val model: InboxModel) : InboxState

    class Failed(val message: String) : InboxState
}

/** One folder as the drawer lists it; [depth] is how far down the folder tree it sits. */
internal data class FolderItem(
    val id: ULong,
    val parent: ULong?,
    val name: String,
    val kind: String,
    val unread: UInt,
    val depth: Int = 0,
)

/** An account and its folders, top folders first and each folder's own just after it. */
internal data class AccountItem(val id: ULong, val name: String, val email: String, val folders: List<FolderItem>)

/** The accounts of the mailbox, and the folder the Mail tab opens on. */
internal data class MailboxOutline(val accounts: List<AccountItem>, val inboxId: ULong?)

/** The most rows the skeleton asks for at once. */
private const val WINDOW = 100u

/** How far down the folder tree the drawer goes; deeper folders sit at this depth. */
internal const val MAX_FOLDER_DEPTH = 4

/** Who a row is from: the name, or the address when the message has no name. */
internal fun senderLabel(name: String, address: String): String = name.ifBlank { address }

/**
 * Puts a flat list of folders in tree order: each folder's own just after it, with its depth.
 * Order among folders of one parent is kept. A folder whose parent is not in the list is a top
 * folder; a loop of parents (hostile data) ends where it closes, and no folder is listed twice.
 */
internal fun folderTree(folders: List<FolderItem>): List<FolderItem> {
    val ids = folders.mapTo(HashSet()) { it.id }
    val byParent = folders.groupBy { it.parent?.takeIf { parent -> parent in ids } }
    val out = ArrayList<FolderItem>(folders.size)
    val seen = HashSet<ULong>()
    // Depth first, with a stack of its own: a chain of folders as long as the data says is safe.
    val pending = ArrayDeque<Pair<FolderItem, Int>>()
    fun pushChildren(parent: ULong?, depth: Int) {
        byParent[parent].orEmpty().asReversed().forEach { pending.addLast(it to depth) }
    }
    pushChildren(null, 0)
    while (pending.isNotEmpty()) {
        val (folder, depth) = pending.removeLast()
        if (!seen.add(folder.id)) continue
        out.add(folder.copy(depth = minOf(depth, MAX_FOLDER_DEPTH)))
        pushChildren(folder.id, depth + 1)
    }
    // Folders only reachable through a loop of parents are still the owner's.
    folders.filter { seen.add(it.id) }.forEach { out.add(it.copy(depth = 0)) }
    return out
}

/**
 * Opens the sample mailbox and lists its accounts and folders, read-only. The sample is the core's
 * own invented mailbox; no account is signed in and nothing is fetched.
 */
internal suspend fun loadOutline(
    core: Core,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): MailboxOutline? = withContext(dispatcher) {
    try {
        core.execute("file.new", """{"sample":true}""")
        val list = core.folders(0uL)
        val accounts = list.accounts.map { account ->
            val own = list.folders.filter { it.account == account.id }.map {
                FolderItem(it.id, it.parent, it.name, it.kind, it.unread)
            }
            AccountItem(account.id, account.displayName.ifBlank { account.name }, account.email, folderTree(own))
        }
        val inbox = list.folders.firstOrNull { it.kind == "inbox" }
        MailboxOutline(accounts, inbox?.id)
    } catch (_: CoreException) {
        null
    }
}

/** A window of the messages of [folder], read-only. */
internal suspend fun loadFolder(
    core: Core,
    folder: ULong,
    name: String,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): InboxState = withContext(dispatcher) {
    try {
        val window = core.messages(folder, "all", 0u, WINDOW)
        val rows = window.rows.map {
            InboxRow(
                id = it.id,
                sender = senderLabel(it.fromName, it.fromAddress),
                subject = it.subject,
                preview = it.preview,
                unread = it.unread,
                whenSeconds = it.`when`,
            )
        }
        val count = core.tn(window.total.toULong(), "{n} message", "{n} messages")
        InboxState.Ready(InboxModel(name, count, rows))
    } catch (_: CoreException) {
        InboxState.Failed(core.t("The mailbox could not be read."))
    }
}
