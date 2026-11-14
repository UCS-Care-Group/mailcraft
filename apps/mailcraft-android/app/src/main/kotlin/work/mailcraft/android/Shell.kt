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
internal class ShellData(
    val accounts: List<AccountItem>,
    val folderName: String,
    val folderId: ULong?,
    val mail: InboxState,
    val version: String,
    /** The language the owner chose in the app: its code, or "" for the system's. */
    val chosenLanguage: String,
)

/** What the shell asks of the app. */
internal class ShellActions(
    val onFolder: (AccountItem, FolderItem) -> Unit,
    val onLanguage: (String) -> Unit,
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
            )
        } else {
            MainFrame(
                texts = texts,
                tab = nav.tab,
                title = if (nav.tab == Tab.Mail) data.folderName else nav.tab.label(texts),
                onTab = { nav = nav.select(it) },
                onMenu = { scope.launch { drawer.open() } },
            ) { modifier ->
                when (nav.tab) {
                    Tab.Mail -> InboxScreen(texts, data.mail, modifier)
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
    content: @Composable (Modifier) -> Unit,
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
                            IconButton(onClick = onMenu) {
                                Icon(AppIcons.Menu, contentDescription = texts.t("Menu"))
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
                Box(modifier = Modifier.padding(padding)) { content(Modifier) }
            }
        }
    }
}

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
