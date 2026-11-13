// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyItemScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PrimaryScrollableTabRow
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.distinctUntilChanged

private val Gutter = 16.dp

/** What the list screen asks of the app. */
internal class MailListActions(
    val onSearch: (String) -> Unit,
    val onEverywhere: (Boolean) -> Unit,
    val onFilter: (ListFilter) -> Unit,
    val onConversations: (Boolean) -> Unit,
    val onSplit: (String) -> Unit,
    val onRefresh: () -> Unit,
    /** The rows on screen are these, first to last. */
    val onRange: (Int, Int) -> Unit,
    val onTriage: (Triage, InboxRow) -> Unit,
    val onUndo: () -> Unit,
    val onNoticeShown: (Int) -> Unit,
    /** A row was tapped: open its message. */
    val onOpen: (InboxRow) -> Unit = {},
) {
    /** These actions with a tap on a row doing [open]. */
    fun withOpen(open: (InboxRow) -> Unit) = MailListActions(
        onSearch, onEverywhere, onFilter, onConversations, onSplit, onRefresh, onRange, onTriage, onUndo,
        onNoticeShown, open,
    )

    companion object {
        val None = MailListActions({}, {}, {}, {}, {}, {}, { _, _ -> }, { _, _ -> }, {}, {})
    }
}

/** Everything the list draws apart from [MailListUi]: the owner's words, the clock, settings and what to pick from. */
internal class MailListContext(
    val texts: Texts,
    val nowMillis: Long,
    val swipe: SwipeConfig,
    val accounts: List<AccountItem>,
    val categories: List<CategoryItem>,
    /** Senders' pictures by address (null: sender images are off and rows have no avatar). */
    val avatars: Map<String, ByteArray?>? = null,
)

/**
 * The Mail tab's list: a bar with the search and the funnel, the Inbox split's tabs, how fresh the
 * list is, and the rows. A row swipes (the two actions the owner chose) and a long press opens all
 * its actions; the list is pulled down to check for mail.
 */
@Composable
internal fun MailListScreen(
    context: MailListContext,
    ui: MailListUi,
    actions: MailListActions,
    modifier: Modifier = Modifier,
) {
    val texts = context.texts
    val query = ui.query
    val snackbar = remember { SnackbarHostState() }
    NoticeEffect(texts, ui.notice, snackbar, actions)
    Box(modifier = modifier.fillMaxSize()) {
        Column(modifier = Modifier.fillMaxSize()) {
            if (query != null) {
                MailListBar(texts, query, actions)
                SplitTabs(ui.tabs, query.split, texts, actions.onSplit)
            }
            freshnessLine(texts, context.nowMillis, ui.updatedAtMillis, ui.offline)?.let {
                Text(
                    it,
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(horizontal = Gutter, vertical = 4.dp),
                )
            }
            ListBody(context, ui, actions)
        }
        SnackbarHost(snackbar, modifier = Modifier.align(Alignment.BottomCenter))
    }
}

@Composable
private fun ListBody(context: MailListContext, ui: MailListUi, actions: MailListActions) {
    val texts = context.texts
    when {
        ui.load == LoadState.Failed -> Centered(texts.t("The mailbox could not be read."))
        ui.load == LoadState.Loading -> Centered(texts.t("Loading…"))
        else -> PullToRefreshBox(isRefreshing = ui.refreshing, onRefresh = actions.onRefresh) {
            if (ui.snapshot.total == 0) {
                Centered(
                    if (ui.query?.narrowed == true) texts.t("No messages match.") else texts.t("No messages."),
                )
            } else {
                Rows(context, ui, actions)
            }
        }
    }
}

