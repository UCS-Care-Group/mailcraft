// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable

/** Invented people and mail (example.org), so that screenshots never hold anything real. */
internal object ShellShots {
    val rows = listOf(
        InboxRow(
            1u, "Ada Example", "Quarterly numbers", "The figures are attached; the second table needs a second look.", true, 1_730_000_000,
            hasAttachments = true, group = "Today",
        ),
        InboxRow(
            2u, "Bo Example", "Lunch on Thursday?", "Same place as last time, if the weather holds.", true, 1_729_900_000,
            flagged = true, categories = listOf("Green category"),
        ),
        InboxRow(
            3u, "Cy Example", "Re: Garden plans", "I have moved the shed to the other corner and drawn it again.", false, 1_729_800_000,
            members = 3, group = "Last week",
        ),
        InboxRow(4u, "Di Example", "Your invoice", "Thank you for your order. The invoice is attached to this message.", false, 1_729_700_000),
        InboxRow(
            5u, "Ed Example", "A very long subject line that has to be cut off at the edge of the screen", "Short.", false, 1_729_600_000,
        ),
    )

    /** The clock the shots stand still at: five minutes after mail was last brought up to date. */
    const val NOW = 1_730_000_300_000L
    const val UPDATED = 1_730_000_000_000L

    val tabs = listOf(
        SplitTabItem("primary", "Primary", 2),
        SplitTabItem("social", "Social", 0),
        SplitTabItem("promotions", "Promotions", 14),
        SplitTabItem("updates", "Updates", 1),
        SplitTabItem("forums", "Forums", 0),
    )

    fun listUi(
        rows: List<InboxRow> = this.rows,
        query: MailQuery = MailQuery(1uL),
        tabs: List<SplitTabItem> = emptyList(),
        load: LoadState = LoadState.Ready,
        offline: Boolean = false,
    ) = MailListUi(
        query = query,
        load = load,
        snapshot = ListSnapshot(1uL, rows.size).with(RowsWindow(1uL, rows.size, 0, rows)) ?: ListSnapshot.Empty,
        tabs = tabs,
        refreshing = false,
        notice = null,
        updatedAtMillis = UPDATED,
        offline = offline,
    )

    val categories = listOf(CategoryItem("Green category", 0x2E8B57), CategoryItem("Red category", 0xD13B3B))

    private val folders = listOf(
        FolderItem(1u, null, "Inbox", "inbox", 2u),
        FolderItem(2u, null, "Drafts", "drafts", 0u),
        FolderItem(3u, null, "Sent Items", "sent", 0u),
        FolderItem(4u, null, "Projects", "other", 14u),
        FolderItem(5u, 4u, "Garden", "other", 0u, depth = 1),
        FolderItem(6u, null, "Deleted Items", "deleted", 0u),
    )

    private val accounts = listOf(
        AccountItem(1u, "Ada Example", "ada@example.org", folders),
        AccountItem(2u, "Shared archive", "archive@example.org", listOf(FolderItem(7u, null, "Inbox", "inbox", 0u))),
    )

    val data = ShellData(
        accounts = accounts,
        folderName = "Inbox",
        folderId = 1uL,
        mail = listUi(),
        categories = categories,
        nowMillis = NOW,
        swipe = SwipeConfig(),
        version = "0.12.0",
        chosenLanguage = "",
    )

    val actions = ShellActions(onFolder = { _, _ -> }, onLanguage = {}, mail = MailListActions.None, onSwipe = {})

    @Composable
    fun Shell(nav: NavState = NavState(), drawer: DrawerValue = DrawerValue.Closed, with: ShellData = data) {
        MailCraftTheme {
            Surface {
                AppShell(EnglishTexts, with, actions, initialNav = nav, initialDrawer = drawer)
            }
        }
    }
}

/** The same data with another list. */
internal fun ShellData.withMail(ui: MailListUi) = copy(mail = ui)
