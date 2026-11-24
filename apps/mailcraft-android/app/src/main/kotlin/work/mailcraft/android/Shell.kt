// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)
@file:Suppress("TooManyFunctions")

package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.VerticalDivider
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.ComposeStart

/**
 * Everything the shell draws, as plain values: it reads no core and no system service. A bag of
 * what the screens show, so it has a field for each.
 */
@Suppress("LongParameterList")
internal class ShellData(
    val accounts: List<AccountItem>,
    val folderName: String,
    val folderId: ULong?,
    val mail: MailListUi,
    val version: String,
    /** The language the owner chose in the app: its code, or "" for the system's. */
    val chosenLanguage: String,
    /** What the Today tab shows. */
    val today: TodayState = TodayState.Loading,
    /** What the People tab shows. */
    val people: PeopleData = PeopleData("", PeopleState.Loading),
    /** What the Tasks tab shows. */
    val tasks: TasksData = TasksData(TasksState.Loading),
    /** What the account screens draw besides the list of accounts. */
    val accountsUi: AccountsUi = AccountsUi(),
    /** What the Settings pages draw besides the accounts. */
    val settingsUi: SettingsUi = SettingsUi(),
    /** What reading the outline came to: no accounts means none only when it is [OutlineStatus.Ready]. */
    val outline: OutlineStatus = OutlineStatus.Ready,
    /** The categories mail can be given, with their colours. */
    val categories: List<CategoryItem> = emptyList(),
    /** The time now, in milliseconds since 1970: what "Updated N min ago" counts from. */
    val nowMillis: Long = 0L,
    /** The two swipes the owner chose on the mail list. */
    val swipe: SwipeConfig = SwipeConfig(),
    /** What the owner chose about reading: the light page in the dark theme, the header open or shut. */
    val readingPrefs: ReadingPrefs = ReadingPrefs(),
    /** The message being read, and how its pictures are coming. */
    val reading: ReadingUi = ReadingUi(),
    /** What the owner said about pictures, and what is kept of them. */
    val pictures: PicturesUi = PicturesUi(),
    /** Senders' pictures by row id, when sender images are on (null when off). */
    val avatars: Map<ULong, ByteArray>? = null,
    /** What the attachments of the open message put in front of the owner (a question, a picture, a notice). */
    val attachments: AttachmentUi = AttachmentUi(),
    /** The message being written, when there is one: the composer covers the app. */
    val compose: ComposeUi = ComposeUi(),
    /** A line to say once (a message queued, a draft kept). */
    val snack: String? = null,
)

/** Whether the mailbox's outline (its accounts and folders) has been read, and how that went. */
internal enum class OutlineStatus { Loading, Failed, Ready }

/** The list the Mail tab shows: an outline read with no folder to show has no list to wait for. */
internal fun shownMail(outlineRead: Boolean, folder: FolderItem?, ui: MailListUi): MailListUi =
    if (outlineRead && folder == null) ui.copy(load = LoadState.Failed) else ui

internal fun outlineStatus(outlineRead: Boolean, outline: MailboxOutline?): OutlineStatus = when {
    !outlineRead -> OutlineStatus.Loading
    outline == null -> OutlineStatus.Failed
    else -> OutlineStatus.Ready
}

/** What the Calendar tab draws and does, as plain values. */
internal class CalendarPane(
    val state: CalendarUiState,
    val actions: CalendarActions,
)

/** What the shell asks of the app. */
@Suppress("LongParameterList")
internal class ShellActions(
    val onFolder: (AccountItem, FolderItem) -> Unit,
    val onLanguage: (String) -> Unit,
    val accounts: AccountActions = AccountActions(),
    val settings: SettingsActions = SettingsActions(),
    val mail: MailListActions = MailListActions.None,
    val onSwipe: (SwipeConfig) -> Unit = {},
    val onReadingPrefs: (ReadingPrefs) -> Unit = {},
    val reading: ReadingActions = ReadingActions.None,
    val onPictures: (PicturePrefs) -> Unit = {},
    val onClearPictures: () -> Unit = {},
    val writing: WritingActions = WritingActions(),
    val organiser: OrganiserActions = OrganiserActions(),
)

