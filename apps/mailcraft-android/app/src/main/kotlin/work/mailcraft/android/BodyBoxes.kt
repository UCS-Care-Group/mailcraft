// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Layout
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

/** Boxes on a line (tables the message lays out `inline-block`, or floats to a side), placed by [placeBoxes]. */
@Composable
internal fun BoxesLine(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    val boxes = blocks.mapNotNull { b -> b.box()?.let { b to it } }
    val right = boxes.map { (b, t) -> t.float && b.align == AlignView.RIGHT }
    val floats = boxes.any { it.second.float }
    val align = blocks.first().align
    Layout(
        modifier = Modifier.fillMaxWidth(),
        content = { boxes.forEach { (b, t) -> TableBlock(t, b.align, env, paper, width) } },
    ) { measurables, constraints ->
        val placeables = measurables.map { it.measure(constraints.copy(minWidth = 0, minHeight = 0)) }
        val widths = placeables.map { it.width }
        val room = if (constraints.hasBoundedWidth) constraints.maxWidth else widths.sum()
        val gaps = boxes.map { it.second.gapBefore.toInt().dp.roundToPx() }
        val spots = placeBoxes(widths, gaps, right, room, align, floats)
        val lines = (spots.maxOfOrNull { it.second } ?: -1) + 1
        val heights = IntArray(lines)
        spots.forEachIndexed { n, (_, l) -> heights[l] = maxOf(heights[l], placeables[n].height) }
        val between = RUN_GAP.dp.roundToPx()
        val tops = IntArray(lines)
        for (l in 1 until lines) tops[l] = tops[l - 1] + heights[l - 1] + between
        val height = if (lines == 0) 0 else tops[lines - 1] + heights[lines - 1]
        val w = room.coerceIn(constraints.minWidth, constraints.maxWidth)
        layout(w, height.coerceIn(constraints.minHeight, constraints.maxHeight)) {
            spots.forEachIndexed { n, (x, l) -> placeables[n].place(x, tops[l]) }
        }
    }
}

/** The gap in dp between the columns of [table]: its own, or the reader's. */
internal fun columnGap(table: TableView): Float = table.spacing?.toInt()?.toFloat() ?: DEFAULT_COLUMN_GAP

/**
 * The room in dp a box on a line keeps free left and right of it (its margins), within [room]: at
 * least [CellMinimum] is left for the box, as on the desktop.
 */
internal fun boxMargins(table: TableView, room: Float): Pair<Float, Float> {
    val left = table.boxMarginLeft.toFloat()
    val right = table.boxMarginRight.toFloat()
    val most = (room - CellMinimum.value).coerceAtLeast(0f)
    val scale = if (left + right > most && left + right > 0f) most / (left + right) else 1f
    return left * scale to right * scale
}

/**
 * The width a table takes of [room]: what it says; a box on a line that says nothing is as wide as
 * the widths its columns ask for ([boxWidth]), or the line when a column takes what is left.
 */
internal fun drawnWidth(table: TableView, room: Dp): Dp {
    if (!table.inline || table.width != null) return tableWidth(table, room)
    return boxWidth(table, columnGap(table))?.dp?.coerceIn(CellMinimum.coerceAtMost(room), room) ?: room
}

/**
 * The width in dp of a box on a line (an inline or floated table) that does not say how wide it is,
 * where every column asks for a width (a social icon in its cell): what they ask for and the gaps.
 * Null when a column takes what is left: such a box takes the line.
 */
internal fun boxWidth(table: TableView, gap: Float): Float? {
    val specs = columnSpecs(gridOf(table))
    if (specs.isEmpty() || specs.any { it.holds && it.asks == null }) return null
    return specs.sumOf { (it.asks ?: 0f).toDouble() }.toFloat() + gap * (specs.size - 1)
}
