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
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView

/** What a cell of a data table is placed by, for the grid that measures it. */
private class CellPlace(val placed: PlacedCell)

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

private fun placeOf(m: Measurable): PlacedCell? = (m.layoutId as? CellPlace)?.placed

/**
 * What [ask] answers, or [fallback] when Compose can't: its own intrinsics throw for a cell taller than
 * a measure's constraints can hold (about 262,000 px, a few thousand lines).
 */
private inline fun intrinsic(fallback: Int, ask: () -> Int): Int =
    try {
        ask()
    } catch (_: IllegalArgumentException) {
        fallback
    }

/** What the cell at [p] asks of its columns, measured as [m]: never narrower than its longest word, up to a point. */
private fun Density.needOf(table: TableView, p: PlacedCell, m: Measurable): CellNeed {
    val pad = cellSides(table, p.cell).dp.toPx()
    val cap = DataMaxWord.toPx() * fontScale + pad
    val most = intrinsic(cap.toInt()) { m.maxIntrinsicWidth(Constraints.Infinity) } + SLACK
    val word = minOf(intrinsic(cap.toInt()) { m.minIntrinsicWidth(Constraints.Infinity) } + SLACK, cap)
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
    Box(modifier = Modifier.horizontalScroll(rememberScrollState())) {
        Layout(
            content = {
                grid.placed.forEach { p ->
                    val info = CollectionItemInfo(p.row, rowSpanOf(p, grid.rows), p.col, p.cols)
                    val plain = p.cell.header && p.cell.background == null
                    val tint = if (plain) Modifier.background(headerTint(paper)) else Modifier
                    val head = p.cell.header
                    Box(
                        modifier = Modifier.layoutId(CellPlace(p)).then(tint).semantics {
                            collectionItemInfo = info
                            if (head) heading()
                        },
                    ) {
                        CellBox(table, p.cell, env, paper, cellRoom(room, p.cols, grid.columns), Modifier.fillMaxSize())
                    }
                }
            },
            modifier = Modifier.semantics { collectionInfo = CollectionInfo(grid.rows, grid.columns) },
        ) { measurables, _ ->
            val cells = measurables.mapNotNull { m -> placeOf(m)?.let { it to m } }
            val needs = cells.map { (p, m) -> needOf(table, p, m) }
            val asked = if (table.width != null) room.toPx() else null
            val edges = edgesOf(dataColumnWidths(needs, grid.columns, room.toPx(), asked))
            fun wide(p: PlacedCell): Int = edges.between(p.col, p.col + p.cols)
            // A cell past the widest a measure takes has no room, and asks for none down the page either.
            fun tall(p: PlacedCell, m: Measurable): Int =
                if (wide(p) == 0) 0 else intrinsic(MAX_BOX_HEIGHT) { m.minIntrinsicHeight(wide(p)) }
            val heights = cells.map { (p, m) -> p to tall(p, m) }
            val tops = dataRowTops(heights, grid.rows)
            val placeables = cells.map { (p, m) ->
                m.measure(Constraints.fixed(wide(p), tops.between(p.row, p.row + p.rows))) to p
            }
            layout(edges.last(), tops.last()) {
                // Columns stand from the left whichever way the phone reads, as in the reading pane.
                placeables.forEach { (pl, p) -> pl.place(edges.at(p.col), tops.at(p.row)) }
            }
        }
    }
}