/** What the People and Tasks tabs (and Today's cards that open them) ask of the app. */
internal class OrganiserActions(
    val people: PeopleActions = PeopleActions(),
    val tasks: TasksActions = TasksActions(),
)

/** What the shell asks of the message being written: the composer's own, a new message, a line said. */
internal class WritingActions(
    val composer: ComposerActions = ComposerActions.None,
    val onNewMessage: () -> Unit = {},
    val onSnackShown: () -> Unit = {},
)

/** The window is "expanded" from this width: the rail and the tablet layout. */
private val ExpandedWidth = 840.dp

/** Above this font scale the bottom bar keeps a label only for the tab that is on. */
private const val LARGE_TEXT = 1.3f

/**
 * The app's frame. Phone: a top bar with the ☰ button, the bottom bar of five tabs. Tablet (an
 * expanded window): the same top bar, and a rail of the five tabs down the side. On both the ☰
 * drawer holds the accounts with their folders, and Settings at its foot.
 */
@Composable
internal fun AppShell(
    texts: Texts,
    data: ShellData,
    actions: ShellActions,
    /** The Calendar tab, when there is a core to read it from. */
    calendar: CalendarPane? = null,
    initialNav: NavState = NavState(),
    initialDrawer: DrawerValue = DrawerValue.Closed,
    /** A tab something outside asked for (an .ics file was opened): the shell goes to it, then says so. */
    requestTab: Tab? = null,
    onTabRequested: () -> Unit = {},
) {
    // The composer covers the app and the app stays composed under it (where the owner was is kept); what
    // is under it is hidden from the screen reader while it is open.
    val composing = data.compose.open
    Box(modifier = Modifier.fillMaxSize()) {
        Box(modifier = if (composing) Modifier.clearAndSetSemantics {} else Modifier) {
            AppFrame(texts, data, actions, calendar, initialNav, initialDrawer, requestTab, onTabRequested)
        }
        if (composing) ComposerRoute(texts, data.compose, actions.writing.composer)
    }
}

@Composable
@Suppress("LongMethod", "LongParameterList")
private fun AppFrame(
    texts: Texts,
    data: ShellData,
    actions: ShellActions,
    calendar: CalendarPane?,
    initialNav: NavState,
    initialDrawer: DrawerValue,
    requestTab: Tab?,
    onTabRequested: () -> Unit,
) {
    val snackbar = remember { SnackbarHostState() }
    LaunchedEffect(data.snack) {
        data.snack?.let {
            snackbar.showSnackbar(it)
            actions.writing.onSnackShown()
        }
    }
    var nav by rememberSaveable(stateSaver = NavState.Saver) { mutableStateOf(initialNav) }
    LaunchedEffect(requestTab) {
        // (Add Account is left alone: a sign-in may be waiting there; the tab's notice stays for later.)
        requestTab?.let { if (!nav.adding) nav = nav.select(it) }
        if (requestTab != null) onTabRequested()
    }
    val drawer = rememberDrawerState(initialDrawer)
    val scope = rememberCoroutineScope()
    val back = nav.back()
    val adding = data.accountsUi.add
    // Leaving Add Account: a sign-in that is waiting is called off, and the form is as new.
    val leaveAdd = {
        if (adding is AddAccountState.Waiting) actions.accounts.onCancelSignIn()
        actions.accounts.onLeaveAdd()
        nav = nav.leaveAddAccount()
    }
    BackHandler(enabled = back != null) { if (nav.adding) leaveAdd() else back?.let { nav = it } }
    // An account is in: the screen closes.
    LaunchedEffect(adding) {
        if (adding is AddAccountState.Done) leaveAdd()
    }

    ModalNavigationDrawer(
        drawerState = drawer,
        gesturesEnabled = drawer.isOpen || !(nav.settings || nav.adding),
        drawerContent = {
            AppDrawer(
                texts = texts,
                accounts = data.accounts,
                selectedFolder = data.folderId.takeIf { !nav.settings && !nav.adding && nav.tab == Tab.Mail },
                onFolder = { account, folder ->
                    actions.onFolder(account, folder)
                    nav = nav.select(Tab.Mail)
                    scope.launch { drawer.close() }
                },
                onAddAccount = {
                    nav = nav.openAddAccount()
                    scope.launch { drawer.close() }
                },
                onSettings = {
                    nav = nav.openSettings()
                    scope.launch { drawer.close() }
                },
            )
        },
    ) {
        WithAppNotices(texts, data.accountsUi, actions.accounts, onRestart = { nav = nav.openAddAccount() }) {
            ShellScreen(
                texts = texts,
                data = data,
                actions = actions,
                calendar = calendar,
                nav = nav,
                onNav = { nav = it },
                onLeaveAdd = leaveAdd,
                onMenu = { scope.launch { drawer.open() } },
                snackbar = snackbar,
            )
        }
    }
    AccountDialogs(texts, data, actions.accounts)
    SpaceQuestion(texts, data, actions.settings)
}

