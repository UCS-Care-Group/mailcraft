// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.TableView

// Boxes on a line: tables a message lays out `inline-block` or floats to a side, as the desktop does (#1966).

/** The boxes of a line in the order they stand: those floated right last, the first of them outermost. */
internal fun boxOrder(blocks: List<BlockView>): Pair<List<BlockView>, List<BlockView>> {
    val right = { b: BlockView -> (b.kind as? BlockKindView.Table)?.table?.float == true && b.align == AlignView.RIGHT }
    return blocks.filterNot(right) to blocks.filter(right).reversed()
}

/**
 * Boxes on a line (tables the message lays out `inline-block`, or floats to a side), as the desktop
 * lays them out: side by side at the top of the line, each as wide as it asks, with the gap the
 * message has before it, wrapping when the line is full; those floated right from the right edge in.
 */
@Composable
internal fun BoxesLine(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    val (left, right) = boxOrder(blocks)
    val first = blocks.first()
    val floats = blocks.any { (it.kind as? BlockKindView.Table)?.table?.float == true }
    val arrange = when {
        floats || first.align == AlignView.LEFT -> Arrangement.Start
        first.align == AlignView.CENTER -> Arrangement.Center
        else -> Arrangement.End
    }
    FlowRow(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = arrange,
        verticalArrangement = Arrangement.spacedBy(RUN_GAP.dp),
    ) {
        left.forEachIndexed { n, block -> BoxOnLine(block, n == 0, env, paper, width) }
        if (right.isNotEmpty()) {
            if (left.isNotEmpty()) Spacer(Modifier.weight(1f))
            right.forEach { block -> BoxOnLine(block, true, env, paper, width) }
        }
    }
}

@Composable
private fun BoxOnLine(block: BlockView, first: Boolean, env: BodyEnv, paper: Paper, width: Dp) {
    val table = (block.kind as? BlockKindView.Table)?.table ?: return
    val gap = if (first) 0 else table.gapBefore.toInt()
    Box(Modifier.padding(start = gap.dp)) { TableBlock(table, block.align, env, paper, width) }
}

/** The gap in dp between the columns of [table]: its own, or the reader's. */
internal fun columnGap(table: TableView): Float = table.spacing?.toInt()?.toFloat() ?: DEFAULT_COLUMN_GAP

/**
 * The room in dp a box on a line keeps free left and right of it (its margins), within [room]: at
 * least [CellMinimum] is left for the box, as on the desktop.
 */
internal fun boxMargins(table: TableView, room: Float): Pair<Float, Float> {
    val left = table.boxMarginLeft.toFloat()
    val right = table.boxMarginRight.toFloat()
    val most = (room - CellMinimum.value).coerceAtLeast(0f)
    val scale = if (left + right > most && left + right > 0f) most / (left + right) else 1f
    return left * scale to right * scale
}

/**
 * The width a table takes of [room]: what it says; a box on a line that says nothing is as wide as
 * the widths its columns ask for ([boxWidth]), or the line when a column takes what is left.
 */
internal fun drawnWidth(table: TableView, room: Dp): Dp {
    if (!table.inline || table.width != null) return tableWidth(table, room)
    return boxWidth(table, columnGap(table))?.dp?.coerceIn(CellMinimum.coerceAtMost(room), room) ?: room
}

/**
 * The width in dp of a box on a line (an inline or floated table) that does not say how wide it is,
 * where every column asks for a width (a social icon in its cell): what they ask for and the gaps.
 * Null when a column takes what is left: such a box takes the line.
 */
internal fun boxWidth(table: TableView, gap: Float): Float? {
    val specs = columnSpecs(gridOf(table))
    if (specs.isEmpty() || specs.any { it.holds && it.asks == null }) return null
    return specs.sumOf { (it.asks ?: 0f).toDouble() }.toFloat() + gap * (specs.size - 1)
}
