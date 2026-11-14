// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import java.time.ZoneId
import java.util.Locale
import java.util.TimeZone
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
                DeviceDateSettings {
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

/**
 * Gives message dates the device's current language and time zone. The language comes from the
 * configuration, so a change of it rebuilds the dates; a time-zone change is no configuration change,
 * so a broadcast for it refreshes the zone.
 */
@Composable
internal fun DeviceDateSettings(content: @Composable () -> Unit) {
    val context = LocalContext.current
    val locale = LocalConfiguration.current.locales.get(0) ?: Locale.getDefault()
    var zone by remember { mutableStateOf(ZoneId.systemDefault()) }
    DisposableEffect(context) {
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context?, intent: Intent?) {
                // The JVM caches its default zone; the platform's change is what the receiver heard.
                TimeZone.setDefault(null)
                zone = ZoneId.systemDefault()
            }
        }
        ContextCompat.registerReceiver(
            context,
            receiver,
            IntentFilter(Intent.ACTION_TIMEZONE_CHANGED),
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        onDispose { context.unregisterReceiver(receiver) }
    }
    CompositionLocalProvider(LocalMessageZone provides zone, LocalMessageLocale provides locale, content = content)
}
