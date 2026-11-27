// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView

// How big a picture, and a cell of pictures, is drawn: the desktop's rules (#1966).

/**
 * The width in dp a cell asks for when it holds only pictures that say how wide they are (a logo, a
 * row of icons): theirs, line by line, with their padding and the cell's, as the desktop sizes such a
 * column. Null when it holds anything else, or nothing.
 */
internal fun picturesWidth(cell: CellView, padding: InsetsView? = cell.padding): Float? {
    if (cell.blocks.isEmpty()) return null
    val lines = linesOf(cell.blocks).map { line ->
        val each = line.blocks.map { pictureWidth(it) ?: return null }
        each.sum() + RUN_GAP * (each.size - 1)
    }
    val pad = padding?.let { it.left.toFloat() + it.right.toFloat() } ?: 0f
    return lines.maxOrNull()?.plus(pad)
}

/** What a column asks for, in dp, when each of its own cells that holds anything holds only pictures of a set width. */
internal fun picturesColumn(own: List<PlacedCell>, padding: (CellView) -> InsetsView?): Float? {
    val held = own.filter { it.cell.blocks.isNotEmpty() }.map { picturesWidth(it.cell, padding(it.cell)) }
    return if (held.isEmpty() || null in held) null else held.maxOf { it ?: 0f }
}

/** The width in dp of a picture block that says how wide it is, with its padding; null for anything else. */
private fun pictureWidth(block: BlockView): Float? {
    val kind = block.kind as? BlockKindView.Image
    val w = kind?.width ?: return null
    val pad = block.imagePadding?.let { it.left.toFloat() + it.right.toFloat() } ?: 0f
    val tiny = isTiny(PictureRef(kind.src, kind.picture, w.toInt(), kind.height?.toInt()))
    return if (tiny) 0f else w.toFloat() + pad
}

/**
 * The width in dp a picture is shown at, before the column holds it: what the message says, or for
 * a picture that says only its height, the width that height gives it in the picture's own shape;
 * else its own width ([naturalWidth] by [naturalHeight] dp), as on the desktop.
 */
internal fun shownWidth(ref: PictureRef, naturalWidth: Float, naturalHeight: Float): Float {
    ref.width?.takeIf { it >= 1 }?.let { return it.toFloat() }
    val tall = ref.height?.takeIf { it >= 1 } ?: return naturalWidth
    return if (naturalHeight > 0f) naturalWidth * tall / naturalHeight else naturalWidth
}

/**
 * A shown picture's height for each unit of its width: the shape the message gives it when it says
 * both its width and height (a browser and the desktop draw it so), else its own.
 */
internal fun shownTall(ref: PictureRef, naturalWidth: Int, naturalHeight: Int): Float {
    val w = ref.width?.takeIf { it >= 1 }
    val h = ref.height?.takeIf { it >= 1 }
    return if (w != null && h != null) h.toFloat() / w else naturalHeight.toFloat() / naturalWidth.coerceAtLeast(1)
}

/** Room in dp round a picture, side by side. */
internal class Around(val left: Float, val top: Float, val right: Float, val bottom: Float)

/**
 * The padding the message gives round a picture [wanted] dp wide in [room] dp, as the desktop keeps it:
 * the picture keeps its room, and the padding left and right gives way to it when the line is short.
 */
internal fun paddingAround(pad: InsetsView?, wanted: Float, room: Float): Around {
    if (pad == null) return Around(0f, 0f, 0f, 0f)
    val left = pad.left.toFloat()
    val right = pad.right.toFloat()
    val spare = (room - wanted.coerceAtMost(room)).coerceAtLeast(0f)
    val scale = if (left + right > spare) spare / (left + right) else 1f
    return Around(left * scale, pad.top.toFloat(), right * scale, pad.bottom.toFloat())
}
