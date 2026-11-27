// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.Alignment
import androidx.compose.ui.layout.IntrinsicMeasurable
import androidx.compose.ui.layout.IntrinsicMeasureScope
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasurePolicy
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.VAlignView

/** A layout table's own width: in dp ([dp]), or a share of the room ([percent]); neither when it gives none. */
internal class OwnWidth(val dp: Float?, val percent: Int?)

/**
 * The gaps between a layout table's columns and rows in dp, the least a column is ([planColumns]), and
 * whether it is [told] its width.
 */
internal class Gaps(val column: Float, val row: Float, val least: Float, val told: Boolean)

/** The most a layout is, in pixels (Compose packs a size into 24 bits). */
internal const val MAX_LAYOUT = (1 shl 24) - 1

/**
 * Where each cell of a section stands, in pixels: its left edge and width ([lefts], [wides]), the top
 * and height of its slot ([tops], [slots]), whether the section is [stacked], and its size.
 */
internal class Arranged(
    val stacked: Boolean,
    val lefts: IntArray,
    val wides: IntArray,
    val tops: IntArray,
    val slots: IntArray,
    val width: Int,
    val height: Int,
)

/**
 * One section of a layout table ([sectionsOf]) as one layout: its [cells] (in reading order, one child
 * each) side by side in columns planned in the width it is really given, or one under another when
 * they don't fit there, as the desktop decides it, at the real width; [rowGap] between rows and between
 * stacked cells. A row with colours or lines ([tinted]) has every cell as tall as the row: a coloured
 * bar beside a card reaches its foot, a cell's lines meet the row's; never taller than its tallest
 * cell (#2006). A cell over several rows stands in its first.
 *
 * Invariants (LayoutSectionTest, NestedTablesTest, TallRowsTest):
 * - I1, what it says of its size is what it draws. Its intrinsic height at a width is the height
 *   measure gives at that width, and measure gives each cell the slot that height was made of: both
 *   are [arrange] of the same width and the same cell heights. Its intrinsic widths say what it needs
 *   to stand side by side (the cells of a row, with the gaps) and at least (its widest cell's least,
 *   as it can stack), and its own width in pixels where it gives one.
 * - I2, bounded work. A measure, or any one intrinsic, asks each cell each intrinsic at most once.
 * - No measure throws: a cell's height is asked with a fallback and held to [MAX_BOX_HEIGHT] (as a
 *   picture's foot is cut), its width to [MAX_BOX_WIDTH], and the section to [MAX_LAYOUT].
 */
