// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.unit.Dp
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.CellView

private const val PERCENT = 100f
private const val TINY = 1e-3f

/**
 * What one cell asks of the columns it covers, in pixels: [min] it can't be narrower than without
 * breaking a word, [max] it takes with all the room it wants, and a width ([px]) or a share of the
 * table ([percent]) where the message gives one. As on the desktop, only a cell of one column sets its
 * column's width that way: a cell over several gives them its words' widths alone.
 */
internal class CellNeed(
    val col: Int,
    val cols: Int,
    val min: Float,
    val max: Float,
    val px: Float? = null,
    val percent: Int? = null,
)

/** Each column's least and wanted width, whether the message fixed it, and the share it asks for. */
private class ColumnNeeds(n: Int) {
    val min = FloatArray(n)
    val max = FloatArray(n)
    val fixed = BooleanArray(n)
    val share = FloatArray(n)
}

private fun columnNeeds(needs: List<CellNeed>, n: Int): ColumnNeeds {
    val out = ColumnNeeds(n)
    for (c in needs.filter { it.cols == 1 && it.col in 0 until n }) {
        val most = if (c.px != null) maxOf(c.px, c.min) else maxOf(c.max, c.min)
        if (c.px != null) out.fixed[c.col] = true
        c.percent?.let { out.share[c.col] = maxOf(out.share[c.col], it.coerceIn(0, PERCENT.toInt()).toFloat()) }
        out.min[c.col] = maxOf(out.min[c.col], c.min)
        out.max[c.col] = maxOf(out.max[c.col], most)
    }
    // The columns a cell spans hold it together: what they lack is shared out among them.
    for (c in needs.filter { it.cols > 1 && it.col in 0 until n }) {
        val span = c.col until minOf(c.col + c.cols, n)
        spread(out.min, span, c.min)
        spread(out.max, span, maxOf(c.max, c.min))
    }
    for (i in 0 until n) out.max[i] = maxOf(out.max[i], out.min[i])
    return out
}

/** What the columns of [span] lack of [need] together, shared out among them. */
private fun spread(values: FloatArray, span: IntRange, need: Float) {
    val have = span.sumOf { values[it].toDouble() }.toFloat()
    if (need > have) span.forEach { values[it] += (need - have) / span.count() }
}

/**
 * The width of each of [columns] columns of a data table, by the reading pane's rule: each column
 * gets what its cells want when that fits in [room] (or in the width the table asks for, [asked]),
 * and is squeezed toward the least its words need when not; never below that, so a table too wide
 * for the screen scrolls sideways instead.
 */
internal fun dataColumnWidths(needs: List<CellNeed>, columns: Int, room: Float, asked: Float?): FloatArray {
    val n = columns.coerceIn(0, MAX_COLUMNS)
    if (n == 0) return FloatArray(0)
    val c = columnNeeds(needs, n)
    val total = maxOf(asked ?: minOf(c.max.sum(), room), c.min.sum())
    for (i in 0 until n) {
        if (c.share[i] > 0f) {
            c.max[i] = maxOf(total * c.share[i] / PERCENT, c.min[i])
            c.fixed[i] = true
        }
    }
    val sumMax = c.max.sum()
    val sumMin = c.min.sum()
    if (sumMax > total) {
        val stretch = maxOf(sumMax - sumMin, TINY)
        return FloatArray(n) { i -> c.min[i] + (total - sumMin) * (c.max[i] - c.min[i]) / stretch }
    }
    // Room to spare goes to the columns that were not told their width, by what they hold.
    val spare = total - sumMax
    val flexible = (0 until n).filter { !c.fixed[it] }.sumOf { c.max[it].toDouble() }.toFloat()
    return FloatArray(n) { i ->
        when {
            flexible > 0f -> if (c.fixed[i]) c.max[i] else c.max[i] + spare * c.max[i] / flexible
            sumMax > 0f -> c.max[i] + spare * c.max[i] / sumMax
            else -> total / n
        }
    }
}

