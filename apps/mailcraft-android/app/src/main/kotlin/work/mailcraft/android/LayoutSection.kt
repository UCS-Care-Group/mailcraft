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
import androidx.compose.ui.layout.Placeable
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
 * cell (#2006). Any other row is as tall as its cells are drawn, each measured with no limit to its
 * height, so a long message in a wrapper table is drawn whole. A cell over several rows stands in its
 * first.
 *
 * Invariants (TableSizeTest, NestedTablesTest, TallRowsTest):
 * - I1, what it says of its size is what it draws, where each cell's own intrinsics say what that
 *   cell draws. Its intrinsic height at a width is [arrange] of that width and the cells' intrinsic
 *   heights; measure arranges the same width with those heights for a row with colour and with the
 *   heights the cells are drawn at for any other. The children a cell holds are made to keep their
 *   side of it (text; pictures and text in a line and boxes on a line through [BoxesPolicy]; tables
 *   in it through this policy and the data table's), and TableSizeTest's matrix checks each of them in
 *   a coloured, a plain and a data cell at four widths. Its intrinsic widths say what it needs to stand
 *   side by side (the cells of a row, with the gaps) and at least (its widest cell's least, as it can
 *   stack), and its own width in pixels where it gives one, widened to that least.
 * - I2, bounded work. A measure, or any one intrinsic, asks each cell each intrinsic at most once.
 * - No measure throws: a cell's height is asked with a fallback, a cell that fills a row with colour
 *   is held to [MAX_BOX_HEIGHT] (as a picture's foot is cut), a cell's width to [MAX_BOX_WIDTH], and the
 *   section to [MAX_STACK] (I3).
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

    /**
     * The width it is laid out in, of [given] pixels: its own where it gives one, never more; one in
     * pixels narrower than what its widest cell needs at least ([needs]) is that wide, as on the desktop.
     */
    private fun Density.roomOf(given: Int, needs: () -> Int): Int {
        val room = if (given == Constraints.Infinity) MAX_BOX_WIDTH else given.coerceIn(0, MAX_BOX_WIDTH)
        val least = CellMinimum.roundToPx().coerceAtMost(room)
        own.dp?.let { return maxOf(it.dp.roundToPx(), needs()).coerceIn(least, room) }
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

    /**
     * The section laid out in [given] pixels, each cell [heightOf] (its index, its width, whether it
     * fills a row with colour) asked once, and its widest cell's least width [needs] at most once.
     */
    fun Density.arrange(given: Int, needs: () -> Int, heightOf: (Int, Int, Boolean) -> Int): Arranged {
        val room = roomOf(given, needs)
        val (stacked, lefts, wides) = across(room)
        val heights = IntArray(cells.size) {
            heightOf(it, wides[it], !stacked && cells[it].row in tinted).coerceIn(0, MAX_STACK)
        }
        val between = rowGap.dp.roundToPx()
        val tops = IntArray(cells.size)
        val slots = IntArray(cells.size)
        var y = 0
        if (stacked) {
            cells.indices.forEach { i ->
                if (i > 0) y = (y + between).coerceAtMost(MAX_STACK)
                tops[i] = y
                slots[i] = heights[i]
                y = (y + heights[i]).coerceAtMost(MAX_STACK)
            }
        } else {
            val byRow = cells.indices.groupBy { cells[it].row }
            byRow.keys.sorted().forEachIndexed { n, r ->
                if (n > 0) y = (y + between).coerceAtMost(MAX_STACK)
                val row = byRow[r].orEmpty()
                val tall = row.maxOfOrNull { heights[it] } ?: 0
                // Each cell's slot is the row: a tinted one fills it, any other stands in it by its valign.
                row.forEach { i ->
                    tops[i] = y
                    slots[i] = tall
                }
                y = (y + tall).coerceAtMost(MAX_STACK)
            }
        }
        return Arranged(stacked, lefts, wides, tops, slots, room, y)
    }

    /** A cell's height at [width]: held to [MAX_BOX_HEIGHT] where it [fills] its row, a fixed measure. */
    private fun IntrinsicMeasurable.heightAt(width: Int, fills: Boolean): Int =
        intrinsic(MAX_BOX_HEIGHT) { minIntrinsicHeight(width) }.coerceAtMost(if (fills) MAX_BOX_HEIGHT else MAX_STACK)

    private fun needsOf(measurables: List<IntrinsicMeasurable>): () -> Int =
        { minIntrinsicWidthOf(measurables, Constraints.Infinity) }

    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val all = measurables.take(cells.size)
        val drawn = arrayOfNulls<Placeable>(all.size)
        // A cell that fills a row with colour is measured at the row's height, so its own is asked first;
        // any other is measured as tall as it is (the row is the tallest of them), so nothing is cut.
        val at = arrange(constraints.maxWidth, needsOf(all)) { i, w, fills ->
            val m = all.getOrNull(i)
            when {
                m == null -> 0
                fills -> m.heightAt(w, fills = true)
                else -> m.measure(Constraints(w, w, 0, Constraints.Infinity)).also { drawn[i] = it }.height
            }
        }
        val placeables = all.mapIndexed { i, m -> drawn[i] ?: m.measure(Constraints.fixed(at.wides[i], at.slots[i])) }
        val height = constraints.constrainHeight(at.height)
        return layout(at.width.coerceIn(constraints.minWidth, constraints.maxWidth), height) {
            placeables.forEachIndexed { i, pl ->
                val down = valignOf(cells[i].cell.valign).align(pl.height, at.slots[i])
                pl.place(at.lefts[i], (at.tops[i] + down).coerceAtMost(MAX_LAYOUT))
            }
        }
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        arrange(width, needsOf(measurables)) { i, w, fills -> measurables.getOrNull(i)?.heightAt(w, fills) ?: 0 }.height

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

/**
 * Blocks one under another, [gapDp] apart (a cell's blocks, a layout table's sections), as a column
 * held to [MAX_STACK] (I3): each is measured with what room is left of a bounded height, and none is
 * measured or drawn once the stack is that tall, so the rest of a hostile message is cut at its foot,
 * never a throw. Its intrinsic height adds up what its blocks say the same way.
 */
internal class StackPolicy(private val gapDp: Float) : MeasurePolicy {
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val gap = gapDp.dp.roundToPx()
        val placeables = ArrayList<Placeable>(measurables.size)
        val tops = ArrayList<Int>(measurables.size)
        var y = 0
        for (m in measurables) {
            if (y >= MAX_STACK) break
            val top = if (placeables.isEmpty()) 0 else (y + gap).coerceAtMost(MAX_STACK)
            val left = if (constraints.hasBoundedHeight) {
                (constraints.maxHeight - top).coerceAtLeast(0)
            } else {
                Constraints.Infinity
            }
            val pl = m.measure(constraints.copy(minWidth = 0, minHeight = 0, maxHeight = left))
            placeables += pl
            tops += top
            y = (top.toLong() + pl.height).coerceAtMost(MAX_STACK.toLong()).toInt()
        }
        val width = (placeables.maxOfOrNull { it.width } ?: 0).coerceIn(constraints.minWidth, constraints.maxWidth)
        return layout(width, y.coerceIn(constraints.minHeight, constraints.maxHeight)) {
            placeables.forEachIndexed { i, pl -> pl.place(0, tops.getOrElse(i) { 0 }) }
        }
    }

    private fun IntrinsicMeasureScope.heightOf(
        measurables: List<IntrinsicMeasurable>,
        ask: (IntrinsicMeasurable) -> Int,
    ): Int {
        val gap = gapDp.dp.roundToPx()
        var y = 0L
        for ((i, m) in measurables.withIndex()) {
            if (y >= MAX_STACK) break
            y += (if (i > 0) gap else 0) + intrinsic(MAX_STACK) { ask(m) }.coerceIn(0, MAX_STACK)
        }
        return y.coerceAtMost(MAX_STACK.toLong()).toInt()
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        heightOf(measurables) { it.minIntrinsicHeight(width) }

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        heightOf(measurables) { it.maxIntrinsicHeight(width) }

    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        measurables.maxOfOrNull { m -> intrinsic(0) { m.minIntrinsicWidth(height) }.coerceIn(0, MAX_BOX_WIDTH) } ?: 0

    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        measurables.maxOfOrNull { m -> intrinsic(0) { m.maxIntrinsicWidth(height) }.coerceIn(0, MAX_BOX_WIDTH) } ?: 0
}
