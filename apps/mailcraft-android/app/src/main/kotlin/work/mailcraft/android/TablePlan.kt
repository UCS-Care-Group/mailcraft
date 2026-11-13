// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/** A layout table is stacked rather than given columns narrower than this, in dp (about 140). */
internal const val MIN_LAYOUT_COLUMN = 140f

/** The gap between the columns of a layout table that says none, in dp. */
internal const val DEFAULT_COLUMN_GAP = 12f

/** Most columns and cells of one table that are drawn. */
internal const val MAX_COLUMNS = 64
internal const val MAX_CELLS = 5_000

/** A cell in its place in the grid of a table: its row, first column and the columns it covers. */
internal class Placed(val cell: CellView, val row: Int, val col: Int, val cols: Int)

/** The cells of a table in a grid: spans honoured, a cell that a row span covers leaves no gap in its row. */
internal class Grid(val rows: Int, val columns: Int, val placed: List<Placed>)

/** [table]'s cells placed in a grid, within the caps. */
internal fun gridOf(table: TableView): Grid {
    val taken = HashSet<Pair<Int, Int>>()
    val placed = ArrayList<Placed>()
    var columns = 0
    for ((r, row) in table.rows.withIndex()) {
        var col = 0
        for (cell in row.cells) {
            col = firstFree(taken, r, col)
            if (col >= MAX_COLUMNS || placed.size >= MAX_CELLS) break
            val at = place(taken, cell, r, col, table.rows.size)
            placed += at
            col += at.cols
        }
        columns = maxOf(columns, col)
    }
    return Grid(table.rows.size, columns, placed)
}

/** The first column from [from] that no row span from above covers in row [r]. */
private fun firstFree(taken: Set<Pair<Int, Int>>, r: Int, from: Int): Int {
    var col = from
    while ((r to col) in taken && col < MAX_COLUMNS) col++
    return col
}

/** [cell] at row [r], column [col]: its spans clamped, and the places they cover taken. */
private fun place(taken: MutableSet<Pair<Int, Int>>, cell: CellView, r: Int, col: Int, rows: Int): Placed {
    val cols = cell.colspan.toInt().coerceIn(1, MAX_COLUMNS - col)
    val down = cell.rowspan.toInt().coerceIn(1, rows - r)
    for (dr in 0 until down) for (dc in 0 until cols) taken.add((r + dr) to (col + dc))
    return Placed(cell, r, col, cols)
}

/** What a column of a layout table asks for. */
internal class ColumnSpec(
    /** Some cell of one column wide holds something. A column that holds nothing only makes room. */
    val holds: Boolean,
    /** A width in dp the cells ask for. */
    val fixed: Float?,
    /** A share of the table's width the cells ask for. */
    val percent: Int?,
)

internal fun columnSpecs(grid: Grid): List<ColumnSpec> = (0 until grid.columns).map { c ->
    val own = grid.placed.filter { it.col == c && it.cols == 1 }
    val widths = own.mapNotNull { it.cell.width }
    ColumnSpec(
        holds = own.any { it.cell.blocks.isNotEmpty() },
        fixed = widths.filterIsInstance<LengthView.Px>().maxOfOrNull { it.px.toFloat() },
        percent = widths.filterIsInstance<LengthView.Percent>().maxOfOrNull { it.percent.toInt() },
    )
}

/**
 * The widths in dp of the columns of a layout table in [room] dp, with [gap] between them, or null
 * when the table is to be stacked: its cells one under the other in reading order. It is stacked when
 * what has a width of its own does not fit, or when a column that holds something (one that asks for a
 * share of the width, or takes what is left) would be narrower than [MIN_LAYOUT_COLUMN]. A column
 * that asks for a width in pixels is not squeezed (a row of icons stays a row), and one that holds
 * nothing takes what it asks for. A share is a share of what the gaps leave.
 */
internal fun planColumns(specs: List<ColumnSpec>, room: Float, gap: Float): List<Float>? {
    val n = specs.size
    if (n <= 1) return specs.map { it.fixed?.coerceAtMost(room) ?: room }
    val gaps = gap * (n - 1)
    val widths = declaredWidths(specs, (room - gaps).coerceAtLeast(0f)).toMutableList()
    val auto = specs.indices.filter { specs[it].fixed == null && specs[it].percent == null && specs[it].holds }
    val left = (room - gaps - widths.sum()).coerceAtLeast(0f)
    auto.forEach { widths[it] = left / auto.size }
    val squeezed = specs.indices.any { specs[it].holds && specs[it].fixed == null && widths[it] < MIN_LAYOUT_COLUMN }
    val over = widths.sum() + gaps > room + HALF
    return if (squeezed || over) null else widths
}

/** What the columns ask for, in dp: a width, or a share of [inner]; 0 for those that take what is left. */
private fun declaredWidths(specs: List<ColumnSpec>, inner: Float): List<Float> = specs.map { c ->
    c.fixed ?: c.percent?.let { inner * it.coerceIn(0, PERCENT_MAX) / PERCENT_MAX } ?: 0f
}

private const val PERCENT_MAX = 100
private const val HALF = 0.5
