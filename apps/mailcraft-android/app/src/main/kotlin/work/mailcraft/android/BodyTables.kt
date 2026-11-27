// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView
import uniffi.mailcraft_mobile.VAlignView

internal val CellMinimum = 24.dp

/** The width a table takes of [room], where it says. */
internal fun tableWidth(table: TableView, room: Dp): Dp = when (val w = table.width) {
    is LengthView.Px -> w.px.toInt().dp.coerceAtMost(room)
    is LengthView.Percent -> room * (w.percent.toInt().coerceIn(0, PERCENT) / PERCENT.toFloat())
    null -> room
}

private const val PERCENT = 100

/**
 * A table. A layout table only positions things: its columns stand side by side when each fits at
 * 110 dp or more (the desktop's rule), and are stacked in reading order when not. A data table is a
 * grid that keeps the widths its words ask for, and scrolls sideways in a box of its own when it is
 * wider than the screen, so that the message never does.
 */
@Composable
internal fun TableBlock(table: TableView, align: AlignView, env: BodyEnv, paper: Paper, room: Dp) {
    val width = drawnWidth(table, room)
    val own = table.background?.let { paper.inner(it.rgb()) } ?: paper
    val shape = RoundedCornerShape(table.radius.toInt().dp)
    // Over what is in it, as the desktop draws it: a cell's own colour never hides the table's frame.
    val outline = if (table.outline) Modifier.frame(table.lines, null, env.rule, own, over = true) else Modifier
    val box = Modifier
        .width(width)
        .then(if (table.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
        .then(outline)
    // A box on a line stands as wide as it is, beside the next; any other table has the line to itself.
    val holder = if (table.inline) Modifier else Modifier.fillMaxWidth()
    Box(modifier = holder, contentAlignment = alignOf(align)) {
        Box(modifier = box) {
            if (table.layout) LayoutTable(table, env, own, width) else DataTable(table, env, own)
        }
    }
}

private fun alignOf(align: AlignView): Alignment = when (align) {
    AlignView.LEFT -> Alignment.TopStart
    AlignView.CENTER -> Alignment.TopCenter
    AlignView.RIGHT -> Alignment.TopEnd
}

/** Rows of a layout table that are planned together: all of them, or each on its own when the rows differ in width. */
internal fun sectionsOf(grid: Grid): List<Grid> {
    val byRow = grid.placed.groupBy { it.row }
    val ragged = byRow.values.map { cells -> cells.sumOf { it.cols } }.distinct().size > 1
    // Rows joined by a row span stay one section: their cells keep the columns they have.
    val joined = grid.placed.any { it.cell.rowspan.toInt() > 1 }
    if (!ragged || joined) return listOf(grid)
    return byRow.values.map { cells ->
        Grid(1, cells.sumOf { it.cols }, cells.map { PlacedCell(it.cell, 0, it.col, it.cols) })
    }
}

@Composable
private fun LayoutTable(table: TableView, env: BodyEnv, paper: Paper, width: Dp) {
    val grid = remember(table) { gridOf(table) }
    val (gap, rowGap) = remember(table) { gapsOf(table) }
    val ruled = remember(table) { ruled(table) }
    val sections = remember(table) { sectionsOf(grid) }
    Column(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(rowGap.dp)) {
        sections.forEach { section ->
            val least = table.minColumn.toFloat()
            val told = table.width != null
            val widths = remember(section, width) {
                planColumns(columnSpecs(section, table), width.value, gap, least, told)
            }
            if (widths == null) {
                section.placed.forEach { CellBox(table, it.cell, env, paper, width, Modifier.fillMaxWidth(), ruled) }
            } else {
                for (r in 0 until section.rows) {
                    LayoutRow(table, section.placed.filter { it.row == r }, widths, gap, env, paper, ruled)
                }
            }
        }
    }
}

/** One row of a layout table, its cells side by side in the [widths] planned for its columns. */
@Composable
private fun LayoutRow(
    table: TableView,
    row: List<PlacedCell>,
    widths: List<Float>,
    gap: Float,
    env: BodyEnv,
    paper: Paper,
    ruled: Boolean,
) {
    val cells = row.sortedBy { it.col }
    // A row with colours or lines (its cells' or the table's grid) has every cell as tall as the row, as a
    // table's are: a coloured bar beside a card reaches its foot, a cell's lines meet the row's.
    val tinted = table.border || cells.any { it.cell.background != null || it.cell.border.hasAny() }
    val tall = if (tinted) Modifier.fillMaxHeight() else Modifier
    Row(modifier = if (tinted) Modifier.height(IntrinsicSize.Min) else Modifier) {
        var next = 0
        cells.forEach { p ->
            if (p.col > next) Spacer(Modifier.width(spanWidth(widths, next, p.col, gap).dp + gap.dp))
            val w = spanWidth(widths, p.col, p.col + p.cols, gap)
            val place = Modifier.alignIn(this, p.cell.valign).then(tall).width(w.dp)
            CellBox(table, p.cell, env, paper, w.dp, place, ruled)
            if (p.col + p.cols < widths.size) Spacer(Modifier.width(gap.dp))
            next = p.col + p.cols
        }
    }
}

/** The width in dp of columns [from] until [until] with the gaps between them. */
internal fun spanWidth(widths: List<Float>, from: Int, until: Int, gap: Float): Float {
    val columns = widths.subList(from.coerceIn(0, widths.size), until.coerceIn(0, widths.size))
    return columns.sum() + gap * (columns.size - 1).coerceAtLeast(0)
}

private fun Modifier.alignIn(row: RowScope, v: VAlignView?): Modifier = with(row) {
    when (v) {
        VAlignView.MIDDLE -> align(Alignment.CenterVertically)
        VAlignView.BOTTOM -> align(Alignment.Bottom)
        else -> align(Alignment.Top)
    }
}

/** One cell: its background, lines and padding, and its blocks in the room left; [ruled]: [ruled] of the table. */
@Composable
internal fun CellBox(
    table: TableView,
    cell: CellView,
    env: BodyEnv,
    paper: Paper,
    width: Dp,
    modifier: Modifier,
    ruled: Boolean = false,
) {
    val own = cell.background?.let { paper.inner(it.rgb()) } ?: paper
    val pad = cellPadding(table, cell, ruled)
    val left = pad.left.toInt()
    val right = pad.right.toInt()
    val sides = if (table.border) ALL_SIDES else cell.border
    val shape = RoundedCornerShape(cell.radius.toInt().dp)
    Box(
        modifier = modifier
            .then(if (cell.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
            .frame(cell.lines, sides, env.rule, own)
            .padding(start = left.dp, end = right.dp, top = pad.top.toInt().dp, bottom = pad.bottom.toInt().dp),
        contentAlignment = when (cell.valign) {
            VAlignView.MIDDLE -> Alignment.CenterStart
            VAlignView.BOTTOM -> Alignment.BottomStart
            else -> Alignment.TopStart
        },
    ) {
        BlockColumn(cell.blocks, env, own, (width - (left + right).dp).coerceAtLeast(CellMinimum))
    }
}