/** The screen the navigation state says: Add Account, Settings, or a tab. */
@Composable
@Suppress("LongParameterList")
private fun ShellScreen(
    texts: Texts,
    data: ShellData,
    actions: ShellActions,
    calendar: CalendarPane?,
    nav: NavState,
    onNav: (NavState) -> Unit,
    onLeaveAdd: () -> Unit,
    onMenu: () -> Unit,
    snackbar: SnackbarHostState,
) {
    if (nav.adding) {
        AddAccountScreen(texts, data.accountsUi.add, actions.accounts, onBack = onLeaveAdd)
    } else if (nav.settings) {
        SettingsScreen(
            texts = texts,
            page = nav.page,
            data = data,
            actions = actions,
            onOpen = { onNav(nav.openPage(it)) },
            onBack = { nav.back()?.let(onNav) },
            onAddAccount = { onNav(nav.openAddAccount()) },
        )
    } else {
        MainFrame(
            texts = texts,
            tab = nav.tab,
            title = if (nav.tab == Tab.Mail) data.folderName else nav.tab.label(texts),
            onTab = { onNav(nav.select(it)) },
            onMenu = onMenu,
            onBack = if (nav.message != null) ({ nav.back()?.let(onNav) }) else null,
            snackbar = snackbar,
            onNewMessage = if (nav.tab == Tab.Mail && nav.message == null) actions.writing.onNewMessage else null,
            mailBar = if (nav.tab == Tab.Mail) data.mail.query?.let { MailBar(it, actions.mail) } else null,
            onDrafts = if (nav.tab == Tab.Mail && nav.message == null) {
                draftsOpener(data, actions, onNav, nav)
            } else {
                null
            },
        ) { modifier, expanded ->
            TabBody(texts, data, actions, calendar, nav, onNav, modifier, expanded)
        }
    }
}

/** What the tab on show draws. */
@Composable
@Suppress("LongParameterList")
private fun TabBody(
    texts: Texts,
    data: ShellData,
    actions: ShellActions,
    calendar: CalendarPane?,
    nav: NavState,
    onNav: (NavState) -> Unit,
    modifier: Modifier,
    expanded: Boolean,
) {
    when (nav.tab) {
        // (No account is said only after a read that worked and found none.)
        Tab.Mail -> if (data.accounts.isEmpty() && data.outline == OutlineStatus.Ready) {
            NoAccount(texts, onAdd = { onNav(nav.openAddAccount()) }, modifier = modifier)
        } else {
            val reply = { row: InboxRow ->
                actions.reading.onCompose(
                    if (isDraftRow(data.accounts, row)) ComposeStart.Draft(row.id) else ComposeStart.Reply(row.id),
                )
            }
            val opening = actions.mail.withOpen { row -> onNav(nav.openMessage(row.id)) }.withReply(reply)
            MailTab(texts, data, actions.copy(mail = opening), modifier, expanded, nav.messageId) {
                nav.back()?.let(onNav)
            }
        }
        Tab.Calendar -> calendar?.let { CalendarScreen(texts, it.state, it.actions, modifier) }
            ?: Centered(texts.t("Pending development"), modifier)
        Tab.Today -> TodayTab(texts, data, actions.organiser.tasks, modifier) { onNav(nav.select(it)) }
        Tab.People -> PeopleScreen(texts, data.people, actions.organiser.people, modifier)
        Tab.Tasks -> TasksScreen(texts, data.tasks, actions.organiser.tasks, modifier)
    }
}

