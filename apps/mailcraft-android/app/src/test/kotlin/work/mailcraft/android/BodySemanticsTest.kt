// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import app.cash.paparazzi.DeviceConfig
import app.cash.paparazzi.Paparazzi
import app.cash.paparazzi.SnapshotHandler
import java.lang.reflect.Proxy
import com.android.ide.common.rendering.api.SessionParams
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

/**
 * A Paparazzi handler that keeps no picture: this test reads the semantics and has nothing to compare.
 * (A proxy, since the picture type it is handed is not on the compile path of an Android test.)
 */
private fun noPictures(): SnapshotHandler {
    val loader = SnapshotHandler::class.java.classLoader
    val frames = Proxy.newProxyInstance(loader, arrayOf(SnapshotHandler.FrameHandler::class.java)) { _, _, _ -> null }
    return Proxy.newProxyInstance(loader, arrayOf(SnapshotHandler::class.java)) { _, method, _ ->
        if (method.name == "newFrameHandler") frames else null
    } as SnapshotHandler
}

/** What a screen reader is told of the corpus messages, read from the semantics tree that was really drawn. */
class BodySemanticsTest {
    @get:Rule
    val paparazzi = Paparazzi(
        deviceConfig = DeviceConfig.PIXEL_5.copy(screenHeight = 3_400),
        renderingMode = SessionParams.RenderingMode.NORMAL,
        snapshotHandler = noPictures(),
    )

    private fun draw(ui: ReadingUi): List<Said> {
        val probe = SemanticsProbe()
        paparazzi.snapshot { probe.Watch { ReadingShots.Pane(ui) } }
        return probe.said
    }

    private fun draw(entry: Corpus.Entry) = draw(ReadingShots.ui(entry))

    @Test
    fun headingsAreHeadingsAndTheSubjectIsOne() {
        val nodes = draw(Corpus.newsletter)
        val headings = nodes.filter { it.heading }.map { it.text }
        assertTrue(headings.toString(), "Spring planting" in headings && "New this week" in headings)
        assertTrue(headings.toString(), Corpus.newsletter.view.subject in headings)
    }

    @Test
    fun aLayoutTableIsReadAsPlainBlocksWithNoWordOfATable() {
        val nodes = draw(Corpus.newsletter)
        assertTrue(nodes.isNotEmpty())
        // (The message's own list is a one-column collection; a table has more columns.)
        assertTrue(nodes.none { (it.tableColumns ?: 0) > 1 || (it.cellColumn ?: 0) > 0 })
        val said = nodes.map { it.text }.filter { it.isNotEmpty() }
        val order = listOf("Spring planting", "Bare-root roses, now half price.", "Order by Friday", "New this week", "Hardy ferns in three sizes.", "Free delivery", "30-day returns", "Gift cards")
        assertTrue(said.toString(), order.all { it in said })
        assertEquals(order, order.sortedBy { said.indexOf(it) })
    }

    @Test
    fun aDataTableIsReadAsATableWithItsCellsAndHeaders() {
        val nodes = draw(Corpus.dataTable)
        // (The message's own list is a one-column collection, its items sit in column 0: a table has more columns.)
        val table = nodes.filter { (it.tableColumns ?: 0) > 1 }
        assertEquals(1, table.size)
        assertEquals(3, table[0].tableRows)
        assertEquals(6, table[0].tableColumns)
        val cells = nodes.filter { (it.cellColumn ?: 0) > 0 }
        assertEquals(15, cells.size)
        assertEquals(5, cells.count { it.heading })
        assertTrue(cells.filter { it.heading }.all { it.cellRow == 0 })
        assertTrue(nodes.any { it.heading && it.cellRow == 0 && it.cellColumn == 0 })
    }

    @Test
    fun aPictureIsSaidByItsDescriptionAndATinyOrUndescribedOneNotAtAll() {
        val ui = ReadingUi(
            id = 9uL,
            load = LoadState.Ready,
            opened = Corpus.opened(
                Corpus.Entry(
                    "pictures",
                    Corpus.plain.view.copy(
                        blocks = listOf(
                            B.image("https://a.example/1.png", "A described picture", 100u, 50u),
                            B.image("https://a.example/2.png", "", 100u, 50u),
                            B.image("https://a.example/3.png", "Spacer", 1u, 1u),
                            B.image("https://a.example/4.png", "Thin", 200u, 2u),
                            B.image("cid:logo", "   ", picture = "cid:logo"),
                        ),
                    ),
                    emptyList(),
                ),
            ),
        )
        val said = draw(ui).mapNotNull { it.description }
        assertTrue(said.toString(), "A described picture" in said)
        assertTrue(said.toString(), said.none { it.contains("Spacer") || it.contains("Thin") })
        assertTrue(isDecorative("") && isDecorative("  ") && !isDecorative("x"))
    }

    @Test
    fun aLinkOffersAnActionThatNamesItsRealDestination() {
        val labels = draw(Corpus.longLinks).flatMap { it.actions }
        assertTrue(labels.toString(), "Open link: https://evil.example/collect" in labels)
        assertTrue(labels.toString(), labels.any { it.endsWith("export?format=pdf&section=beds&lang=en&token=abcdefghijklmnopqrstuvwxyz0123456789") })
    }

    @Test
    fun aLinkIsAnnouncedWithItsRealDestinationNotItsText() {
        val shown = Corpus.longLinks.view.blocks[3]
        val opened = Corpus.opened(Corpus.longLinks)
        val text = styled(shown.spans, Paper.of(0xFFFFFF, 0x1B5E8C), opened.links)
        assertEquals(1, text.links.size)
        val labels = linkActionLabels(EnglishTexts, text.links)
        assertEquals(listOf("Open link: https://evil.example/collect"), labels)
        // The words say the bank; the label says where it goes.
        assertEquals("https://bank.example/login", text.text.text)
    }

    @Test
    fun aLinkTheCoreDidNotVouchForIsPlainText() {
        val span = B.span("click", link = "javascript:alert(1)")
        val text = styled(listOf(span), Paper.of(0xFFFFFF, 0x1B5E8C), emptyMap())
        assertTrue(text.links.isEmpty())
        assertNull(text.linkAt(0))
    }

    @Test
    fun linkOffsetsFindTheLinkAndOnlyTheLink() {
        val opened = Corpus.opened(Corpus.plain)
        val line = Corpus.plain.view.blocks[6]
        val text = styled(line.spans, Paper.of(0xFFFFFF, 0x1B5E8C), opened.links)
        val at = text.text.text.indexOf("ada@")
        assertEquals("mailto:ada@example.org", text.linkAt(at)?.shown)
        assertNull(text.linkAt(0))
        assertEquals("tel:+442079460000", text.linkAt(text.text.text.indexOf("+44"))?.shown)
        assertEquals(LinkGoes.Dialer, text.linkAt(text.text.text.indexOf("+44"))?.goes)
    }

    @Test
    fun aMessageOfManyLinksOffersAHandfulOfActions() {
        val many = (1..40).map { LinkRange(it, it + 1, LinkInfo(LinkGoes.Browser, "https://x$it.example/")) }
        assertEquals(MAX_LINK_ACTIONS, linkActionLabels(EnglishTexts, many).size)
    }
}
