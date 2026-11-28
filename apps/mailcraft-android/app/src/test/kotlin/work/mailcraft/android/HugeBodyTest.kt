// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.LineHeightView
import uniffi.mailcraft_mobile.TableView

/**
 * I3: a message taller than Compose can lay out (2^24 px) is drawn up to [MAX_STACK] and cut there,
 * never a throw, in a plain cell, a coloured one, a data cell and a line of a picture and text (#1966).
 * Its text comes as the bridge sends it: blocks of at most `MAX_BLOCK_CHARS` characters.
 */
class HugeBodyTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})
    private val paper = Paper.of(WHITE, LINK)

    /** One block as the bridge cuts a long one: 2,000 lines of the largest text, 200 px a line. */
    private fun piece(size: UByte = 48u, tall: UByte? = 200u): BlockView =
        B.block(BlockKindView.Paragraph, B.span(LINES, size = size), space = 0u).copy(lineHeight = tall?.let { LineHeightView.Px(it) })

    /** More than 2^24 px of text at 2.75 px to the dp: 36,000 lines of 550 px. */
    private val huge = List(PIECES) { piece() }

    /** The height in px [content] is drawn at, as tall as it is. */
    private fun heightOf(content: @Composable () -> Unit): Int {
        var height = -1
        paparazzi.snapshot {
            Box(modifier = Modifier.wrapContentHeight(unbounded = true).onGloballyPositioned { height = it.size.height }) { content() }
        }
        return height
    }

    private fun table(layout: Boolean, background: UInt? = null): TableView = B.table(
        layout = layout,
        border = !layout,
        width = LengthView.Px(600u),
        rows = listOf(listOf(B.cell(*huge.toTypedArray(), background = background), B.cell(B.p("Twine", 0u)))),
    )

    private fun assertHeld(height: Int) = assertTrue("height $height", height in MAX_BOX_HEIGHT..MAX_LAYOUT)

    @Test
    fun aPlainCellTallerThanComposeTakesIsCutNotThrown() =
        assertHeld(heightOf { TableBlock(table(layout = true), AlignView.LEFT, env, paper, 392.dp) })

    @Test
    fun aColouredCellTallerThanComposeTakesIsCutNotThrown() {
        val height = heightOf { TableBlock(table(layout = true, background = TINT), AlignView.LEFT, env, paper, 392.dp) }
        assertTrue("height $height", height in 1..MAX_LAYOUT)
    }

    @Test
    fun aDataCellTallerThanComposeTakesIsCutNotThrown() {
        val height = heightOf { TableBlock(table(layout = false), AlignView.LEFT, env, paper, 392.dp) }
        assertTrue("height $height", height in 1..MAX_LAYOUT)
    }

    @Test
    fun aLineOfAPictureAndTextTallerThanComposeTakesIsCutNotThrown() {
        val picture = B.image("https://cdn-images.example/tray.png", "Seed tray", 96u, 48u)
        val line = listOf(picture) + huge.map { it.copy(beside = true) }
        assertHeld(heightOf { BlockColumn(line, env, paper, 392.dp) })
    }

    @Test
    fun aParagraphOf130000LinesAsTheBridgeSendsItIsDrawn() {
        // 130,000 lines of `x<br>` come as 65 blocks of 2,000 lines (`long_text_tests`).
        val blocks = List(BRIDGE_PIECES) { piece(size = 16u, tall = null) }
        val height = heightOf { BlockColumn(blocks, env, paper, 392.dp) }
        assertTrue("height $height", height in MAX_BOX_HEIGHT..MAX_LAYOUT)
    }

    private companion object {
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
        const val TINT = 0xFFF4D6u
        const val PIECES = 18
        const val BRIDGE_PIECES = 65
        val LINES = List(2_000) { "x" }.joinToString("\n")
    }
}
