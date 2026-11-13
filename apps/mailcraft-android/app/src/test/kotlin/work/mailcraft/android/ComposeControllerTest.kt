// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.ComposeFormat
import uniffi.mailcraft_mobile.ComposeKind
import uniffi.mailcraft_mobile.ComposeParagraph
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.ListMark
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.SuggestionRecord

/** The composer's logic against a stand-in for the core: invented people, nothing sent anywhere. */
class ComposeControllerTest {
    // Unconfined: the stand-in never suspends for real, so each step runs to its end where it is made.
    private fun rig(view: uniffi.mailcraft_mobile.ComposeView = composeView()): Pair<FakeCompose, ComposeController> {
        val source = FakeCompose(view)
        val controller = ComposeController(source, CoroutineScope(Dispatchers.Unconfined))
        controller.start(ComposeStart.New(0uL, null))
        return source to controller
    }

    private fun ComposeController.type(text: String) = onBody(TextFieldValue(text, TextRange(text.length)))

    @Test
    fun aNewMessageShowsWhatTheCoreStartedItWith() {
        val (source, c) = rig(composeView(text = "\n-- \nAvery", signature = "Avery", paragraphs = listOf(ComposeParagraph(0u, 0u, ListMark.NONE, 0u.toUByte(), 0u, false), ComposeParagraph(1u, 10u, ListMark.NONE, 0u.toUByte(), 0u, true))))
        val ui = c.ui.value
        assertEquals(listOf("start New"), source.calls)
        assertEquals("\n-- \nAvery", ui.body.text)
        assertEquals("Avery", ui.view?.signature)
        // The signature is the engine's paragraph: after the owner's own text, marked as such.
        assertTrue(ui.view!!.paragraphs.last().signature)
        assertEquals(3, ui.accounts.size)
    }

    @Test
    fun aReplyShowsItsQuoteAndSubject() {
        val (_, c) = rig(composeView(kind = ComposeKind.REPLY, subject = "RE: Garden plan", to = listOf(Cast.ada), quote = "From: Ada Quill\nSent: Monday\nSubject: Garden plan\n\nSee you at ten."))
        val view = c.ui.value.view!!
        assertEquals("RE: Garden plan", c.ui.value.subject)
        assertEquals(listOf("Ada Quill"), c.ui.value.people[RecipientLine.TO].orEmpty().map { it.name })
        assertTrue(view.quote.contains("See you at ten."))
    }

    @Test
    fun aMessageThatCouldNotBeStartedSaysWhy() {
        val source = FakeCompose()
        source.startAnswer = ComposeResult.Failed("12 messages are being written already")
        val c = ComposeController(source, CoroutineScope(Dispatchers.Unconfined))
        c.start(ComposeStart.Reply(3uL))
        assertEquals("12 messages are being written already", c.ui.value.failed)
        assertNull(c.ui.value.view)
        assertTrue(c.ui.value.open)
        c.close()
        assertEquals(ComposeDone.Closed, c.ui.value.done)
        assertTrue(source.calls.none { it.startsWith("close") })
    }

    @Test
    fun typingGoesToTheCoreAsOneRange() {
        val (source, c) = rig(composeView(text = "Hello"))
        c.type("Hello!")
        c.type("Hello!!")
        assertEquals(listOf("start New", "edit 5-5:!", "edit 6-6:!"), source.calls)
        assertEquals("Hello!!", c.ui.value.body.text)
        assertEquals("Hello!!", c.ui.value.view?.text)
    }

    @Test
    fun anEmojiIsOneEditAndTheTextStaysWhole() {
        val (source, c) = rig(composeView(text = "ab"))
        c.type("a😀b")
        assertEquals("edit 1-1:😀", source.calls.last())
        assertEquals("a😀b", c.ui.value.view?.text)
    }

    @Test
    fun whatTheCoreChangedOfTheTextIsTakenInWhenNothingIsWaiting() {
        val (source, c) = rig(composeView(text = ""))
        // The core turns a typed "- " into a list item: its text is the one that stands.
        source.onEdit = { it.copy(text = "item") }
        c.type("- ")
        assertEquals("item", c.ui.value.body.text)
    }

    @Test
    fun aRefusedEditIsToldAndTheCoresTextStands() {
        val (source, c) = rig(composeView(text = "short"))
        source.refuseEdits = "this message is too long to be edited on a phone"
        c.type("short and long")
        val ui = c.ui.value
        assertEquals("this message is too long to be edited on a phone", (ui.notice as ComposeNotice.Said).why)
        assertEquals("short", ui.body.text)
    }

    @Test
    fun formattingNamesTheSelectedRange() {
        val (source, c) = rig(composeView(text = "hello world"))
        c.onBody(TextFieldValue("hello world", TextRange(0, 5)))
        c.format(ComposeFormat.Bold)
        assertEquals("format Bold 0-5", source.calls.last())
        assertEquals(1, c.ui.value.view?.spans?.size)
        // Backwards is the same range.
        c.onBody(TextFieldValue("hello world", TextRange(11, 6)))
        c.format(ComposeFormat.Italic)
        assertEquals("format Italic 6-11", source.calls.last())
    }

