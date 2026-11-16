// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle

import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView

/** The text of a block's spans. */
internal fun plainText(block: BlockView): String = block.spans.joinToString("") { it.text }.trim()

/** A picture is decorative (and hidden from a screen reader) when it has no description. */
internal fun isDecorative(alt: String): Boolean = alt.isBlank()

/** The state of the picture a block names: by its part reference, else its web address. */
internal fun BodyEnv.stateOf(kind: BlockKindView.Image): PictureState? = pictures[kind.picture ?: kind.src]

/**
 * A picture in a message. Where it is: the picture, as wide as the message says (or as it is) and no
 * wider than the column. Where it is not (not allowed, waiting, on its way, failed): a plain box of the
 * declared size, clamped to the column, with the picture's description, or the description alone when
 * the message gives no size. A muted surface, nothing animated, nothing to tap. A picture with no
 * description, or declared 2 px or smaller, says nothing to a screen reader.
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
        modifier = Modifier.width(wanted).aspectRatio(decoded.width.toFloat() / decoded.height.coerceAtLeast(1)),
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
        val shown = w.dp.coerceAtMost(room).coerceAtLeast(1.dp)
        Box(
            modifier = Modifier.width(shown).aspectRatio(w.toFloat() / h.coerceAtLeast(1)).background(muted),
            contentAlignment = Alignment.Center,
        ) {
            if (alt.isNotBlank()) AltText(alt, ink, MaterialTheme.typography.labelSmall, Modifier.padding(BoxPad))
        }
    } else if (alt.isNotBlank()) {
        AltText(alt, ink, MaterialTheme.typography.labelMedium, Modifier.background(muted).padding(ChipPad, ChipPadV))
    }
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
