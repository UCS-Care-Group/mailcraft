// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView

/** How a picture of the message is coming along. */
internal sealed interface PictureState {
    /** Asked for, not here yet. */
    data object Pending : PictureState

    /** Allowed, but the system's data saving holds it back until the owner says so. */
    data object Waiting : PictureState

    /** It could not be had. */
    data object Failed : PictureState

    /** Not asked for: the message has more pictures, or heavier ones, than are downloaded for one. */
    data object LeftOut : PictureState

    class Ready(val bytes: ByteArray) : PictureState
}

/** What the reading pane draws apart from the owner's words: the message, and how its pictures are coming. */
internal data class ReadingUi(
    val id: ULong? = null,
    val load: LoadState = LoadState.Loading,
    val opened: OpenedMessage? = null,
    val access: PictureAccess = PictureAccess.Blocked,
    /** By the reference a block names (a part of the message) or by its web address. */
    val pictures: Map<String, PictureState> = emptyMap(),
    /** Pictures are allowed but data saving holds the downloads back. */
    val waiting: Boolean = false,
    val prefs: PicturePrefs = PicturePrefs(),
) {
    val facts: MessageFacts? get() = opened?.facts

    /** Whether the banner offering Download pictures is shown: the message names pictures on the web, none allowed. */
    val offersPictures: Boolean
        get() = access == PictureAccess.Blocked && (opened?.view?.remotePictures ?: 0u) > 0u

    /** The ways the banner offers to allow them. */
    val choices: List<AllowScope> get() = facts?.let(PicturePolicy::choices).orEmpty()
}

/** One picture block of a message. */
internal class PictureRef(val src: String, val part: String?, val width: Int?, val height: Int?)

/** The most pictures of one message that are asked for, and the most bytes taken for them. */
internal const val MAX_PICTURES_PER_MESSAGE = 60
internal const val MAX_MESSAGE_PICTURE_BYTES = 24 * 1024 * 1024

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

/** The picture blocks of [blocks] in reading order, inside tables too. */
internal fun pictureRefs(blocks: List<BlockView>): List<PictureRef> = allBlocks(blocks)
    .mapNotNull { (it.kind as? BlockKindView.Image)?.let(::refOf) }
    .toList()

private fun refOf(kind: BlockKindView.Image) =
    PictureRef(kind.src, kind.picture, kind.width?.toInt(), kind.height?.toInt())

/** Whether [src] is a picture on the web (not a part of the message, nor one written into it). */
internal fun isWebPicture(src: String): Boolean {
    val lower = src.trim().lowercase()
    return lower.startsWith("http://") || lower.startsWith("https://")
}

/** Whether a picture's declared size says it is a spacer or a pixel: 2 px or smaller either way. */
internal fun isTiny(ref: PictureRef): Boolean =
    (ref.width != null && ref.width <= TINY_PICTURE) || (ref.height != null && ref.height <= TINY_PICTURE)

/** Most messages whose cost is kept track of. */
private const val MAX_MESSAGES_KEPT = 200

/** What downloading has cost each message: how many pictures were asked for and how many bytes came. */
internal class PictureBudget {
    private class Spent(var asked: Int = 0, var bytes: Long = 0)

    private val spent = LinkedHashMap<ULong, Spent>()

    /** Whether one more picture may be asked for [id]'s message: it counts as asked. */
    @Synchronized
    fun take(id: ULong): Boolean {
        val cost = spent.getOrPut(id) { Spent() }
        if (spent.size > MAX_MESSAGES_KEPT) spent.remove(spent.keys.first())
        val out = cost.asked >= MAX_PICTURES_PER_MESSAGE || cost.bytes >= MAX_MESSAGE_PICTURE_BYTES
        if (!out) cost.asked += 1
        return !out
    }

    /** [size] bytes came for [id]'s message. */
    @Synchronized
    fun add(id: ULong, size: Int) {
        spent.getOrPut(id) { Spent() }.bytes += size
    }
}
