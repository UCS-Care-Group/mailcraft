// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
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

private val CellMinimum = 24.dp
private const val DATA_SIDE = 8
private const val DATA_TOP = 5

/** The width a table takes of [room], where it says. */
internal fun tableWidth(table: TableView, room: Dp): Dp = when (val w = table.width) {
    is LengthView.Px -> w.px.toInt().dp.coerceAtMost(room)
    is LengthView.Percent -> room * (w.percent.toInt().coerceIn(0, PERCENT) / PERCENT.toFloat())
    null -> room
}

private const val PERCENT = 100

/**
 * A table. A layout table only positions things: its columns stand side by side when each fits at
 * about 140 dp or more, and are stacked in reading order when not. A data table is a grid that keeps
 * the widths its words ask for, and scrolls sideways in a box of its own when it is wider than the
 * screen, so that the message never does.
 */
@Composable
internal fun TableBlock(table: TableView, align: AlignView, env: BodyEnv, paper: Paper, room: Dp) {
    val width = tableWidth(table, room)
    val own = table.background?.let { Paper.of(it.rgb(), paper.link) } ?: paper
    val shape = RoundedCornerShape(table.radius.toInt().dp)
    val outline = if (table.outline) Modifier.frame(table.lines, null, env.rule, own) else Modifier
    val box = Modifier
        .width(width)
        .then(if (table.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
        .then(outline)
    Box(modifier = Modifier.fillMaxWidth(), contentAlignment = alignOf(align)) {
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
    if (!ragged) return listOf(grid)
    return byRow.values.map { cells ->
        Grid(1, cells.sumOf { it.cols }, cells.map { Placed(it.cell, 0, it.col, it.cols) })
    }
}

@Composable
private fun LayoutTable(table: TableView, env: BodyEnv, paper: Paper, width: Dp) {
    val grid = remember(table) { gridOf(table) }
    val gap = (table.spacing?.toInt()?.toFloat() ?: DEFAULT_COLUMN_GAP)
    val sections = remember(table) { sectionsOf(grid) }
    Column(modifier = Modifier.fillMaxWidth()) {
        sections.forEach { section ->
            val widths = remember(section, width) { planColumns(columnSpecs(section), width.value, gap) }
            if (widths == null) {
                section.placed.forEach { CellBox(table, it.cell, env, paper, width, Modifier.fillMaxWidth()) }
            } else {
                for (r in 0 until section.rows) {
                    LayoutRow(table, section.placed.filter { it.row == r }, widths, gap, env, paper)
                }
            }
        }
    }
}

/** One row of a layout table, its cells side by side in the [widths] planned for its columns. */
@Composable
private fun LayoutRow(
    table: TableView,
    row: List<Placed>,
    widths: List<Float>,
    gap: Float,
    env: BodyEnv,
    paper: Paper,
) {
    val cells = row.sortedBy { it.col }
    val tinted = cells.any { it.cell.background != null || it.cell.border.hasAny() }
    Row(modifier = if (tinted) Modifier.height(IntrinsicSize.Min) else Modifier) {
        var next = 0
        cells.forEach { p ->
            if (p.col > next) Spacer(Modifier.width(spanWidth(widths, next, p.col, gap).dp + gap.dp))
            val w = spanWidth(widths, p.col, p.col + p.cols, gap)
            CellBox(table, p.cell, env, paper, w.dp, Modifier.alignIn(this, p.cell.valign).width(w.dp))
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

/** One cell: its background, lines and padding, and its blocks in the room that is left. */
@Composable
internal fun CellBox(table: TableView, cell: CellView, env: BodyEnv, paper: Paper, width: Dp, modifier: Modifier) {
    val own = cell.background?.let { Paper.of(it.rgb(), paper.link) } ?: paper
    val pad = cell.padding
    // A cell of a data table that says nothing of its padding has the grid's own.
    val roomy = !table.layout && pad == null
    val left = pad?.left?.toInt() ?: if (roomy) DATA_SIDE else 0
    val right = pad?.right?.toInt() ?: if (roomy) DATA_SIDE else 0
    val top = pad?.top?.toInt() ?: if (roomy) DATA_TOP else 0
    val bottom = pad?.bottom?.toInt() ?: if (roomy) DATA_TOP else 0
    val sides = if (table.border) ALL_SIDES else cell.border
    val shape = RoundedCornerShape(cell.radius.toInt().dp)
    Box(
        modifier = modifier
            .then(if (cell.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
            .frame(cell.lines, sides, env.rule, own)
            .padding(start = left.dp, end = right.dp, top = top.dp, bottom = bottom.dp),
    ) {
        BlockColumn(cell.blocks, env, own, (width - (left + right).dp).coerceAtLeast(CellMinimum))
    }
}
