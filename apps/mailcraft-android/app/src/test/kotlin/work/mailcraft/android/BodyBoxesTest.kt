// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.cash.paparazzi.SnapshotHandler
import com.android.ide.common.rendering.api.SessionParams
import java.lang.reflect.Proxy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/** Spacing, padding and the boxes of a message, as the desktop draws them (#1966). */
class BodyBoxesTest {
    /** The last picture Paparazzi drew, kept to read its pixels (a proxy: its type is not on the compile path). */
    private var drawn: Any? = null

    private fun keepPictures(): SnapshotHandler {
        val loader = SnapshotHandler::class.java.classLoader
        val frames = Proxy.newProxyInstance(loader, arrayOf(SnapshotHandler.FrameHandler::class.java)) { _, method, args ->
            if (method.name == "handle") drawn = args?.firstOrNull()
            null
        }
        return Proxy.newProxyInstance(loader, arrayOf(SnapshotHandler::class.java)) { _, method, _ ->
            if (method.name == "newFrameHandler") frames else null
        } as SnapshotHandler
    }

    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Shot,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = keepPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    /** [table] drawn 360 dp wide on white: where it is, in the screen's pixels, and the colour at a point of it. */
    private fun drawTable(table: TableView) = draw { TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), 360.dp) }

    /** [content] drawn: where it is, in the screen's pixels, and the colour at a point of it. */
    private fun draw(content: @Composable () -> Unit): Pair<Rect, (Float, Float) -> Int> {
        var bounds = Rect.Zero
        paparazzi.snapshot {
            Box(modifier = Modifier.onGloballyPositioned { bounds = it.boundsInRoot() }) { content() }
        }
        val image = drawn ?: error("nothing drawn")
        val width = image.javaClass.getMethod("getWidth").invoke(image) as Int
        val scale = width / Shot.screenWidth.toFloat()
        val rgb = image.javaClass.getMethod("getRGB", Int::class.javaPrimitiveType, Int::class.javaPrimitiveType)
        return bounds to { x, y -> (rgb.invoke(image, (x * scale).toInt(), (y * scale).toInt()) as Int) and RGB }
    }

    @Test
    fun aBlankLineIsAParagraphGapAndEachMoreAWholeLine() {
        val before = listOf(B.p("Monthly news", 0u))
        val gaps = (0..4).map { spaceAbove(before, B.p("Second part", it.toUByte())).value }
        assertEquals(0f, gaps[0], 0.1f)
        assertEquals(BASE_SIZE * 10f / 13f, gaps[1], 0.1f)
        // Two blank paragraphs are well apart from one: a whole line of text more.
        assertEquals(BASE_SIZE * 1.4f, gaps[2] - gaps[1], 0.1f)
        assertEquals(gaps[3], gaps[4], 0.1f)
    }

    @Test
    fun theFirstBlockHasNoRoomAboveAndAMarginWinsWhereItIsMore() {
        assertEquals(0f, spaceAbove(null, B.p("Hi Sam,", 2u)).value, 0f)
        val margin = B.p("Thanks.", 1u).copy(gap = 40u)
        assertEquals(40f, spaceAbove(listOf(B.p("Hi")), margin).value, 0f)
        assertEquals(2f, spaceAbove(listOf(B.item("one")), B.item("two")).value, 0f)
        assertEquals(4f, spaceAbove(listOf(B.h(2u, "News")), B.p("Text", 0u)).value, 0f)
    }

    @Test
    fun eachLineKnowsTheLineBeforeIt() {
        val first = B.p("Monthly news", 0u)
        val lines = linesOf(listOf(first, B.p("Second part", 3u)))
        assertEquals(listOf(null, listOf(first)), lines.map { it.before })
    }

    @Test
    fun blankLinesGrowWithTheTextAndAMarginInPixelsDoesNot() {
        val before = listOf(B.p("Monthly news", 0u))
        val blank = B.p("Second part", 2u)
        assertEquals(2 * spaceAbove(before, blank).value, spaceAbove(before, blank, fontScale = 2f).value, 0.1f)
        val margin = B.p("Second part", 0u).copy(gap = 30u)
        assertEquals(30f, spaceAbove(before, margin, fontScale = 2f).value, 0f)
    }

    private fun insets(top: Int, right: Int, bottom: Int, left: Int) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    @Test
    fun aCellsPaddingFollowsTheDesktop() {
        val text = B.p("Invoice INV-1", 0u)
        val plain = B.table(layout = true, rows = listOf(listOf(B.cell(text))))
        val framed = plain.copy(outline = true)
        val data = B.table(layout = false, rows = listOf(listOf(B.cell(text))))
        // What the message says wins.
        assertEquals(insets(16, 16, 16, 16), cellPadding(framed, ruled = false, cell = B.cell(text, padding = 16u)))
        // A box: a framed or coloured table, or a coloured cell.
        assertEquals(insets(9, 12, 9, 12), cellPadding(framed, ruled = false, cell = B.cell(text)))
        assertEquals(insets(9, 12, 9, 12), cellPadding(plain, ruled = false, cell = B.cell(text, background = 0xF6F6F6u)))
        assertEquals(insets(9, 12, 9, 12), cellPadding(plain.copy(background = 0xFFFFFFu), ruled = false, cell = B.cell(text)))
        // A data table's grid, a band that holds nothing, and a cell that only places things.
        assertEquals(insets(5, 8, 5, 8), cellPadding(data, ruled = false, cell = B.cell(text)))
        assertEquals(insets(3, 0, 3, 0), cellPadding(framed, ruled = false, cell = B.cell(background = 0x2DA44Eu)))
        assertEquals(insets(0, 0, 0, 0), cellPadding(plain, ruled = false, cell = B.cell(text)))
    }

    @Test
    fun aRuledTableGivesEveryCellABoxsRoom() {
        val text = B.p("Seed trays", 0u)
        val line = B.noSides.copy(bottom = true)
        val ruledTable = B.table(layout = true, rows = listOf(listOf(B.cell(text).copy(border = line), B.cell(text))))
        assertTrue(ruled(ruledTable))
        assertEquals(insets(9, 12, 9, 12), cellPadding(ruledTable, B.cell(text), ruled(ruledTable)))
        assertFalse(ruled(ruledTable.copy(outline = true)))
        assertFalse(ruled(B.table(layout = true, rows = listOf(listOf(B.cell(text).copy(border = B.noSides.copy(left = true)))))))
    }

    @Test
    fun cellsOfAPaintedTableMeetAndThoseOfAPlainOneAreApart() {
        val text = B.cell(B.p("a", 0u))
        val plain = B.table(layout = true, rows = listOf(listOf(text, text), listOf(text, text)))
        assertEquals(DEFAULT_COLUMN_GAP to 6f, gapsOf(plain))
        assertEquals(0f to 0f, gapsOf(plain.copy(outline = true)))
        assertEquals(3f to 3f, gapsOf(plain.copy(spacing = 3u)))
        val picture = B.cell(B.image("cid:slice@example.org", "slice", 300u, 20u))
        assertEquals(DEFAULT_COLUMN_GAP to 0f, gapsOf(B.table(layout = true, rows = listOf(listOf(picture), listOf(picture)))))
    }

    @Test
    fun aBoxOnALineKeepsItsMarginsWhileThereIsRoom() {
        val box = B.table(layout = true, rows = listOf(listOf(B.cell(B.p("a", 0u))))).copy(inline = true, boxMarginLeft = 10u, boxMarginRight = 20u)
        assertEquals(10.dp to 20.dp, boxMargins(box, 300.dp))
        val (left, right) = boxMargins(box, 70.dp)
        assertEquals(30f, (left + right).value, 0.01f)
    }

    @Test
    fun aBoxOnALineAloneKeepsItsMargins() {
        val box = B.table(layout = true, width = LengthView.Px(100u), rows = listOf(listOf(B.cell(B.p("Status", 0u)))))
            .copy(inline = true, background = ACCENT.toUInt(), boxMarginLeft = 10u, boxMarginRight = 20u)
        val (bounds, at) = draw { BodyLineView(linesOf(listOf(B.tableBlock(box).copy(space = 0u))).first(), env, Paper.of(WHITE, LINK), 360.dp) }
        val density = Shot.density.dpiValue / 160f
        val middle = bounds.center.y
        assertNotEquals("the margin is gone", ACCENT, at(bounds.left + 5f * density, middle))
        assertEquals(ACCENT, at(bounds.left + 15f * density, middle))
    }

    @Test
    fun theFrameOfAButtonIsSeenRoundItsFill() {
        val (bounds, at) = drawTable(BoxCorpus.buttonTable)
        val middle = bounds.center.y
        // Just inside the frame's left line: the frame, not the button's fill.
        val edge = at(bounds.left + 1f, middle)
        assertNotEquals("the fill covers the frame", NAVY, edge)
        assertNotEquals("no frame", WHITE, edge)
        assertEquals("no fill", NAVY, at(bounds.center.x, bounds.top + bounds.height * 0.2f))
    }

    @Test
    fun anAccentBarReachesTheFootOfItsCard() {
        val (bounds, at) = drawTable(BoxCorpus.cardTable)
        val density = Shot.density.dpiValue / 160f
        val x = bounds.left + 3f * density
        assertEquals(ACCENT, at(x, bounds.top + bounds.height / 2))
        assertEquals(ACCENT, at(x, bounds.bottom - 3f * density))
    }

    private companion object {
        val Shot = DeviceConfig.PIXEL_5.copy(screenHeight = 2_400)
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
        const val RGB = 0xFFFFFF
        const val NAVY = BoxCorpus.NAVY
        const val ACCENT = BoxCorpus.ACCENT
    }
}
