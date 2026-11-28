// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/** The gap between the columns of a layout table that says none, in dp. */
internal const val DEFAULT_COLUMN_GAP = 12f

/** Most columns and cells of one table that are drawn. */
internal const val MAX_COLUMNS = 64
internal const val MAX_CELLS = 5_000

/** Most rows of one grid that are drawn: a table keeps 1,000, and a cell with no room in its row starts another. */
internal const val MAX_GRID_ROWS = 10_000

/** Most rows a cell may span, as the core holds it. */
internal const val MAX_ROW_SPAN = 200

/** Most places in a grid that cells may cover in all: past it the rest of the table is left out. */
private const val MAX_SLOTS = 200_000

/** A cell in its place in the grid of a table: its row, first column, the columns and the rows it covers. */
internal class PlacedCell(val cell: CellView, val row: Int, val col: Int, val cols: Int, val rows: Int = 1)

/** The cells of a table in a grid: spans honoured, a cell that a row span covers leaves no gap in its row. */
internal class Grid(val rows: Int, val columns: Int, val placed: List<PlacedCell>)

/**
 * [table]'s cells in the places the core gives them (the reading pane's own rule), within the caps.
 * The core never lays one cell over another; a cell that would be, or that lies past the caps, is left out.
 */
internal fun gridOf(table: TableView): Grid {
    val taken = HashSet<Long>()
    val placed = ArrayList<PlacedCell>()
    var rows = 0
    var columns = 0
    for (p in table.rows.asSequence().flatMap { it.cells }.mapNotNull(::placedWithinCaps)) {
        if (placed.size >= MAX_CELLS) break
        val slots = (p.row until p.row + p.rows).flatMap { y ->
            (p.col until p.col + p.cols).map { x -> y.toLong() * MAX_COLUMNS + x }
        }
        if (taken.size + slots.size <= MAX_SLOTS && slots.none { it in taken }) {
            taken.addAll(slots)
            placed += p
            rows = maxOf(rows, p.row + 1)
            columns = maxOf(columns, p.col + p.cols)
        }
    }
    // A row span ends with the last row a cell starts in. (The core holds it to its own last row, which
    // can be an empty row below that: that row has no cells to draw here.)
    val held = placed.map {
        if (it.row + it.rows <= rows) it else PlacedCell(it.cell, it.row, it.col, it.cols, rows - it.row)
    }
    return Grid(rows, columns, held)
}

/** [cell] at the place the core gives it, its spans held to the caps; null when it starts past them. */
private fun placedWithinCaps(cell: CellView): PlacedCell? {
    val row = cell.row.toLong()
    val col = cell.col.toInt()
    if (row >= MAX_GRID_ROWS || col >= MAX_COLUMNS) return null
    val r = row.toInt()
    val cols = cell.cols.toInt().coerceIn(1, MAX_COLUMNS - col)
    val down = cell.rows.toLong().coerceIn(1, minOf(MAX_ROW_SPAN, MAX_GRID_ROWS - r).toLong()).toInt()
    return PlacedCell(cell, r, col, cols, down)
}

/** The rows [p] covers as a screen reader is told: its row span, within the rows of [rows] that are left. */
internal fun rowSpanOf(p: PlacedCell, rows: Int): Int = p.rows.coerceIn(1, (rows - p.row).coerceAtLeast(1))

/** What a column of a layout table asks for. */
internal class ColumnSpec(
    /** Some cell of one column wide holds something. A column that holds nothing only makes room. */
    val holds: Boolean,
    /** A width in dp the cells ask for. */
    val fixed: Float?,
    /** A share of the table's width the cells ask for. */
    val percent: Int?,
    /** What its cells' pictures ask for, in dp, when they hold only pictures of a set width ([picturesWidth]). */
    val pictures: Float? = null,
) {
    /** The width in dp it asks for beside other columns: its own, or its pictures'. */
    val asks: Float? get() = fixed ?: pictures
}

/**
 * What each column of [grid] asks for. With the [table] it belongs to, a cell's padding is the one it is
 * drawn with ([cellPadding]); without, the one it says.
 */
