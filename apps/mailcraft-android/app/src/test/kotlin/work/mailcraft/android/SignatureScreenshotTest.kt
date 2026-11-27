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
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.CellView
import uniffi.mailcraft_mobile.InsetsView
import uniffi.mailcraft_mobile.LengthView
import uniffi.mailcraft_mobile.LineStyleView
import uniffi.mailcraft_mobile.LineView
import uniffi.mailcraft_mobile.MarkerView
import uniffi.mailcraft_mobile.SidesView
import uniffi.mailcraft_mobile.TableView
import uniffi.mailcraft_mobile.VAlignView

/**
 * Signatures and notices as the bridge hands them over for invented messages (example.org), shaped
 * after what `crates/mobile`'s tests read from typical HTML (#1966): a logo, a thin bar of colour and
 * the name and numbers side by side with a row of four floated icons; a reply signed the way a word
 * processor writes it, with pictures that say only their height; a voicemail notice with its bar.
 */
internal object SignatureCorpus {
    private const val GREEN = 0x2E7D32u
    private const val BLUE = 0x0B6AA2u
    private const val LOGO = "cid:logo@example.org"
    private val icons = listOf("a", "b", "c", "d")

    private fun inset(top: Int, right: Int, bottom: Int, left: Int) = InsetsView(top.toUByte(), right.toUByte(), bottom.toUByte(), left.toUByte())

    private fun row(vararg cells: CellView): TableView = B.table(layout = true, rows = listOf(cells.toList())).copy(spacing = 0u)

    /** A cell that holds nothing but a colour: the band the bridge gives three dp above and below. */
    private fun bar(width: Int, tint: UInt) = B.cell(width = LengthView.Px(width.toUShort()), background = tint).copy(padding = inset(3, 0, 3, 0))

    private fun bareLink(text: String, to: String) = B.span(text, link = to).copy(bare = true)

    /** Four social icons, each in a table floated left, touching. */
    fun icons(): List<BlockView> = icons.mapIndexed { n, name ->
        val src = "https://img.example.org/$name.png"
        val cell = B.cell(B.image(src, name.uppercase(), 20u, 20u, link = "https://social.example/$name")).copy(padding = inset(0, 6, 0, 0))
        B.tableBlock(B.table(layout = true, rows = listOf(listOf(cell))).copy(spacing = 0u, inline = true, float = true)).copy(beside = n > 0, space = 0u)
    }

    fun iconPictures(): Map<String, PictureState> = icons.associate { "https://img.example.org/$it.png" to PictureState.Ready(B.png) }

    private fun nameLines(vararg extra: BlockView) = listOf(
        B.block(BlockKindView.Paragraph, B.span("Ada Example", bold = true), B.span("\nHead gardener"), space = 0u),
        BlockView(BlockKindView.Paragraph, listOf(B.span("T: "), bareLink("01632 960000", "tel:+441632960000")), 0u, uniffi.mailcraft_mobile.AlignView.LEFT, 1u, false, 0u, null, 0u, 0u, null, B.noLine),
    ) + extra

    private fun view(id: ULong, subject: String, blocks: List<BlockView>) =
        Corpus.plain.view.copy(id = id, subject = subject, remotePictures = 0u, blocks = blocks)

    private val links = listOf("tel:+441632960000", "tel:+447700900123", "https://fernhollow.example/", "https://voice.example/play/1") +
        icons.map { "https://social.example/$it" } + listOf("https://social.example/fh", "https://video.example/fh")

    /** The four icons floated right, under a line of text: they stand at the right edge, the first outermost. */
    val floatedRight = Corpus.Entry(
        "floated_right",
        view(53uL, "Follow us", listOf(B.p("Find us here:", 0u)) + icons().map { it.copy(align = uniffi.mailcraft_mobile.AlignView.RIGHT) }),
        links,
    )

    /** Logo, bar, text: the logo's cell says no width, its picture does. */
    val logoBar = Corpus.Entry(
        "signature",
        view(
            50uL, "Re: Shed plans",
            listOf(
                B.p("Thanks, see you Thursday.", 0u),
                B.tableBlock(
                    row(
                        B.cell(B.image(LOGO, "Fern Hollow", 88u, 88u, picture = LOGO)).copy(valign = VAlignView.TOP),
                        bar(3, GREEN),
                        B.cell(*nameLines(*icons().toTypedArray()).toTypedArray()).copy(padding = inset(0, 0, 0, 12), valign = VAlignView.TOP),
                    ),
                ),
            ),
        ),
        links,
    )

    private val wordLine = LineView(2u, LineStyleView.SOLID, GREEN)

