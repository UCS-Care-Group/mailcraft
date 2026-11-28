// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.IntrinsicMeasurable
import androidx.compose.ui.layout.IntrinsicMeasureScope
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasurePolicy
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.TableView

// Boxes on a line: tables a message lays out `inline-block` or floats to a side, as the desktop does (#1966).

/** How many boxes from [start] stand in one line [room] wide: as many as fit, at least one. */
private fun lineEnd(widths: List<Int>, gaps: List<Int>, start: Int, room: Int): Pair<Int, Int> {
    var end = start
    var used = 0
    while (end < widths.size) {
        val next = used + (if (end > start) gaps.getOrElse(end) { 0 } else 0) + widths[end]
        if (end > start && next > room) break
        used = next
        end++
    }
    return end to used
}

/** The left edges of boxes [range] in one line: from [from] on, those floated [right] from [room] in. */
@Suppress("LongParameterList")
private fun placeLine(
    widths: List<Int>,
    gaps: List<Int>,
    right: List<Boolean>,
    range: IntRange,
    from: Int,
    room: Int,
): List<Int> {
    var at = from
    var rightAt = room
    return range.map { n ->
        if (right.getOrElse(n) { false }) {
            rightAt -= widths[n]
            rightAt.coerceAtLeast(0)
        } else {
            if (n > range.first) at += gaps.getOrElse(n) { 0 }
            at.also { at += widths[n] }
        }
    }
}

/**
 * Where boxes [widths] wide stand in a line [room] wide, as the desktop places them: each its left edge
 * and its line (the first is 0). In source order, as many to a line as fit (at least one), each after
 * the first with its gap ([gaps]) before it. Those floated [right] stand from the right edge of their
 * line in, the first outermost; the others from the left, or where [align] puts the line when none of
 * the run [floats].
 */
@Suppress("LongParameterList")
internal fun placeBoxes(
    widths: List<Int>,
    gaps: List<Int>,
    right: List<Boolean>,
    room: Int,
    align: AlignView,
    floats: Boolean,
): List<Pair<Int, Int>> {
    val out = ArrayList<Pair<Int, Int>>(widths.size)
    var start = 0
    var line = 0
    while (start < widths.size) {
        val (end, used) = lineEnd(widths, gaps, start, room)
        val at = when {
            floats || align == AlignView.LEFT -> 0
            align == AlignView.CENTER -> ((room - used) / 2).coerceAtLeast(0)
            else -> (room - used).coerceAtLeast(0)
        }
        placeLine(widths, gaps, right, start until end, at, room).forEach { out += it to line }
        line++
        start = end
    }
    return out
}

private fun BlockView.box(): TableView? = (kind as? BlockKindView.Table)?.table

/**
 * How boxes on a line, or the pictures and text of one line, are measured and placed: by [placeBoxes],
 * each [gapsDp] after the one before it and [lineGapDp] between lines. Each is as wide as it says it
 * is at most, within the line, as the desktop's `boxes_line` draws a box that does not say its width,
 * and is placed by that width: so asked for its height at a width (a row of a table with colour sizes
 * its cells so), it answers with the lines the measure makes there (#1966).
 */
