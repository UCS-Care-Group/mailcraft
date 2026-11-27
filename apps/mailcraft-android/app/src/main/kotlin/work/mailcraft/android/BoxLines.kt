// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.TableView

// The arithmetic of boxes on a line, held to what Compose can lay out (#1966).

/**
 * The most a side of a layout can be, in px: what Compose takes (2^24 - 1). Boxes on a line are placed,
 * never measured with `Constraints.fixed`, so they are held to this, not to a picture's [MAX_BOX_HEIGHT].
 */
// lean: lines past this overlap at its edge; a message that tall is hostile, and nothing crashes.
internal const val MAX_LAYOUT_SIZE = (1 shl 24) - 1

private fun sum(a: Int, b: Int): Int = (a.toLong() + b).coerceIn(0, MAX_LAYOUT_SIZE.toLong()).toInt()

/** The tops of lines of boxes [heights] high, [between] apart, and the height of them all. */
internal fun lineTops(heights: IntArray, between: Int): Pair<IntArray, Int> {
    val tops = IntArray(heights.size)
    for (l in 1 until heights.size) tops[l] = sum(sum(tops[l - 1], heights[l - 1]), between)
    val all = if (heights.isEmpty()) 0 else sum(tops.last(), heights.last())
    return tops to all
}

/** The width of boxes [widths] wide side by side, with the [gaps] in front of each after the first. */
internal fun boxesWidth(widths: List<Int>, gaps: List<Int>): Int =
    widths.indices.fold(0) { at, n -> sum(sum(at, widths[n]), if (n > 0) gaps.getOrElse(n) { 0 } else 0) }

/** The height of each line of boxes placed at [spots] that are [heights] high. */
internal fun lineHeights(spots: List<Pair<Int, Int>>, heights: List<Int>): IntArray {
    val lines = IntArray((spots.maxOfOrNull { it.second } ?: -1) + 1)
    spots.forEachIndexed { n, (_, l) -> lines[l] = maxOf(lines[l], heights.getOrElse(n) { 0 }) }
    return lines
}

private fun BlockView.inlineBox(): TableView? = (kind as? BlockKindView.Table)?.table?.takeIf { it.inline }

internal fun partsBoxes(line: List<BlockView>, next: BlockView): Boolean {
    val box = next.inlineBox() ?: return false
    val before = line.last().inlineBox() ?: return false
    val run = line.asReversed().asSequence().take(MAX_BOX_RUN).takeWhile { it.inlineBox() != null }.count()
    return box.float != before.float || run >= MAX_BOX_RUN
}

/** The most boxes in one run on a line: the desktop's `MAX_RUN`. */
private const val MAX_BOX_RUN = 64
