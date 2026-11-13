// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import android.content.Intent
import android.net.Uri
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
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.IntentCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.Event

/**
 * The one window. An AppCompat activity, because that is what carries the per-app language to
 * Android 12 and before.
 */
class MainActivity : AppCompatActivity() {
    /** What another app asked for, until the window has taken it up. */
    private val incoming = mutableStateOf<Incoming?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // A window built again (turned, or its language changed) has already taken its intent up.
        if (savedInstanceState == null) incoming.value = incomingOf(intent)
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
                    Root(started, version, chosen, { incoming.value }, { incoming.value = null }) {
                        chosen = it
                        LanguageChoice.choose(it)
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        incoming.value = incomingOf(intent)
    }
}

/**
 * What an intent from another app asks for ([routeIncoming] decides; nothing here is believed): its action,
 * link, text, subject and the addresses of the files it carries. A bundle that can't be read is nothing.
 * An intent restored from the recent apps list is not asked for again.
 */
internal fun incomingOf(intent: Intent?): Incoming? {
    if (intent == null || intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return null
    return try {
        routeIncoming(
            action = intent.action,
            data = intent.dataString,
            text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString(),
            subject = intent.getCharSequenceExtra(Intent.EXTRA_SUBJECT)?.toString(),
            streams = streamsOf(intent).map { it.toString() },
        )
    } catch (_: RuntimeException) {
        // A hostile extra that will not unparcel: the intent is ignored.
        null
    }
}

/** The addresses of the files an intent carries: one for a send, a list for several. */
private fun streamsOf(intent: Intent): List<Uri> = when (intent.action) {
    Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
    Intent.ACTION_SEND_MULTIPLE ->
        IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
    else -> emptyList()
}

@Composable
internal fun Root(
    started: Started,
    version: String,
    chosenLanguage: String,
    incoming: () -> Incoming? = { null },
    onIncomingHandled: () -> Unit = {},
    onLanguage: (String) -> Unit,
) {
    when (started) {
        is Started.Ready -> CoreShell(started.core, version, chosenLanguage, incoming, onIncomingHandled, onLanguage)
        Started.Failed -> Centered(stringResource(R.string.start_failed))
    }
}

/** What reading the sample mailbox's outline came to; [outline] is null when it could not be read. */
private class Outlined(val outline: MailboxOutline?)

/** How often the clock and the network are looked at again: the "Updated N min ago" line follows it. */
private const val TICK_MILLIS = 30_000L

@Composable
private fun CoreShell(
    core: Core,
    version: String,
    chosenLanguage: String,
    incoming: () -> Incoming?,
    onIncomingHandled: () -> Unit,
    onLanguage: (String) -> Unit,
) {
    val texts = remember(core) { CoreTexts(core) }
    val source = remember(core) { CoreMailSource(core) }
    val controller = remember(core) { MailListController(source) }
    val hostContext = LocalContext.current
    val appContext = hostContext.applicationContext
    val prefs = remember { DeviceSwipePrefs(appContext) }
    val scope = rememberCoroutineScope()
    val reader = remember(core) { ReadingStack(core, appContext, scope) }
    val pictures = reader.pictures
    val writing = rememberComposeHost(core, texts, scope)
    TakeIncoming(incoming, onIncomingHandled, writing, appContext)
    val attachmentUi by reader.attachments.ui.collectAsState()
    val reading by pictures.reading.ui.collectAsState()
    val avatars by pictures.senderImages.images.collectAsState()
    val pictureSettings by pictures.settings.collectAsState()
    var outlined by remember { mutableStateOf<Outlined?>(null) }
    var swipe by remember { mutableStateOf(prefs.load()) }
    var query by rememberSaveable(stateSaver = MailQuery.Saver) { mutableStateOf(MailQuery(0uL)) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    val categories by produceState(emptyList<CategoryItem>(), core) { value = source.categories() }
    val ui by controller.ui.collectAsState()
    val outline = outlined?.outline
    val folderId = query.folder.takeIf { it != 0uL } ?: outline?.inboxId
    val folder = outline?.accounts?.flatMap { it.folders }?.firstOrNull { it.id == folderId }

    ShellEffects(core, controller, outline, folderId, query, { outlined = Outlined(it) }) { now = it }

    val mailActions = remember(controller, scope) {
        mailListActions(controller, scope, pictures.senderImages) { change -> query = change(query) }
    }
    val data = ShellData(
        accounts = outline?.accounts.orEmpty(),
        folderName = folder?.name ?: texts.t("Mail"),
        folderId = folderId,
        mail = if (outlined != null && folderId == null) ui.copy(load = LoadState.Failed) else ui,
        categories = categories,
        nowMillis = now,
        swipe = swipe,
        version = version,
        chosenLanguage = chosenLanguage,
        reading = reading,
        pictures = pictureSettings,
        avatars = if (pictureSettings.prefs.senderImages) avatars else null,
        attachments = attachmentUi,
        compose = writing.ui(),
        snack = writing.snack,
    )
    val signatures = remember(core) { CoreSignatures(core) }
    AppShell(
        texts = texts,
        data = data,
        actions = shellActions(reader, writing, mailActions, onLanguage, { query = MailQuery(it.id) }, signatures) {
            swipe = it
            prefs.save(it)
        },
    )
}

/** Starts the message another app asked for (a mailto: link or a share), once. */
@Composable
private fun TakeIncoming(incoming: () -> Incoming?, handled: () -> Unit, writing: ComposeHost, context: Context) {
    val shareReader = remember(context) { ResolverShareReader(context) }
    val asked = incoming()
    LaunchedEffect(asked) {
        if (asked != null) {
            handled()
            writing.beginIncoming(asked, shareReader, context.packageName + ".files")
        }
    }
}

/** What the shell asks of the app: the list, the reading pane, the composer, Settings. */
@Suppress("LongParameterList")
private fun shellActions(
    reader: ReadingStack,
    writing: ComposeHost,
    mail: MailListActions,
    onLanguage: (String) -> Unit,
    onFolder: (FolderItem) -> Unit,
    signatures: SignatureSource,
    onSwipe: (SwipeConfig) -> Unit,
) = ShellActions(
    onFolder = { _, selected -> onFolder(selected) },
    onLanguage = onLanguage,
    mail = mail,
    onSwipe = onSwipe,
    reading = reader.actions(writing::begin),
    onPictures = reader.pictures::changed,
    onClearPictures = reader.pictures::clear,
    composer = writing.actions(),
    onNewMessage = writing::newMessage,
    onSnackShown = writing::snackShown,
    signatures = signatures,
)

/** What runs beside the screen: reading the outline, the core's events, the clock and the list asked for. */
@Composable
private fun ShellEffects(
    core: Core,
    controller: MailListController,
    outline: MailboxOutline?,
    folderId: ULong?,
    query: MailQuery,
    onOutline: (MailboxOutline?) -> Unit,
    onNow: (Long) -> Unit,
) {
    LaunchedEffect(core) { onOutline(loadOutline(core)) }
    LaunchedEffect(core) { pumpEvents(core, controller) { onOutline(readOutline(core)) } }
    LaunchedEffect(core) { tick(controller, onNow) }
    LaunchedEffect(outline) { controller.accountsHaveServer(outline?.accounts?.any { it.server } == true) }
    LaunchedEffect(folderId, query) { if (folderId != null) controller.show(query.copy(folder = folderId)) }
}

/** Looks at the network and the clock now and every [TICK_MILLIS]. */
private suspend fun tick(controller: MailListController, setNow: (Long) -> Unit) {
    while (true) {
        controller.checkNetwork()
        setNow(System.currentTimeMillis())
        delay(TICK_MILLIS)
    }
}

/** Reads the core's events: a newer revision reads the list's pages again, a finished sync marks it fresh. */
private suspend fun pumpEvents(core: Core, controller: MailListController, outlineChanged: suspend () -> Unit) {
    while (true) {
        val event = core.nextEvent() ?: break
        when (event) {
            is Event.RevChanged -> {
                controller.revChanged(event.rev)
                outlineChanged()
            }
            is Event.SyncProgress -> if (event.total > 0uL && event.done >= event.total) controller.updated()
            else -> Unit
        }
    }
}

/** The list screen's actions: each changes what is asked for (the query) or runs on the controller. */
private fun mailListActions(
    controller: MailListController,
    scope: CoroutineScope,
    senderImages: SenderImages,
    change: ((MailQuery) -> MailQuery) -> Unit,
) = MailListActions(
    onSearch = { text -> change { it.copy(search = text.trim()) } },
    onEverywhere = { on -> change { it.copy(everywhere = on) } },
    onFilter = { filter -> change { it.copy(filter = filter) } },
    onConversations = { on -> change { it.copy(conversations = on) } },
    onSplit = { code -> change { it.copy(split = code) } },
    onRefresh = { scope.launch { controller.refresh() } },
    onRange = { first, last ->
        scope.launch { controller.showRange(first, last) }
        val rows = controller.ui.value.snapshot
        senderImages.onScreen((first..last).mapNotNull { rows.row(it) })
    },
    onTriage = { triage, row -> scope.launch { controller.triage(triage, listOf(row)) } },
    onUndo = { scope.launch { controller.undo() } },
    onNoticeShown = { seq -> controller.noticeShown(seq) },
)
