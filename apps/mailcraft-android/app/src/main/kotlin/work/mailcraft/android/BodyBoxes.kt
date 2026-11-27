// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.TableView

// Boxes on a line: tables a message lays out `inline-block` or floats to a side, as the desktop does (#1966).

/** The boxes of a line apart: those that stand from the left, and those floated right, each in source order. */
internal fun boxSides(blocks: List<BlockView>): Pair<List<BlockView>, List<BlockView>> =
    blocks.partition { (it.kind as? BlockKindView.Table)?.table?.float != true || it.align != AlignView.RIGHT }

/**
 * Boxes on a line (tables the message lays out `inline-block`, or floats to a side), as the desktop
 * lays them out: side by side at the top of the line, each as wide as it asks, with the gap the
 * message has before it, wrapping when the line is full. Those floated right stand from the right
 * edge in, the first outermost, and wrap in source order: laid out right to left.
 */
@Composable
internal fun BoxesLine(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    val (left, right) = boxSides(blocks)
    val first = blocks.first()
    val floats = blocks.any { (it.kind as? BlockKindView.Table)?.table?.float == true }
    val arrange = when {
        floats || first.align == AlignView.LEFT -> Arrangement.Start
        first.align == AlignView.CENTER -> Arrangement.Center
        else -> Arrangement.End
    }
    val lines = Arrangement.spacedBy(RUN_GAP.dp)
    val ltr = LocalLayoutDirection.current
    Row(modifier = Modifier.fillMaxWidth()) {
        if (left.isNotEmpty()) {
            FlowRow(Modifier.weight(1f), horizontalArrangement = arrange, verticalArrangement = lines) {
                left.forEachIndexed { n, block -> BoxOnLine(block, n == 0, env, paper, width) }
            }
        }
        if (right.isNotEmpty()) {
            val rest = if (left.isEmpty()) Modifier.weight(1f) else Modifier
            CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Rtl) {
                FlowRow(rest, verticalArrangement = lines) {
                    right.forEach { block ->
                        CompositionLocalProvider(LocalLayoutDirection provides ltr) {
                            BoxOnLine(block, true, env, paper, width)
                        }
                    }
                }
            }
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
