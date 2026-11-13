// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:OptIn(ExperimentalLayoutApi::class)

package work.mailcraft.android

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LineHeightView
import uniffi.mailcraft_mobile.MarkerView

/** What the body of a message is drawn with and what it does. */
internal class BodyEnv(
    val texts: Texts,
    val links: Map<String, LinkInfo>,
    /** The owner's colours for the lines a message does not colour itself. */
    val rule: Color,
    val onLink: (LinkInfo) -> Unit,
    val onLinkDetails: (LinkInfo) -> Unit,
)

/** Blocks that stand in one line (a picture in a line of text, text after a picture), and those that stand alone. */
internal class BodyLine(val blocks: List<BlockView>)

/** The blocks grouped into lines: a block that goes on in the line of the one before joins it. */
internal fun linesOf(blocks: List<BlockView>): List<BodyLine> {
    val lines = ArrayList<MutableList<BlockView>>()
    for (block in blocks) {
        if (block.beside && lines.isNotEmpty()) lines.last().add(block) else lines.add(mutableListOf(block))
    }
    return lines.map { BodyLine(it) }
}

/** Blank space above a paragraph, in dp: the message's own gap when it says, else one step for each blank line. */
internal fun spaceAbove(block: BlockView): Dp {
    val steps = block.space.toInt().coerceIn(0, MAX_SPACE_STEPS)
    return if (block.gap.toInt() > 0) block.gap.toInt().dp else (steps * SPACE_STEP).dp
}

private const val MAX_SPACE_STEPS = 3
private const val SPACE_STEP = 10
private const val QUOTE_INDENT = 12
private const val MAX_QUOTE_LEVELS = 8
private const val SPACE_FOR_QUOTE_BAR = 3

/** One line of a body, with the space above it. */
@Composable
internal fun BodyLineView(line: BodyLine, env: BodyEnv, paper: Paper, width: Dp) {
    val first = line.blocks.first()
    val pad = Modifier.padding(top = spaceAbove(first))
    Box(modifier = pad.quoted(first.quote.toInt(), env.rule)) {
        if (line.blocks.size == 1) {
            SingleBlock(first, env, paper, width)
        } else {
            InlineRun(line.blocks, env, paper, width)
        }
    }
}

@Composable
private fun SingleBlock(block: BlockView, env: BodyEnv, paper: Paper, width: Dp) {
    val inset = Modifier.padding(start = block.insetLeft.toInt().dp, end = block.insetRight.toInt().dp)
    val room = (width - (block.insetLeft.toInt() + block.insetRight.toInt()).dp - quoteWidth(block.quote.toInt()))
        .coerceAtLeast(MIN_ROOM)
    Box(modifier = inset.fillMaxWidth(), contentAlignment = alignment(block.align)) {
        when (val kind = block.kind) {
            is BlockKindView.Table -> TableBlock(kind.table, block.align, env, paper, room)
            is BlockKindView.Image -> PictureBlock(block, kind, env, paper, room)
            is BlockKindView.Rule -> RuleBlock(block, env)
            is BlockKindView.ListItem -> ListItem(block, kind, env, paper)
            is BlockKindView.Heading -> TextBlock(block, env, paper, heading = kind.level.toInt())
            is BlockKindView.Preformatted -> TextBlock(block, env, paper, mono = true)
            else -> TextBlock(block, env, paper)
        }
    }
}

private val MIN_ROOM = 40.dp
private val QuoteBar = 2.dp
private val RunGap = 4.dp
private val RunLineGap = 2.dp

internal fun quoteWidth(levels: Int): Dp = (levels.coerceIn(0, MAX_QUOTE_LEVELS) * QUOTE_INDENT).dp

private fun alignment(align: AlignView): Alignment = when (align) {
    AlignView.LEFT -> Alignment.TopStart
    AlignView.CENTER -> Alignment.TopCenter
    AlignView.RIGHT -> Alignment.TopEnd
}

/** A bar for each level of quoting, with the text moved in past them. */
private fun Modifier.quoted(levels: Int, color: Color): Modifier {
    val count = levels.coerceIn(0, MAX_QUOTE_LEVELS)
    if (count == 0) return this
    return this
        .drawBehind {
            for (level in 0 until count) {
                val x = (level * QUOTE_INDENT + SPACE_FOR_QUOTE_BAR).dp.toPx()
                drawLine(color, Offset(x, 0f), Offset(x, size.height), strokeWidth = QuoteBar.toPx())
            }
        }
        .padding(start = quoteWidth(count))
}

/** A run of blocks in one line: text and pictures side by side, wrapping when the line is full. */
@Composable
private fun InlineRun(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    if (blocks.all { it.kind !is BlockKindView.Image && it.kind !is BlockKindView.Table }) {
        // Text that goes on in the line of the text before it is one text.
        val merged = blocks.first().let { head ->
            BlockView(
                head.kind, blocks.flatMap { it.spans }, head.quote, head.align, head.space, false, head.gap,
                head.lineHeight, head.insetLeft, head.insetRight, head.imagePadding, head.line,
            )
        }
        SingleBlock(merged, env, paper, width)
        return
    }
    FlowRow(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(RunGap),
        verticalArrangement = Arrangement.spacedBy(RunLineGap),
    ) {
        blocks.forEach { block ->
            when (val kind = block.kind) {
                is BlockKindView.Image -> PictureBlock(block, kind, env, paper, width)
                is BlockKindView.Table -> TableBlock(kind.table, block.align, env, paper, width)
                else -> TextBlock(block, env, paper)
            }
        }
    }
}

/** Blocks one under another: the cells of a table and the like. */
@Composable
internal fun BlockColumn(blocks: List<BlockView>, env: BodyEnv, paper: Paper, width: Dp) {
    androidx.compose.foundation.layout.Column(modifier = Modifier.fillMaxWidth()) {
        linesOf(blocks).forEach { BodyLineView(it, env, paper, width) }
    }
}
