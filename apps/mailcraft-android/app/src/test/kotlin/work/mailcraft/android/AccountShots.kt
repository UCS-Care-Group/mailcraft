// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/** The account screens with invented people (example.org and lumen.example); nothing here is anybody's. */
internal object AccountShots {
    private val folders = listOf(FolderItem(1u, null, "Inbox", "inbox", 2u), FolderItem(2u, null, "Sent Items", "sent", 0u))

    private val ada = AccountItem(1u, "Ada Example", "ada@example.org", folders)
    private val bo = AccountItem(2u, "Bo Lumen", "bo@lumen.example", listOf(FolderItem(3u, null, "Inbox", "inbox", 0u)))

    private val statuses = mapOf(
        StatusKey(1u, PermissionKind.Tasks) to PermissionStatus.Allowed,
        StatusKey(1u, PermissionKind.SharedMail) to PermissionStatus.NotAllowed,
        StatusKey(1u, PermissionKind.SharedSend) to PermissionStatus.Checking,
        StatusKey(2u, PermissionKind.Tasks) to PermissionStatus.SignInNeeded,
        StatusKey(2u, PermissionKind.SharedMail) to PermissionStatus.Unknown,
        StatusKey(2u, PermissionKind.SharedSend) to PermissionStatus.NotAllowed,
    )

    private const val LINK = "https://login.example.org/lumen.example/v2.0/adminconsent?client_id=00000000-0000-0000-0000-000000000000&scope=example"

    private val none = AccountActions()

    private fun data(ui: AccountsUi, accounts: List<AccountItem> = listOf(ada, bo)) = ShellData(
        accounts = accounts,
        folderName = "Inbox",
        folderId = 1uL,
        mail = InboxState.Loading,
        version = "0.12.0",
        chosenLanguage = "",
        accountsUi = ui,
    )

    @Composable
    private fun Themed(content: @Composable () -> Unit) {
        MailCraftTheme { Surface(modifier = Modifier.fillMaxSize()) { content() } }
    }

    @Composable
    fun AddForm(notice: Notice? = null) = Themed {
        AddAccountScreen(EnglishTexts, AddAccountState.Form("ada@lumen.example", notice), none, onBack = {})
    }

    @Composable
    fun Waiting() = Themed { AddAccountScreen(EnglishTexts, AddAccountState.Waiting("ada@lumen.example"), none, onBack = {}) }

    @Composable
    fun Approval(copied: Boolean = false) = Themed {
        AddAccountScreen(EnglishTexts, AddAccountState.Approval("ada@lumen.example", LINK, copied), none, onBack = {})
    }

    @Composable
    fun Question(state: QuestionState) = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) {
            PermissionQuestionCard(EnglishTexts, state, "ada@example.org", none)
        }
    }

    @Composable
    fun Remove() = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) { RemoveAccountCard(EnglishTexts, ada, none) }
    }

    @Composable
    fun Accounts() = Themed {
        AppShell(
            EnglishTexts,
            data(AccountsUi(statuses = statuses)),
            ShellActions(onFolder = { _, _ -> }, onLanguage = {}),
            initialNav = NavState().openPage(SettingsPage.Accounts),
        )
    }

    @Composable
    fun SettingsRows() = Themed {
        AppShell(
            EnglishTexts,
            data(AccountsUi()),
            ShellActions(onFolder = { _, _ -> }, onLanguage = {}),
            initialNav = NavState().openSettings(),
        )
    }

    @Composable
    fun NoAccountMail() = Themed {
        AppShell(
            EnglishTexts,
            data(AccountsUi(), accounts = emptyList()).let { ShellData(emptyList(), "Mail", null, InboxState.Loading, it.version, "") },
            ShellActions(onFolder = { _, _ -> }, onLanguage = {}),
        )
    }
}
