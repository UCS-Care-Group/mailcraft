// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

private const val TOLERANCE = 0.1

/** A phone tall enough to show a whole invented message, so that the shot shows everything it draws. */
private val Tall = DeviceConfig.PIXEL_5.copy(screenHeight = 3_400)

internal object ReadingShots {
    fun ui(entry: Corpus.Entry) = ReadingUi(
        id = entry.view.id,
        load = LoadState.Ready,
        opened = Corpus.opened(entry),
    )

    @Composable
    fun Pane(ui: ReadingUi) {
        MailCraftTheme {
            Surface {
                ReadingPane(EnglishTexts, ui.id ?: 0uL, ui, ReadingActions.None)
            }
        }
    }
}

/** The five corpus messages in the reading pane on a phone: light and dark, at the system's text size and at twice that. */
class ReadingScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Tall,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, ui: ReadingUi) {
        for ((suffix, night, scale) in listOf(
            Triple("light_100", false, 1f),
            Triple("dark_100", true, 1f),
            Triple("light_200", false, 2f),
            Triple("dark_200", true, 2f),
        )) {
            paparazzi.unsafeUpdateConfig(Tall.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT, fontScale = scale))
            paparazzi.snapshot("reading_${name}_$suffix") { ReadingShots.Pane(ui) }
        }
    }

    @Test
    fun plain() = shoot("plain", ReadingShots.ui(Corpus.plain))

    @Test
    fun newsletterLayoutTable() = shoot("newsletter", ReadingShots.ui(Corpus.newsletter))

    @Test
    fun dataTable() = shoot("data_table", ReadingShots.ui(Corpus.dataTable))

    @Test
    fun quotedReply() = shoot("quoted_reply", ReadingShots.ui(Corpus.reply))

    @Test
    fun longLinks() = shoot("long_links", ReadingShots.ui(Corpus.longLinks))
}

/** The reading pane on a tablet: the list's place beside it is the list's own shot. */
class ReadingTabletScreenshotTest {
    private val tablet = DeviceConfig.NEXUS_10

    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = tablet,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    @Test
    fun newsletterOnATablet() {
        paparazzi.unsafeUpdateConfig(tablet.copy(nightMode = NightMode.NOTNIGHT, fontScale = 1f))
        paparazzi.snapshot("reading_tablet_newsletter_light_100") { ReadingShots.Pane(ReadingShots.ui(Corpus.newsletter)) }
    }

    @Test
    fun dataTableOnATabletDark() {
        paparazzi.unsafeUpdateConfig(tablet.copy(nightMode = NightMode.NIGHT, fontScale = 1f))
        paparazzi.snapshot("reading_tablet_data_dark_100") { ReadingShots.Pane(ReadingShots.ui(Corpus.dataTable)) }
    }
}