@Composable
private fun Rows(context: MailListContext, ui: MailListUi, actions: MailListActions) {
    val texts = context.texts
    val state = rememberLazyListState()
    var menuFor by remember { mutableStateOf<InboxRow?>(null) }
    // The rows on screen are reported as they change, and again when the list is read afresh, so
    // that pages dropped for a newer revision are read when they are in view.
    LaunchedEffect(ui.query, ui.snapshot.rev, ui.snapshot.total) {
        snapshotFlow { state.firstVisibleItemIndex to (state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0) }
            .distinctUntilChanged()
            .collect { (first, last) -> actions.onRange(first, last) }
    }
    LaunchedEffect(ui.query) { state.scrollToItem(0) }
    LazyColumn(state = state, contentPadding = PaddingValues(bottom = Gutter), modifier = Modifier.fillMaxSize()) {
        items(
            count = ui.snapshot.total,
            key = { index -> ui.snapshot.row(index)?.id?.toString() ?: "page-$index" },
        ) { index ->
            val row = ui.snapshot.row(index)
            if (row == null) RowPlaceholder() else ListedRow(context, row, actions) { menuFor = row }
        }
    }
    menuFor?.let { row ->
        RowActionsSheet(context, row, actions.onTriage) { menuFor = null }
    }
}

@Composable
private fun LazyItemScope.ListedRow(
    context: MailListContext,
    row: InboxRow,
    actions: MailListActions,
    onMenu: () -> Unit,
) {
    Column(modifier = Modifier.animateItem()) {
        row.group?.let { GroupHeader(context.texts, it) }
        SwipeableRow(
            texts = context.texts,
            row = row,
            swipe = context.swipe,
            actions = RowActions(
                onTriage = { actions.onTriage(it, row) },
                onMenu = onMenu,
                onOpen = { actions.onOpen(row) },
            ),
            avatar = context.avatars?.let { AvatarState(it[PictureAddresses.sender(row.address).orEmpty()]) },
        )
        HorizontalDivider(modifier = Modifier.fillMaxWidth())
    }
}

@Composable
private fun SplitTabs(tabs: List<SplitTabItem>, selected: String, texts: Texts, onSplit: (String) -> Unit) {
    if (tabs.isEmpty()) return
    val at = tabs.indexOfFirst { it.code == selected }.takeIf { it >= 0 } ?: 0
    // A scrollable row of underlined tabs, with no scrollbar; each tab says its unread count.
    PrimaryScrollableTabRow(selectedTabIndex = at, edgePadding = Gutter) {
        tabs.forEachIndexed { index, tab ->
            Tab(
                selected = index == at,
                onClick = { onSplit(tab.code) },
                text = { Text(tabLabel(texts, tab), maxLines = 1) },
            )
        }
    }
}

/** A tab's name, with its unread count when it has any. */
internal fun tabLabel(texts: Texts, tab: SplitTabItem): String =
    if (tab.unread > 0) "${texts.t(tab.name)} ${tab.unread}" else texts.t(tab.name)

/** Shows each notice once, as a snackbar with an Undo button when the change can be taken back. */
@Composable
private fun NoticeEffect(texts: Texts, event: NoticeEvent?, host: SnackbarHostState, actions: MailListActions) {
    LaunchedEffect(event?.seq) {
        val shown = event ?: return@LaunchedEffect
        val result = host.showSnackbar(
            message = noticeText(texts, shown.notice),
            actionLabel = if (shown.notice.undoable) texts.t("Undo") else null,
            duration = if (shown.notice.undoable) SnackbarDuration.Long else SnackbarDuration.Short,
        )
        if (result == SnackbarResult.ActionPerformed) actions.onUndo()
        actions.onNoticeShown(shown.seq)
    }
}

/** The words of a notice. */
internal fun noticeText(texts: Texts, notice: Notice): String = when (notice) {
    Notice.NoAccount -> texts.t("No account is connected, so there is no mail to check for.")
    Notice.NotYet -> texts.t("Checking for new mail is not available in this version yet.")
    Notice.Failed -> texts.t("That did not work.")
    is Notice.Done -> doneText(texts, notice.triage, notice.count)
}

private fun doneText(texts: Texts, triage: Triage, count: Int): String {
    val n = count.toULong()
    return when (triage) {
        Triage.Archive -> texts.tn(n, "{n} message archived", "{n} messages archived")
        Triage.Delete -> texts.tn(n, "{n} message deleted", "{n} messages deleted")
        Triage.Junk -> texts.tn(n, "{n} message moved to Junk", "{n} messages moved to Junk")
        is Triage.MoveTo -> texts.tn(n, "{n} message moved", "{n} messages moved")
        is Triage.MarkRead, is Triage.Flag, is Triage.Category ->
            texts.tn(n, "{n} message changed", "{n} messages changed")
    }
}
