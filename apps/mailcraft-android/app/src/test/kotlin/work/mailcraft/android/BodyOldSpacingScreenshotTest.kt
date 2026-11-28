// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import com.android.ide.common.rendering.api.SessionParams
import com.android.resources.NightMode
import org.junit.Rule
import org.junit.Test
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.LineStyleView
import uniffi.mailcraft_mobile.LineView
import uniffi.mailcraft_mobile.TableView

/**
 * The room a message asks for in three old ways (#1966), as the bridge hands them over
 * (`crates/mobile/src/spacing_tests.rs` reads the same HTML): an invoice table with `cellpadding=10`, an
 * Outlook message whose cells and paragraphs give only `mso-padding-alt` and `mso-margin-top-alt`, and a
 * card with a 4 px coloured bar (`<div style="height:4px;background:…">`) above it. Invented, example.org.
 */
internal object OldSpacingCorpus {
    const val BAR = 0xC00030
    const val NOTE = 0xDEEAF6

    private fun pad(top: Int, right: Int, bottom: Int, left: Int) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    private fun padded(cell: uniffi.mailcraft_mobile.CellView, insets: InsetsView) = cell.copy(padding = insets)

    /** `<table border=1 cellspacing=0 cellpadding=10>`: every cell 10 px in. */
    private val invoiceTable: TableView = run {
        val ten = pad(10, 10, 10, 10)
        fun row(vararg text: String, header: Boolean = false) = text.map { padded(B.cell(B.p(it, 0u), header = header), ten) }
        B.table(
            layout = false,
            border = true,
            rows = listOf(row("Item", "Qty", "Price", header = true), row("Seed trays", "10", "£24.00"), row("Twine, 50 m", "2", "£6.40"), row("Total", "12", "£30.40")),
        ).copy(spacing = 0u)
    }

    /** Outlook's notice box: `mso-padding-alt:6pt 12pt` on a shaded cell, and a paragraph with `mso-margin-top-alt:24px`. */
    private val noticeTable = B.table(
        layout = true,
        width = LengthView.Percent(100u),
        rows = listOf(listOf(padded(B.cell(B.p("The hall is booked from 9 until 4. Bring your own cups.", 0u), background = NOTE.toUInt()), pad(8, 16, 8, 16)))),
    )

    /** The 4 px bar: an empty cell of colour, its height as room above. */
    val barTable = B.table(
        layout = true,
        width = LengthView.Percent(100u),
        rows = listOf(listOf(padded(B.cell(background = BAR.toUInt()), pad(4, 0, 0, 0)))),
    )

    private val cardTable = B.table(
        layout = true,
        width = LengthView.Percent(100u),
        rows = listOf(listOf(padded(B.cell(B.p("Autumn fair", 0u), B.p("Saturday 14 November at the old mill. Stalls from ten.", 1u)), pad(16, 16, 16, 16)))),
    ).copy(outline = true, lines = List(4) { LineView(1u, LineStyleView.SOLID, 0xD0D7DEu) })

    private fun message(id: ULong, subject: String, blocks: List<BlockView>) = Corpus.Entry(
        "old spacing",
        Corpus.newsletter.view.copy(id = id, subject = subject, remotePictures = 0u, blocks = blocks),
        emptyList(),
    )

    val invoice = message(
        51uL, "Fern Hollow: invoice INV-0107",
        listOf(B.p("Thank you for your order. Your invoice:", 0u), B.tableBlock(invoiceTable), B.p("Payment is due by 1 December.")),
    )

    val outlook = message(
        52uL, "Village hall on Saturday",
        listOf(B.p("Hi all,", 0u), B.tableBlock(noticeTable).copy(space = 0u, gap = 12u), B.p("See you there.", 0u).copy(gap = 24u), B.p("Ada", 0u)),
    )

    val card = message(
        53uL, "Fern Hollow: the autumn fair",
        listOf(B.tableBlock(barTable).copy(space = 0u), B.tableBlock(cardTable).copy(space = 0u)),
    )
}

class BodyOldSpacingScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    /** In the dark theme the light page is off: the boxes are recoloured dark. */
    private val darkPage = ReadingTools(null, { _, _ -> }, {}, ReadingPrefs(lightMessages = false), {})

    private fun shoot(entry: Corpus.Entry, name: String, night: Boolean) {
        paparazzi.unsafeUpdateConfig(DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
        paparazzi.snapshot(name) { ReadingShots.Pane(ReadingShots.ui(entry), if (night) darkPage else ReadingTools.None) }
    }

    @Test
    fun cellpaddingLight() = shoot(OldSpacingCorpus.invoice, "cellpadding_light", night = false)

    @Test
    fun cellpaddingDark() = shoot(OldSpacingCorpus.invoice, "cellpadding_dark", night = true)

    @Test
    fun msoPaddingLight() = shoot(OldSpacingCorpus.outlook, "mso_padding_light", night = false)

    @Test
    fun msoPaddingDark() = shoot(OldSpacingCorpus.outlook, "mso_padding_dark", night = true)

    @Test
    fun barLight() = shoot(OldSpacingCorpus.card, "bar_light", night = false)

    @Test
    fun barDark() = shoot(OldSpacingCorpus.card, "bar_dark", night = true)
}
