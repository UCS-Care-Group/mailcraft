// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalMaterial3Api::class)

package work.mailcraft.android

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.VerticalDivider
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

/** Everything the shell draws, as plain values: it reads no core and no system service. */
internal data class ShellData(
    val accounts: List<AccountItem>,
    val folderName: String,
    val folderId: ULong?,
    val mail: MailListUi,
    val categories: List<CategoryItem>,
    /** The time now, in milliseconds since 1970: what "Updated N min ago" counts from. */
    val nowMillis: Long,
    /** The two swipes the owner chose on the mail list. */
    val swipe: SwipeConfig,
    val version: String,
    /** The language the owner chose in the app: its code, or "" for the system's. */
    val chosenLanguage: String,
    /** The message being read, and how its pictures are coming. */
    val reading: ReadingUi = ReadingUi(),
    /** What the owner said about pictures, and what is kept of them. */
    val pictures: PicturesUi = PicturesUi(),
    /** Senders' pictures by row id, when sender images are on (null when off). */
    val avatars: Map<ULong, ByteArray>? = null,
)

/** What the shell asks of the app. */
internal class ShellActions(
    val onFolder: (AccountItem, FolderItem) -> Unit,
    val onLanguage: (String) -> Unit,
    val mail: MailListActions,
    val onSwipe: (SwipeConfig) -> Unit,
    val reading: ReadingActions = ReadingActions.None,
    val onPictures: (PicturePrefs) -> Unit = {},
    val onClearPictures: () -> Unit = {},
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
    initialNav: NavState = NavState(),
    initialDrawer: DrawerValue = DrawerValue.Closed,
) {
    var nav by rememberSaveable(stateSaver = NavState.Saver) { mutableStateOf(initialNav) }
    val drawer = rememberDrawerState(initialDrawer)
    val scope = rememberCoroutineScope()
    val back = nav.back()
    BackHandler(enabled = back != null) { back?.let { nav = it } }

    ModalNavigationDrawer(
        drawerState = drawer,
        gesturesEnabled = drawer.isOpen || !nav.settings,
        drawerContent = {
            AppDrawer(
                texts = texts,
                accounts = data.accounts,
                selectedFolder = data.folderId.takeIf { !nav.settings && nav.tab == Tab.Mail },
                onFolder = { account, folder ->
                    actions.onFolder(account, folder)
                    nav = nav.select(Tab.Mail)
                    scope.launch { drawer.close() }
                },
                onSettings = {
                    nav = nav.openSettings()
                    scope.launch { drawer.close() }
                },
            )
        },
    ) {
        if (nav.settings) {
            SettingsScreen(
                texts = texts,
                page = nav.page,
                data = data,
                onOpen = { nav = nav.openPage(it) },
                onBack = { back?.let { previous -> nav = previous } },
                onLanguage = actions.onLanguage,
                onSwipe = actions.onSwipe,
                onPictures = actions.onPictures,
                onClearPictures = actions.onClearPictures,
            )
        } else {
            MainFrame(
                texts = texts,
                tab = nav.tab,
                title = if (nav.tab == Tab.Mail) data.folderName else nav.tab.label(texts),
                onTab = { nav = nav.select(it) },
                onMenu = { scope.launch { drawer.open() } },
                onBack = if (nav.message != null) ({ back?.let { nav = it } }) else null,
            ) { modifier, expanded ->
                when (nav.tab) {
                    Tab.Mail -> {
                        val opening = actions.mail.withOpen { row -> nav = nav.openMessage(row.id) }
                        MailTab(texts, data, actions.copy(mail = opening), modifier, expanded, nav.messageId)
                    }
                    else -> Centered(texts.t("Pending development"), modifier)
                }
            }
        }
    }
}

@Composable
private fun MainFrame(
    texts: Texts,
    tab: Tab,
    title: String,
    onTab: (Tab) -> Unit,
    onMenu: () -> Unit,
    onBack: (() -> Unit)?,
    content: @Composable (Modifier, Boolean) -> Unit,
) {
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
                    TopAppBar(
                        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
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
) {
    val context = MailListContext(texts, data.nowMillis, data.swipe, data.accounts, data.categories, data.avatars)
    if (expanded) {
        Row(modifier = modifier.fillMaxSize()) {
            MailListScreen(context, data.mail, actions.mail, Modifier.width(ListWidth))
            VerticalDivider()
            if (messageId != null) {
                ReadingPane(texts, messageId, data.reading, actions.reading, Modifier.weight(1f))
            } else {
                Centered(texts.t("Select a message to read it."), Modifier.weight(1f))
            }
        }
    } else if (messageId != null) {
        ReadingPane(texts, messageId, data.reading, actions.reading, modifier)
    } else {
        MailListScreen(context, data.mail, actions.mail, modifier)
    }
}

/** These actions with the list's own changed. */
private fun ShellActions.copy(mail: MailListActions) =
    ShellActions(onFolder, onLanguage, mail, onSwipe, reading, onPictures, onClearPictures)

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
