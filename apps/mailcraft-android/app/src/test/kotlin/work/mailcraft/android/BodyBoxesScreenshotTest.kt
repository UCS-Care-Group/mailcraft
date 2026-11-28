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
 * Boxes and the room in them (#1966), as the bridge hands over invented look-alikes (example.org) of
 * four kinds of message: a reply in a bordered white box, an invoice in a framed box round a shaded
 * table, a status card with an accent bar and a coloured pill, a framed button with a filled cell, and
 * a newsletter that spaces its parts with blank paragraphs.
 */
internal object BoxCorpus {
    const val NAVY = 0x1F3864
    const val ACCENT = 0x2DA44E

    private fun frame(width: Int, rgb: Int) = List(4) { LineView(width.toUByte(), LineStyleView.SOLID, rgb.toUInt()) }

    private fun pad(top: Int, right: Int, bottom: Int, left: Int) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    private fun framed(table: TableView, width: Int, rgb: Int) = table.copy(outline = true, lines = frame(width, rgb))

    /** A reply wrapped in a white table with a grey line round it; a blank paragraph after the greeting. */
    private val replyTable = framed(
        B.table(
            layout = true,
            width = LengthView.Percent(100u),
            rows = listOf(listOf(B.cell(B.p("Hi Sam,", 0u), B.p("Thanks for the notes from Tuesday. The new shed plans look right to me; Cy will bring the timber list.", 1u), B.p("Ada", 1u)))),
        ).copy(background = 0xFFFFFFu),
        1, 0xCCCCCC,
    )

    /** An invoice: a framed box with its own padding and margins round a shaded data table. */
    private val invoiceTable = framed(
        B.table(
            layout = true,
            width = LengthView.Percent(100u),
            rows = listOf(
                listOf(
                    B.cell(
                        B.p("Invoice INV-0042", 0u),
                        B.tableBlock(
                            B.table(
                                layout = false,
                                rows = listOf(
                                    listOf(B.cell(B.p("Seed trays (10)", 0u)), B.cell(B.p("£24.00", 0u))),
                                    listOf(B.cell(B.p("Total", 0u)), B.cell(B.p("£24.00", 0u))),
                                ),
                            ).copy(background = 0xF6F6F6u),
                        ),
                    ).copy(padding = pad(16, 16, 16, 16)),
                ),
            ),
        ),
        1, 0x444444,
    )

    /** A status card: a 4 px accent bar beside its text, a coloured pill, a thin frame. */
    val cardTable = framed(
        B.table(
            layout = true,
            width = LengthView.Percent(100u),
            rows = listOf(
                listOf(
                    B.cell(width = LengthView.Px(4u), background = ACCENT.toUInt()),
                    B.cell(
                        B.block(uniffi.mailcraft_mobile.BlockKindView.Paragraph, B.span("Resolved", color = 0x1A7F37u, background = 0xDAFBE1u), space = 0u),
                        B.p("All systems operational. The delayed builds have caught up.", 1u),
                    ).copy(padding = pad(16, 16, 16, 16)),
                ),
            ),
        ).copy(radius = 6u),
        1, 0xD0D7DE,
    )

    /** A button: a table with a frame whose one cell is filled. */
    val buttonTable = framed(
        B.table(
            layout = true,
            rows = listOf(listOf(B.cell(B.p("For further information", 0u).copy(spans = listOf(B.span("For further information", link = "https://example.org/info", color = 0xFFFFFFu))), background = NAVY.toUInt()).copy(padding = pad(8, 16, 8, 16)))),
        ),
        2, 0x8EAADB,
    )

    private fun message(id: ULong, subject: String, blocks: List<BlockView>) = Corpus.Entry(
        "boxes",
        Corpus.newsletter.view.copy(id = id, subject = subject, remotePictures = 0u, blocks = blocks),
        listOf("https://example.org/info"),
    )

    val boxes = message(
        41uL, "Fern Hollow: your order",
        listOf(
            B.tableBlock(replyTable).copy(space = 0u),
            B.tableBlock(invoiceTable).copy(insetLeft = 12u, insetRight = 12u),
            B.tableBlock(cardTable),
            B.tableBlock(buttonTable),
        ),
    )

    /** Parts set apart by blank paragraphs (`<p>&nbsp;</p>`), line breaks and a spacer: blank lines above the next part. */
    val spacers = message(
        42uL, "Fern Hollow: monthly news",
        listOf(
            B.p("Monthly news", 0u),
            B.p("A paragraph's gap above this one.", 1u),
            B.p("Two blank paragraphs above this one.", 3u),
            B.p("Two line breaks above this one.", 3u),
            B.p("A spacer above this one.", 2u),
            B.p("And straight on, no gap.", 0u),
        ),
    )
}

/** Tall enough for every box. */
private val Shot = DeviceConfig.PIXEL_5.copy(screenHeight = 2_600)

class BodyBoxesScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Shot,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    /** In the dark theme the light page is off: the boxes are recoloured dark, and their frames must still show. */
    private val darkPage = ReadingTools(null, { _, _ -> }, {}, ReadingPrefs(lightMessages = false), {})

    private fun shoot(entry: Corpus.Entry, name: String, night: Boolean) {
        paparazzi.unsafeUpdateConfig(Shot.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
        paparazzi.snapshot(name) { ReadingShots.Pane(ReadingShots.ui(entry), if (night) darkPage else ReadingTools.None) }
    }

    @Test
    fun boxesLight() = shoot(BoxCorpus.boxes, "boxes_light", night = false)

    @Test
    fun boxesDark() = shoot(BoxCorpus.boxes, "boxes_dark", night = true)

    @Test
    fun spacersLight() = shoot(BoxCorpus.spacers, "spacers_light", night = false)

    @Test
    fun spacersDark() = shoot(BoxCorpus.spacers, "spacers_dark", night = true)
}
