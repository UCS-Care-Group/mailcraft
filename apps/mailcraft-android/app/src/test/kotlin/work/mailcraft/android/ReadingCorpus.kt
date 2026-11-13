// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import java.util.Base64
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.AttachmentRecord
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.LineStyleView
import uniffi.mailcraft_mobile.LineView
import uniffi.mailcraft_mobile.MarkerView
import uniffi.mailcraft_mobile.MessageView
import uniffi.mailcraft_mobile.RowView
import uniffi.mailcraft_mobile.SidesView
import uniffi.mailcraft_mobile.SpanView
import uniffi.mailcraft_mobile.TableView

/** Builders for display blocks, so that the tests and the screenshots hold invented messages only (example.org). */
internal object B {
    val noLine = LineView(0u, LineStyleView.SOLID, null)
    val noSides = SidesView(top = false, right = false, bottom = false, left = false)

    fun span(
        text: String,
        bold: Boolean = false,
        italic: Boolean = false,
        link: String? = null,
        color: UInt? = null,
        background: UInt? = null,
        size: UByte? = null,
    ) = SpanView(text, bold, italic, false, false, false, link, color, background, size, null)

    fun block(
        kind: BlockKindView,
        vararg spans: SpanView,
        quote: UByte = 0u,
        align: AlignView = AlignView.LEFT,
        space: UByte = 1u,
        beside: Boolean = false,
    ) = BlockView(kind, spans.toList(), quote, align, space, beside, 0u, null, 0u, 0u, null, noLine)

    fun p(text: String, space: UByte = 1u) = block(BlockKindView.Paragraph, span(text), space = space)

    fun h(level: UByte, text: String) = block(BlockKindView.Heading(level), span(text, bold = true))

    fun item(text: String) = block(BlockKindView.ListItem(MarkerView.Bullet, 0u), span(text), space = 0u)

    fun image(src: String, alt: String, width: UShort? = null, height: UShort? = null, picture: String? = null, link: String? = null) =
        block(BlockKindView.Image(src, width, height, link, picture), span(alt), space = 0u)

    fun cell(vararg blocks: BlockView, header: Boolean = false, width: LengthView? = null, background: UInt? = null, padding: UByte? = null, colspan: UShort = 1u) =
        CellView(
            blocks.toList(), header, colspan, 1u, background, width, noSides, emptyList(),
            padding?.let { InsetsView(it, it, it, it) }, 0u, null,
        )

    fun table(layout: Boolean, rows: List<List<CellView>>, width: LengthView? = null, border: Boolean = false) = TableView(
        rows.map { RowView(it) }, (rows.maxOfOrNull { r -> r.sumOf { it.colspan.toInt() } } ?: 0).toUShort(), layout, border, false, width, null,
        emptyList(), 0u, null, inline = false, spaced = false, boxMarginLeft = 0u, boxMarginRight = 0u, float = false,
    )

    fun tableBlock(table: TableView, align: AlignView = AlignView.LEFT) = block(BlockKindView.Table(table), align = align, space = 1u)

    /** The picture a test draws for a part of the message: 96 by 48 pixels of one colour. */
    val png: ByteArray = Base64.getDecoder().decode(
        "iVBORw0KGgoAAAANSUhEUgAAAGAAAAAwCAIAAABhdOiYAAAAiklEQVR4nO3YQQ0AIRAEQdQhDE0rEAc8IGT2UQkKOj19cGOucg4EBjpnAgAB9NYQBgHEoK/fGRMDiEEmlrzraxBADDIxDWr8y0WkAWKQiWlQusQiDRCDTCyfGxdFgBhkYu2OxypADDIxDUqXWKQBYpCJ5XPjoggQg0ys3fFYBYhBJqZB6RKLdF0D2ibkuYde7pkTAAAAAElFTkSuQmCC",
    )
}

/** Five invented messages that stand for what mail looks like: plain, a newsletter, a data table, a reply, long links. */
internal object Corpus {
    private val ada = AddressRecord("Ada Example", "ada@example.org")
    private val me = AddressRecord("Bo Example", "bo@example.org")

    class Entry(val name: String, val view: MessageView, val links: List<String>)

    private fun view(id: ULong, subject: String, blocks: List<BlockView>, remote: UInt = 0u, attachments: List<AttachmentRecord> = emptyList()) =
        MessageView(
            1uL, id, 1uL, 1uL, ada, listOf(me), emptyList(), subject, 1_730_000_000L, false, false, emptyList(),
            attachments, blocks, false, remote,
        )

    val plain = Entry(
        "plain",
        view(
            1uL, "Lunch on Thursday?",
            listOf(
                B.p("Hi Bo,", 0u),
                B.p("Are you free for lunch on Thursday? The garden cafe has the long table again, so there is room for the whole team."),
                B.p("Things to settle:"),
                B.item("a time (noon works for me)"),
                B.item("who brings the plans for the shed"),
                B.item("whether Cy joins us"),
                contactLine(),
                B.p("Ada"),
            ),
        ),
        listOf("mailto:ada@example.org", "tel:+442079460000"),
    )