/**
 * The top of each of [rows] rows and the bottom of the last, in pixels, from the height each placed
 * cell needs ([heights], by cell): a row is as tall as the cells that end in it need, so a cell over
 * several rows makes only the last of them taller, as in the reading pane. No top is past [cap]
 * ([tallestFor] the table's width), which a measure always takes.
 */
internal fun dataRowTops(heights: List<Pair<PlacedCell, Int>>, rows: Int, cap: Int = MAX_BOX_HEIGHT): IntArray {
    val n = rows.coerceAtLeast(0)
    val tops = IntArray(n + 1)
    val ending = heights.groupBy { (p, _) -> p.row + p.rows - 1 }
    for (r in 0 until n) {
        val bottom = ending[r].orEmpty().maxOfOrNull { (p, h) -> tops.at(p.row) + h.coerceIn(0, cap) } ?: 0
        // lean: a table taller than this is cut at its foot, as a picture is; drawn in pieces if one ever needs more.
        tops[r + 1] = maxOf(tops[r], bottom).coerceAtMost(cap)
    }
    return tops
}

/**
 * The widest a data table is drawn, in pixels. A measure packs a width and a height into 31 bits, so
 * one this wide may be at most [WIDE_TABLE_HEIGHT] tall; one up to [MAX_BOX_WIDTH], [MAX_BOX_HEIGHT].
 * lean: past these a table is cut (its last columns, its foot); drawn in pieces if one ever needs more.
 */
internal const val WIDE_TABLE_WIDTH = 32_000

/** The tallest a data table wider than [MAX_BOX_WIDTH] is drawn, in pixels. */
internal const val WIDE_TABLE_HEIGHT = 65_000

/** The tallest a data table [width] pixels wide may be drawn, beside that width in one measure. */
internal fun tallestFor(width: Int): Int = if (width <= MAX_BOX_WIDTH) MAX_BOX_HEIGHT else WIDE_TABLE_HEIGHT

/**
 * Where the column edges of [widths] lie, rounded: the left edge of each column, and the right edge of
 * the last, never past [WIDE_TABLE_WIDTH].
 */
internal fun edgesOf(widths: FloatArray): IntArray {
    val edges = IntArray(widths.size + 1)
    var at = 0f
    widths.forEachIndexed { i, w ->
        at += w
        // A column that would end past it is left out whole, not drawn as a sliver.
        edges[i + 1] = Math.round(at).takeIf { it in 0..WIDE_TABLE_WIDTH } ?: edges[i]
    }
    return edges
}

/**
 * The room a cell over [cols] of [columns] columns plans a layout table in (whether its columns stack):
 * its even share of [room], all of it for a cell across the table. Pictures and the rest keep [room].
 * lean: an even share, as the columns' own widths are known only once measured; plan by those
 * (measure, then compose again) if a table in a data table ever needs it.
 */
internal fun cellRoom(room: Dp, cols: Int, columns: Int): Dp =
    room * cols.coerceIn(1, columns.coerceAtLeast(1)) / columns.coerceAtLeast(1)

/**
 * The least width in dp the pictures of [cell] ask of its column, as on the desktop: one that came
 * ([shown]) and says its width keeps up to [PICTURE_KEEPS] dp of it, any other gives way to [PICTURE_LEAST];
 * null when the cell holds no picture.
 */
internal fun picturesLeast(cell: CellView, shown: (BlockKindView.Image) -> Boolean): Float? =
    cell.blocks.mapNotNull { it.kind as? BlockKindView.Image }.maxOfOrNull { k ->
        val wanted = k.width?.toFloat() ?: PICTURE_LEAST
        wanted.coerceAtMost(if (k.width != null && shown(k)) PICTURE_KEEPS else PICTURE_LEAST)
    }

private const val PICTURE_KEEPS = 160f
private const val PICTURE_LEAST = 48f

/** The value at [i], or 0 past the ends. */
internal fun IntArray.at(i: Int): Int = getOrNull(i) ?: 0

/** How far it is from the value at [from] to the one at [until], never less than nothing. */
internal fun IntArray.between(from: Int, until: Int): Int = (at(until) - at(from)).coerceAtLeast(0)
