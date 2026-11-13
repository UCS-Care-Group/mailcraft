// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

private const val TOLERANCE = 0.1

/** What the list screens show, apart from the shell around them. */
private object ListShots {
    private val accounts = listOf(AccountItem(1u, "Ada Example", "ada@example.org", emptyList()))

    @Composable
    fun List(ui: MailListUi, swipe: SwipeConfig = SwipeConfig()) {
        MailCraftTheme {
            Surface {
                MailListScreen(
                    context = MailListContext(EnglishTexts, ShellShots.NOW, swipe, accounts, ShellShots.categories),
                    ui = ui,
                    actions = MailListActions.None,
                )
            }
        }
    }

    @Composable
    fun Filtered(menuOpen: Boolean) {
        MailCraftTheme {
            Surface {
                Column {
                    MailListBar(EnglishTexts, MailQuery(1uL, filter = ListFilter.Unread), MailListActions.None, menuOpen)
                }
            }
        }
    }

    @Composable
    fun Swiping(fraction: Float) {
        MailCraftTheme {
            Surface {
                Column {
                    val rows = ShellShots.rows
                    MailRowContent(EnglishTexts, rows[0], SwipeConfig(), RowActions({}, {}))
                    SwipeInProgress(EnglishTexts, rows[1], SwipeConfig(), fraction)
                    MailRowContent(EnglishTexts, rows[2], SwipeConfig(), RowActions({}, {}))
                }
            }
        }
    }
}

/** The list on a phone, light and dark, at the system's text size and at twice that. */
class PhoneListScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float, content: @Composable () -> Unit) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { content() }
    }

    private fun shell(ui: MailListUi): @Composable () -> Unit = {
        ShellShots.Shell(with = ShellShots.data.withMail(ui))
    }

    @Test
    fun listLight100() = shoot("phone_list_light_100", false, 1f, shell(ShellShots.listUi()))

    @Test
    fun listLight200() = shoot("phone_list_light_200", false, 2f, shell(ShellShots.listUi()))

    @Test
    fun listDark100() = shoot("phone_list_dark_100", true, 1f, shell(ShellShots.listUi()))

    @Test
    fun listDark200() = shoot("phone_list_dark_200", true, 2f, shell(ShellShots.listUi()))

    @Test
    fun splitAndFilterLight100() = shoot(
        "phone_split_filter_light_100", false, 1f,
        shell(ShellShots.listUi(query = MailQuery(1uL, filter = ListFilter.Unread, split = "promotions"), tabs = ShellShots.tabs)),
    )

    @Test
    fun splitAndFilterDark200() = shoot(
        "phone_split_filter_dark_200", true, 2f,
        shell(ShellShots.listUi(query = MailQuery(1uL, filter = ListFilter.Flagged, split = "social"), tabs = ShellShots.tabs)),
    )

    @Test
    fun filterRowLight100() = shoot("phone_filter_row_light_100", false, 1f) { ListShots.Filtered(menuOpen = true) }

    @Test
    fun filterRowDark200() = shoot("phone_filter_row_dark_200", true, 2f) { ListShots.Filtered(menuOpen = false) }

    @Test
    fun swipeArchiveLight100() = shoot("phone_swipe_archive_light_100", false, 1f) { ListShots.Swiping(0.35f) }

    @Test
    fun swipeDeleteDark200() = shoot("phone_swipe_delete_dark_200", true, 2f) { ListShots.Swiping(-0.35f) }

    @Test
    fun emptyLight100() = shoot("phone_empty_light_100", false, 1f, shell(ShellShots.listUi(rows = emptyList()).copy(updatedAtMillis = null)))

    @Test
    fun noMatchDark200() = shoot(
        "phone_nomatch_dark_200", true, 2f,
        shell(ShellShots.listUi(rows = emptyList(), query = MailQuery(1uL, search = "zebra"))),
    )

    @Test
    fun offlineLight100() = shoot("phone_offline_light_100", false, 1f, shell(ShellShots.listUi(offline = true)))

    @Test
    fun offlineDark200() = shoot("phone_offline_dark_200", true, 2f, shell(ShellShots.listUi(offline = true)))

    @Test
    fun loadingLight100() = shoot("phone_loading_light_100", false, 1f, shell(ShellShots.listUi(load = LoadState.Loading)))

    @Test
    fun failedDark100() = shoot("phone_failed_dark_100", true, 1f, shell(ShellShots.listUi(load = LoadState.Failed)))

    @Test
    fun searchLight100() = shoot(
        "phone_search_light_100", false, 1f,
        shell(ShellShots.listUi(rows = ShellShots.rows.take(2), query = MailQuery(1uL, search = "example", everywhere = true))),
    )
}

/** The list on a tablet, beside the space the reading pane will take. */
class TabletListScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.NEXUS_10,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean, fontScale: Float) {
        paparazzi.unsafeUpdateConfig(
            DeviceConfig.NEXUS_10.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = fontScale),
        )
        paparazzi.snapshot(name) { ShellShots.Shell(with = ShellShots.data.withMail(ShellShots.listUi(tabs = ShellShots.tabs))) }
    }

    @Test
    fun listLight100() = shoot("tablet_list_light_100", false, 1f)

    @Test
    fun listLight200() = shoot("tablet_list_light_200", false, 2f)

    @Test
    fun listDark100() = shoot("tablet_list_dark_100", true, 1f)

    @Test
    fun listDark200() = shoot("tablet_list_dark_200", true, 2f)
}