/** The Today tab. Its cards open the tab of what they show. */
@Composable
private fun TodayTab(texts: Texts, data: ShellData, tasks: TasksActions, modifier: Modifier, go: (Tab) -> Unit) {
    TodayScreen(
        texts,
        data.today,
        TodayActions(
            onMail = { go(Tab.Mail) },
            onEvent = { go(Tab.Calendar) },
            onTask = {
                tasks.onOpen(it)
                go(Tab.Tasks)
            },
        ),
        modifier,
    )
}

/** The questions that float over whatever screen is showing: a permission to ask for, an account to remove. */
@Composable
private fun AccountDialogs(texts: Texts, data: ShellData, actions: AccountActions) {
    data.accountsUi.question?.let { question ->
        val account = data.accounts.firstOrNull { it.id == question.ask.account }
        PermissionQuestionDialog(texts, question, account?.email.orEmpty(), actions)
    }
    data.accountsUi.removing?.let { RemoveAccountDialog(texts, it, actions, data.accountsUi.removeFailed) }
}

/** The question that floats over whatever screen is showing while a sync waits for room. */
@Composable
private fun SpaceQuestion(texts: Texts, data: ShellData, actions: SettingsActions) {
    data.settingsUi.space?.let { SpaceDialog(texts, it, actions) }
    RemovalDialogs(texts, data.settingsUi.removal, actions)
}

private val FabGap = Arrangement.spacedBy(12.dp)

/** What the top bar holds on the Mail tab: the search and the funnel, over the list's query. */
private class MailBar(val query: MailQuery, val actions: MailListActions)

/**
 * Opens the Drafts folder of the account whose folder is on show (the first account when none is),
 * found by the folder's role; null when there is none, or it is the folder on show.
 */
private fun draftsOpener(
    data: ShellData,
    actions: ShellActions,
    onNav: (NavState) -> Unit,
    nav: NavState,
): (() -> Unit)? {
    val account = data.accounts.firstOrNull { a -> a.folders.any { it.id == data.folderId } }
        ?: data.accounts.firstOrNull()
    val drafts = account?.folders?.firstOrNull { it.kind == "drafts" } ?: return null
    if (drafts.id == data.folderId) return null
    return {
        actions.onFolder(account, drafts)
        onNav(nav.select(Tab.Mail))
    }
}

