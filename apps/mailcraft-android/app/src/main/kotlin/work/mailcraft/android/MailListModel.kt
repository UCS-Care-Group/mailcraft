// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.runtime.saveable.Saver
import androidx.compose.runtime.saveable.listSaver

/** What the funnel button filters the list by. [code] is the core's own word for it. */
internal enum class ListFilter(val code: String) {
    All("all"),
    Unread("unread"),
    Flagged("flagged"),
    ToMe("toMe"),
    Attachments("attachments"),
    ;

    fun label(texts: Texts): String = when (this) {
        All -> texts.t("All")
        Unread -> texts.t("Unread")
        Flagged -> texts.t("Flagged")
        ToMe -> texts.t("To me")
        Attachments -> texts.t("Has attachments")
    }
}

/** The list the owner asked for: where, filtered how, which tab of the Inbox split, and what search. */
internal data class MailQuery(
    val folder: ULong,
    val filter: ListFilter = ListFilter.All,
    /** The Inbox split's tab (its code), or "" for the whole folder. */
    val split: String = "",
    val conversations: Boolean = false,
    val search: String = "",
    /** A search looks through every account, not the folder alone. */
    val everywhere: Boolean = false,
) {
    /** Whether anything narrows the list: what an empty result is then said to be about. */
    val narrowed: Boolean get() = filter != ListFilter.All || search.isNotBlank() || split.isNotEmpty()

    companion object {
        /** Kept across a change of configuration; what can't be read is the start (folder 0: the Inbox). */
        val Saver: Saver<MailQuery, Any> = listSaver(
            save = {
                listOf(it.folder.toLong(), it.filter.ordinal, it.split, it.conversations, it.search, it.everywhere)
            },
            restore = { fromSaved(it) },
        )

        internal fun fromSaved(saved: List<Any?>): MailQuery = MailQuery(
            folder = (saved.getOrNull(AT_FOLDER) as? Long)?.takeIf { it > 0 }?.toULong() ?: 0uL,
            filter = (saved.getOrNull(AT_FILTER) as? Int)?.let { ListFilter.entries.getOrNull(it) } ?: ListFilter.All,
            split = (saved.getOrNull(AT_SPLIT) as? String).orEmpty(),
            conversations = saved.getOrNull(AT_CONVERSATIONS) as? Boolean ?: false,
            search = (saved.getOrNull(AT_SEARCH) as? String).orEmpty(),
            everywhere = saved.getOrNull(AT_EVERYWHERE) as? Boolean ?: false,
        )

        private const val AT_FOLDER = 0
        private const val AT_FILTER = 1
        private const val AT_SPLIT = 2
        private const val AT_CONVERSATIONS = 3
        private const val AT_SEARCH = 4
        private const val AT_EVERYWHERE = 5
    }
}

/** One line of the message list, as the screen draws it. */
internal data class InboxRow(
    val id: ULong,
    val sender: String,
    val subject: String,
    val preview: String,
    val unread: Boolean,
    val whenSeconds: Long,
    val flagged: Boolean = false,
    val hasAttachments: Boolean = false,
    val important: Boolean = false,
    /** Messages the row stands for: more than one for a conversation. */
    val members: Int = 1,
    /** The heading above the row when it starts a group, in English (the screen translates it). */
    val group: String? = null,
    val categories: List<String> = emptyList(),
    val account: ULong = 0uL,
    val folder: ULong = 0uL,
)

/** One tab of an Inbox split. */
internal data class SplitTabItem(val code: String, val name: String, val unread: Int)

/** A category the owner can put on mail, with its colour as `0xRRGGBB`. */
internal data class CategoryItem(val name: String, val color: Int)

/** A window of rows as the core answers it. */
internal data class RowsWindow(val rev: ULong, val total: Int, val offset: Int, val rows: List<InboxRow>)

/** The rows are fetched and kept in pages of this many. */
internal const val PAGE_SIZE = 50

/**
 * The rows read so far, by page, all as they were at one revision. Rows that were never asked for
 * are not here: a list of a hundred thousand messages is never read whole, and a change reads again
 * only the pages on screen.
 */
