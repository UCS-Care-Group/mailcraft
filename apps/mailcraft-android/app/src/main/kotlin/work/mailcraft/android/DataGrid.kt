// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

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
 * several rows makes only the last of them taller, as in the reading pane. No top is past
 * [MAX_BOX_HEIGHT], which a measure always takes.
 */
internal fun dataRowTops(heights: List<Pair<PlacedCell, Int>>, rows: Int): IntArray {
    val n = rows.coerceAtLeast(0)
    val tops = IntArray(n + 1)
    val ending = heights.groupBy { (p, _) -> p.row + p.rows - 1 }
    for (r in 0 until n) {
        val bottom = ending[r].orEmpty().maxOfOrNull { (p, h) -> tops.at(p.row) + h.coerceIn(0, MAX_BOX_HEIGHT) } ?: 0
        // lean: a table taller than this is cut at its foot, as a picture is; drawn in pieces if one ever needs more.
        tops[r + 1] = maxOf(tops[r], bottom).coerceAtMost(MAX_BOX_HEIGHT)
    }
    return tops
}

/**
 * Where the column edges of [widths] lie, rounded: the left edge of each column, and the right edge of
 * the last, never past [MAX_BOX_WIDTH].
 */
internal fun edgesOf(widths: FloatArray): IntArray {
    val edges = IntArray(widths.size + 1)
    var at = 0f
    widths.forEachIndexed { i, w ->
        at += w
        edges[i + 1] = Math.round(at).coerceIn(0, MAX_BOX_WIDTH)
    }
    return edges
}

/** The value at [i], or 0 past the ends. */
internal fun IntArray.at(i: Int): Int = getOrNull(i) ?: 0

/** How far it is from the value at [from] to the one at [until], never less than nothing. */
internal fun IntArray.between(from: Int, until: Int): Int = (at(until) - at(from)).coerceAtLeast(0)
