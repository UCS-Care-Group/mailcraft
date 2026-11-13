// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.LineStyleView
import uniffi.mailcraft_mobile.LineView
import uniffi.mailcraft_mobile.SidesView

internal val ALL_SIDES = SidesView(top = true, right = true, bottom = true, left = true)

internal fun SidesView.hasAny() = top || right || bottom || left

private const val DASH = 6f
private const val DOT = 2f
private const val INVISIBLE_CONTRAST = 1.2
private const val MAX_LINE = 4
private const val TOP = 0
private const val RIGHT = 1
private const val BOTTOM = 2

/**
 * The lines on [sides] of a box (all of them for null), as the message draws them, thin and in the
 * reader's own colour where it says nothing; a colour that would not be seen on [paper] is the reader's.
 */
internal fun Modifier.frame(lines: List<LineView>, sides: SidesView?, fallback: Color, paper: Paper): Modifier {
    val wanted = sides ?: ALL_SIDES
    if (!wanted.hasAny()) return this
    val flags = booleanArrayOf(wanted.top, wanted.right, wanted.bottom, wanted.left)
    return drawBehind {
        flags.forEachIndexed { side, on -> if (on) drawSide(side, lines.getOrNull(side), fallback, paper) }
    }
}

private fun DrawScope.drawSide(side: Int, line: LineView?, fallback: Color, paper: Paper) {
    val thickness = (line?.width?.toInt() ?: 0).coerceIn(1, MAX_LINE).dp.toPx()
    val seen = line?.color?.rgb()?.takeIf { Readability.contrast(it, paper.rgb) >= INVISIBLE_CONTRAST }
    val effect = when (line?.style) {
        LineStyleView.DASHED -> PathEffect.dashPathEffect(floatArrayOf(DASH * thickness, DASH * thickness))
        LineStyleView.DOTTED -> PathEffect.dashPathEffect(floatArrayOf(DOT * thickness, DOT * thickness))
        else -> null
    }
    val half = thickness / 2
    val (from, to) = when (side) {
        TOP -> Offset(0f, half) to Offset(size.width, half)
        RIGHT -> Offset(size.width - half, 0f) to Offset(size.width - half, size.height)
        BOTTOM -> Offset(0f, size.height - half) to Offset(size.width, size.height - half)
        else -> Offset(half, 0f) to Offset(half, size.height)
    }
    drawLine(seen?.let { composeColor(it) } ?: fallback, from, to, strokeWidth = thickness, pathEffect = effect)
}
