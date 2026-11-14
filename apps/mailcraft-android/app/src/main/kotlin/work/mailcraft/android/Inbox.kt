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

/** The Inbox as the screen shows it: its name and count in the owner's language, and its rows. */
internal data class InboxModel(val title: String, val count: String, val rows: List<InboxRow>)

internal sealed interface InboxState {
    data object Loading : InboxState

    class Ready(val model: InboxModel) : InboxState

    class Failed(val message: String) : InboxState
}

/** The most rows the skeleton asks for at once. */
private const val WINDOW = 100u

/** Who a row is from: the name, or the address when the message has no name. */
internal fun senderLabel(name: String, address: String): String = name.ifBlank { address }

/**
 * Opens the sample mailbox and reads its Inbox, read-only. The sample is the core's own invented
 * mailbox; no account is signed in and nothing is fetched.
 */
internal suspend fun loadInbox(
    core: Core,
    dispatcher: CoroutineDispatcher = Dispatchers.Default,
): InboxState = withContext(dispatcher) {
    try {
        core.execute("file.new", """{"sample":true}""")
        val folders = core.folders(0uL)
        val inbox = folders.folders.firstOrNull { it.kind == "inbox" }
        if (inbox == null) {
            InboxState.Failed(core.t("The sample mailbox has no Inbox."))
        } else {
            val window = core.messages(inbox.id, "all", 0u, WINDOW)
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
            InboxState.Ready(InboxModel(inbox.name, count, rows))
        }
    } catch (_: CoreException) {
        InboxState.Failed(core.t("The mailbox could not be read."))
    }
}
