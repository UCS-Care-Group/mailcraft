// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.BroadcastReceiver
import android.content.Intent
import android.content.IntentFilter
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import java.time.LocalDateTime
import java.time.ZoneId
import java.util.Locale
import java.util.TimeZone
import kotlinx.coroutines.flow.MutableStateFlow
import uniffi.mailcraft_mobile.Core

/**
 * The one window. An AppCompat activity, because that is what carries the per-app language to
 * Android 12 and before.
 */
class MainActivity : AppCompatActivity() {
    /** A calendar file another app opened with us, waiting for the window to take it in. */
    private val handedOver = mutableStateOf<IcsRead?>(null)

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handedOver.value = readIcs(contentResolver, intent).takeIf { it !is IcsRead.NotOurs }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) {
            handedOver.value = readIcs(contentResolver, intent).takeIf { it !is IcsRead.NotOurs }
        }
        enableEdgeToEdge()
        val application = application as? MailCraftApplication
        application?.begin()
        val starting = application?.started ?: MutableStateFlow(Phase.Failed)
        val version = getString(R.string.app_version)
        setContent {
            // What the window shows: the core's phase, passed on once a ready core has this window's
            // language (the core opens off the main thread; until then the window says it is opening).
            var started by remember { mutableStateOf<Started>(Phase.Loading) }
            LaunchedEffect(starting) {
                starting.collect { phase ->
                    if (phase is Phase.Ready) {
                        // The language in use is the owner's choice when there is one, else the system's;
                        // the window is built again when it changes, so the words are read afresh.
                        phase.value.core.setLanguage(LanguageChoice.inUse(resources.configuration.locales))
                        createChannels(applicationContext, channelSpecs(phase.value.core::t))
                    }
                    started = phase
                }
            }
            MailCraftTheme {
                DeviceDateSettings {
                    Surface(modifier = Modifier.fillMaxSize()) {
                        var chosen by remember { mutableStateOf(LanguageChoice.current()) }
                        Root(started, version, chosen, handedOver.value, { handedOver.value = null }) {
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
internal fun Root(
    started: Started,
    version: String,
    chosenLanguage: String,
    handedOver: IcsRead? = null,
    onHandled: () -> Unit = {},
    onLanguage: (String) -> Unit,
) {
    when (started) {
        is Phase.Ready -> {
            val ready = started.value
            CoreShell(ready.core, ready.hub, version, chosenLanguage, handedOver, onHandled, onLanguage)
        }
        Phase.Loading -> Opening()
        Phase.Failed -> Centered(stringResource(R.string.start_failed))
    }
}

/** The window while the core opens the mailbox, before any of its words can be asked for. */
@Composable
internal fun Opening() {
    Centered(stringResource(R.string.starting))
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
    handedOver: IcsRead?,
    onHandled: () -> Unit,
    onLanguage: (String) -> Unit,
) {
    val texts = remember(core) { CoreTexts(core) }
    // The mailbox was opened with the core, when the application started, and not by a window. It is
    // read again whenever it changed (`AccountsHub.reloads`: an account's work, or a revision the core
    // announced, a synchronisation's among them), and so are the messages of the folder on screen.
    val reload by hub.reloads.collectAsState()
    val outlined by produceState<Outlined?>(null, core, reload) {
        value = Outlined(loadOutline(core))
    }
    var picked by rememberSaveable { mutableStateOf<Long?>(null) }
    val folder = chosenFolder(outlined?.outline, picked)
    val shownFolder = remember { arrayOfNulls<ULong>(1) }
    val mail by produceState<InboxState>(InboxState.Loading, core, outlined, folder) {
        // The last folder's messages must not stay under the new folder's name while it loads; the same
        // folder read again keeps what it shows until the new list is in.
        if (folder?.id != shownFolder[0]) value = InboxState.Loading
        shownFolder[0] = folder?.id
        value = loadMail(core, outlined, folder)
    }
    val calendar = rememberCalendar(core, texts)
    val calendarState by calendar.state.collectAsState()
    // The calendar is read again when the mailbox changed (a sync, an account, a command of its own).
    LaunchedEffect(calendar, reload, LocalMessageZone.current) { calendar.refresh() }
    var requestTab by remember { mutableStateOf<Tab?>(null) }
    // A calendar file is taken in once the mailbox is there to take it.
    LaunchedEffect(handedOver, outlined) {
        val read = handedOver
        if (read != null && outlined != null) {
            when (read) {
                is IcsRead.Got -> calendar.importIcs(read.file.name, read.file.bytes)
                else -> calendar.report(texts.t("That file could not be read as a calendar."))
            }
            requestTab = Tab.Calendar
            onHandled()
        }
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
            outline = when {
                outlined == null -> OutlineStatus.Loading
                outlined?.outline == null -> OutlineStatus.Failed
                else -> OutlineStatus.Ready
            },
        ),
        actions = ShellActions(
            onFolder = { _, selected -> picked = selected.id.toLong() },
            onLanguage = onLanguage,
            accounts = controller.actions(),
        ),
        calendar = CalendarPane(calendarState, CalendarActions.of(calendar)),
        requestTab = requestTab,
        onTabRequested = { requestTab = null },
    )
}

/**
 * The Calendar tab's controller for this window. Its dates are the phone's zone and language
 * ([DeviceDateSettings]); the core shows its calendar in the phone's zone by itself.
 */
@Composable
private fun rememberCalendar(core: Core, texts: Texts): CalendarController {
    val zone by rememberUpdatedState(LocalMessageZone.current)
    val locale by rememberUpdatedState(LocalMessageLocale.current)
    val scope = rememberCoroutineScope()
    return remember(core, texts) {
        CalendarController(
            backend = CoreCalendarBackend(core, texts),
            scope = scope,
            zone = { zone },
            weekStart = { weekStartOf(locale) },
            now = { LocalDateTime.now(zone) },
            texts = texts,
        )
    }
}

/** Puts a plain text on the clipboard; what can't be is left. */
internal fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java) ?: return
    clipboard.setPrimaryClip(ClipData.newPlainText("", text))
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
