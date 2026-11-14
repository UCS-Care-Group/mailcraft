// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import uniffi.mailcraft_mobile.Core

/**
 * The one window. An AppCompat activity, because that is what carries the per-app language to
 * Android 12 and before.
 */
class MainActivity : AppCompatActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val application = application as? MailCraftApplication
        val started = application?.started ?: Started.Failed
        if (started is Started.Ready) {
            // The language in use is the owner's choice when there is one, else the system's; the
            // window is built again when it changes, so the words are read afresh.
            started.core.setLanguage(LanguageChoice.inUse(resources.configuration.locales))
            createChannels(applicationContext, channelSpecs(started.core::t))
        }
        val version = getString(R.string.app_version)
        setContent {
            MailCraftTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    var chosen by remember { mutableStateOf(LanguageChoice.current()) }
                    Root(started, version, chosen) {
                        chosen = it
                        LanguageChoice.choose(it)
                    }
                }
            }
        }
    }
}

@Composable
internal fun Root(started: Started, version: String, chosenLanguage: String, onLanguage: (String) -> Unit) {
    when (started) {
        is Started.Ready -> CoreShell(started.core, started.hub, version, chosenLanguage, onLanguage)
        Started.Failed -> Centered(stringResource(R.string.start_failed))
    }
}

/** What reading the mailbox's outline came to; [outline] is null when it could not be read. */
private class Outlined(val outline: MailboxOutline?)

/** The folder the Mail tab shows: the one the owner picked if it is still there, else the first Inbox. */
private fun chosenFolder(outline: MailboxOutline?, picked: Long?): FolderItem? {
    val all = outline?.accounts?.flatMap { it.folders }.orEmpty()
    val wanted = picked?.toULong()?.takeIf { id -> all.any { it.id == id } } ?: outline?.inboxId
    return all.firstOrNull { it.id == wanted }
}

/** The messages of [folder] for the Mail tab, or why there are none yet. */
private suspend fun loadMail(core: Core, outlined: Outlined?, folder: FolderItem?): InboxState = when {
    outlined == null -> InboxState.Loading
    outlined.outline == null -> InboxState.Failed(core.t("The mailbox could not be read."))
    folder == null -> InboxState.Loading
    else -> loadFolder(core, folder.id, folder.name)
}

@Composable
private fun CoreShell(
    core: Core,
    hub: AccountsHub,
    version: String,
    chosenLanguage: String,
    onLanguage: (String) -> Unit,
) {
    val texts = remember(core) { CoreTexts(core) }
    // The mailbox is opened once, when the application starts (`AccountsHub`), and not by a window.
    val restored by hub.restored.collectAsState()
    val reload by hub.reloads.collectAsState()
    val outlined by produceState<Outlined?>(null, core, restored, reload) {
        value = if (restored) Outlined(loadOutline(core)) else null
    }
    var picked by rememberSaveable { mutableStateOf<Long?>(null) }
    val folder = chosenFolder(outlined?.outline, picked)
    val mail by produceState<InboxState>(InboxState.Loading, core, outlined, folder) {
        // The last folder's messages must not stay under the new folder's name while it loads.
        value = InboxState.Loading
        value = loadMail(core, outlined, folder)
    }
    val accounts = outlined?.outline?.accounts.orEmpty()
    SideEffect { hub.showing(accounts) }
    val controller = hub.controller
    val accountsUi by controller.ui.collectAsState()
    AppShell(
        texts = texts,
        data = ShellData(
            accounts = accounts,
            folderName = folder?.name ?: texts.t("Mail"),
            folderId = folder?.id,
            mail = mail,
            version = version,
            chosenLanguage = chosenLanguage,
            accountsUi = accountsUi,
        ),
        actions = ShellActions(
            onFolder = { _, selected -> picked = selected.id.toLong() },
            onLanguage = onLanguage,
            accounts = controller.actions(),
        ),
    )
}

/** Puts a plain text on the clipboard; what can't be is left. */
internal fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("", text))
}