    @Test
    fun peopleTypedBecomeChipsAtACommaOrWhenTheFieldIsLeft() {
        val (source, c) = rig()
        c.typed(RecipientLine.TO, "ada@example.org, bo")
        assertEquals(listOf("ada@example.org"), c.ui.value.people[RecipientLine.TO].orEmpty().map { it.address })
        assertEquals(" bo", c.ui.value.typed[RecipientLine.TO])
        c.typed(RecipientLine.TO, "bo@example.org")
        c.commit(RecipientLine.TO)
        assertEquals(listOf("ada@example.org", "bo@example.org"), c.ui.value.view?.to?.map { it.address })
        assertEquals("", c.ui.value.typed[RecipientLine.TO])
        assertTrue(source.calls.any { it == "recipients TO ada@example.org, bo@example.org" })
    }

    @Test
    fun twoPeopleAddedInARowDoNotLoseEachOther() {
        val (_, c) = rig()
        c.typed(RecipientLine.CC, "ada@example.org;")
        c.typed(RecipientLine.CC, "bo@example.org;")
        c.typed(RecipientLine.CC, "cy@example.org;")
        assertEquals(listOf("ada@example.org", "bo@example.org", "cy@example.org"), c.ui.value.view?.cc?.map { it.address })
    }

    @Test
    fun aPersonIsTakenOffByTheirPlace() {
        val (_, c) = rig(composeView(to = listOf(Cast.ada, Cast.bo)))
        c.remove(RecipientLine.TO, 0)
        assertEquals(listOf("bo@example.org"), c.ui.value.view?.to?.map { it.address })
        c.remove(RecipientLine.TO, 9)
        c.remove(RecipientLine.TO, -1)
        assertEquals(1, c.ui.value.view?.to?.size)
    }

    @Test
    fun suggestionsComeAfterTwoLettersAndAreTakenByATap() {
        val (source, c) = rig()
        source.suggestions += SuggestionRecord("Ada Quill", "ada@example.org", contact = true)
        c.typed(RecipientLine.TO, "a")
        assertTrue(c.ui.value.suggestions.isEmpty())
        c.typed(RecipientLine.TO, "ad")
        assertEquals(RecipientLine.TO, c.ui.value.suggestFor)
        assertEquals(1, c.ui.value.suggestions.size)
        c.pick(RecipientLine.TO, c.ui.value.suggestions[0])
        assertEquals(listOf("ada@example.org"), c.ui.value.view?.to?.map { it.address })
        assertTrue(c.ui.value.suggestions.isEmpty())
        assertEquals("", c.ui.value.typed[RecipientLine.TO])
    }

    @Test
    fun switchingFromChangesTheAccountAndItsSignature() {
        val (source, c) = rig(composeView(signature = "Avery"))
        c.from(2uL)
        assertEquals(2uL, c.ui.value.view?.account)
        assertEquals("Support", c.ui.value.view?.signature)
        c.from(3uL)
        assertEquals("", c.ui.value.view?.signature)
        assertEquals(listOf("start New", "from 2", "from 3"), source.calls)
    }

    @Test
    fun subjectChangesGoOnInOrder() {
        val (source, c) = rig()
        c.onSubject("Plan")
        c.onSubject("Plan B")
        assertEquals(listOf("start New", "subject Plan", "subject Plan B"), source.calls)
        assertEquals("Plan B", c.ui.value.subject)
    }

    @Test
    fun aFileFromThePickerIsAttachedAndOneThatCouldNotBeReadIsTold() {
        val (source, c) = rig()
        c.attach("report.pdf", ByteArray(10))
        assertEquals("attach report.pdf 10", source.calls.last())
        assertEquals(1, c.ui.value.view?.files?.size)
        c.attach("gone.pdf", null)
        assertEquals(ComposeNotice.Unreadable, c.ui.value.notice)
        c.noticeShown()
        assertNull(c.ui.value.notice)
        c.removeFile(0u)
        assertTrue(c.ui.value.view?.files.orEmpty().isEmpty())
    }

    @Test
    fun aPictureGoesInWhereTheCaretIs() {
        val (source, c) = rig(composeView(text = "ab"))
        c.onBody(TextFieldValue("ab", TextRange(1)))
        c.picture(ByteArray(4), "A fern")
        assertEquals("picture 4 A fern 1", source.calls.last())
    }

