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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import java.io.File
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.Event

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
        is Started.Ready -> CoreShell(started.core, version, chosenLanguage, onLanguage)
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
private fun CoreShell(core: Core, version: String, chosenLanguage: String, onLanguage: (String) -> Unit) {
    val context = LocalContext.current
    val texts = remember(core) { CoreTexts(core) }
    val store = remember(context) { MailboxFile(File(context.noBackupFilesDir, "mailbox")) }
    // Bumped when the mailbox's accounts changed: the outline is read again.
    var reload by remember { mutableIntStateOf(0) }
    val outlined by produceState<Outlined?>(null, core, reload) {
        if (reload == 0) store.restore(core)
        value = Outlined(loadOutline(core))
    }
    var picked by rememberSaveable { mutableStateOf<Long?>(null) }
    val folder = chosenFolder(outlined?.outline, picked)
    val mail by produceState<InboxState>(InboxState.Loading, core, outlined, folder) {
        value = loadMail(core, outlined, folder)
    }
    val accounts = outlined?.outline?.accounts.orEmpty()
    val controller = rememberAccountsController(core, texts, accounts) {
        store.keep(core)
        reload++
    }
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

/**
 * The controller of the account screens over [core], and the loop that hands it what the core asks of
 * the owner (a permission question, a sign-in that is over). [changed] runs after the accounts changed.
 */
@Composable
private fun rememberAccountsController(
    core: Core,
    texts: Texts,
    accounts: List<AccountItem>,
    changed: suspend () -> Unit,
): AccountsController {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val latest by rememberUpdatedState(accounts)
    val onChanged by rememberUpdatedState(changed)
    val handback = (context.applicationContext as? MailCraftApplication)?.handback ?: SignInHandback()
    val controller = remember(core) {
        AccountsController(
            api = CoreAccounts(core, handback),
            scope = scope,
            texts = texts,
            accounts = { latest },
            changed = { onChanged() },
            copy = { text -> copyToClipboard(context, text) },
        )
    }
    // (Revision changes and the rest of the stream are for the screens that read them.)
    LaunchedEffect(core, controller) {
        while (true) {
            when (val event = core.nextEvent() ?: break) {
                is Event.PermissionQuestion -> controller.askFor(event.account, PermissionKind.of(event.permission))
                is Event.SignInNeeded -> controller.signInNeeded(event.account)
                else -> Unit
            }
        }
    }
    return controller
}

/** Puts a plain text on the clipboard; what can't be is left. */
private fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("", text))
}
