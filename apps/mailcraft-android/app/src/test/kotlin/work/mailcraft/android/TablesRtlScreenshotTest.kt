// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.LayoutDirection
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test

/** #1966: data tables with spans, a wide statement, and right-to-left text; light, and dark with the light page off. */
class TablesRtlScreenshotTest {
    private val phone = DeviceConfig.PIXEL_5.copy(screenHeight = 1_800)

    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = phone,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private val darkPage = ReadingTools(
        MailListContext(EnglishTexts, ShellShots.NOW, SwipeConfig(), emptyList(), ShellShots.categories),
        { _, _ -> },
        {},
        ReadingPrefs(lightMessages = false),
        {},
    )

    private fun shoot(entry: Corpus.Entry) {
        for ((suffix, night) in listOf("light_100" to false, "dark_100" to true)) {
            paparazzi.unsafeUpdateConfig(phone.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
            val tools = if (night) darkPage else ReadingTools.None
            paparazzi.snapshot("reading_${entry.name}_$suffix") { ReadingShots.Pane(ReadingShots.ui(entry), tools) }
        }
    }

    @Test
    fun orderTableWithRowAndColumnSpans() = shoot(TablesRtl.order)

    @Test
    fun wideStatementOfEightColumns() = shoot(TablesRtl.statement)

    @Test
    fun rightToLeftReplyWithALeftToRightQuote() = shoot(TablesRtl.rtlReply)

    @Test
    fun mixedDirectionTable() = shoot(TablesRtl.mixed)

    /** A cell too tall to measure beside its width is held to the most a measure takes, not a crash. */
    @Test
    fun aVeryTallCellInANarrowTable() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        paparazzi.snapshot("reading_${TablesRtl.tallNarrow.name}_light_100") { ReadingShots.Pane(ReadingShots.ui(TablesRtl.tallNarrow)) }
    }

    @Test
    fun aVeryTallCellInAWideTable() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        paparazzi.snapshot("reading_${TablesRtl.tallWide.name}_light_100") { ReadingShots.Pane(ReadingShots.ui(TablesRtl.tallWide)) }
    }

    /** A coloured box of about 6,000 lines is held to the most a measure takes, not a crash. */
    @Test
    fun aVeryTallColouredBox() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        paparazzi.snapshot("reading_${TablesRtl.tallTinted.name}_light_100") { ReadingShots.Pane(ReadingShots.ui(TablesRtl.tallTinted)) }
    }

    @Test
    fun aVeryTallColouredBoxInADataCell() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        paparazzi.snapshot("reading_${TablesRtl.tallNested.name}_light_100") { ReadingShots.Pane(ReadingShots.ui(TablesRtl.tallNested)) }
    }

    /** On a phone that reads from the right, the message still lays out from the left: only its own Hebrew reads from the right. */
    @Test
    fun aRightToLeftPhoneDoesNotMirrorTheMessage() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        paparazzi.snapshot("reading_rtl_phone_light_100") {
            CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Rtl) { ReadingShots.Pane(ReadingShots.ui(TablesRtl.mixed)) }
        }
    }
}
