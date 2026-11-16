// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.input.OffsetMapping
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.ComposeKind
import uniffi.mailcraft_mobile.ComposeParagraph
import uniffi.mailcraft_mobile.ComposeSpan
import uniffi.mailcraft_mobile.ListMark

class ComposeModelTest {
    @Test
    fun noChangeIsNoEdit() {
        assertNull(diffEdit("same", "same"))
        assertNull(diffEdit("", ""))
    }

    @Test
    fun typingIsAnInsertion() {
        assertEquals(BodyEdit(5, 5, "!"), diffEdit("hello", "hello!"))
        assertEquals(BodyEdit(0, 0, "Oh "), diffEdit("hello", "Oh hello"))
        assertEquals(BodyEdit(2, 2, "XY"), diffEdit("abcd", "abXYcd"))
    }

    @Test
    fun deletingAndReplacingAreOneRange() {
        assertEquals(BodyEdit(4, 5, ""), diffEdit("hello", "hell"))
        assertEquals(BodyEdit(1, 4, "ipp"), diffEdit("hello", "hippo"))
        assertEquals(BodyEdit(0, 5, ""), diffEdit("hello", ""))
        assertEquals(BodyEdit(0, 0, "hello"), diffEdit("", "hello"))
    }

    @Test
    fun aRepeatedLetterIsReadAsTheLastOne() {
        val edit = diffEdit("aa", "aaa")
        assertEquals("aaa", "aa".replaceRange(edit!!.from, edit.to, edit.text))
    }

    @Test
    fun anEmojiIsNeverCutInTwo() {
        val smile = "😀"
        val grin = "😁"
        // Both are one high surrogate and a low one that differs: the whole character is the change.
        assertEquals(BodyEdit(1, 3, grin), diffEdit("a${smile}b", "a${grin}b"))
        assertEquals(BodyEdit(1, 3, ""), diffEdit("a${smile}b", "ab"))
        assertEquals(BodyEdit(1, 1, smile), diffEdit("ab", "a${smile}b"))
        // The same low half after different highs.
        assertEquals(BodyEdit(0, 2, "😀"), diffEdit("🈀", "😀"))
    }

    @Test
    fun everyEditMakesTheNewTextWhateverTheTexts() {
        val texts = listOf("", "a", "ab", "abab", "a😀b", "😀😀", "line\nline", "x".repeat(50), "xx😁xx")
        for (old in texts) for (new in texts) {
            val edit = diffEdit(old, new)
            val made = if (edit == null) old else old.replaceRange(edit.from, edit.to, edit.text)
            assertEquals("$old -> $new", new, made)
            if (edit != null) {
                assertFalse(edit.from in 1 until old.length + 1 && old.getOrNull(edit.from - 1)?.isHighSurrogate() == true && old.getOrNull(edit.from)?.isLowSurrogate() == true)
            }
        }
    }

    @Suppress("LongParameterList", "MaxLineLength")
    private fun paragraph(start: Int, end: Int, list: ListMark = ListMark.NONE, level: Int = 0, number: Int = 0, signature: Boolean = false) =
        ComposeParagraph(start.toUInt(), end.toUInt(), list, level.toUByte(), number.toUInt(), signature)

    @Test
    fun markersAreTheListsOwn() {
        assertEquals("• ", markerOf(ListMark.BULLET, 0, 0u))
        assertEquals("    • ", markerOf(ListMark.BULLET, 1, 0u))
        assertEquals("2. ", markerOf(ListMark.NUMBER, 0, 2u))
        assertEquals("", markerOf(ListMark.NONE, 3, 0u))
        // A level the core never gives is held to a depth.
        assertTrue(markerOf(ListMark.BULLET, 1_000_000, 0u).length <= 8 * 4 + 2)
    }

    @Test
    fun markersShowInFrontOfTheItemsAndNotInTheText() {
        val view = composeView(
            text = "Plan\none\ntwo",
            paragraphs = listOf(paragraph(0, 4), paragraph(5, 8, ListMark.BULLET), paragraph(9, 12, ListMark.BULLET)),
        )
        val shown = ListMarks(markersOf(view)).filter(AnnotatedString(view.text))
        assertEquals("Plan\n• one\n• two", shown.text.text)
        assertEquals("Plan\none\ntwo", view.text)
    }

    @Test
    fun theCaretNeverLandsInsideAMarker() {
        val view = composeView(text = "Plan\none", paragraphs = listOf(paragraph(0, 4), paragraph(5, 8, ListMark.BULLET)))
        val mapping: OffsetMapping = ListMarks(markersOf(view)).filter(AnnotatedString(view.text)).offsetMapping
        assertEquals(0, mapping.originalToTransformed(0))
        assertEquals(4, mapping.originalToTransformed(4))
        // The start of the item is after its marker.
        assertEquals(7, mapping.originalToTransformed(5))
        assertEquals(10, mapping.originalToTransformed(8))
        assertEquals(4, mapping.transformedToOriginal(4))
        // Inside the marker is the item's first character.
        assertEquals(5, mapping.transformedToOriginal(5))
        assertEquals(5, mapping.transformedToOriginal(6))
        assertEquals(5, mapping.transformedToOriginal(7))
        assertEquals(8, mapping.transformedToOriginal(10))
        assertEquals(0, mapping.transformedToOriginal(0))
    }

