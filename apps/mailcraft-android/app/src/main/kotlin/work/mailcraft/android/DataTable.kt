// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.layoutId
import androidx.compose.ui.semantics.CollectionInfo
import androidx.compose.ui.semantics.CollectionItemInfo
import androidx.compose.ui.semantics.collectionInfo
import androidx.compose.ui.semantics.collectionItemInfo
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.TableView

/** What a cell of a data table is placed by, for the grid that measures it. */
private class CellPlace(val placed: Placed)

private val DataMinColumn = 48.dp
internal val DataMaxColumn = 280.dp

private const val HEADER_ON_LIGHT = 0xEDEFF1
private const val HEADER_ON_DARK = 0x2A2F33

private fun headerTint(paper: Paper): Color =
    composeColor(if (Readability.isLight(paper.rgb)) HEADER_ON_LIGHT else HEADER_ON_DARK)

/**
 * The width of each of [columns] columns, in pixels, from the widths the cells ask for ([wanted], by
 * cell): each column as wide as its widest single-column cell, within [minimum] and [maximum], and a
 * cell over several columns widens those it covers when they are too narrow together.
 */
internal fun dataColumnWidths(wanted: List<Pair<Placed, Int>>, columns: Int, minimum: Int, maximum: Int): IntArray {
    val widths = IntArray(columns.coerceAtLeast(1))
    for ((p, w) in wanted) {
        if (p.cols == 1) widths[p.col] = maxOf(widths[p.col], w.coerceIn(minimum, maximum))
    }
    for ((p, w) in wanted) {
        val covered = p.col until minOf(p.col + p.cols, widths.size)
        val have = covered.sumOf { widths[it] }
        if (p.cols > 1 && have < w) for (c in covered) widths[c] += (w - have) / p.cols + 1
    }
    return widths
}

private fun placeOf(m: Measurable): Placed? = (m.layoutId as? CellPlace)?.placed

/**
 * A data table: a grid that keeps the widths its words ask for (each column within 48 to 280 dp) and
 * scrolls sideways in a box of its own when it is wider than the screen. Each cell says where it is
 * in the table to a screen reader.
 */
@Composable
internal fun DataTable(table: TableView, env: BodyEnv, paper: Paper) {
    val grid = remember(table) { gridOf(table) }
    Box(modifier = Modifier.horizontalScroll(rememberScrollState())) {
        Layout(
            content = {
                grid.placed.forEach { p ->
                    val info = CollectionItemInfo(p.row, 1, p.col, p.cols)
                    val plain = p.cell.header && p.cell.background == null
                    val tint = if (plain) Modifier.background(headerTint(paper)) else Modifier
                    val head = p.cell.header
                    Box(
                        modifier = Modifier.layoutId(CellPlace(p)).then(tint).semantics {
                            collectionItemInfo = info
                            if (head) heading()
                        },
                    ) {
                        CellBox(table, p.cell, env, paper, DataMaxColumn, Modifier.fillMaxWidth())
                    }
                }
            },
            modifier = Modifier.semantics { collectionInfo = CollectionInfo(grid.rows, grid.columns) },
        ) { measurables, _ ->
            val cells = measurables.mapNotNull { m -> placeOf(m)?.let { it to m } }
            val wanted = cells.map { (p, m) -> p to m.maxIntrinsicWidth(Constraints.Infinity) }
            val columns = dataColumnWidths(wanted, grid.columns, DataMinColumn.roundToPx(), DataMaxColumn.roundToPx())
            val starts = IntArray(columns.size + 1)
            columns.forEachIndexed { i, w -> starts[i + 1] = starts[i] + w }
            val placeables = cells.map { (p, m) ->
                m.measure(Constraints.fixedWidth(starts[p.col + p.cols] - starts[p.col])) to p
            }
            val heights = IntArray(grid.rows.coerceAtLeast(1))
            placeables.forEach { (pl, p) -> heights[p.row] = maxOf(heights[p.row], pl.height) }
            val tops = IntArray(heights.size + 1)
            heights.forEachIndexed { i, h -> tops[i + 1] = tops[i] + h }
            layout(starts.last(), tops.last()) {
                placeables.forEach { (pl, p) -> pl.placeRelative(starts[p.col], tops[p.row]) }
            }
        }
    }
}
