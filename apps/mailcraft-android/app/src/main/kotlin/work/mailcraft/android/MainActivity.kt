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
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import androidx.lifecycle.ViewModelProvider
import java.time.ZoneId
import java.util.Locale
import java.util.TimeZone
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core

/**
 * The one window. An AppCompat activity, because that is what carries the per-app language to
 * Android 12 and before.
 */
class MainActivity : AppCompatActivity() {
    /** What outlives this window when Android makes it again: a file handed over, the calendar's editor. */
    private val window: WindowModel by lazy { ViewModelProvider(this)[WindowModel::class.java] }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // The intent this window is for from now on, so that a window made again reads this one.
        setIntent(intent)
        takeHandedOver(intent)
    }

    /** Reads the calendar file [intent] carries, if it carries one, off the main thread. */
    private fun takeHandedOver(intent: Intent) {
        val resolver = applicationContext.contentResolver
        window.handoff.take { readIcs(resolver, intent) }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // A new window reads what it was opened with; a window made again after the app was put away
        // reads it again only when the file it carried was never taken in.
        if (savedInstanceState == null || window.handoff.readAgain) takeHandedOver(intent)
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
                        val handedOver by window.handoff.file.collectAsState()
                        Root(started, version, chosen, handedOver, window.handoff::handled, window) {
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
    window: WindowModel? = null,
    onLanguage: (String) -> Unit,
) {
    when (started) {
        is Phase.Ready -> CoreShell(started.value, version, chosenLanguage, handedOver, onHandled, window, onLanguage)
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

/** How often the clock and the network are looked at again: the "Updated N min ago" line follows it. */
private const val TICK_MILLIS = 30_000L

/** The folder the Mail tab shows: the one the owner picked if it is still there, else the first Inbox. */
private fun chosenFolder(outline: MailboxOutline?, picked: ULong): FolderItem? {
    val all = outline?.accounts?.flatMap { it.folders }.orEmpty()
    val wanted = picked.takeIf { id -> id != 0uL && all.any { it.id == id } } ?: outline?.inboxId
    return all.firstOrNull { it.id == wanted }
}

@Composable
private fun CoreShell(
    opened: Opened,
    version: String,
    chosenLanguage: String,
    handedOver: IcsRead?,
    onHandled: () -> Unit,
    window: WindowModel?,
    onLanguage: (String) -> Unit,
) {
    val (core, hub, list) = Triple(opened.core, opened.hub, opened.mail)
    val texts = remember(core) { CoreTexts(core) }
    // The mailbox was opened with the core, when the application started, and not by a window. It is
    // read again whenever it changed (`AccountsHub.reloads`: an account's work, or a revision the core
    // announced, a synchronisation's among them). The list reads its own pages again on a revision
    // (`MailListController.handle`), only those on screen.
    val reload by hub.reloads.collectAsState()
    val (outlined, retryOutline) = rememberOutline(core, reload)
    val categories by produceState(emptyList<CategoryItem>(), list, reload) { value = list.categories() }
    val appContext = LocalContext.current.applicationContext
    val prefs = remember { DeviceSwipePrefs(appContext) }
    var swipe by remember { mutableStateOf(prefs.load()) }
    // (A folder of 0 is the first Inbox.)
    var query by rememberSaveable(stateSaver = MailQuery.Saver) { mutableStateOf(MailQuery(0uL)) }
    val ui by list.ui.collectAsState()
    val outline = outlined?.outline
    val folder = chosenFolder(outline, query.folder)
    val accounts = outline?.accounts.orEmpty()
    val now = rememberTicks(list)
    LaunchedEffect(folder?.id, query) { folder?.let { list.show(query.copy(folder = it.id)) } }
    val calendar = rememberCalendarTab(core, texts, window, reload, outlined != null, handedOver, onHandled)
    SideEffect { hub.showing(accounts) }
    // The right to notify is asked for once there is an account to bring mail, never at a first start.
    AskForNotificationsOnce(hasAccount = accounts.isNotEmpty()) {
        (appContext as? MailCraftApplication)?.followNotificationSwitch()
    }
    val accountsUi by hub.controller.ui.collectAsState()
    val scope = rememberCoroutineScope()
    val (readingUi, readingActions) = rememberReading(core, list, scope)
    val mailActions = remember(list, scope, hub) {
        mailListActions(list, scope, retryOutline, hub::syncNow) { change -> query = change(query) }
    }
    AppShell(
        texts = texts,
        data = ShellData(
            accounts = accounts,
            folderName = folder?.name ?: texts.t("Mail"),
            folderId = folder?.id,
            mail = shownMail(outlined != null, folder, ui),
            version = version,
            chosenLanguage = chosenLanguage,
            accountsUi = accountsUi,
            outline = outlineStatus(outlined != null, outline),
            categories = categories,
            nowMillis = now,
            swipe = swipe,
            reading = readingUi,
        ),
        actions = ShellActions(
            onFolder = { _, selected -> query = MailQuery(selected.id) },
            onLanguage = onLanguage,
            accounts = hub.controller.actions(),
            mail = mailActions,
            onSwipe = {
                swipe = it
                prefs.save(it)
            },
            reading = readingActions,
        ),
        calendar = calendar.pane,
        requestTab = calendar.requestTab, onTabRequested = calendar.onTabRequested,
    )
}

/**
 * The message being read is this window's own; marking it read goes through the list, which makes the
 * owner's commands in turn, so that an undo it offers is never taken by the mark.
 */
@Composable
private fun rememberReading(
    core: Core,
    list: MailListController,
    scope: CoroutineScope,
): Pair<ReadingUi, ReadingActions> {
    val reading = remember(core, list) { ReadingController(CoreReadSource(core, list::markRead), scope) }
    val links = remember(core) { CoreLinks(core) }
    val ui by reading.ui.collectAsState()
    return ui to ReadingActions(onOpen = reading::open, onClose = reading::close, onLink = links::open)
}

/**
 * The mailbox's outline, read when [reload] moves on, and what reads it again when it could not be read
 * (a pull to refresh: without an outline the list has no query of its own to refresh).
 */
@Composable
private fun rememberOutline(core: Core, reload: Int): Pair<Outlined?, () -> Unit> {
    var tries by remember { mutableIntStateOf(0) }
    val outlined by produceState<Outlined?>(null, core, reload, tries) {
        value = Outlined(loadOutline(core))
    }
    return outlined to { if (outlined?.let { it.outline == null } == true) tries++ }
}

/** The time now, read again every [TICK_MILLIS] together with what the device says of the network. */
@Composable
private fun rememberTicks(list: MailListController): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(list) {
        while (true) {
            list.checkNetwork()
            now = System.currentTimeMillis()
            delay(TICK_MILLIS)
        }
    }
    return now
}

/** The list screen's actions: each changes what is asked for (the query) or runs on the controller. */
private fun mailListActions(
    controller: MailListController,
    scope: CoroutineScope,
    /** Reads the mailbox's outline again when it could not be read: without it the list has no query to refresh. */
    retryOutline: () -> Unit,
    /** Send/Receive on the owner's word ([AccountsHub.syncNow]): the one way a pull to refresh asks for mail. */
    syncNow: () -> Unit,
    change: ((MailQuery) -> MailQuery) -> Unit,
) = MailListActions(
    onSearch = { text -> change { it.copy(search = text.trim()) } },
    onEverywhere = { on -> change { it.copy(everywhere = on) } },
    onFilter = { filter -> change { it.copy(filter = filter) } },
    onConversations = { on -> change { it.copy(conversations = on) } },
    onSplit = { code -> change { it.copy(split = code) } },
    onRefresh = {
        retryOutline()
        syncNow()
        // (The pages on screen are read again now too; the round's own changes come as revisions.)
        scope.launch { controller.reload() }
    },
    onRange = { first, last -> scope.launch { controller.showRange(first, last) } },
    onTriage = { triage, row -> scope.launch { controller.triage(triage, listOf(row)) } },
    onUndo = { command -> scope.launch { controller.undo(command) } },
    onNoticeShown = { seq -> controller.noticeShown(seq) },
)

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
