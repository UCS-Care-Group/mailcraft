// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.MessageView

/** What the reading view needs besides the message: the list's own triage, and the owner's reading choices. */
internal class ReadingTools(
    /** What the move and category pickers list; null when there is no list to act on (the toolbar is not shown). */
    val context: MailListContext?,
    /** The list's triage: the very command a row's swipe or long press runs. */
    val onTriage: (Triage, InboxRow) -> Unit,
    /** The message left the list's place (archived, moved, deleted): the view goes back to the list. */
    val onLeave: () -> Unit,
    val prefs: ReadingPrefs,
    val onPrefs: (ReadingPrefs) -> Unit,
    /** The message fills a phone's screen: the bar over it has the back button, since the app's own bar is gone. */
    val phone: Boolean = false,
) {
    companion object {
        val None = ReadingTools(null, { _, _ -> }, {}, ReadingPrefs(), {})
    }
}

/** The message as a list row, which is what the list's triage and its pickers take. */
internal fun rowOf(view: MessageView): InboxRow = InboxRow(
    id = view.id,
    sender = shortName(view.from),
    subject = view.subject,
    preview = "",
    unread = view.unread,
    whenSeconds = view.`when`,
    flagged = view.flagged,
    memberIds = listOf(view.id),
    categories = view.categories,
    account = view.account,
    folder = view.folder,
    address = view.from.address,
)
