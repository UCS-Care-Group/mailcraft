// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.TableView

// The room round and between the blocks of a message, by the desktop's rules (`crates/ui-egui/src/reading/layout.rs`).

/**
 * The room above [block] after the line [before] it, in dp, by the desktop's rule: the first blank line
 * is a paragraph's gap and each further one a line of body text, a margin the message gives in pixels
 * wins where it is more, and lists, headings, pictures and data tables keep a little room of their
 * own. The blank lines grow with the text as [density] scales it (Android's own curve at large sizes); the
 * room the message or a kind of block asks for in pixels does not. The first block of a column has none.
 */
internal fun spaceAbove(before: List<BlockView>?, block: BlockView, density: Density = Density(1f)): Dp {
    if (before.isNullOrEmpty()) return 0.dp
    val prev = lineKind(before)
    val steps = block.space.toInt().coerceIn(0, MAX_SPACE_STEPS)
    val asked = if (steps == 0) 0f else with(density) { (PARAGRAPH_GAP + (steps - 1) * BLANK_LINE).sp.toDp().value }
    val kind = block.kind
    val least = when {
        kind is BlockKindView.ListItem && prev is BlockKindView.ListItem -> LIST_GAP
        kind is BlockKindView.Heading || prev is BlockKindView.Heading -> HEADING_GAP
        kind.isDataTable() || prev.isDataTable() -> DATA_TABLE_GAP
        kind is BlockKindView.Image || prev is BlockKindView.Image -> PICTURE_GAP
        else -> 0f
    }
    return maxOf(asked, least, block.gap.toFloat()).dp
}

private fun BlockKindView.isDataTable() = this is BlockKindView.Table && !table.layout

/**
 * What a line counts as for the room below it, as on the desktop: a run of pictures is a picture, a run
 * of boxes on a line (a lone one too, whatever it holds) neither text nor a grid, pictures and text text.
 */
private fun lineKind(line: List<BlockView>): BlockKindView {
    val first = line.first().kind
    val box = { k: BlockKindView -> k is BlockKindView.Table && k.table.inline }
    return when {
        line.all { it.kind is BlockKindView.Image } -> first
        // Neither text nor a grid: what a rule counts as too.
        line.all { box(it.kind) } -> BlockKindView.Rule
        line.size == 1 -> first
        else -> BlockKindView.Paragraph
    }
}

/** A paragraph's gap and a blank line: the desktop's 10 and 18 pixels at its 13 pixel text, at the phone's size. */
private const val PARAGRAPH_GAP = BASE_SIZE * 10f / 13f
private const val BLANK_LINE = BASE_SIZE * DEFAULT_LINE
private const val LIST_GAP = 2f
private const val HEADING_GAP = 4f
private const val DATA_TABLE_GAP = 8f
private const val PICTURE_GAP = 4f
private const val MAX_SPACE_STEPS = 3

/**
 * The margins of a box on a line, as on the desktop: together at most what leaves the box [MIN_BOX] of
 * [width], the left one kept first.
 */
internal fun boxMargins(table: TableView, width: Dp): Pair<Dp, Dp> {
    val left = table.boxMarginLeft.toFloat()
    val clear = (left + table.boxMarginRight.toFloat()).coerceAtMost((width.value - MIN_BOX).coerceAtLeast(0f))
    val kept = left.coerceAtMost(clear)
    return kept.dp to (clear - kept).dp
}

private const val MIN_BOX = 24f

private fun insets(side: Int, top: Int) = InsetsView(top.toUByte(), side.toUByte(), top.toUByte(), side.toUByte())

private val NoPadding = insets(0, 0)
private val DataPadding = insets(8, 5)
private val BoxPadding = insets(12, 9)

/** A cell that is only a colour or a line: a band or a divider. */
private val BandPadding = insets(0, 3)
private const val ROW_GAP = 6f

/** Whether the table itself is painted: a colour, or lines. */
private fun painted(table: TableView) = table.background != null || table.border || table.outline

/**
 * Whether a layout table is ruled: lines above or under some of its cells, and nothing else painted.
 * Every cell then has the same room round what it holds, as in a data table.
 */
internal fun ruled(table: TableView): Boolean {
    if (!table.layout || painted(table)) return false
    val cells = table.rows.flatMap { it.cells }
    if (cells.any { it.background != null || it.border.left || it.border.right }) return false
    return cells.any { it.border.top || it.border.bottom }
}

/**
 * The room between a cell's edge and what it holds, by the desktop's rule: what the message says, else
 * a data table's grid room, a band's sliver for a cell that holds nothing, a box's room for a cell
 * with a colour or lines (its own or its table's), and none for a cell that only places things. [ruled]
 * is [ruled] of the table, worked out once for all its cells.
 */
internal fun cellPadding(table: TableView, cell: CellView, ruled: Boolean): InsetsView = cell.padding ?: when {
    !table.layout -> DataPadding
    cell.blocks.isEmpty() -> BandPadding
    ruled || painted(table) || cell.background != null || cell.border.hasAny() -> BoxPadding
    else -> NoPadding
}

/**
 * The gaps between the columns and between the rows of a layout table, in dp: the message's spacing,
 * else none where the table is painted or ruled (its cells meet), else room to tell its columns and
 * rows apart; none between rows of pictures alone, the slices of one picture.
 */
internal fun gapsOf(table: TableView): Pair<Float, Float> {
    table.spacing?.let { return it.toFloat() to it.toFloat() }
    if (!table.layout || painted(table) || ruled(table)) return 0f to 0f
    val slices = table.rows.size >= 2 &&
        table.rows.all { r -> r.cells.all { c -> c.blocks.all { it.kind is BlockKindView.Image } } }
    return DEFAULT_COLUMN_GAP to if (slices) 0f else ROW_GAP
}
