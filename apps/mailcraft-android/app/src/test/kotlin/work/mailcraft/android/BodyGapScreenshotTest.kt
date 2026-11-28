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
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.TableView
import uniffi.mailcraft_mobile.VAlignView

/**
 * A newsletter footer whose pictures were not downloaded (#1966): a "follow us" row of four small
 * icons in a tinted cell, as the bridge hands over a typical sender's template. The rows below follow
 * straight on: no empty space where the pictures would be. A tall poster above is a box of limited height.
 */
internal object GapCorpus {
    private const val NAVY = 0x263D51u

    private fun inset(top: Int, right: Int, bottom: Int, left: Int) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    private fun icon(name: String): uniffi.mailcraft_mobile.BlockView {
        val cell = B.cell(
            B.image("https://cdn-images.example/icons/${name.lowercase()}.png", "$name icon", 24u, 24u, link = "https://social.example/${name.lowercase()}")
                .copy(align = AlignView.CENTER),
        ).copy(padding = inset(0, 10, 9, 0), valign = VAlignView.TOP)
        return B.tableBlock(B.table(layout = true, rows = listOf(listOf(cell))).copy(inline = true, float = true))
    }

    private fun tinted(tint: UInt?, vararg blocks: uniffi.mailcraft_mobile.BlockView, pad: InsetsView? = null): CellView =
        B.cell(*blocks, background = tint).copy(padding = pad, valign = VAlignView.TOP)

    /** The template's content table; [tint] is the footer cells' colour, or none. */
    fun table(tint: UInt? = NAVY): TableView = B.table(
        layout = true,
        width = LengthView.Px(600u),
        rows = listOf(
            listOf(B.cell(B.image("https://cdn-images.example/poster.png", "Autumn plant fair poster", 600u, 1200u))),
            listOf(B.cell(B.p("Dear reader, our autumn plant fair opens on Saturday at Fern Hollow. Come early for the best ferns.", 0u))),
            listOf(
                tinted(
                    tint,
                    B.h(3u, "Contact us"),
                    B.p("01632 960000\nhello@fernhollow.example"),
                    B.tableBlock(
                        B.table(layout = true, rows = listOf(listOf(B.cell(icon("Facebook"), icon("Instagram"), icon("LinkedIn"), icon("X"))))),
                        AlignView.CENTER,
                    ),
                ),
            ),
            listOf(tinted(tint, B.p("Head Office\n1 Example Lane, Exampleton EX1 2PL", 0u), B.p("Fern Hollow Nurseries Ltd is registered in England."), pad = inset(9, 18, 9, 18))),
            listOf(B.cell(B.p("This email was sent to bo@example.org", 0u), padding = 9u)),
        ),
    ).copy(background = 0xFFFFFFu)

    val footer = Corpus.Entry(
        "gap",
        Corpus.newsletter.view.copy(
            id = 40uL,
            subject = "Fern Hollow: autumn plant fair",
            remotePictures = 4u,
            blocks = listOf(B.tableBlock(table(), AlignView.CENTER)),
        ),
        emptyList(),
    )
}

/** Tall enough for the whole message, and for a gap after it to show. */
private val Shot = DeviceConfig.PIXEL_5.copy(screenHeight = 3_400)

class BodyGapScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Shot,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, night: Boolean) {
        paparazzi.unsafeUpdateConfig(Shot.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
        paparazzi.snapshot(name) { ReadingShots.Pane(ReadingShots.ui(GapCorpus.footer)) }
    }

    @Test
    fun undownloadedIconsLeaveNoGapLight() = shoot("pictures_follow_row_light", night = false)

    @Test
    fun undownloadedIconsLeaveNoGapDark() = shoot("pictures_follow_row_dark", night = true)
}