internal class SectionPolicy(
    private val cells: List<PlacedCell>,
    private val specs: List<ColumnSpec>,
    private val gaps: Gaps,
    private val tinted: Set<Int>,
    private val own: OwnWidth,
) : MeasurePolicy {
    private val gap get() = gaps.column
    private val rowGap get() = gaps.row

    /** The width it is laid out in, of [given] pixels: its own where it gives one, never more. */
    private fun Density.roomOf(given: Int): Int {
        val room = if (given == Constraints.Infinity) MAX_BOX_WIDTH else given.coerceIn(0, MAX_BOX_WIDTH)
        val least = CellMinimum.roundToPx().coerceAtMost(room)
        own.dp?.let { return it.dp.roundToPx().coerceIn(least, room) }
        own.percent?.let { return room * it.coerceIn(0, PERCENT) / PERCENT }
        return room
    }

    /** Where the cells stand across [room] pixels, before any is asked anything: lefts, widths, stacked. */
    private fun Density.across(room: Int): Triple<Boolean, IntArray, IntArray> {
        val widths = planColumns(specs, room.toDp().value, gap, gaps.least, gaps.told)
            ?: return Triple(true, IntArray(cells.size), IntArray(cells.size) { room })
        val lefts = IntArray(cells.size)
        val wides = IntArray(cells.size)
        cells.forEachIndexed { i, p ->
            val left = (spanWidth(widths, 0, p.col, gap) + if (p.col > 0) gap else 0f).dp.roundToPx()
            lefts[i] = left.coerceIn(0, room)
            wides[i] = spanWidth(widths, p.col, p.col + p.cols, gap).dp.roundToPx().coerceIn(0, room - lefts[i])
        }
        return Triple(false, lefts, wides)
    }

    /** The section laid out in [given] pixels, each cell [heightOf] (its index, its width) asked once. */
    fun Density.arrange(given: Int, heightOf: (Int, Int) -> Int): Arranged {
        val room = roomOf(given)
        val (stacked, lefts, wides) = across(room)
        val heights = IntArray(cells.size) { heightOf(it, wides[it]).coerceIn(0, MAX_BOX_HEIGHT) }
        val between = rowGap.dp.roundToPx()
        val tops = IntArray(cells.size)
        val slots = IntArray(cells.size)
        var y = 0
        if (stacked) {
            cells.indices.forEach { i ->
                if (i > 0) y = (y + between).coerceAtMost(MAX_LAYOUT)
                tops[i] = y
                slots[i] = heights[i]
                y = (y + heights[i]).coerceAtMost(MAX_LAYOUT)
            }
        } else {
            val byRow = cells.indices.groupBy { cells[it].row }
            byRow.keys.sorted().forEachIndexed { n, r ->
                if (n > 0) y = (y + between).coerceAtMost(MAX_LAYOUT)
                val row = byRow[r].orEmpty()
                val tall = row.maxOfOrNull { heights[it] } ?: 0
                // Each cell's slot is the row: a tinted one fills it, any other stands in it by its valign.
                row.forEach { i ->
                    tops[i] = y
                    slots[i] = tall
                }
                y = (y + tall).coerceAtMost(MAX_LAYOUT)
            }
        }
        return Arranged(stacked, lefts, wides, tops, slots, room, y)
    }

    private fun IntrinsicMeasurable.heightAt(width: Int): Int =
        intrinsic(MAX_BOX_HEIGHT) { minIntrinsicHeight(width) }

    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val all = measurables.take(cells.size)
        val heights = IntArray(all.size)
        val at = arrange(constraints.maxWidth) { i, w -> all[i].heightAt(w).also { heights[i] = it } }
        val placeables = all.mapIndexed { i, m ->
            val w = at.wides[i]
            val tall = !at.stacked && cells[i].row in tinted
            // A tinted row's cells fill it; any other cell is no taller than its slot, which is its own height.
            m.measure(if (tall) Constraints.fixed(w, at.slots[i]) else Constraints(w, w, 0, at.slots[i]))
        }
        val height = constraints.constrainHeight(at.height)
        return layout(at.width.coerceIn(constraints.minWidth, constraints.maxWidth), height) {
            placeables.forEachIndexed { i, pl ->
                val down = valignOf(cells[i].cell.valign).align(pl.height, at.slots[i])
                pl.place(at.lefts[i], (at.tops[i] + down).coerceAtMost(MAX_LAYOUT))
            }
        }
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        arrange(width) { i, w -> measurables.getOrNull(i)?.heightAt(w) ?: 0 }.height

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        minIntrinsicHeight(measurables, width)

    /** Its own width in pixels, where it gives one. */
    private fun Density.told(): Int? = own.dp?.dp?.roundToPx()?.coerceAtMost(MAX_BOX_WIDTH)

    // The cells of each row side by side with the gaps (and the widths their columns are told), the widest row.
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int {
        val wants = measurables.take(cells.size).mapIndexed { i, m ->
            val p = cells[i]
            val fixed = (p.col until p.col + p.cols).sumOf { c -> specs.getOrNull(c)?.asks?.dp?.roundToPx() ?: 0 }
            maxOf(intrinsic(MAX_BOX_WIDTH) { m.maxIntrinsicWidth(height) }, fixed)
        }
        val gaps = gap.dp.roundToPx() * (specs.size - 1).coerceAtLeast(0)
        val rows = cells.indices.groupBy { cells[it].row }.values.maxOfOrNull { row -> row.sumOf { wants[it] } } ?: 0
        val content = (rows + gaps).coerceIn(0, MAX_BOX_WIDTH)
        return told()?.let { maxOf(it, minIntrinsicWidthOf(measurables, height)) } ?: content
    }

    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        minIntrinsicWidthOf(measurables, height)

    // At least its widest cell's least: it can stack.
    private fun minIntrinsicWidthOf(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        measurables.take(cells.size).maxOfOrNull { intrinsic(MAX_BOX_WIDTH) { it.minIntrinsicWidth(height) } }
            ?.coerceIn(0, MAX_BOX_WIDTH) ?: 0
}

private const val PERCENT = 100

/** Where a cell that is shorter than its row stands in it. */
internal fun valignOf(v: VAlignView?): Alignment.Vertical = when (v) {
    VAlignView.MIDDLE -> Alignment.CenterVertically
    VAlignView.BOTTOM -> Alignment.Bottom
    else -> Alignment.Top
}