    private fun contactLine() = BlockView(
        BlockKindView.Paragraph,
        listOf(B.span("Write back to "), B.span("ada@example.org", link = "mailto:ada@example.org"), B.span(" or call "), B.span("+44 20 7946 0000", link = "tel:+442079460000"), B.span(".")),
        0u, AlignView.LEFT, 1u, false, 0u, null, 0u, 0u, null, B.noLine,
    )

    private const val SHOP = "https://shop.example/"

    val newsletter = Entry(
        "newsletter",
        view(
            2uL, "Fern Hollow: this week's offers",
            listOf(
                B.tableBlock(
                    B.table(
                        layout = true,
                        width = LengthView.Px(600u),
                        rows = listOf(
                            listOf(B.cell(B.image("https://img.shop.example/banner.png", "Fern Hollow nursery", 560u, 120u), background = 0x2E7D32u, padding = 8u)),
                            listOf(
                                B.cell(B.h(2u, "Spring planting"), B.p("Bare-root roses, now half price."), B.p("Order by Friday", 1u), width = LengthView.Percent(50u)),
                                B.cell(B.h(2u, "New this week"), B.p("Hardy ferns in three sizes."), width = LengthView.Percent(50u)),
                            ),
                            listOf(
                                B.cell(B.p("Free delivery"), width = LengthView.Percent(33u)),
                                B.cell(B.p("30-day returns"), width = LengthView.Percent(33u)),
                                B.cell(B.p("Gift cards"), width = LengthView.Percent(33u)),
                            ),
                            listOf(
                                B.cell(
                                    BlockView(
                                        BlockKindView.Paragraph,
                                        listOf(B.span("You get this because you signed up. "), B.span("Unsubscribe", link = "${SHOP}unsubscribe?u=bo")),
                                        0u, AlignView.CENTER, 1u, false, 0u, null, 0u, 0u, null, B.noLine,
                                    ),
                                ),
                            ),
                        ),
                    ),
                ),
            ),
            remote = 1u,
        ),
        listOf("${SHOP}unsubscribe?u=bo"),
    )

    val dataTable = Entry(
        "data",
        view(
            3uL, "Quarterly numbers",
            listOf(
                B.p("The figures for the garden project are below.", 0u),
                B.tableBlock(
                    B.table(
                        layout = false,
                        border = true,
                        rows = listOf(
                            listOf("Item", "Supplier", "Delivery date", "Unit price", "Quantity", "Total").map { B.cell(B.p(it, 0u), header = true) },
                            listOf("Raised bed boards", "Hollow & Sons Timber", "12 March 2027", "€14.20", "24", "€340.80").map { B.cell(B.p(it, 0u)) },
                            listOf("Compost, 50 litre", "Fern Hollow nursery", "14 March 2027", "€6.50", "40", "€260.00").map { B.cell(B.p(it, 0u)) },
                        ),
                    ),
                ),
                B.p("Let me know if the second row needs another look."),
            ),
            attachments = listOf(AttachmentRecord(0u, "numbers.xlsx", "application/vnd.ms-excel", 48_213uL, false)),
        ),
        emptyList(),
    )

    val reply = Entry(
        "reply",
        view(
            4uL, "Re: Garden plans",
            listOf(
                B.p("Sounds good. I moved the shed to the other corner, drawing attached.", 0u),
                B.block(BlockKindView.Paragraph, B.span("On Tuesday, Cy Example wrote:"), quote = 1u),
                B.block(BlockKindView.Paragraph, B.span("Could we put the shed by the fence? The sun reaches the beds all afternoon."), quote = 1u, space = 0u),
                B.block(BlockKindView.Paragraph, B.span("On Monday, Ada Example wrote:"), quote = 2u),
                B.block(BlockKindView.Paragraph, B.span("Here are the first plans for the garden."), quote = 2u, space = 0u),
            ),
        ),
        emptyList(),
    )

    private const val LONG = "https://docs.example.org/projects/garden/2027/spring/planting-schedule/rev-0042/export?format=pdf&section=beds&lang=en&token=abcdefghijklmnopqrstuvwxyz0123456789"

    val longLinks = Entry(
        "links",
        view(
            5uL, "Links you asked for",
            listOf(
                B.p("The planting schedule is here:", 0u),
                B.block(BlockKindView.Paragraph, B.span(LONG, link = LONG)),
                B.p("And the bank's page, as the sender wrote it:"),
                B.block(BlockKindView.Paragraph, B.span("https://bank.example/login", link = "https://evil.example/collect")),
            ),
        ),
        listOf(LONG, "https://evil.example/collect"),
    )

    val all = listOf(plain, newsletter, dataTable, reply, longLinks)

    /** The message as the reading pane gets it from the core. */
    fun opened(entry: Entry, junk: Boolean = false, vouched: Boolean = true): OpenedMessage = OpenedMessage(
        entry.view,
        MessageFacts(entry.view.from.address, junk = junk, vouched = vouched, known = false),
        entry.links.associateWith { LinkInfo(if (it.startsWith("mailto:")) LinkGoes.Composer else if (it.startsWith("tel:")) LinkGoes.Dialer else LinkGoes.Browser, it) },
        entry.view.attachments.map { item(it.name, index = it.index, size = it.size.toLong()) },
    )
}
