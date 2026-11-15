// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView

/** What the reading pane draws apart from the owner's words: the message that is open. */
internal data class ReadingUi(
    val id: ULong? = null,
    val load: LoadState = LoadState.Loading,
    val opened: OpenedMessage? = null,
)

/** What a block of a picture says of its size. */
internal class PictureRef(val src: String, val part: String?, val width: Int?, val height: Int?)

/** A picture declared this small or smaller is a tracking pixel or a spacer: never asked for. */
internal const val TINY_PICTURE = 2

/** [blocks] and every block inside their tables, in reading order. */
internal fun allBlocks(blocks: List<BlockView>): Sequence<BlockView> = blocks.asSequence().flatMap { block ->
    val kind = block.kind
    val inside = if (kind is BlockKindView.Table) {
        kind.table.rows.asSequence().flatMap { it.cells.asSequence() }.flatMap { allBlocks(it.blocks) }
    } else {
        emptySequence()
    }
    sequenceOf(block) + inside
}

/** Whether a picture's declared size says it is a spacer or a pixel: 2 px or smaller either way. */
internal fun isTiny(ref: PictureRef): Boolean =
    (ref.width != null && ref.width <= TINY_PICTURE) || (ref.height != null && ref.height <= TINY_PICTURE)
