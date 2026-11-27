// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.rememberScrollState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.IntrinsicMeasurable
import androidx.compose.ui.layout.IntrinsicMeasureScope
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.MeasurePolicy
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.semantics.CollectionInfo
import androidx.compose.ui.semantics.CollectionItemInfo
import androidx.compose.ui.semantics.collectionInfo
import androidx.compose.ui.semantics.collectionItemInfo
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/**
 * The least room for a cell that holds something, and the widest a long word makes its column, as on
 * the desktop; the word's limit grows with the phone's text size, so that larger text keeps its words.
 */
private val DataMinCell = 24.dp
private val DataMaxWord = 110.dp

/** Room past a measured width, in pixels, so that a word never breaks for want of a rounding (as on the desktop). */
private const val SLACK = 2f

private const val HEADER_ON_LIGHT = 0xEDEFF1
private const val HEADER_ON_DARK = 0x2A2F33

private fun headerTint(paper: Paper): Color =
    composeColor(if (Readability.isLight(paper.rgb)) HEADER_ON_LIGHT else HEADER_ON_DARK)

/**
 * What [ask] answers, or [fallback] when Compose can't: its own intrinsics throw for a cell taller than
 * a measure's constraints can hold (about 262,000 px, a few thousand lines).
 */
internal inline fun intrinsic(fallback: Int, ask: () -> Int): Int =
    try {
        ask()
    } catch (_: IllegalArgumentException) {
        fallback
    }

/**
 * What the cell at [p] asks of its columns, measured as [m]: never narrower than its longest word, up
 * to a point, nor than its pictures' [picturesLeast] ([shown]: those that came); all of [room] (pixels)
 * at most when Compose can't tell.
 */
internal fun Density.needOf(
    table: TableView,
    p: PlacedCell,
    m: IntrinsicMeasurable,
    room: Float,
    shown: (BlockKindView.Image) -> Boolean,
): CellNeed {
    val pad = cellSides(table, p.cell).dp.toPx()
    val cap = DataMaxWord.toPx() * fontScale + pad
    val most = intrinsic(room.toInt()) { m.maxIntrinsicWidth(Constraints.Infinity) } + SLACK
    val least = intrinsic(cap.toInt()) { m.minIntrinsicWidth(Constraints.Infinity) } + SLACK
    val words = minOf(least, cap)
    val pictures = picturesLeast(p.cell.blocks, shown)?.dp?.toPx()?.plus(pad)?.let { minOf(it, least) }
    // Only text is held to the cap: a cell of pictures alone asks what they do, as on the desktop.
    val word = when {
        pictures == null -> words
        p.cell.blocks.all { it.kind is BlockKindView.Image } -> pictures
        else -> maxOf(words, pictures)
    }
    val floor = if (p.cell.blocks.isEmpty()) 0f else DataMinCell.toPx() + pad
    return CellNeed(
        p.col, p.cols, maxOf(minOf(word, most), floor), maxOf(most, floor),
        px = (p.cell.width as? LengthView.Px)?.px?.toInt()?.dp?.toPx(),
        percent = (p.cell.width as? LengthView.Percent)?.percent?.toInt(),
    )
}

/**
 * A data table: a grid whose columns fit the screen ([room]) as far as their words allow, and which
 * scrolls sideways in a box of its own when they don't. A cell over several rows or columns covers
 * them all; a header cell with no colour of its own is shaded a little. Each cell says where it is in
 * the table to a screen reader.
 */
@Composable
internal fun DataTable(table: TableView, env: BodyEnv, paper: Paper, room: Dp) {
    val grid = remember(table) { gridOf(table) }
    val policy = remember(table, grid, room, env) {
        DataTablePolicy(table, grid, room) { env.stateOf(it) is PictureState.Ready }
    }
    Box(modifier = Modifier.horizontalScroll(rememberScrollState())) {
        Layout(
            content = {
                grid.placed.forEach { p ->
                    val info = CollectionItemInfo(p.row, rowSpanOf(p, grid.rows), p.col, p.cols)
                    val plain = p.cell.header && p.cell.background == null
                    val tint = if (plain) Modifier.background(headerTint(paper)) else Modifier
                    val head = p.cell.header
                    Box(
                        modifier = tint.semantics {
                            collectionItemInfo = info
                            if (head) heading()
                        },
                    ) {
                        CellBox(table, p.cell, env, paper, room, Modifier.fillMaxSize())
                    }
                }
            },
            modifier = Modifier.semantics { collectionInfo = CollectionInfo(grid.rows, grid.columns) },
            measurePolicy = policy,
        )
    }
}

