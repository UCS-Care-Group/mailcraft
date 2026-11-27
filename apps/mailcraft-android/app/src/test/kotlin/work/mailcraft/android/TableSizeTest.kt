// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/**
 * I1 and I2 of [SectionPolicy] and the data table: what a table says of its size is what it draws,
 * and the work of a table inside tables is bounded however deep a message nests them (#1966).
 */
class TableSizeTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private val env = BodyEnv(EnglishTexts, emptyMap(), emptyMap(), Color.Gray, {}, {})

    /** For each width in px: the height [table] says it takes there, and the height it is drawn at. */
    private fun heights(table: TableView, widths: List<Int>): List<Triple<Int, Int, Int>> {
        val out = ArrayList<Triple<Int, Int, Int>>()
        paparazzi.snapshot {
            Probe(widths, out) { TableBlock(table, AlignView.LEFT, env, Paper.of(WHITE, LINK), 360.dp) }
        }
        return out
    }

    @Composable
    private fun Probe(widths: List<Int>, out: MutableList<Triple<Int, Int, Int>>, content: @Composable () -> Unit) {
        Layout(content = content) { measurables, constraints ->
            val m = measurables.first()
            widths.forEach { w -> out += Triple(w, m.minIntrinsicHeight(w), 0) }
            // A measure takes each child once: the last width is measured, the others asked.
            val w = widths.last()
            val pl = m.measure(Constraints(0, w, 0, Constraints.Infinity))
            out[out.lastIndex] = Triple(w, out.last().second, pl.height)
            layout(constraints.maxWidth, pl.height) { pl.place(0, 0) }
        }
    }

    private fun assertSaysWhatItDraws(table: TableView) {
        for (w in listOf(300, 450, 700, 990)) {
            val (_, said, drawn) = heights(table, listOf(w)).last()
            assertTrue("drawn at nothing at $w px", drawn > 0)
            assertEquals("at $w px it said $said and drew $drawn", said, drawn)
        }
    }

    private fun note(text: String, width: LengthView? = null) =
        B.cell(B.tableBlock(B.table(layout = true, width = width, rows = listOf(listOf(B.cell(B.p(text, 0u), background = 0xFFF4D6u))))))

    private val words = "the delivery leaves the shop on Tuesday morning ".repeat(3)

    @Test
    fun aLayoutTableSaysWhatItDraws() {
        val cell = { t: String -> B.cell(B.p(t, 0u)) }
        assertSaysWhatItDraws(B.table(layout = true, rows = listOf(listOf(cell("Seed trays $words"), B.cell(B.p(words, 0u), background = 0xFFF4D6u)), listOf(cell("Twine"), cell(words)))))
        assertSaysWhatItDraws(B.table(layout = true, width = LengthView.Px(150u), rows = listOf(listOf(cell(words)))))
        assertSaysWhatItDraws(B.table(layout = true, width = LengthView.Percent(60u), rows = listOf(listOf(cell(words), cell(words)))))
    }

    @Test
    fun aDataTableWithTablesInItSaysWhatItDraws() {
        assertSaysWhatItDraws(
            B.table(
                layout = false,
                border = true,
                rows = listOf(listOf(B.cell(B.p("Seed trays $words", 0u)), note(words, LengthView.Px(150u))), listOf(B.cell(B.p("Twine", 0u)), note(words))),
            ),
        )
    }

    /** [n] bordered tables one inside the next, the innermost with a sentence. */
    private fun nested(n: Int): TableView =
        (1 until n).fold(B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.p("the bottom line", 0u)))))) { inner, _ ->
            B.table(layout = false, border = true, rows = listOf(listOf(B.cell(B.p("a level", 0u), B.tableBlock(inner)))))
        }

    @Test
    fun twentyTablesInsideOneAnotherAreDrawnQuickly() {
        heights(nested(2), listOf(990))
        val start = System.nanoTime()
        val (_, said, drawn) = heights(nested(DEEP), listOf(990)).last()
        val took = (System.nanoTime() - start) / 1_000_000
        assertTrue("took $took ms", took < 2_000)
        assertEquals(said, drawn)
    }

    private companion object {
        const val DEEP = 20
        const val WHITE = 0xFFFFFF
        const val LINK = 0x0645AD
    }
}
