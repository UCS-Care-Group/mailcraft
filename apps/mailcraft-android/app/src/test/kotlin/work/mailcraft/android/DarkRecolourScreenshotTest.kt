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
import uniffi.mailcraft_mobile.AlignView
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.LineStyleView
import uniffi.mailcraft_mobile.LineView
import uniffi.mailcraft_mobile.SpanView
import uniffi.mailcraft_mobile.TableView

/** Invented messages with colours of their own, for the dark recolouring (#1966). */
internal object Coloured {
    private fun para(vararg spans: SpanView, align: AlignView = AlignView.LEFT, space: UByte = 1u) =
        BlockView(BlockKindView.Paragraph, spans.toList(), 0u, align, space, false, 0u, null, 0u, 0u, null, B.noLine)

    private fun lines(rgb: UInt) = List(4) { LineView(1u, LineStyleView.SOLID, rgb) }

    private fun boxed(table: TableView, background: UInt, line: UInt? = null) =
        table.copy(background = background, outline = line != null, lines = line?.let(::lines) ?: emptyList())

    /** A short reply its sender wrapped in a white box with a thin line, the original quoted below as Outlook writes it. */
    val boxedReply = Corpus.Entry(
        "boxed",
        Corpus.view(
            30uL, "RE: Shed delivery",
            listOf(
                B.tableBlock(
                    boxed(
                        B.table(layout = true, rows = listOf(listOf(B.cell(para(B.span("Thursday morning suits us. The gate code is 4512.", color = 0x000000u), space = 0u))))),
                        background = 0xFFFFFFu, line = 0x000000u,
                    ),
                ),
                para(B.span("-----Original Message-----", color = 0x000000u)),
                para(B.span("From: ", bold = true, color = 0x000000u), B.span("Cy Example <cy@example.org>", color = 0x000000u), space = 0u),
                para(B.span("Subject: ", bold = true, color = 0x000000u), B.span("Shed delivery", color = 0x000000u), space = 0u),
                para(B.span("When would you like the shed delivered? We have Thursday or Friday.", color = 0x1F497Du)),
            ),
        ),
        emptyList(),
    )

    private const val SHOP = "https://shop.example/"

    /** A newsletter: a grey surround, a white column, a highlight, a navy footer with an orange heading and a filled button. */
    val brand = Corpus.Entry(
        "brand",
        Corpus.view(
            31uL, "Fern Hollow: autumn bulbs",
            listOf(
                B.tableBlock(
                    boxed(
                        B.table(
                            layout = true,
                            rows = listOf(
                                listOf(
                                    B.cell(
                                        B.tableBlock(
                                            boxed(
                                                B.table(
                                                    layout = true,
                                                    width = LengthView.Px(560u),
                                                    rows = listOf(
                                                        listOf(
                                                            B.cell(
                                                                B.block(BlockKindView.Heading(2u), B.span("Autumn bulbs are in", bold = true, color = 0x263D51u)),
                                                                para(B.span("Tulips, crocus and daffodils, ready to plant before the first frost.", color = 0x333333u)),
                                                                para(B.span("Order by Sunday", color = 0x000000u, background = 0xFFF59Du), B.span(" for free delivery.", color = 0x333333u)),
                                                                para(B.span("Sold out: ", bold = true, color = 0xFFFFFFu, background = 0xD32F2Fu), B.span(" giant alliums.", color = 0x333333u)),
                                                                padding = 16u,
                                                            ),
                                                        ),
                                                        listOf(
                                                            B.cell(
                                                                B.block(BlockKindView.Heading(3u), B.span("Visit the nursery", bold = true, color = 0xF7A136u), align = AlignView.CENTER),
                                                                para(B.span("Open every day from nine until five.", color = 0xFFFFFFu), align = AlignView.CENTER),
                                                                B.tableBlock(
                                                                    boxed(
                                                                        B.table(
                                                                            layout = true,
                                                                            rows = listOf(listOf(B.cell(para(B.span("Find us", bold = true, link = "${SHOP}visit", color = 0xF7A136u), align = AlignView.CENTER, space = 0u), background = 0x1B2B3Au, padding = 10u))),
                                                                        ),
                                                                        background = 0x1B2B3Au, line = 0xF7A136u,
                                                                    ),
                                                                    align = AlignView.CENTER,
                                                                ),
                                                                background = 0x263D51u,
                                                                padding = 16u,
                                                            ),
                                                        ),
                                                    ),
                                                ),
                                                background = 0xFFFFFFu,
                                            ),
                                            align = AlignView.CENTER,
                                        ),
                                        padding = 12u,
                                    ),
                                ),
                            ),
                        ),
                        background = 0xEEEEEEu,
                    ),
                ),
            ),
        ),
        listOf("${SHOP}visit"),
    )

    /** A message whose sender made it dark already: its colours are left as they are. */
    val darkBody = Corpus.Entry(
        "dark_body",
        Corpus.view(
            32uL, "Night sky walk",
            listOf(
                B.tableBlock(
                    boxed(
                        B.table(
                            layout = true,
                            rows = listOf(
                                listOf(
                                    B.cell(
                                        B.block(BlockKindView.Heading(2u), B.span("Night sky walk", bold = true, color = 0xFFD54Fu)),
                                        para(B.span("Meet at the garden gate at eight. Bring a torch with a red filter.", color = 0xE0E0E0u)),
                                        para(B.span("Rain moves it to Saturday.", color = 0x90CAF9u)),
                                        padding = 16u,
                                    ),
                                ),
                            ),
                        ),
                        background = 0x121A2Au,
                    ),
                ),
            ),
        ),
        emptyList(),
    )
}

/** A message's own colours with the light page off: as written in the light theme, recoloured dark in the dark one. */
class DarkRecolourScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private val darkPage = ReadingTools(null, { _, _ -> }, {}, ReadingPrefs(lightMessages = false), {})

    private fun shoot(entry: Corpus.Entry) {
        for ((suffix, night) in listOf("light" to false, "dark" to true)) {
            paparazzi.unsafeUpdateConfig(DeviceConfig.PIXEL_5.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
            paparazzi.snapshot("recolour_${entry.name}_$suffix") { ReadingShots.Pane(ReadingShots.ui(entry), darkPage) }
        }
    }

    @Test
    fun boxedReply() = shoot(Coloured.boxedReply)

    @Test
    fun brandNewsletter() = shoot(Coloured.brand)

    @Test
    fun darkBody() = shoot(Coloured.darkBody)

    /** A message with its own design for dark windows keeps its colours. */
    @Test
    fun ownDarkDesign() {
        paparazzi.unsafeUpdateConfig(DeviceConfig.PIXEL_5.copy(nightMode = NightMode.NIGHT))
        val entry = Corpus.Entry("own", Coloured.boxedReply.view.copy(darkDesign = true), emptyList())
        paparazzi.snapshot("recolour_own_dark_design_dark") { ReadingShots.Pane(ReadingShots.ui(entry), darkPage) }
    }
}
