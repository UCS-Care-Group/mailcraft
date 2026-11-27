// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
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

/** The tops of lines of boxes [heights] high, [between] apart, and the height of them all. */
internal fun lineTops(heights: IntArray, between: Int): Pair<IntArray, Int> {
    val tops = IntArray(heights.size)
    for (l in 1 until heights.size) {
        tops[l] = (tops[l - 1] + heights[l - 1].coerceIn(0, MAX_BOX_HEIGHT) + between).coerceAtMost(MAX_BOX_HEIGHT)
    }
    val all = if (heights.isEmpty()) 0 else tops.last() + heights.last()
    return tops to all.coerceIn(0, MAX_BOX_HEIGHT)
}

/** The height of each line of boxes placed at [spots] that are [heights] high. */
internal fun lineHeights(spots: List<Pair<Int, Int>>, heights: List<Int>): IntArray {
    val lines = IntArray((spots.maxOfOrNull { it.second } ?: -1) + 1)
    spots.forEachIndexed { n, (_, l) -> lines[l] = maxOf(lines[l], heights.getOrElse(n) { 0 }) }
    return lines
}

/**
 * How boxes on a line are measured and placed: by [placeBoxes]. Asked for its height at a width (a row
 * of a table with colour sizes its cells so), it answers from the boxes' own widths, not the whole
 * width a stand-in for each would report.
 */
private class BoxesPolicy(
    val gapsDp: List<Int>,
    val right: List<Boolean>,
    val align: AlignView,
    val floats: Boolean,
) : MeasurePolicy {
    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val placeables = measurables.map { it.measure(constraints.copy(minWidth = 0, minHeight = 0)) }
        val widths = placeables.map { it.width }
        val room = (if (constraints.hasBoundedWidth) constraints.maxWidth else widths.sum()).coerceIn(0, MAX_BOX_WIDTH)
        val spots = placeBoxes(widths, gapsDp.map { it.dp.roundToPx() }, right, room, align, floats)
        val (tops, height) = lineTops(lineHeights(spots, placeables.map { it.height }), RUN_GAP.dp.roundToPx())
        val w = room.coerceIn(constraints.minWidth, constraints.maxWidth)
        return layout(w, height.coerceIn(constraints.minHeight, constraints.maxHeight)) {
            spots.forEachIndexed { n, (x, l) -> placeables[n].place(x, tops[l]) }
        }
    }

    private fun IntrinsicMeasureScope.heightAt(measurables: List<IntrinsicMeasurable>, width: Int, max: Boolean): Int {
        val room = width.coerceIn(0, MAX_BOX_WIDTH)
        val widths = measurables.map { it.maxIntrinsicWidth(MAX_BOX_HEIGHT).coerceIn(0, room) }
        val heights = measurables.mapIndexed { n, m ->
            val h = if (max) m.maxIntrinsicHeight(widths[n]) else m.minIntrinsicHeight(widths[n])
            h.coerceIn(0, MAX_BOX_HEIGHT)
        }
        val spots = placeBoxes(widths, gapsDp.map { it.dp.roundToPx() }, right, room, align, floats)
        return lineTops(lineHeights(spots, heights), RUN_GAP.dp.roundToPx()).second
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        heightAt(measurables, width, max = false)

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int) =
        heightAt(measurables, width, max = true)
}

/** Boxes on a line (tables the message lays out `inline-block`, or floats to a side), placed by [placeBoxes]. */
@Composable
internal fun BoxesLine(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    val boxes = blocks.mapNotNull { b -> b.box()?.let { b to it } }
    val policy = BoxesPolicy(
        gapsDp = boxes.map { it.second.gapBefore.toInt() },
        right = boxes.map { (b, t) -> t.float && b.align == AlignView.RIGHT },
        align = blocks.first().align,
        floats = boxes.any { it.second.float },
    )
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
