// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What a screen reader is told of the corpus messages: the semantics, apart from how they are drawn. */
class BodySemanticsTest {
    private fun say(items: List<Announced>): List<String> = items.map {
        when (it) {
            is Announced.Heading -> "heading${it.level}:${it.text}"
            is Announced.Text -> "text:${it.text}"
            is Announced.Picture -> "picture:${it.description}"
            is Announced.Table -> "table:${it.rows}x${it.columns}"
            is Announced.Cell -> "cell:${it.row},${it.column}${if (it.header) ",header" else ""}"
            Announced.Separator -> "separator"
        }
    }

    @Test
    fun aLayoutTableIsReadAsPlainBlocksInOrderWithNoWordOfATable() {
        val said = say(announced(Corpus.newsletter.view.blocks))
        assertTrue(said.none { it.startsWith("table") || it.startsWith("cell") })
        assertEquals(
            listOf(
                "picture:Fern Hollow nursery",
                "heading2:Spring planting", "text:Bare-root roses, now half price.", "text:Order by Friday",
                "heading2:New this week", "text:Hardy ferns in three sizes.",
                "text:Free delivery", "text:30-day returns", "text:Gift cards",
                "text:You get this because you signed up. Unsubscribe",
            ),
            said,
        )
    }

    @Test
    fun aDataTableIsReadAsATableWithItsCells() {
        val said = say(announced(Corpus.dataTable.view.blocks))
        assertEquals("text:The figures for the garden project are below.", said.first())
        assertTrue("table:3x6" in said)
        assertEquals(6, said.count { it.endsWith(",header") })
        assertEquals(18, said.count { it.startsWith("cell:") })
        assertEquals("cell:1,1,header", said[said.indexOf("table:3x6") + 1])
        assertEquals("text:Item", said[said.indexOf("table:3x6") + 2])
    }

    @Test
    fun picturesWithNoDescriptionOrTwoPixelsOrSmallerAreLeftOut() {
        val blocks = listOf(
            B.image("https://a.example/1.png", "A described picture", 100u, 50u),
            B.image("https://a.example/2.png", "", 100u, 50u),
            B.image("https://a.example/3.png", "Spacer", 1u, 1u),
            B.image("https://a.example/4.png", "Thin", 200u, 2u),
            B.image("cid:logo", "   ", picture = "cid:logo"),
        )
        assertEquals(listOf("picture:A described picture"), say(announced(blocks)))
        assertTrue(isDecorative("") && isDecorative("  ") && !isDecorative("x"))
    }

    @Test
    fun headingsAndRulesAreSaidAsWhatTheyAre() {
        val said = say(announced(listOf(B.h(1u, "Title"), B.p("Body"), B.block(uniffi.mailcraft_mobile.BlockKindView.Rule))))
        assertEquals(listOf("heading1:Title", "text:Body", "separator"), said)
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
