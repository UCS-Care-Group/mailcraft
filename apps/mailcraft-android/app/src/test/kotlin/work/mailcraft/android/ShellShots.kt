// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import java.time.ZoneOffset
import java.util.Locale

/** Invented people and mail (example.org), so that screenshots never hold anything real. */
internal object ShellShots {
    private val rows = listOf(
        InboxRow(1u, "Ada Example", "Quarterly numbers", "The figures are attached; the second table needs a second look.", true, 1_730_000_000),
        InboxRow(2u, "Bo Example", "Lunch on Thursday?", "Same place as last time, if the weather holds.", true, 1_729_900_000),
        InboxRow(3u, "Cy Example", "Re: Garden plans", "I have moved the shed to the other corner and drawn it again.", false, 1_729_800_000),
        InboxRow(4u, "Di Example", "Your invoice", "Thank you for your order. The invoice is attached to this message.", false, 1_729_700_000),
        InboxRow(5u, "Ed Example", "A very long subject line that has to be cut off at the edge of the screen", "Short.", false, 1_729_600_000),
    )

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
        mail = InboxState.Ready(InboxModel("Inbox", "5 messages", rows)),
        version = "0.12.0",
        chosenLanguage = "",
    )

    val actions = ShellActions(onFolder = { _, _ -> }, onLanguage = {})

    @Composable
    fun Shell(nav: NavState = NavState(), drawer: DrawerValue = DrawerValue.Closed) {
        // Dates are drawn in one zone and language, whatever machine records or checks the shot.
        CompositionLocalProvider(LocalMessageZone provides ZoneOffset.UTC, LocalMessageLocale provides Locale.US) {
            MailCraftTheme {
                Surface {
                    AppShell(EnglishTexts, data, actions, initialNav = nav, initialDrawer = drawer)
                }
            }
        }
    }
}