internal fun columnSpecs(grid: Grid, table: TableView? = null): List<ColumnSpec> {
    val ruled = table?.let(::ruled) ?: false
    val padding = { cell: CellView -> table?.let { cellPadding(it, cell, ruled) } ?: cell.padding }
    return (0 until grid.columns).map { c -> columnSpec(grid, c, padding) }
}

private fun columnSpec(grid: Grid, c: Int, padding: (CellView) -> InsetsView?): ColumnSpec {
    val own = grid.placed.filter { it.col == c && it.cols == 1 }
    val widths = own.mapNotNull { it.cell.width }
    // A column that only a spanning cell covers holds what that cell holds, so that it is not left at nothing.
    val spanned = own.isEmpty() && grid.placed.any { it.cell.blocks.isNotEmpty() && c in it.col until it.col + it.cols }
    return ColumnSpec(
        holds = spanned || own.any { it.cell.blocks.isNotEmpty() },
        fixed = widths.filterIsInstance<LengthView.Px>().maxOfOrNull { it.px.toFloat() },
        percent = widths.filterIsInstance<LengthView.Percent>().maxOfOrNull { it.percent.toInt() },
        pictures = picturesColumn(own, padding).takeIf { widths.isEmpty() },
    )
}

/**
 * The widths in dp of the columns of a layout table in [room] dp, with [gap] between them, or null
 * when the table is to be stacked: its cells one under the other in reading order. It is stacked when
 * what has a width of its own does not fit, or when a column that holds something (one that asks for a
 * share of the width, or takes what is left) would be narrower than [least] (the desktop's rule, which
 * the bridge sends with the table: `TableView.minColumn`). A column
 * that asks for a width in pixels is not squeezed (a row of icons stays a row), and one that holds
 * nothing takes what it asks for. A share is a share of what the gaps leave. In a table [told] its own
 * width, room no column takes goes as the desktop gives it ([spare]); one that is not has none to spare.
 */
internal fun planColumns(
    specs: List<ColumnSpec>,
    room: Float,
    gap: Float,
    least: Float,
    told: Boolean = false,
): List<Float>? {
    val n = specs.size
    if (n <= 1) return specs.map { it.fixed?.coerceAtMost(room) ?: room }
    val gaps = gap * (n - 1)
    val widths = declaredWidths(specs, (room - gaps).coerceAtLeast(0f)).toMutableList()
    val auto = specs.indices.filter { specs[it].asks == null && specs[it].percent == null && specs[it].holds }
    val left = (room - gaps - widths.sum()).coerceAtLeast(0f)
    auto.forEach { widths[it] = left / auto.size }
    if (auto.isEmpty() && told) spare(specs, widths, left)
    val squeezed = specs.indices.any { specs[it].holds && specs[it].asks == null && widths[it] < least }
    val over = widths.sum() + gaps > room + HALF
    return if (squeezed || over) null else widths
}

/**
 * Gives [left] dp, the room a table told its width has to spare, to the columns of [widths] by what each
 * has: to those whose pictures ask for their width (a column the message gives a width or a share keeps
 * it; [ColumnSpec.pictures] is only set where it gives none), else
 * to them all. The desktop's room to spare (#1966).
 */
private fun spare(specs: List<ColumnSpec>, widths: MutableList<Float>, left: Float) {
    val pictures = specs.indices.filter { specs[it].pictures != null }
    val takers = pictures.ifEmpty { specs.indices.toList() }
    val by = takers.sumOf { widths[it].toDouble() }.toFloat()
    if (by <= 0f) return
    takers.forEach { widths[it] += left * widths[it] / by }
}

/** What the columns ask for, in dp: a width, or a share of [inner]; 0 for those that take what is left. */
private fun declaredWidths(specs: List<ColumnSpec>, inner: Float): List<Float> = specs.map { c ->
    c.asks ?: c.percent?.let { inner * it.coerceIn(0, PERCENT_MAX) / PERCENT_MAX } ?: 0f
}

private const val PERCENT_MAX = 100
private const val HALF = 0.5
