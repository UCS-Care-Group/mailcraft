// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.TableView

/** What a screen reader is told of one thing in a message body, in reading order. */
internal sealed interface Announced {
    class Heading(val level: Int, val text: String) : Announced

    class Text(val text: String) : Announced

    class Picture(val description: String) : Announced

    /** A data table: read as a table of [rows] by [columns]. */
    class Table(val rows: Int, val columns: Int) : Announced

    /** A cell of a data table, at a row and column (from 1). */
    class Cell(val row: Int, val column: Int, val header: Boolean) : Announced

    /** A rule is read as a separator. */
    data object Separator : Announced
}

/** The text of a block's spans. */
internal fun plainText(block: BlockView): String = block.spans.joinToString("") { it.text }.trim()

/** A picture is decorative (and hidden from a screen reader) when it has no description. */
internal fun isDecorative(alt: String): Boolean = alt.isBlank()

/** How many rows and columns a data table has, for a screen reader. */
internal fun tableSize(table: TableView): Pair<Int, Int> =
    table.rows.size to maxOf(table.columns.toInt(), table.rows.maxOfOrNull { it.cells.size } ?: 0)

/**
 * What a screen reader is told for [blocks], in reading order: a layout table is read as plain blocks
 * (its cells one after another, no word of a table), a data table as a table with its cells, a
 * picture by its description or not at all when it has none, a heading as a heading.
 */
internal fun announced(blocks: List<BlockView>): List<Announced> {
    val out = ArrayList<Announced>()
    blocks.forEach { announceBlock(it, out) }
    return out
}

private fun announceBlock(block: BlockView, out: MutableList<Announced>) {
    when (val kind = block.kind) {
        is BlockKindView.Table -> announceTable(kind.table, out)
        is BlockKindView.Image -> announcePicture(block, kind, out)
        is BlockKindView.Rule -> out += Announced.Separator
        is BlockKindView.Heading -> out += Announced.Heading(kind.level.toInt(), plainText(block))
        else -> plainText(block).takeIf { it.isNotEmpty() }?.let { out += Announced.Text(it) }
    }
}

private fun announcePicture(block: BlockView, kind: BlockKindView.Image, out: MutableList<Announced>) {
    val alt = plainText(block)
    val ref = PictureRef(kind.src, kind.picture, kind.width?.toInt(), kind.height?.toInt())
    if (!isDecorative(alt) && !isTiny(ref)) out += Announced.Picture(alt)
}

private fun announceTable(table: TableView, out: MutableList<Announced>) {
    if (table.layout) {
        table.rows.forEach { row -> row.cells.forEach { out += announced(it.blocks) } }
        return
    }
    val (rows, columns) = tableSize(table)
    out += Announced.Table(rows, columns)
    table.rows.forEachIndexed { r, row ->
        row.cells.forEachIndexed { c, cell ->
            out += Announced.Cell(r + 1, c + 1, cell.header)
            out += announced(cell.blocks)
        }
    }
}
