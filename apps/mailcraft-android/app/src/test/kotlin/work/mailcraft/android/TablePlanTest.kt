// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.LengthView

class TablePlanTest {
    private fun spec(holds: Boolean = true, fixed: Float? = null, percent: Int? = null) = ColumnSpec(holds, fixed, percent)

    @Test
    fun twoHalvesStandSideBySideWhereEachIsAboutOneFortyDpOrMore() {
        val widths = planColumns(listOf(spec(percent = 50), spec(percent = 50)), room = 328f, gap = 12f)
        assertNotNull(widths)
        assertEquals(158f, widths!![0], 0.01f)
    }

    @Test
    fun threeThirdsAreStackedOnAPhoneAndSideBySideOnATablet() {
        val thirds = listOf(spec(percent = 33), spec(percent = 33), spec(percent = 33))
        assertNull(planColumns(thirds, room = 328f, gap = 12f))
        assertNotNull(planColumns(thirds, room = 600f, gap = 12f))
    }

    @Test
    fun theRuleIsOneFortyDpExactly() {
        assertEquals(140f, MIN_LAYOUT_COLUMN, 0f)
        val two = listOf(spec(), spec())
        assertNotNull(planColumns(two, room = 2 * 140f + 12f, gap = 12f))
        assertNull(planColumns(two, room = 2 * 140f + 11f, gap = 12f))
    }

    @Test
    fun aRowOfIconsIsNotSqueezedIntoAStack() {
        // Four 32 dp icons and gaps fit in 328 dp: they stay a row though each is far under 140.
        val icons = List(4) { spec(fixed = 32f) }
        assertEquals(List(4) { 32f }, planColumns(icons, 328f, 12f))
        // But fixed widths that do not fit stack.
        assertNull(planColumns(List(4) { spec(fixed = 100f) }, 328f, 12f))
    }

    @Test
    fun aColumnThatHoldsNothingTakesWhatItAsksAndIsNotCounted() {
        val widths = planColumns(listOf(spec(), spec(holds = false, fixed = 24f), spec()), room = 400f, gap = 0f)
        assertNotNull(widths)
        assertEquals(24f, widths!![1], 0f)
        assertEquals(188f, widths[0], 0.01f)
    }

    @Test
    fun oneColumnIsNeverStacked() {
        assertEquals(listOf(328f), planColumns(listOf(spec()), 328f, 12f))
        assertEquals(listOf(200f), planColumns(listOf(spec(fixed = 200f)), 328f, 12f))
        assertEquals(listOf(328f), planColumns(listOf(spec(fixed = 900f)), 328f, 12f))
    }

    @Test
    fun theNewsletterStacksItsThreeColumnsOnAPhoneButNotItsTwo() {
        val table = (Corpus.newsletter.view.blocks[0].kind as BlockKindView.Table).table
        val room = 328f
        val plans = table.rows.map { row ->
            planColumns(row.cells.map { c -> spec(fixed = null, percent = (c.width as? LengthView.Percent)?.percent?.toInt()) }, room, DEFAULT_COLUMN_GAP)
        }
        assertNotNull(plans[1])
        assertNull(plans[2])
    }

    @Test
    fun spansAndRowSpansPlaceCellsWithoutGaps() {
        val t = B.table(
            layout = false,
            rows = listOf(
                listOf(B.cell(B.p("a")), B.cell(B.p("b"), colspan = 2u)),
                listOf(B.cell(B.p("c")), B.cell(B.p("d")), B.cell(B.p("e"))),
            ),
        )
        val grid = gridOf(t)
        assertEquals(2, grid.rows)
        assertEquals(3, grid.columns)
        assertEquals(listOf(0, 1, 0, 1, 2), grid.placed.map { it.col })
        assertEquals(2, grid.placed[1].cols)
    }

    @Test
    fun aLayoutTableWhoseRowsDifferInWidthIsPlannedRowByRow() {
        val table = (Corpus.newsletter.view.blocks[0].kind as BlockKindView.Table).table
        val sections = sectionsOf(gridOf(table))
        assertEquals(listOf(1, 2, 3, 1), sections.map { it.columns })
        // A table of rows of one width is planned as one.
        val even = B.table(layout = true, rows = listOf(listOf(B.cell(B.p("a")), B.cell(B.p("b"))), listOf(B.cell(B.p("c")), B.cell(B.p("d")))))
        assertEquals(1, sectionsOf(gridOf(even)).size)
        assertTrue(sectionsOf(gridOf(B.table(layout = true, rows = emptyList()))).size <= 1)
    }

    @Test
    fun hostileTablesStayWithinTheCaps() {
        val wide = B.table(layout = false, rows = listOf(List(500) { B.cell(B.p("x")) }, List(500) { B.cell(B.p("y"), colspan = 500u) }))
        val grid = gridOf(wide)
        assertTrue(grid.columns <= MAX_COLUMNS)
        val tall = B.table(layout = false, rows = List(3_000) { List(4) { B.cell(B.p("z")) } })
        assertTrue(gridOf(tall).placed.size <= MAX_CELLS)
        assertEquals(0, gridOf(B.table(layout = true, rows = emptyList())).placed.size)
        assertEquals(listOf<Float>(), planColumns(emptyList(), 100f, 0f))
    }

    @Test
    fun linesJoinABlockThatGoesOnInTheLineBefore() {
        val blocks = listOf(B.p("a"), B.block(BlockKindView.Paragraph, B.span("b"), beside = true), B.p("c"))
        assertEquals(listOf(2, 1), linesOf(blocks).map { it.blocks.size })
        assertTrue(linesOf(emptyList()).isEmpty())
        assertEquals(1, linesOf(listOf(B.block(BlockKindView.Paragraph, B.span("x"), beside = true))).size)
    }

    @Test
    fun theSpanOfColumnsAddsTheGapsBetweenThem() {
        assertEquals(100f + 12f + 50f, spanWidth(listOf(100f, 50f, 10f), 0, 2, 12f), 0f)
        assertEquals(10f, spanWidth(listOf(100f, 50f, 10f), 2, 3, 12f), 0f)
        assertEquals(0f, spanWidth(listOf(1f), 5, 9, 12f), 0f)
    }

    @Test
    fun dataColumnsAreAsWideAsTheirWidestCellWithinTheirLimits() {
        val a = Placed(B.cell(B.p("a")), 0, 0, 1)
        val b = Placed(B.cell(B.p("b")), 0, 1, 1)
        val wide = Placed(B.cell(B.p("w"), colspan = 2u), 1, 0, 2)
        val widths = dataColumnWidths(listOf(a to 10, b to 500), 2, minimum = 48, maximum = 280)
        assertEquals(listOf(48, 280), widths.toList())
        // A cell over two columns widens them when they are too narrow together.
        val narrow = dataColumnWidths(listOf(a to 50, b to 50, wide to 400), 2, minimum = 48, maximum = 280)
        assertTrue(narrow.sum() >= 400)
        assertEquals(1, dataColumnWidths(emptyList(), 0, 48, 280).size)
    }

    @Test
    fun aPictureIsDecodedToABoundedSize() {
        assertEquals(1, sampleSize(100, 100))
        assertEquals(1, sampleSize(MAX_SIDE, 10))
        assertEquals(2, sampleSize(MAX_SIDE + 1, 10))
        assertEquals(8, sampleSize(16_000, 5))
    }
}
