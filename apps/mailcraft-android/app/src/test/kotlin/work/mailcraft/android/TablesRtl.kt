// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.MessageView
import uniffi.mailcraft_mobile.TableView

/**
 * #1966: invented messages with data tables and text that reads from the right, as the core sends
 * them (the places and directions are what mailcraft-mobile's `tables_rtl_tests` checks for the
 * same HTML). The people, shops and Hebrew and Arabic text are made up.
 */
internal object TablesRtl {
    private val dana = AddressRecord("Dana Example", "dana@example.org")
    private val me = AddressRecord("Bo Example", "bo@example.org")

    private fun view(id: ULong, subject: String, blocks: List<BlockView>) = MessageView(
        1uL, id, 1uL, 1uL, dana, listOf(me), emptyList(), subject, 1_730_000_000L, false, false, emptyList(),
        emptyList(), blocks, false, 0u, true,
    )

    private fun th(text: String, at: Pair<Int, Int>) = B.cell(B.block(BlockKindView.Paragraph, B.span(text, bold = true), space = 0u), header = true, at = at)

    private fun td(text: String, at: Pair<Int, Int>, align: AlignView = AlignView.LEFT, rtl: Boolean = false, colspan: UShort = 1u, rowspan: UShort = 1u) =
        B.cell(B.block(BlockKindView.Paragraph, B.span(text), align = align, space = 0u, rtl = rtl), colspan = colspan, rowspan = rowspan, at = at)

    /** An order: the order number spans its two lines, the total the first three columns. */
    val orderTable: TableView = B.table(
        layout = false,
        border = true,
        rows = listOf(
            listOf(th("Order", 0 to 0), th("Item", 0 to 1), th("Qty", 0 to 2), th("Price", 0 to 3)),
            listOf(td("A-1042", 1 to 0, rowspan = 2u), td("Seed trays", 1 to 1), td("4", 1 to 2, AlignView.RIGHT), td("€12.00", 1 to 3, AlignView.RIGHT)),
            listOf(td("Twine, 100 m", 2 to 1), td("2", 2 to 2, AlignView.RIGHT), td("€5.40", 2 to 3, AlignView.RIGHT)),
            listOf(td("Total", 3 to 0, colspan = 3u), td("€17.40", 3 to 3, AlignView.RIGHT)),
        ),
    )

    val order = Corpus.Entry(
        "order",
        view(
            10uL, "Your order A-1042",
            listOf(B.p("Your order from the garden shop:", 0u), B.tableBlock(orderTable), B.p("It leaves the shop on Tuesday.")),
        ),
        emptyList(),
    )

    /** A bank statement of eight columns: too many for a phone's width, so it scrolls at the least its words need. */
    val statement = Corpus.Entry(
        "statement",
        view(
            11uL, "Your March statement",
            listOf(
                B.p("Your statement for March:", 0u),
                B.tableBlock(
                    B.table(
                        layout = false,
                        border = true,
                        rows = listOf(listOf("Date", "Ref", "Payee", "Category", "Paid in", "Paid out", "Balance", "Note").mapIndexed { c, h -> th(h, 0 to c) }) +
                            (0 until 3).map { n ->
                                val r = n + 1
                                listOf(
                                    td("0$r Mar", r to 0), td("TX-90$n", r to 1), td("Fern Hollow nursery", r to 2), td("Garden", r to 3),
                                    td("", r to 4, AlignView.RIGHT), td("€6$n.50", r to 5, AlignView.RIGHT), td("€1,2${n}0.00", r to 6, AlignView.RIGHT), td("Compost", r to 7),
                                )
                            },
                    ),
                ),
                B.p("Questions? Reply to this message."),
            ),
        ),
        emptyList(),
    )

