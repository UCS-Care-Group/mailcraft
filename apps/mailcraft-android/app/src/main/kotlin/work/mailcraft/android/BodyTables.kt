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
import androidx.compose.ui.layout.IntrinsicMeasurable
import androidx.compose.ui.layout.IntrinsicMeasureScope
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasurePolicy
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.constrainHeight
import androidx.compose.ui.unit.constrainWidth
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView
import uniffi.mailcraft_mobile.VAlignView

private val CellMinimum = 24.dp

/**
 * How much less than its room a layout table plans its columns in: in a cell of a data table, the room
 * past the cell's share of the table ([cellRoom]), so that a table there stacks when the cell is narrow;
 * none anywhere else.
 */
internal val LocalTableShortfall = compositionLocalOf { 0.dp }

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
    val own = table.background?.let { paper.inner(it.rgb()) } ?: paper
    val shape = RoundedCornerShape(table.radius.toInt().dp)
    // Over what is in it, as the desktop draws it: a cell's own colour never hides the table's frame.
    val outline = if (table.outline) Modifier.frame(table.lines, null, env.rule, own, over = true) else Modifier
    // A data table is as wide as its columns, which fit the room when their words let them.
    val box = (if (table.layout) Modifier.width(width) else Modifier.widthIn(max = width))
        .then(if (table.background != null) Modifier.background(composeColor(own.rgb), shape) else Modifier)
        .then(outline)
    Box(modifier = Modifier.fillMaxWidth(), contentAlignment = alignOf(align)) {
        Box(modifier = box) {
            if (table.layout) LayoutTable(table, env, own, width) else DataTable(table, env, own, width)
        }
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

@Composable
private fun LayoutTable(table: TableView, env: BodyEnv, paper: Paper, width: Dp) {
    val grid = remember(table) { gridOf(table) }
    val (gap, rowGap) = remember(table) { gapsOf(table) }
    val ruled = remember(table) { ruled(table) }
    val sections = remember(table) { sectionsOf(grid) }
    val shortfall = LocalTableShortfall.current
    // What its own cells hold plans in their own widths.
    CompositionLocalProvider(LocalTableShortfall provides 0.dp) {
        Column(modifier = Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(rowGap.dp)) {
            sections.forEach { section ->
                // In a cell of a data table, whether the columns stand side by side is told by the cell's
                // share of it; when they do, they are as wide as the room.
                val plan = (width - shortfall).coerceAtLeast(CellMinimum)
                val widths = remember(section, plan, width) {
                    val specs = columnSpecs(section)
                    planColumns(specs, plan.value, gap)?.let { planColumns(specs, width.value, gap) ?: it }
                }
                if (widths == null) {
                    section.placed.forEach {
                        CellBox(table, it.cell, env, paper, width, Modifier.fillMaxWidth(), ruled)
                    }
                } else {
                    val byRow = remember(section) { section.placed.groupBy { it.row } }
                    for (r in 0 until section.rows) {
                        LayoutRow(table, byRow[r].orEmpty(), widths, gap, env, paper, ruled)
                    }
                }
            }
        }
    }
}

/**
 * One row of a layout table, its cells side by side in the [widths] planned for its columns, each at
 * its own height and the side it asks for. A row with colours or lines (its cells' or the table's grid)
 * has every cell as tall as the row, as a table's are: a coloured bar beside a card reaches its foot, a
 * cell's lines meet the row's; and no taller than the tallest cell's content (#2006).
 *
 * Its own layout, not a Row of `height(IntrinsicSize.Min)`: that throws for a cell taller than a
 * measure holds (about 262,000 px: a digest of a few thousand lines in a coloured `<div>`, which is a
 * one-cell layout table). Here the row asks each cell's height with a fallback, holds it to
 * [MAX_BOX_HEIGHT] (the rest is cut, as a picture's foot is) and only then measures the cells.
 */
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
    val tinted = table.border || cells.any { it.cell.background != null || it.cell.border.hasAny() }
    val policy = remember(cells, widths, gap, tinted) { LayoutRowPolicy(cells, widths, gap, tinted) }
    Layout(
        content = {
            cells.forEach { p ->
                val w = spanWidth(widths, p.col, p.col + p.cols, gap)
                CellBox(table, p.cell, env, paper, w.dp, Modifier, ruled)
            }
        },
        measurePolicy = policy,
    )
}

/** How [LayoutRow] measures and places its [cells] (one child each, in order). */
private class LayoutRowPolicy(
    private val cells: List<PlacedCell>,
    private val widths: List<Float>,
    private val gap: Float,
    private val tinted: Boolean,
) : MeasurePolicy {
    /** The left edge of each cell and the row's width, in pixels, each width and gap rounded as it stands. */
    private fun Density.edges(): Pair<List<Int>, Int> {
        var x = 0
        var next = 0
        val lefts = cells.map { p ->
            if (p.col > next) x += (spanWidth(widths, next, p.col, gap) + gap).dp.roundToPx()
            val left = x
            x += wideOf(p)
            if (p.col + p.cols < widths.size) x += gap.dp.roundToPx()
            next = p.col + p.cols
            left
        }
        return lefts to x
    }

    private fun Density.wideOf(p: PlacedCell): Int =
        spanWidth(widths, p.col, p.col + p.cols, gap).dp.roundToPx().coerceIn(0, MAX_BOX_WIDTH)

    /** The row's height, its cells [wide] as given: its tallest cell's, held to [MAX_BOX_HEIGHT]. */
    private fun heightOf(measurables: List<IntrinsicMeasurable>, wide: List<Int>): Int =
        measurables.zip(wide).maxOfOrNull { (m, w) ->
            intrinsic(MAX_BOX_HEIGHT) { m.minIntrinsicHeight(w) }.coerceIn(0, MAX_BOX_HEIGHT)
        } ?: 0

    private fun Density.heightOf(measurables: List<IntrinsicMeasurable>): Int =
        heightOf(measurables, cells.map { wideOf(it) })

    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val (lefts, width) = edges()
        // As a Row would: a cell is no wider than the room left of the row's own.
        val wide = cells.zip(lefts).map { (p, x) ->
            wideOf(p).coerceAtMost((constraints.maxWidth - x).coerceAtLeast(0))
        }
        val row = if (tinted) heightOf(measurables, wide) else 0
        val placeables = measurables.zip(wide).map { (m, w) ->
            m.measure(if (tinted) Constraints.fixed(w, row) else Constraints(w, w, 0, MAX_BOX_HEIGHT))
        }
        val height = if (tinted) row else placeables.maxOfOrNull { it.height } ?: 0
        return layout(constraints.constrainWidth(width), constraints.constrainHeight(height)) {
            placeables.zip(cells).zip(lefts).forEach { (pair, x) ->
                val (pl, p) = pair
                pl.place(x, valignOf(p.cell.valign).align(pl.height, height))
            }
        }
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        heightOf(measurables)

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        heightOf(measurables)

    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        edges().second

    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        edges().second
}

/** The width in dp of columns [from] until [until] with the gaps between them. */
internal fun spanWidth(widths: List<Float>, from: Int, until: Int, gap: Float): Float {
    val columns = widths.subList(from.coerceIn(0, widths.size), until.coerceIn(0, widths.size))
    return columns.sum() + gap * (columns.size - 1).coerceAtLeast(0)
}

/** Where a cell that is shorter than its row stands in it. */
internal fun valignOf(v: VAlignView?): Alignment.Vertical = when (v) {
    VAlignView.MIDDLE -> Alignment.CenterVertically
    VAlignView.BOTTOM -> Alignment.Bottom
    else -> Alignment.Top
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
