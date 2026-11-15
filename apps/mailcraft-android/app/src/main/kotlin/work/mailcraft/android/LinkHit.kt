// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.text.TextLayoutResult

/**
 * The characters a tap may be on, given the nearest offset: the one at [offset] and the one before it
 * (on the right half of a character the nearest offset is the boundary after it), those inside the text.
 */
internal fun hitCandidates(offset: Int, length: Int): List<Int> =
    listOf(offset, offset - 1).filter { it in 0 until length }

/** The link whose drawn text [at] is on: a character is a hit only when the tap is inside its box. */
internal fun linkAt(layout: TextLayoutResult?, text: StyledText, at: Offset): LinkInfo? {
    val shown = layout ?: return null
    val offset = shown.getOffsetForPosition(at)
    val length = shown.layoutInput.text.length
    val hit = hitCandidates(offset, length).firstOrNull { shown.getBoundingBox(it).contains(at) }
    return hit?.let { text.linkAt(it) }
}