    /** A reply written from the right, in Hebrew and Arabic, over a quote that reads from the left. */
    val rtlReply = Corpus.Entry(
        "rtl_reply",
        view(
            12uL, "Re: ההזמנה",
            listOf(
                B.block(BlockKindView.Paragraph, B.span("שלום בו, ההזמנה יצאה היום ותגיע ביום שלישי, 3 במרץ."), align = AlignView.RIGHT, space = 0u, rtl = true),
                B.block(BlockKindView.Paragraph, B.span("مرحبا، الطلب رقم 1042 جاهز للشحن منذ الصباح."), align = AlignView.RIGHT, rtl = true),
                B.block(BlockKindView.ListItem(uniffi.mailcraft_mobile.MarkerView.Bullet, 0u), B.span("מגשי זרעים"), align = AlignView.RIGHT, space = 0u, rtl = true),
                B.block(BlockKindView.Paragraph, B.span("דנה"), align = AlignView.RIGHT, rtl = true),
                B.block(BlockKindView.Paragraph, B.span("On Monday, Bo Example wrote:"), quote = 1u),
                B.block(BlockKindView.Paragraph, B.span("Could you send the seed trays this week?"), quote = 1u, space = 0u),
            ),
        ),
        emptyList(),
    )

    fun lines(n: Int) = (1..n).joinToString("\n") { "Line $it of the delivery note" }

    /** A cell of about 5,000 lines in a table of the phone's width: taller than a measure can pack beside its width. */
    val tallNarrow = Corpus.Entry(
        "tall_narrow",
        view(14uL, "Delivery note", listOf(B.p("The full note:", 0u), B.tableBlock(B.table(layout = false, border = true, rows = listOf(listOf(td("Note", 0 to 0), td(lines(5_000), 0 to 1))))))),
        emptyList(),
    )

    /** A note of [n] lines in a coloured box: a layout table of one tinted cell, as a coloured `<div>` becomes. */
    fun tintedNote(n: Int): TableView = B.table(layout = true, rows = listOf(listOf(B.cell(B.p(lines(n), 0u), background = 0xFFF4D6u))))

    /** The same note in a table of one cell with no colour: a wrapper `<table>` round a long message. */
    fun plainNote(n: Int): TableView = B.table(layout = true, rows = listOf(listOf(B.cell(B.p(lines(n), 0u)))))

    /** A digest of about 6,000 lines in a coloured box. */
    val tallTinted = Corpus.Entry(
        "tall_tinted",
        view(16uL, "Weekly digest", listOf(B.p("This week:", 0u), B.tableBlock(tintedNote(6_000)))),
        emptyList(),
    )

    /** The same coloured box inside a cell of a data table. */
    val tallNested = Corpus.Entry(
        "tall_nested",
        view(
            17uL, "Weekly digest, filed",
            listOf(
                B.p("This week:", 0u),
                B.tableBlock(B.table(layout = false, border = true, rows = listOf(listOf(td("Digest", 0 to 0), B.cell(B.tableBlock(tintedNote(6_000)), at = 0 to 1))))),
            ),
        ),
        emptyList(),
    )

    /** Thirty columns of long words, wider than a measure takes (about 10,000 px), and a footer over all of them of about 1,100 lines. */
    val tallWide = Corpus.Entry(
        "tall_wide",
        view(
            15uL, "Delivery note, wide",
            listOf(
                B.p("The full note:", 0u),
                B.tableBlock(
                    B.table(
                        layout = false,
                        border = true,
                        rows = listOf(
                            (0 until 30).map { c -> td("Reference_number_of_column_$c", 0 to c) },
                            listOf(td(lines(1_100), 1 to 0, colspan = 30u)),
                        ),
                    ),
                ),
            ),
        ),
        emptyList(),
    )

    /** A table whose cells read both ways: each stands at its own side. */
    val mixed = Corpus.Entry(
        "mixed_table",
        view(
            13uL, "Price list / מחירון",
            listOf(
                B.p("The prices, in both languages:", 0u),
                B.tableBlock(
                    B.table(
                        layout = false,
                        border = true,
                        rows = listOf(
                            listOf(B.cell(B.block(BlockKindView.Paragraph, B.span("מוצר", bold = true), align = AlignView.RIGHT, space = 0u, rtl = true), header = true, at = 0 to 0), th("Item", 0 to 1), th("Price", 0 to 2)),
                            listOf(td("מגש זרעים", 1 to 0, AlignView.RIGHT, rtl = true), td("Seed tray", 1 to 1), td("€3.00", 1 to 2, AlignView.RIGHT)),
                            listOf(td("صندوق أدوات", 2 to 0, AlignView.RIGHT, rtl = true), td("Tool box", 2 to 1), td("€24.00", 2 to 2, AlignView.RIGHT)),
                        ),
                    ),
                ),
            ),
        ),
        emptyList(),
    )
}