@Composable
@Suppress("LongMethod", "LongParameterList")
private fun MainFrame(
    texts: Texts,
    tab: Tab,
    title: String,
    onTab: (Tab) -> Unit,
    onMenu: () -> Unit,
    onBack: (() -> Unit)?,
    snackbar: SnackbarHostState,
    onNewMessage: (() -> Unit)?,
    mailBar: MailBar?,
    onDrafts: (() -> Unit)?,
    content: @Composable (Modifier, Boolean) -> Unit,
) {
    var searchOpen by rememberSaveable { mutableStateOf(false) }
    BoxWithConstraints(modifier = Modifier.fillMaxSize()) {
        val expanded = maxWidth >= ExpandedWidth
        val largeText = LocalDensity.current.fontScale > LARGE_TEXT
        Row(modifier = Modifier.fillMaxSize()) {
            if (expanded) {
                NavigationRail {
                    Tab.entries.forEach { entry ->
                        NavigationRailItem(
                            selected = entry == tab,
                            onClick = { onTab(entry) },
                            icon = { Icon(entry.icon(), contentDescription = null) },
                            label = { TabLabel(entry.label(texts)) },
                        )
                    }
                }
            }
            Scaffold(
                modifier = Modifier.weight(1f),
                topBar = {
                    // The list's search and funnel sit in the bar (not beside a message that is open on a phone).
                    val bar = mailBar?.takeIf { onBack == null || expanded }
                    val searching = bar != null && (searchOpen || bar.query.search.isNotBlank())
                    TopAppBar(
                        title = {
                            if (bar != null && searching) {
                                MailSearchField(texts, bar.query, bar.actions)
                            } else {
                                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        },
                        actions = {
                            if (bar != null) {
                                MailBarActions(texts, bar.query, bar.actions, searching, { searchOpen = it })
                            }
                        },
                        navigationIcon = {
                            if (onBack != null) {
                                IconButton(onClick = onBack) {
                                    Icon(AppIcons.Back, contentDescription = texts.t("Back"))
                                }
                            } else {
                                IconButton(onClick = onMenu) {
                                    Icon(AppIcons.Menu, contentDescription = texts.t("Menu"))
                                }
                            }
                        },
                    )
                },
                snackbarHost = { SnackbarHost(snackbar) },
                floatingActionButton = {
                    if (onNewMessage != null) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = FabGap) {
                            if (onDrafts != null) {
                                SmallFloatingActionButton(onClick = onDrafts) {
                                    Icon(ListIcons.Drafts, contentDescription = texts.t("Drafts"))
                                }
                            }
                            FloatingActionButton(onClick = onNewMessage) {
                                Icon(AppIcons.Plus, contentDescription = texts.t("New message"))
                            }
                        }
                    }
                },
                bottomBar = {
                    if (!expanded) {
                        NavigationBar {
                            Tab.entries.forEach { entry ->
                                NavigationBarItem(
                                    selected = entry == tab,
                                    onClick = { onTab(entry) },
                                    icon = { Icon(entry.icon(), contentDescription = null) },
                                    label = { TabLabel(entry.label(texts)) },
                                    alwaysShowLabel = !largeText,
                                )
                            }
                        }
                    }
                },
            ) { padding ->
                Box(modifier = Modifier.padding(padding)) { content(Modifier, expanded) }
            }
        }
    }
}

/** How wide the list is beside the reading pane, on a tablet. */
private val ListWidth = 400.dp

/**
 * The Mail tab. A phone shows the list, or the message that was opened on a screen of its own; an
 * expanded window shows the list beside the reading pane.
 */
@Composable
private fun MailTab(
    texts: Texts,
    data: ShellData,
    actions: ShellActions,
    modifier: Modifier,
    expanded: Boolean,
    messageId: ULong?,
    onLeave: () -> Unit,
) {
    val context = MailListContext(texts, data.nowMillis, data.swipe, data.accounts, data.categories, data.avatars)
    val tools = ReadingTools(context, actions.mail.onTriage, onLeave, data.readingPrefs, actions.onReadingPrefs)
    if (expanded) {
        Row(modifier = modifier.fillMaxSize()) {
            MailListScreen(context, data.mail, actions.mail, Modifier.width(ListWidth))
            VerticalDivider()
            if (messageId != null) {
                ReadingPane(
                    texts, messageId, data.reading, actions.reading, Modifier.weight(1f),
                    data.attachments, isDraft(data), tools,
                )
            } else {
                Centered(texts.t("Select a message to read it."), Modifier.weight(1f))
            }
        }
    } else if (messageId != null) {
        ReadingPane(texts, messageId, data.reading, actions.reading, modifier, data.attachments, isDraft(data), tools)
    } else {
        MailListScreen(context, data.mail, actions.mail, modifier)
    }
}

/** Whether the message in the reading pane is in a Drafts folder: it is continued, not answered. */
private fun isDraft(data: ShellData): Boolean {
    val folder = data.reading.opened?.view?.folder ?: return false
    return data.accounts.flatMap { it.folders }.firstOrNull { it.id == folder }?.kind == "drafts"
}

/** These actions with the list's own changed. */
private fun ShellActions.copy(mail: MailListActions) =
    ShellActions(
        onFolder, onLanguage, accounts, settings, mail, onSwipe, onReadingPrefs, reading, onPictures,
        onClearPictures, writing, organiser,
    )

@Composable
private fun TabLabel(text: String) {
    Text(text, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

private fun Tab.icon(): ImageVector = when (this) {
    Tab.Today -> AppIcons.Today
    Tab.Mail -> AppIcons.Mail
    Tab.Calendar -> AppIcons.Calendar
    Tab.People -> AppIcons.People
    Tab.Tasks -> AppIcons.Tasks
}
