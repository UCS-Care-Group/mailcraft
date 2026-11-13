// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PeopleTest {
    private fun row(id: ULong, name: String) = PersonRow(id, name, "")

    @Test
    fun aListKnowsWhenItIsComplete() {
        assertTrue(PeopleList(listOf(row(1u, "Ada")), 1).complete)
        assertFalse(PeopleList(listOf(row(1u, "Ada")), 2).complete)
    }

    @Test
    fun theNextWindowFollowsAndNobodyIsListedTwice() {
        val first = PeopleList(listOf(row(1u, "Ada"), row(2u, "Bo")), 4)
        val next = PeopleList(listOf(row(2u, "Bo"), row(3u, "Cy")), 4)
        val joined = first.append(next)
        assertEquals(listOf<ULong>(1u, 2u, 3u), joined.rows.map { it.id })
        assertEquals(4, joined.total)
    }

    @Test
    fun anEmptyWindowChangesNothingButTheTotal() {
        val first = PeopleList(listOf(row(1u, "Ada")), 5)
        assertEquals(first.rows, first.append(PeopleList(emptyList(), 3)).rows)
    }

    @Test
    fun peopleAreFiledUnderTheLetterOfTheirName() {
        assertEquals("A", sectionOf("ada Example"))
        assertEquals("É", sectionOf("  élan"))
        assertEquals("#", sectionOf("42 Club"))
        assertEquals("#", sectionOf(""))
        assertEquals("#", sectionOf("\u0000\r\n"))
    }

    @Test
    fun theListGetsOneHeadingForEachRunOfALetter() {
        val items = peopleItems(listOf(row(1u, "Ada"), row(2u, "Al"), row(3u, "Bo")))
        assertEquals(5, items.size)
        assertEquals(listOf("A", "B"), items.filterIsInstance<PeopleItem.Heading>().map { it.letter })
    }

    @Test
    fun aMailLinkIsOneAddressAndNothingElse() {
        assertEquals("mailto:ada@example.org", mailtoLink("ada@example.org"))
        assertEquals("mailto:ada+news@example.org", mailtoLink("  ada+news@example.org "))
        assertEquals("mailto:%C3%A9l@example.org", mailtoLink("él@example.org"))
        for (junk in listOf(
            "", "ada", "@example.org", "ada@", "a@b@c", "ada@example.org, bo@example.org", "ada@example.org?bcc=x@y.z",
            "ada@example.org\r\nBcc: x@y.z", "a da@example.org", "<ada@example.org>", "ada@exa%6Dple.org",
            "a".repeat(400) + "@x.org",
        )) {
            assertNull(junk, mailtoLink(junk))
        }
    }

    @Test
    fun aDialLinkIsOneNumberWithItsFormattingTakenOut() {
        assertEquals("tel:+442079460000", dialLink("+44 (20) 7946-0000"))
        assertEquals("tel:0123456789", dialLink("0123.456.789"))
        val junk = listOf(
            "", "+", "12", "call me", "555-0100 ext. 5", "555;123456", "1".repeat(30), "tel:5551234", "+4+4 207946000",
        )
        for (junk in junk) {
            assertNull(junk, dialLink(junk))
        }
    }

    @Test
    fun aCardSaysWhoAPersonIsOnOneLine() {
        val card = PersonCard(1u, "Ada", "Example Ltd", "Engineer", "", emptyList(), emptyList(), emptyList())
        assertEquals("Engineer · Example Ltd", card.subtitle)
    }
}
