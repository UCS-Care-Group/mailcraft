// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
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

    /** A picture of its own size (96 by 48 dp) in a line with text too long to stand beside it: the text wraps under it. */
    @Test
    fun aPictureInALineOfText() {
        paparazzi.unsafeUpdateConfig(phone.copy(nightMode = NightMode.NOTNIGHT))
        val src = "https://cdn-images.example/tray.png"
        val env = BodyEnv(EnglishTexts, emptyMap(), mapOf(src to PictureState.Ready(B.png)), Color.Gray, {}, {})
        val text = "Here is the seed tray we spoke of on Tuesday. It holds forty cells and stacks with the others, so the north bed can be sown in one go."
        val line = listOf(B.image(src, "Seed tray", 96u, 48u), B.p(text, 0u).copy(beside = true))
        paparazzi.snapshot("reading_picture_line_light_100") {
            // Decoded at once, as the other shots do: a picture decoded in the background may come after the shot.
            CompositionLocalProvider(LocalDecodeInPlace provides true) {
                Box(modifier = Modifier.background(Color.White).padding(16.dp)) { BlockColumn(line, env, Paper.of(0xFFFFFF, 0x0645AD), 360.dp) }
            }
        }
    }

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