/**
 * How a data table of [grid] in [room] measures and places its cells (one child each, in the order of
 * [Grid.placed]).
 *
 * Invariants (DataTableTest, NestedTablesTest, TallRowsTest):
 * - I1, what it says of its size is what it draws. Its intrinsic height, at any width (it scrolls in a
 *   box of its own, so the width it is asked at changes nothing), is the height measure gives, and its
 *   intrinsic widths are the width measure gives: all come from [columns] and [rowsOf].
 * - I2, bounded work. A measure, or any one intrinsic, asks each cell each intrinsic at most once (its
 *   widths: least and most; its height: those and its height at its width). How deep tables nest is
 *   held to [MAX_PHONE_TABLE_DEPTH], so the work is bounded however a message nests them.
 * - No measure throws: edges stay within [WIDE_TABLE_WIDTH], tops within [tallestFor] that width.
 */
private class DataTablePolicy(
    private val table: TableView,
    private val grid: Grid,
    private val room: Dp,
    private val shown: (BlockKindView.Image) -> Boolean,
) : MeasurePolicy {
    private fun cellsOf(measurables: List<IntrinsicMeasurable>) = grid.placed.zip(measurables)

    /** The column edges in pixels, each cell asked its least and most width once. */
    private fun Density.columns(cells: List<Pair<PlacedCell, IntrinsicMeasurable>>): IntArray {
        val needs = cells.map { (p, m) -> needOf(table, p, m, room.toPx(), shown) }
        val asked = if (table.width != null) room.toPx() else null
        return edgesOf(dataColumnWidths(needs, grid.columns, room.toPx(), asked))
    }

    /** The row tops in pixels in [edges], each cell asked its height at its width once. */
    private fun rowsOf(cells: List<Pair<PlacedCell, IntrinsicMeasurable>>, edges: IntArray): IntArray {
        val tallest = tallestFor(edges.last())
        // A cell past the widest a measure takes has no room, and asks for none down the page either.
        val heights = cells.map { (p, m) ->
            val w = edges.between(p.col, p.col + p.cols)
            p to if (w == 0) 0 else intrinsic(tallest) { m.minIntrinsicHeight(w) }
        }
        return dataRowTops(heights, grid.rows, tallest)
    }

    override fun MeasureScope.measure(measurables: List<Measurable>, constraints: Constraints): MeasureResult {
        val cells = cellsOf(measurables)
        val edges = columns(cells)
        val tops = rowsOf(cells, edges)
        val placeables = grid.placed.zip(measurables).map { (p, m) ->
            m.measure(Constraints.fixed(edges.between(p.col, p.col + p.cols), tops.between(p.row, p.row + p.rows))) to p
        }
        return layout(edges.last(), tops.last()) {
            // Columns stand from the left whichever way the phone reads, as in the reading pane.
            placeables.forEach { (pl, p) -> pl.place(edges.at(p.col), tops.at(p.row)) }
        }
    }

    override fun IntrinsicMeasureScope.minIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int {
        val cells = cellsOf(measurables)
        return rowsOf(cells, columns(cells)).last()
    }

    override fun IntrinsicMeasureScope.maxIntrinsicHeight(measurables: List<IntrinsicMeasurable>, width: Int): Int =
        minIntrinsicHeight(measurables, width)

    override fun IntrinsicMeasureScope.minIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        columns(cellsOf(measurables)).last()

    override fun IntrinsicMeasureScope.maxIntrinsicWidth(measurables: List<IntrinsicMeasurable>, height: Int): Int =
        columns(cellsOf(measurables)).last()
}