    /** A reply signed as a word processor writes it: a line down the logo's cell, icons that say only their height. */
    val wordSigned = Corpus.Entry(
        "word_signed",
        view(
            51uL, "Shed plans",
            listOf(
                B.p("Hi Bo,", 0u),
                B.p("The plans are attached.", 0u),
                B.tableBlock(
                    row(
                        B.cell(B.image("cid:image001.png@01DA0000.00000000", "Fern Hollow", 96u, 96u, picture = "cid:image001.png@01DA0000.00000000"), width = LengthView.Px(110u))
                            .copy(padding = inset(0, 10, 0, 0), valign = VAlignView.TOP, border = SidesView(top = false, right = true, bottom = false, left = false), lines = listOf(B.noLine, wordLine, B.noLine, B.noLine)),
                        B.cell(
                            B.block(BlockKindView.Paragraph, B.span("Cy Example", bold = true, color = GREEN, size = 16u), space = 0u),
                            B.block(BlockKindView.Paragraph, B.span("Nursery manager | Fern Hollow", color = 0x808080u, size = 12u), space = 0u),
                            BlockView(BlockKindView.Paragraph, listOf(B.span("M: ", size = 12u), bareLink("07700 900123", "tel:+447700900123").copy(size = 12u, color = GREEN)), 0u, uniffi.mailcraft_mobile.AlignView.LEFT, 0u, false, 0u, null, 0u, 0u, null, B.noLine),
                            B.block(BlockKindView.Paragraph, bareLink("fernhollow.example", "https://fernhollow.example/").copy(size = 12u, color = GREEN), space = 0u),
                            B.image("cid:image002.png@01DA0000.00000000", "Social", null, 18u, picture = "cid:image002.png@01DA0000.00000000", link = "https://social.example/fh"),
                            B.image("cid:image003.png@01DA0000.00000000", "Video", null, 18u, picture = "cid:image003.png@01DA0000.00000000", link = "https://video.example/fh").copy(beside = true),
                        ).copy(padding = inset(0, 0, 0, 10), valign = VAlignView.TOP),
                    ),
                ),
                B.block(BlockKindView.Rule, space = 1u).copy(gap = 4u),
                B.block(BlockKindView.Paragraph, B.span("From: ", bold = true), B.span("Bo Example <bo@example.org>\n"), B.span("Sent: ", bold = true), B.span("Tuesday\n"), B.span("Subject: ", bold = true), B.span("Shed plans"), space = 0u),
            ),
        ),
        links,
    )

    val wordPictures: Map<String, PictureState> = listOf(
        "cid:image001.png@01DA0000.00000000", "cid:image002.png@01DA0000.00000000", "cid:image003.png@01DA0000.00000000",
    ).associateWith { PictureState.Ready(B.png) }

    /** A voicemail notice: a tinted box with a bar beside its heading, a list, a button. */
    val voicemail = Corpus.Entry(
        "voicemail",
        view(
            52uL, "New voicemail from Dee Example",
            listOf(
                B.tableBlock(
                    B.table(
                        layout = true, width = LengthView.Percent(100u),
                        rows = listOf(
                            listOf(
                                B.cell(
                                    B.tableBlock(row(bar(4, BLUE), B.cell(B.block(BlockKindView.Paragraph, B.span("You have a new voicemail", bold = true, size = 16u), space = 0u)).copy(padding = inset(0, 0, 0, 12)))),
                                    background = 0xF3F6F9u,
                                ).copy(padding = inset(12, 16, 12, 16)),
                            ),
                        ),
                    ).copy(spacing = 0u),
                ),
                B.p("From: +44 1632 960111 (Dee Example)"),
                B.block(BlockKindView.ListItem(MarkerView.Bullet, 0u), B.span("Received: Tuesday 10:42")),
                B.item("Length: 0:37"),
                B.item("Mailbox: Shop floor"),
                B.p("Listen to the attached recording, or call 1632 to hear it."),
                B.tableBlock(
                    row(
                        B.cell(
                            BlockView(BlockKindView.Paragraph, listOf(bareLink("Play message", "https://voice.example/play/1").copy(color = 0xFFFFFFu)), 0u, uniffi.mailcraft_mobile.AlignView.LEFT, 0u, false, 0u, null, 0u, 0u, null, B.noLine),
                            background = BLUE,
                        ).copy(padding = inset(8, 14, 8, 14)),
                    ),
                ),
            ),
        ),
        links,
    )
}

/** Tall enough for each message, short enough to read the shot. */
private val Shot = DeviceConfig.PIXEL_5.copy(screenHeight = 1_400)

class SignatureScreenshotTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = Shot,
        renderingMode = SessionParams.RenderingMode.NORMAL,
        maxPercentDifference = TOLERANCE,
    )

    private fun shoot(name: String, entry: Corpus.Entry, pictures: Map<String, PictureState> = emptyMap()) {
        for (night in listOf(false, true)) {
            paparazzi.unsafeUpdateConfig(Shot.copy(nightMode = if (night) NightMode.NIGHT else NightMode.NOTNIGHT))
            val access = if (pictures.isEmpty()) PictureAccess.Blocked else PictureAccess.Allowed
            paparazzi.snapshot("${name}_${if (night) "dark" else "light"}") {
                ReadingShots.Pane(ReadingShots.ui(entry, access = access, pictures = pictures))
            }
        }
    }

    @Test
    fun logoBarAndIconsNotDownloaded() = shoot("signature_waiting", SignatureCorpus.logoBar)

    @Test
    fun logoBarAndIconsDownloaded() =
        shoot("signature_ready", SignatureCorpus.logoBar, SignatureCorpus.iconPictures() + ("cid:logo@example.org" to PictureState.Ready(B.png)))

    @Test
    fun wordSignedReplyWithHeightOnlyPictures() = shoot("signature_word", SignatureCorpus.wordSigned, SignatureCorpus.wordPictures)

    @Test
    fun wordSignedReplyNotDownloaded() = shoot("signature_word_waiting", SignatureCorpus.wordSigned)

    @Test
    fun iconsFloatedRight() = shoot("signature_floated_right", SignatureCorpus.floatedRight)

    @Test
    fun voicemailNotice() = shoot("signature_voicemail", SignatureCorpus.voicemail)
}
