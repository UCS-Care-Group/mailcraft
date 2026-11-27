// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.remember
import androidx.compose.ui.AbsoluteAlignment
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView
import uniffi.mailcraft_mobile.VAlignView

private val CellMinimum = 24.dp

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
    val depth = LocalTableDepth.current
    if (depth >= MAX_PHONE_TABLE_DEPTH) {
        FlatTable(table, env, paper, room)
        return
    }
    val width = tableWidth(table, room)
    val own = table.background?.let { paper.inner(it.rgb()) } ?: paper
    val shape = RoundedCornerShape(table.radius.toInt().dp)
    // Over what is in it, as the desktop draws it: a cell's own colour never hides the table's frame.
    val outline = if (table.outline) Modifier.frame(table.lines, null, env.rule, own, over = true) else Modifier
    // A data table is as wide as its columns, which fit the room when their words let them. A layout
    // table takes its own width itself, of the room it is really given ([SectionPolicy]).
    val box = (if (table.layout) Modifier else Modifier.widthIn(max = width))
        .then(if (table.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
        .then(outline)
    Box(modifier = Modifier.fillMaxWidth(), contentAlignment = alignOf(align)) {
        Box(modifier = box) {
            CompositionLocalProvider(LocalTableDepth provides depth + 1) {
                if (table.layout) LayoutTable(table, env, own, width) else DataTable(table, env, own, width)
            }
        }
    }
}

/** How many tables stand around what is drawn. */
private val LocalTableDepth = compositionLocalOf { 0 }

/**
 * The most tables the phone draws inside one another; one deeper is drawn as its cells' blocks one
 * under another, as the bridge sends it. Every table asks what is in it of its size, and the work
 * grows with each table inside another.
 * lean: four, as most mail nests a table or two; raise with care, as the work is per level.
 */
internal const val MAX_PHONE_TABLE_DEPTH = 4

@Composable
private fun FlatTable(table: TableView, env: BodyEnv, paper: Paper, room: Dp) {
    Column {
        table.rows.forEach { r -> r.cells.forEach { c -> BlockColumn(c.blocks, env, paper, room) } }
    }
}

// The side the message names, whichever way the phone reads.
private fun alignOf(align: AlignView): Alignment = when (align) {
    AlignView.LEFT -> AbsoluteAlignment.TopLeft
    AlignView.CENTER -> Alignment.TopCenter
    AlignView.RIGHT -> AbsoluteAlignment.TopRight
}

/** Rows of a layout table that are planned together: all of them, or each on its own when the rows differ in width. */
internal fun sectionsOf(grid: Grid): List<Grid> {
    val byRow = grid.placed.groupBy { it.row }
    val ragged = byRow.values.map { cells -> cells.sumOf { it.cols } }.distinct().size > 1
    // Rows joined by a row span stay one section: their cells keep the columns they have.
    val joined = grid.placed.any { it.rows > 1 }
    if (!ragged || joined) return listOf(grid)
    return byRow.values.map { cells ->
        Grid(1, cells.sumOf { it.cols }, cells.map { PlacedCell(it.cell, 0, it.col, it.cols) })
    }
}

/**
 * A layout table: its sections ([sectionsOf]) one under another, each laid out by [SectionPolicy] in
 * the width it is really given. A cell is composed for the width its columns are planned at in [width],
 * the room the table is composed in, and measured at the width it really gets: only what reads the
 * composed width tells the difference (a data table in it fits that room and scrolls in its own box
 * when the cell is narrower; a picture is held to it as well as to the cell).
 */
@Composable
private fun LayoutTable(table: TableView, env: BodyEnv, paper: Paper, width: Dp) {
    val grid = remember(table) { gridOf(table) }
    val (gap, rowGap) = remember(table) { gapsOf(table) }
    val ruled = remember(table) { ruled(table) }
    val sections = remember(table) { sectionsOf(grid) }
    Column(verticalArrangement = Arrangement.spacedBy(rowGap.dp)) {
        sections.forEach { section ->
            val cells = remember(section) { section.placed.sortedWith(compareBy({ it.row }, { it.col })) }
            val specs = remember(section) { columnSpecs(section) }
            val planned = remember(specs, width) { planColumns(specs, width.value, gap) }
            val policy = remember(cells, specs, table) {
                val tinted = cells.filter { table.border || it.cell.background != null || it.cell.border.hasAny() }
                SectionPolicy(cells, specs, gap, rowGap, tinted.map { it.row }.toSet(), table.width)
            }
            Layout(
                content = {
                    cells.forEach { p ->
                        val w = planned?.let { spanWidth(it, p.col, p.col + p.cols, gap).dp } ?: width
                        CellBox(table, p.cell, env, paper, w, Modifier, ruled)
                    }
                },
                measurePolicy = policy,
            )
        }
    }
}

/** The width in dp of columns [from] until [until] with the gaps between them. */
internal fun spanWidth(widths: List<Float>, from: Int, until: Int, gap: Float): Float {
    val columns = widths.subList(from.coerceIn(0, widths.size), until.coerceIn(0, widths.size))
    return columns.sum() + gap * (columns.size - 1).coerceAtLeast(0)
}

/** The room left and right of what a cell of a data table holds together, in dp. */
internal fun cellSides(table: TableView, cell: CellView): Int =
    cellPadding(table, cell, ruled = false).let { it.left.toInt() + it.right.toInt() }

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