    @Test
    fun sendingWithNobodyToSendToIsTolbAndNothingIsSent() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.send()
        assertEquals(ComposeNotice.NoRecipient, c.ui.value.notice)
        assertTrue(source.calls.none { it == "send" })
        assertNull(c.ui.value.done)
    }

    @Test
    fun aPersonJustPickedCountsEvenBeforeTheCoreHasAnswered() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.pick(RecipientLine.TO, SuggestionRecord("Ada Quill", "ada@example.org", contact = true))
        c.send()
        assertNull(c.ui.value.notice)
        assertTrue(source.calls.any { it == "send" })
    }

    @Test
    fun whatIsTypedOnALineWhenSendingIsSentTo() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.typed(RecipientLine.TO, "ada@example.org")
        c.send()
        assertTrue(source.calls.indexOf("recipients TO ada@example.org") in 0 until source.calls.indexOf("send"))
        assertEquals(ComposeDone.Sent(queued = true).let { (it as ComposeDone.Sent).queued }, (c.ui.value.done as ComposeDone.Sent).queued)
    }

    @Test
    fun aSendThatWaitsForTheNextCheckSaysSo() {
        val (source, c) = rig(composeView(to = listOf(Cast.ada)))
        source.sendAnswer = SendResult.Sent(queued = true)
        c.send()
        assertTrue((c.ui.value.done as ComposeDone.Sent).queued)
        val (other, d) = rig(composeView(to = listOf(Cast.ada)))
        other.sendAnswer = SendResult.Sent(queued = false)
        d.send()
        assertEquals(false, (d.ui.value.done as ComposeDone.Sent).queued)
    }

    @Test
    fun aSendTheCoreRefusesLeavesTheMessageOpen() {
        val (source, c) = rig(composeView(to = listOf(Cast.ada)))
        source.sendAnswer = SendResult.Failed("this account has no Outbox")
        c.send()
        val ui = c.ui.value
        assertNull(ui.done)
        assertEquals(false, ui.sending)
        assertEquals("this account has no Outbox", (ui.notice as ComposeNotice.Said).why)
        assertNotNull(ui.view)
    }

    @Test
    fun nothingGoesToTheCoreAfterTheSend() {
        val (source, c) = rig(composeView(to = listOf(Cast.ada)))
        c.send()
        val before = source.calls.size
        c.onSubject("too late")
        c.close()
        c.save()
        assertEquals(before, source.calls.size)
    }

    @Test
    fun closingKeepsAMessageWithChangesAsADraft() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.type("Hi there")
        c.close()
        assertEquals(ComposeDone.Saved, c.ui.value.done)
        assertEquals("close discard=false", source.calls.last())
    }

    @Test
    fun closingAMessageNobodyChangedLeavesNothing() {
        val (_, c) = rig()
        c.close()
        assertEquals(ComposeDone.Closed, c.ui.value.done)
    }

    @Test
    fun discardingSaysSo() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.type("Hi there")
        c.discard()
        assertEquals(ComposeDone.Discarded, c.ui.value.done)
        assertEquals("close discard=true", source.calls.last())
    }

    @Test
    fun theAutosaveSaysWhenItWorked() {
        val (source, c) = rig(composeView(text = "Hi"))
        c.type("Hi there")
        c.save()
        assertTrue(c.ui.value.saved)
        c.type("Hi there!")
        assertEquals(false, c.ui.value.saved)
        source.saveWorks = false
        c.save()
        assertEquals(false, c.ui.value.saved)
    }

    @Test
    fun picturesInTheTextAreFetchedOnceForDrawing() {
        val view = composeView(text = "￼").copy(
            pictures = listOf(uniffi.mailcraft_mobile.ComposePicture(7u, 0u, "fern.png", "A fern", 4uL)),
        )
        val source = FakeCompose(view)
        source.pictureData[7u] = byteArrayOf(1, 2, 3)
        val c = ComposeController(source, CoroutineScope(Dispatchers.Unconfined))
        c.start(ComposeStart.New(0uL, null))
        assertEquals(listOf<Byte>(1, 2, 3), c.ui.value.pictures[7u]?.toList())
    }

    @Test
    fun leavingTheScreenLetsEverythingGo() {
        val (_, c) = rig(composeView(text = "Hi"))
        c.reset()
        assertEquals(ComposeUi(), c.ui.value)
        c.onSubject("nothing happens")
    }

    @Test
    fun sharedFilesAreAttachedOnceTheMessageIsOpenAndWhatWasRefusedIsSaid() {
        val source = FakeCompose(composeView())
        val c = ComposeController(source, CoroutineScope(Dispatchers.Unconfined))
        val files = listOf(SharedFile("a.txt", byteArrayOf(1), ShareKind.Other), SharedFile("b.png", byteArrayOf(2), ShareKind.Png))
        c.start(ComposeStart.Shared(0uL, "A page", "text"), files, "big.iso was not attached: it is larger than 25 MB.")
        assertEquals(listOf("start Shared"), source.calls.take(1))
        assertEquals(2, source.calls.count { it.startsWith("attach") })
        val ui = c.ui.value
        assertEquals("big.iso was not attached: it is larger than 25 MB.", ui.skipped)
        assertTrue(ui.edits >= 2)
        assertFalse(ui.saved)
        // It stays until the owner has read it.
        c.dismissSkipped()
        assertNull(c.ui.value.skipped)
    }

    @Test
    fun aShareWithNothingRefusedSaysNothing() {
        val source = FakeCompose(composeView())
        val c = ComposeController(source, CoroutineScope(Dispatchers.Unconfined))
        c.start(ComposeStart.New(0uL, null), listOf(SharedFile("a.txt", byteArrayOf(1), ShareKind.Other)), null)
        assertNull(c.ui.value.skipped)
    }
}
