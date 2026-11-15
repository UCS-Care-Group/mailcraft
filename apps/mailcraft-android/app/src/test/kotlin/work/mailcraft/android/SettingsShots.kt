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

private const val GIB = 1024L * 1024 * 1024

/** The Settings pages with invented people (example.org and lumen.example); nothing here is anybody's. */
internal object SettingsShots {
    private val ada = AccountItem(1u, "Ada Example", "ada@example.org", listOf(FolderItem(1u, null, "Inbox", "inbox", 2u)))
    private val bo = AccountItem(2u, "Bo Lumen", "bo@lumen.example", listOf(FolderItem(3u, null, "Inbox", "inbox", 0u)))

    val ui = SettingsUi(
        switches = Switches(downloadPictures = false, senderImages = true, newMailOnly = true, primaryOnly = false),
        keepOffline = mapOf(1uL to "1m", 2uL to "6m"),
        storage = StorageUse(500, 212L * 1024 * 1024, 4_120, listOf(250, 500, 1024, 2048, 5120, 10240)),
        signatures = mapOf(
            1uL to SignatureState("Ada Example\nExample Org\nada@example.org", "Ada Example\nExample Org\nada@example.org"),
            2uL to SignatureState("", "Bo", editable = true),
        ),
    )

    private fun data(settings: SettingsUi = ui) = ShellData(
        accounts = listOf(ada, bo),
        folderName = "Inbox",
        folderId = 1uL,
        mail = InboxState.Loading,
        version = "0.12.0",
        chosenLanguage = "",
        settingsUi = settings,
    )

    @Composable
    private fun Themed(content: @Composable () -> Unit) {
        MailCraftTheme { Surface(modifier = Modifier.fillMaxSize()) { content() } }
    }

    @Composable
    fun Page(page: SettingsPage?, settings: SettingsUi = ui) = Themed {
        AppShell(
            EnglishTexts,
            data(settings),
            ShellActions(onFolder = { _, _ -> }, onLanguage = {}),
            initialNav = page?.let { NavState().openPage(it) } ?: NavState().openSettings(),
        )
    }

    @Composable
    fun RemoveQuestion(removal: Removal) = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) { RemoveDataCard(EnglishTexts, removal, SettingsActions()) }
    }

    @Composable
    fun RemovalNotice() = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) {
            RemovalNoticeCard(EnglishTexts, "The list of sign-ins kept on this phone was damaged, so some older sign-ins may still be kept on it. The phone's own settings for MailCraft let you clear what it keeps.", SettingsActions())
        }
    }

    @Composable
    fun Licences() = Themed {
        androidx.compose.foundation.layout.Column {
            AboutScreenPage(EnglishTexts, "0.12.0", SettingsActions(), startOnLicences = true)
        }
    }

    @Composable
    fun Space() = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) {
            SpaceCard(EnglishTexts, SpaceAsk(1u, "12m", SpaceWarning(480L * 1024 * 1024, 700L * 1024 * 1024)), SettingsActions())
        }
    }

    @Composable
    fun BigSpace() = Themed {
        Box(modifier = Modifier.fillMaxSize().padding(24.dp)) {
            SpaceCard(EnglishTexts, SpaceAsk(1u, null, SpaceWarning(3 * GIB, GIB + GIB / 2)), SettingsActions())
        }
    }
}
