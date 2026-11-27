// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import androidx.compose.ui.unit.Constraints
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** #1966: a data table's cells in the places the core gives them, its columns fitted to the screen, its rows under spans. */
class DataTableTest {
    private fun need(col: Int, min: Float, max: Float, cols: Int = 1, px: Float? = null, percent: Int? = null) = CellNeed(col, cols, min, max, px, percent)

    @Test
    fun columnsThatFitKeepTheWidthTheirCellsWant() {
        val widths = dataColumnWidths(listOf(need(0, 40f, 100f), need(1, 60f, 150f)), 2, room = 328f, asked = null)
        assertArrayEquals(floatArrayOf(100f, 150f), widths, 0.01f)
    }

    @Test
    fun aTableWiderThanTheScreenIsSqueezedTowardItsWordsFirst() {
        val widths = dataColumnWidths(listOf(need(0, 50f, 300f), need(1, 80f, 300f)), 2, room = 328f, asked = null)
        assertEquals(328f, widths.sum(), 0.01f)
        assertTrue(widths[0] >= 50f && widths[1] >= 80f)
    }

    @Test
    fun aTableWhoseWordsDontFitScrollsAtTheLeastTheyNeed() {
        val needs = (0 until 8).map { need(it, 70f, 200f) }
        val widths = dataColumnWidths(needs, 8, room = 328f, asked = null)
        assertArrayEquals(FloatArray(8) { 70f }, widths, 0.01f)
    }

    @Test
    fun aTableThatAsksForItsWidthGivesTheSpareToColumnsNotToldTheirs() {
        val widths = dataColumnWidths(listOf(need(0, 30f, 40f, px = 60f), need(1, 30f, 40f)), 2, room = 300f, asked = 300f)
        assertArrayEquals(floatArrayOf(60f, 240f), widths, 0.01f)
        val shared = dataColumnWidths(listOf(need(0, 30f, 40f, percent = 50), need(1, 30f, 40f)), 2, room = 300f, asked = 300f)
        assertEquals(150f, shared[0], 0.01f)
    }

    @Test
    fun aCellOverColumnsSharesWhatTheyLackAmongThem() {
        val widths = dataColumnWidths(listOf(need(0, 20f, 30f), need(1, 20f, 30f), need(0, 50f, 200f, cols = 2)), 2, room = 328f, asked = null)
        assertEquals(200f, widths.sum(), 0.01f)
        assertEquals(widths[0], widths[1], 0.01f)
    }

    @Test
    fun aWidthOnACellOverColumnsIsLeftToItsWordsAsOnTheDesktop() {
        val widths = dataColumnWidths(listOf(need(0, 40f, 80f, cols = 2, px = 240f)), 2, room = 328f, asked = null)
        assertEquals(80f, widths.sum(), 0.01f)
    }

    @Test
    fun aCellEverSoTallOrWideStillFitsWhatAMeasureTakes() {
        // About 5,000 lines in one cell, a cell over 200 rows of 1,300 px, an Int's worth of height.
        val tall = dataRowTops(listOf(placed(0, 1) to 310_000, placed(1, 200) to 260_000, placed(1, 1) to Int.MAX_VALUE), 201)
        assertTrue(tall.all { it in 0..MAX_BOX_HEIGHT })
        val wide = edgesOf(FloatArray(10) { 1_000f })
        assertTrue(wide.all { it in 0..MAX_BOX_WIDTH })
        for ((w, h) in listOf(wide.between(0, 10) to tall.between(0, 201), wide.between(0, 1) to tall.between(1, 201))) {
            Constraints.fixed(w, h)
        }
    }

    @Test
    fun hostileNeedsStayInsideTheGrid() {
        assertEquals(0, dataColumnWidths(emptyList(), 0, 328f, null).size)
        assertEquals(MAX_COLUMNS, dataColumnWidths(emptyList(), 100_000, 328f, null).size)
        val widths = dataColumnWidths(listOf(need(-3, 10f, 10f), need(99, 10f, 10f), need(1, 10f, 10f, cols = 1_000)), 2, 328f, null)
        assertTrue(widths.all { it.isFinite() && it >= 0f })
    }

    private fun placed(row: Int, rows: Int) = PlacedCell(B.cell(B.p("x")), row, 0, 1, rows)

    @Test
    fun aCellOverRowsMakesOnlyTheLastOfThemTaller() {
        // The order number spans the two lines of its order; its own height is 100.
        val heights = listOf(placed(0, 1) to 30, placed(1, 2) to 100, placed(1, 1) to 30, placed(2, 1) to 30, placed(3, 1) to 30)
        assertArrayEquals(intArrayOf(0, 30, 60, 130, 160), dataRowTops(heights, 4))
        // A short one leaves its rows as they are.
        assertArrayEquals(intArrayOf(0, 30, 60), dataRowTops(listOf(placed(0, 2) to 10, placed(0, 1) to 30, placed(1, 1) to 30), 2))
        assertArrayEquals(intArrayOf(0), dataRowTops(listOf(placed(5, 9) to 1_000), 0))
    }

    private fun spans(t: uniffi.mailcraft_mobile.TableView) = gridOf(t).placed.map { listOf(it.cell.blocks[0].spans[0].text, it.row, it.col, it.rows, it.cols) }

    @Test
    fun anOrderTableTakesThePlacesTheCoreGives() {
        // The places mailcraft-mobile's tables_rtl_tests sends for the same order.
        val t = TablesRtl.orderTable
        val grid = gridOf(t)
        assertEquals(4, grid.rows)
        assertEquals(4, grid.columns)
        val got = spans(t)
        assertEquals(listOf("A-1042", 1, 0, 2, 1), got[4])
        assertEquals(listOf("Twine, 100 m", 2, 1, 1, 1), got[8])
        assertEquals(listOf("Total", 3, 0, 1, 3), got[11])
    }

    @Test
    fun aPlaceThatOverlapsOrLiesPastTheCapsIsLeftOut() {
        val t = B.table(
            layout = false,
            rows = listOf(
                listOf(B.cell(B.p("a"), rowspan = 2u, colspan = 2u, at = 0 to 0)),
                listOf(B.cell(B.p("over"), at = 1 to 1), B.cell(B.p("past"), at = 1 to 64), B.cell(B.p("far"), at = 1 to 2, colspan = 60_000u)),
                listOf(B.cell(B.p("deep"), at = 20_000 to 0), B.cell(B.p("long"), at = 2 to 0, rowspan = 65_000u)),
            ),
        )
        val got = spans(t)
        assertEquals(listOf(listOf("a", 0, 0, 2, 2), listOf("far", 1, 2, 1, 62), listOf("long", 2, 0, 1, 1)), got)
    }
}
