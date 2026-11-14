// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
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
        is Started.Ready -> CoreShell(started.core, version, chosenLanguage, onLanguage)
        Started.Failed -> Centered(stringResource(R.string.start_failed))
    }
}

/** What reading the sample mailbox's outline came to; [outline] is null when it could not be read. */
private class Outlined(val outline: MailboxOutline?)

@Composable
private fun CoreShell(core: Core, version: String, chosenLanguage: String, onLanguage: (String) -> Unit) {
    val texts = remember(core) { CoreTexts(core) }
    val outlined by produceState<Outlined?>(null, core) { value = Outlined(loadOutline(core)) }
    var picked by rememberSaveable { mutableStateOf<Long?>(null) }
    val outline = outlined?.outline
    val folderId = picked?.toULong() ?: outline?.inboxId
    val folder = outline?.accounts?.flatMap { it.folders }?.firstOrNull { it.id == folderId }
    val mail by produceState<InboxState>(InboxState.Loading, core, outlined, folderId) {
        // The last folder's messages must not stay under the new folder's name while it loads.
        value = InboxState.Loading
        value = when {
            outlined == null -> InboxState.Loading
            outline == null -> InboxState.Failed(core.t("The mailbox could not be read."))
            folder == null || folderId == null -> InboxState.Failed(core.t("The sample mailbox has no Inbox."))
            else -> loadFolder(core, folderId, folder.name)
        }
    }
    val data = ShellData(
        accounts = outline?.accounts.orEmpty(),
        folderName = folder?.name ?: texts.t("Mail"),
        folderId = folderId,
        mail = mail,
        version = version,
        chosenLanguage = chosenLanguage,
    )
    AppShell(
        texts = texts,
        data = data,
        actions = ShellActions(onFolder = { _, selected -> picked = selected.id.toLong() }, onLanguage = onLanguage),
    )
}
