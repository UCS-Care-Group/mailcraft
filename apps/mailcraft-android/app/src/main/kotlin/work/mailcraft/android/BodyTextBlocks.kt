// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LineHeightView
import uniffi.mailcraft_mobile.MarkerView

internal fun textAlign(align: AlignView): TextAlign = when (align) {
    AlignView.LEFT -> TextAlign.Start
    AlignView.CENTER -> TextAlign.Center
    AlignView.RIGHT -> TextAlign.End
}

/** The size of a heading of [level] (1 to 6), in sp. */
internal fun headingSize(level: Int): Int = HEADING_SIZES.getOrElse(level - 1) { BASE_SIZE }

private val HEADING_SIZES = intArrayOf(26, 22, 19, 17)

/** A paragraph, a heading (when [heading] is its level) or preformatted text, with its links. */
@Composable
internal fun TextBlock(block: BlockView, env: BodyEnv, paper: Paper, heading: Int = 0, mono: Boolean = false) {
    val text = styled(block.spans, paper, env.links)
    if (text.text.isEmpty()) return
    val base = if (heading > 0) headingSize(heading) else BASE_SIZE
    val style = TextStyle(
        color = composeColor(paper.ink),
        fontSize = base.sp,
        fontWeight = if (heading > 0) FontWeight.Bold else null,
        textAlign = textAlign(block.align),
        lineHeight = lineHeightOf(block, base),
        fontFamily = if (mono) FontFamily.Monospace else null,
    )
    val semantic = if (heading > 0) Modifier.semantics { heading() } else Modifier
    LinkedText(text, style, env, semantic.fillMaxWidth())
}

/** The line height the message asks for, or one that reads well. */
internal fun lineHeightOf(block: BlockView, size: Int): TextUnit = when (val h = block.lineHeight) {
    is LineHeightView.Scale -> (size * h.hundredths.toInt() / HUNDRED).sp
    is LineHeightView.Px -> h.px.toInt().coerceAtLeast(1).sp
    null -> (size * DEFAULT_LINE).sp
}

/** A list item: its marker, and its text beside it. */
@Composable
internal fun ListItem(block: BlockView, kind: BlockKindView.ListItem, env: BodyEnv, paper: Paper) {
    val marker = when (val m = kind.marker) {
        is MarkerView.Bullet -> BULLET
        is MarkerView.Number -> "${m.number}."
        is MarkerView.Label -> m.text
        is MarkerView.Nothing -> ""
    }
    val indent = (kind.depth.toInt().coerceIn(0, MAX_LIST_DEPTH) * LIST_INDENT).dp
    Row(modifier = Modifier.padding(start = indent), horizontalArrangement = Arrangement.spacedBy(MARKER_GAP.dp)) {
        BasicText(
            marker,
            modifier = Modifier.width(MARKER_WIDTH.dp),
            style = TextStyle(color = composeColor(paper.ink), fontSize = BASE_SIZE.sp, textAlign = TextAlign.End),
        )
        Box(modifier = Modifier.weight(1f)) { TextBlock(block, env, paper) }
    }
}

/** A horizontal rule, as thick and as coloured as the message says (a line of 1 to 4 pixels). */
@Composable
internal fun RuleBlock(block: BlockView, env: BodyEnv) {
    val line = block.line
    val thickness = line.width.toInt().coerceIn(1, MAX_RULE)
    val color = line.color?.let { composeColor(it.rgb()) } ?: env.rule
    Spacer(
        modifier = Modifier
            .padding(vertical = RULE_MARGIN.dp)
            .fillMaxWidth()
            .height(thickness.dp)
            .background(color),
    )
}

private const val BULLET = "•"
private const val HUNDRED = 100f
private const val DEFAULT_LINE = 1.4f
private const val MAX_LIST_DEPTH = 8
private const val LIST_INDENT = 18
private const val MARKER_WIDTH = 24
private const val MARKER_GAP = 4
private const val RULE_MARGIN = 6
private const val MAX_RULE = 4