internal class BoxesPolicy(
    private val gapsDp: List<Int>,
    private val right: List<Boolean>,
    private val align: AlignView,
    private val floats: Boolean,
    private val lineGapDp: Float = RUN_GAP,
) : MeasurePolicy {
    /** How wide each of [measurables] stands in a line [room] px wide: what it says, within the line. */
    private fun widthsIn(measurables: List<IntrinsicMeasurable>, room: Int): List<Int> =
        measurables.map { intrinsic(room) { it.maxIntrinsicWidth(MAX_BOX_HEIGHT) }.coerceIn(0, room) }

    /** The room of a line [given] px wide: past the most a box is, or unbounded, the boxes side by side. */
    private fun IntrinsicMeasureScope.roomOf(measurables: List<IntrinsicMeasurable>, given: Int): Int =
        if (given in 0..MAX_BOX_WIDTH) {
            given
        } else {
            boxesWidth(widthsIn(measurables, MAX_BOX_WIDTH), gaps()).coerceAtMost(MAX_BOX_WIDTH)
        }

    private fun IntrinsicMeasureScope.gaps() = gapsDp.map { it.dp.roundToPx() }

    /** The tops of the lines of boxes at [spots], [heights] high, and the height of them all. */
    private fun IntrinsicMeasureScope.lines(spots: List<Pair<Int, Int>>, heights: List<Int>) =
        lineTops(lineHeights(spots, heights), lineGapDp.dp.roundToPx())

    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val room = roomOf(measurables, if (constraints.hasBoundedWidth) constraints.maxWidth else -1)
        val widths = widthsIn(measurables, room)
        val placeables = measurables.mapIndexed { n, m ->
            m.measure(Constraints(0, widths.getOrElse(n) { 0 }, 0, constraints.maxHeight))
        }
        val spots = placeBoxes(widths, gaps(), right, room, align, floats)
        val (tops, height) = lines(spots, placeables.map { it.height })
        val w = room.coerceIn(constraints.minWidth, constraints.maxWidth)
        return layout(w, height.coerceIn(constraints.minHeight, constraints.maxHeight)) {
            spots.forEachIndexed { n, (x, l) -> placeables.getOrNull(n)?.place(x, tops.getOrElse(l) { 0 }) }
        }
    }

    private fun IntrinsicMeasureScope.heightAt(measurables: List<IntrinsicMeasurable>, width: Int, max: Boolean): Int {
        val room = roomOf(measurables, width)
        val widths = widthsIn(measurables, room)
        val heights = measurables.mapIndexed { n, m ->
            val w = widths.getOrElse(n) { 0 }
            intrinsic(MAX_BOX_HEIGHT) { if (max) m.maxIntrinsicHeight(w) else m.minIntrinsicHeight(w) }
                .coerceIn(0, MAX_STACK)
        }
        return lines(placeBoxes(widths, gaps(), right, room, align, floats), heights).second
    }

    // Side by side they are as wide as all of them and the gaps; wrapped, as the widest.
    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        boxesWidth(widthsIn(measurables, MAX_BOX_WIDTH), gaps()).coerceAtMost(MAX_BOX_WIDTH)

    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int) =
        measurables.maxOfOrNull { m -> intrinsic(0) { m.minIntrinsicWidth(MAX_BOX_HEIGHT) }.coerceIn(0, MAX_BOX_WIDTH) }
            ?: 0

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        heightAt(measurables, width, max = false)

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        heightAt(measurables, width, max = true)
}

/** Boxes on a line (tables the message lays out `inline-block`, or floats to a side), placed by [placeBoxes]. */
@Composable
internal fun BoxesLine(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    val boxes = remember(blocks) { blocks.mapNotNull { b -> b.box()?.let { b to it } } }
    val policy = remember(blocks) {
        BoxesPolicy(
            gapsDp = boxes.map { it.second.gapBefore.toInt() },
            right = boxes.map { (b, t) -> t.float && b.align == AlignView.RIGHT },
            align = blocks.first().align,
            floats = boxes.any { it.second.float },
        )
    }
    Layout(
        modifier = Modifier.fillMaxWidth(),
        content = { boxes.forEach { (b, t) -> BoxOnALine(t, b.align, env, paper, width) } },
        measurePolicy = policy,
    )
}

/**
 * The width a table takes of [room]: what it says; a box on a line that says nothing is as wide as
 * the widths its columns ask for ([boxWidth]), or the line when a column takes what is left.
 */
internal fun drawnWidth(table: TableView, room: Dp): Dp {
    if (!table.inline || table.width != null) return tableWidth(table, room)
    return boxWidth(table, gapsOf(table).first)?.dp?.coerceIn(CellMinimum.coerceAtMost(room), room) ?: room
}

/**
 * The width in dp of a box on a line (an inline or floated table) that does not say how wide it is,
 * where every column asks for a width (a social icon in its cell): what they ask for and the gaps.
 * Null when a column takes what is left: such a box takes the line.
 */
internal fun boxWidth(table: TableView, gap: Float): Float? {
    val specs = columnSpecs(gridOf(table), table)
    if (specs.isEmpty() || specs.any { it.holds && it.asks == null }) return null
    return specs.sumOf { (it.asks ?: 0f).toDouble() }.toFloat() + gap * (specs.size - 1)
}
