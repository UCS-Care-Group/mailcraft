// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.layout
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import kotlin.math.roundToInt

/** The text of a block's spans. */
internal fun plainText(block: BlockView): String = block.spans.joinToString("") { it.text }.trim()

/** A picture is decorative (and hidden from a screen reader) when it has no description. */
internal fun isDecorative(alt: String): Boolean = alt.isBlank()

/** The state of the picture a block names: by its part reference, else its web address. */
internal fun BodyEnv.stateOf(kind: BlockKindView.Image): PictureState? = pictures[kind.picture ?: kind.src]

/**
 * A picture in a message. Where it is: the picture, as wide as the message says (or as it is) and no
 * wider than the column. Where it is not (not allowed, waiting, on its way, failed): a plain box of the
 * declared size, clamped to the column and to [MAX_PLACEHOLDER_HEIGHT], with the picture's description,
 * or the description alone when the message gives no size. A muted surface, nothing animated, nothing
 * to tap. A picture with no description, or declared 2 px or smaller, says nothing to a screen reader.
 */
@Composable
internal fun PictureBlock(block: BlockView, kind: BlockKindView.Image, env: BodyEnv, paper: Paper, room: Dp) {
    val ref = PictureRef(kind.src, kind.picture, kind.width?.toInt(), kind.height?.toInt())
    if (isTiny(ref)) return
    val alt = plainText(block)
    val state = env.stateOf(kind)
    val link = kind.link?.let { env.links[it] }
    val tap = if (link == null) {
        Modifier
    } else {
        Modifier.combinedClickable(
            role = Role.Button,
            onClick = { env.onLink(link) },
            onLongClick = { env.onLinkDetails(link) },
        )
    }
    val described = if (isDecorative(alt)) {
        Modifier.clearAndSetSemantics {}
    } else {
        Modifier.semantics { contentDescription = alt }
    }
    val decoded = rememberDecoded((state as? PictureState.Ready)?.bytes)
    Box(modifier = tap.then(described)) {
        if (decoded != null) {
            ShownPicture(decoded, ref, room, alt)
        } else {
            Placeholder(ref, alt, room, paper)
        }
    }
}

@Composable
private fun ShownPicture(decoded: Decoded, ref: PictureRef, room: Dp, alt: String) {
    val density = LocalDensity.current.density
    val natural = (decoded.width / density).dp
    val wanted = (ref.width?.dp ?: natural).coerceAtMost(room).coerceAtLeast(1.dp)
    Image(
        bitmap = decoded.bitmap,
        contentDescription = alt.ifBlank { null },
        contentScale = ContentScale.Fit,
        modifier = Modifier.pictureBox(wanted, decoded.height.toFloat() / decoded.width.coerceAtLeast(1), null),
    )
}

@Composable
private fun Placeholder(ref: PictureRef, alt: String, room: Dp, paper: Paper) {
    val onLight = Readability.isLight(paper.rgb)
    val muted = composeColor(if (onLight) MUTED_ON_LIGHT else MUTED_ON_DARK)
    val ink = composeColor(Readability.adjust(MUTED_INK, if (onLight) MUTED_ON_LIGHT else MUTED_ON_DARK))
    val w = ref.width
    val h = ref.height
    if (w != null && h != null) {
        val box = Modifier.pictureBox(w.dp.coerceAtMost(room), h.toFloat() / w.coerceAtLeast(1), MAX_PLACEHOLDER_HEIGHT)
        Box(
            modifier = box.background(muted),
            contentAlignment = Alignment.Center,
        ) {
            if (alt.isNotBlank()) AltText(alt, ink, MaterialTheme.typography.labelSmall, Modifier.padding(BoxPad))
        }
    } else if (alt.isNotBlank()) {
        AltText(alt, ink, MaterialTheme.typography.labelMedium, Modifier.background(muted).padding(ChipPad, ChipPadV))
    }
}

/** The most height a picture that is not shown takes, in dp: a tall one does not push the message away. */
internal val MAX_PLACEHOLDER_HEIGHT = 120.dp

/**
 * A picture's box in pixels: [wanted] wide but no wider than [room], [tall] high for each unit of width,
 * no higher than [cap], never nothing, and never more than [MAX_BOX_WIDTH] by [MAX_BOX_HEIGHT], which
 * `Constraints.fixed` always takes. (It throws past what it can pack: a width under 8192 px leaves room
 * for a height of 262143 px. A message's picture can be 1 by 40 million pixels.)
 */
internal fun boxSize(wanted: Int, room: Int, tall: Float, cap: Int): IntSize {
    val w = wanted.coerceAtMost(room).coerceIn(1, MAX_BOX_WIDTH)
    val h = (w * tall.toDouble()).takeUnless { it.isNaN() } ?: 1.0
    return IntSize(w, h.coerceIn(1.0, cap.coerceIn(1, MAX_BOX_HEIGHT).toDouble()).roundToInt())
}

private const val MAX_BOX_WIDTH = 8_000
private const val MAX_BOX_HEIGHT = 200_000

/**
 * A picture's box sized from the width there really is, unlike `width().aspectRatio()`: a tinted row
 * measures its cells' least height at the row's whole width, and a shape alone answered with a box
 * that wide, an empty space screens tall under a row of small icons (#1966). Asked the same way, this
 * answers what it draws.
 */
private fun Modifier.pictureBox(wanted: Dp, tall: Float, cap: Dp?): Modifier = layout { measurable, constraints ->
    val size = boxSize(wanted.roundToPx(), constraints.maxWidth, tall, cap?.roundToPx() ?: Int.MAX_VALUE)
    val placeable = measurable.measure(Constraints.fixed(size.width, size.height))
    layout(size.width, size.height) { placeable.place(0, 0) }
}

@Composable
private fun AltText(alt: String, ink: Color, style: TextStyle, modifier: Modifier) {
    Text(alt, color = ink, style = style, maxLines = 2, overflow = TextOverflow.Ellipsis, modifier = modifier)
}

private val BoxPad = 4.dp
private val ChipPad = 6.dp
private val ChipPadV = 2.dp
private const val MUTED_ON_LIGHT = 0xE4E8EB
private const val MUTED_ON_DARK = 0x2B3033
private const val MUTED_INK = 0x5C6670