internal class ListSnapshot(
    val rev: ULong,
    val total: Int,
    private val pages: Map<Int, List<InboxRow>> = emptyMap(),
) {
    fun row(index: Int): InboxRow? =
        if (index < 0) null else pages[index / PAGE_SIZE]?.getOrNull(index % PAGE_SIZE)

    /** Which of the pages that [first] to [last] touch are not here yet. */
    fun missing(first: Int, last: Int): List<Int> = pagesOf(first, last, total).filter { it !in pages }

    /** The window [window] added, when it is of this revision; one of a newer revision replaces all of this. */
    fun with(window: RowsWindow): ListSnapshot? = when {
        window.rev < rev -> null
        window.rev > rev || window.total != total -> ListSnapshot(window.rev, window.total, pageOf(window))
        else -> ListSnapshot(rev, total, pages + pageOf(window))
    }

    /** The pages held, for the tests. */
    val heldPages: Set<Int> get() = pages.keys

    private fun pageOf(window: RowsWindow): Map<Int, List<InboxRow>> =
        if (window.offset % PAGE_SIZE == 0) mapOf(window.offset / PAGE_SIZE to window.rows) else emptyMap()

    companion object {
        val Empty = ListSnapshot(0uL, 0)

        /** The numbers of the pages that rows [first] to [last] of a list of [total] lie in. */
        fun pagesOf(first: Int, last: Int, total: Int): List<Int> {
            if (total <= 0) return emptyList()
            val from = first.coerceIn(0, total - 1) / PAGE_SIZE
            val to = last.coerceIn(0, total - 1) / PAGE_SIZE
            return (from..to).toList()
        }
    }
}

/** Where the list is: being read, shown, or not readable. */
internal enum class LoadState { Loading, Ready, Failed }

/** What happened that the owner is told of, once. [Undo] has a button that takes it back. */
internal sealed interface Notice {
    data object NoAccount : Notice

    data object NotYet : Notice

    data object Failed : Notice

    class Done(val triage: Triage, val count: Int) : Notice

    /** Whether the notice offers to undo what it says. */
    val undoable: Boolean get() = this is Done && triage.destructive
}

/** A notice with a number, so that the same notice twice is two. */
internal data class NoticeEvent(val seq: Int, val notice: Notice)

/** Everything the list screen draws, as plain values. */
internal data class MailListUi(
    val query: MailQuery?,
    val load: LoadState,
    val snapshot: ListSnapshot,
    val tabs: List<SplitTabItem>,
    val refreshing: Boolean,
    val notice: NoticeEvent?,
    /** When mail was last brought up to date (milliseconds since 1970), if it was since this started. */
    val updatedAtMillis: Long?,
    val offline: Boolean,
) {
    companion object {
        val Start = MailListUi(null, LoadState.Loading, ListSnapshot.Empty, emptyList(), false, null, null, false)
    }
}

/** A change to mail, as the swipes, the row's menu and the triage commands make it. */
internal sealed interface Triage {
    /** A change the owner would want to undo: it takes mail out of the list. */
    val destructive: Boolean get() = false

    class MarkRead(val read: Boolean) : Triage

    class Flag(val on: Boolean) : Triage

    data object Archive : Triage {
        override val destructive get() = true
    }

    data object Delete : Triage {
        override val destructive get() = true
    }

    data object Junk : Triage {
        override val destructive get() = true
    }

    class MoveTo(val folder: ULong) : Triage {
        override val destructive get() = true
    }

    class Category(val name: String, val on: Boolean) : Triage
}

/** What a swipe can do; the owner picks one for each direction. */
internal enum class SwipeAction(val code: String) {
    Archive("archive"),
    Delete("delete"),
    ToggleRead("toggleRead"),
    ToggleFlag("toggleFlag"),
    Junk("junk"),
    ;

    fun label(texts: Texts): String = when (this) {
        Archive -> texts.t("Archive")
        Delete -> texts.t("Delete")
        ToggleRead -> texts.t("Read / unread")
        ToggleFlag -> texts.t("Flag")
        Junk -> texts.t("Junk")
    }

    /** The change this swipe makes to [row]. */
    fun triage(row: InboxRow): Triage = when (this) {
        Archive -> Triage.Archive
        Delete -> Triage.Delete
        ToggleRead -> Triage.MarkRead(read = row.unread)
        ToggleFlag -> Triage.Flag(on = !row.flagged)
        Junk -> Triage.Junk
    }
}

/** The two swipes: [start] is a swipe to the right, [end] to the left. */
internal data class SwipeConfig(
    val start: SwipeAction = SwipeAction.Archive,
    val end: SwipeAction = SwipeAction.Delete,
) {
    companion object {
        fun fromCodes(start: String?, end: String?): SwipeConfig {
            val d = SwipeConfig()
            return SwipeConfig(
                SwipeAction.entries.firstOrNull { it.code == start } ?: d.start,
                SwipeAction.entries.firstOrNull { it.code == end } ?: d.end,
            )
        }
    }
}