    @Test
    fun noListsIsNoTransformation() {
        val shown = ListMarks(emptyList()).filter(AnnotatedString("plain"))
        assertEquals("plain", shown.text.text)
        assertEquals(3, shown.offsetMapping.originalToTransformed(3))
        // A marker the text has outgrown (the keyboard is ahead of the core) is left out.
        assertEquals("ab", ListMarks(listOf(50 to "• ")).filter(AnnotatedString("ab")).text.text)
    }

    @Test
    fun theLookOfTheCoreIsDrawnAndHostileRangesAreHeld() {
        val view = composeView(
            text = "hello world",
            spans = listOf(
                ComposeSpan(0u, 5u, bold = true, italic = false, underline = false, link = null),
                ComposeSpan(6u, 11u, bold = false, italic = false, underline = false, link = "https://example.org/"),
                ComposeSpan(9u, 4_000_000u, bold = true, italic = true, underline = true, link = null),
                ComposeSpan(8u, 3u, bold = true, italic = false, underline = false, link = null),
                ComposeSpan(UInt.MAX_VALUE, UInt.MAX_VALUE, bold = true, italic = false, underline = false, link = null),
            ),
            paragraphs = listOf(paragraph(0, 11, signature = true)),
        )
        val shown = styledBody("hello", view, Color.Blue, Color.Gray)
        assertEquals("hello", shown.text)
        assertTrue(shown.spanStyles.all { it.start in 0..5 && it.end in 0..5 && it.start <= it.end })
        assertTrue(shown.spanStyles.any { it.item.fontWeight != null })
        // A shorter text than the core's: nothing outside it.
        assertEquals("", styledBody("", view, Color.Blue, Color.Gray).text)
    }

    @Test
    fun theSignatureIsDrawnQuieter() {
        val view = composeView(text = "Hi\n-- \nAvery", paragraphs = listOf(paragraph(0, 2), paragraph(3, 12, signature = true)))
        val shown = styledBody(view.text, view, Color.Blue, Color.Gray)
        val quiet = shown.spanStyles.filter { it.item.color == Color.Gray }
        assertEquals(1, quiet.size)
        assertEquals(3 to 12, quiet[0].start to quiet[0].end)
    }

    @Test
    fun peopleAreTypedInAnyOfTheUsualWays() {
        assertEquals(listOf("ada@example.org"), parseTyped("ada@example.org").map { it.address })
        assertEquals(listOf("ada@example.org", "bo@example.org"), parseTyped("ada@example.org, bo@example.org;").map { it.address })
        val named = parseTyped("\"Quill, Ada\" <ada@example.org>; Bo <bo@example.org>")
        assertEquals(listOf("Quill, Ada" to "ada@example.org", "Bo" to "bo@example.org"), named.map { it.name to it.address })
        assertTrue(parseTyped("").isEmpty())
        assertTrue(parseTyped(" ,;, \n").isEmpty())
        assertEquals(200, parseTyped("a@example.org,".repeat(1_000)).size)
    }

    @Test
    fun anAddressLooksDeliverableOrNot() {
        assertTrue(looksDeliverable("ada@example.org"))
        for (bad in listOf("", "ada", "@example.org", "ada@", "ada@example", "ada@.org", "ada @example.org", "ada@example.org.")) assertFalse(bad, looksDeliverable(bad))
    }

    @Test
    fun theAccountItIsSentFromComesFirstAndSharedMailboxesLast() {
        val order = fromOrder(Cast.accounts, 3uL).map { it.account }
        assertEquals(listOf(3uL, 1uL, 2uL), order)
        assertEquals(listOf(1uL, 3uL, 2uL), fromOrder(Cast.accounts, 1uL).map { it.account })
        assertEquals("Support desk <support@example.org> (shared mailbox)", fromLabel(EnglishTexts, Cast.accounts[1]))
        assertEquals("a@example.org", fromLabel(EnglishTexts, uniffi.mailcraft_mobile.FromChoice(1uL, "", "a@example.org", shared = false)))
    }

    @Test
    fun theLookAtTheCaretIsTheOfTheCharacterBefore() {
        val view = composeView(
            text = "hello world",
            spans = listOf(
                ComposeSpan(0u, 5u, bold = true, italic = false, underline = false, link = null),
                ComposeSpan(6u, 11u, bold = false, italic = true, underline = false, link = "https://example.org/"),
            ),
        )
        assertEquals(LookAt(bold = true), lookAt(view, 3))
        assertEquals(LookAt(bold = true), lookAt(view, 5))
        assertEquals(LookAt(), lookAt(view, 6))
        assertEquals(LookAt(italic = true, link = true), lookAt(view, 11))
        assertEquals(LookAt(bold = true), lookAt(view, 0))
        assertEquals(LookAt(), lookAt(view, 9_999))
    }

    @Test
    fun theTitleSaysWhatItIs() {
        assertEquals("New message", composeTitle(EnglishTexts, ComposeKind.NEW))
        assertEquals("Reply", composeTitle(EnglishTexts, ComposeKind.REPLY_ALL))
        assertEquals("Forward", composeTitle(EnglishTexts, ComposeKind.FORWARD))
    }
}
