// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.BlockKindView

/** Tests the reading pane's logic against invented messages; nothing is ever fetched from anywhere. */
class ReadingControllerTest {
    private val dir: File = Files.createTempDirectory("reading").toFile()
    private val banner = "https://img.shop.example/banner.png"
    private val tracker = "https://tracker.example/p.gif"

    @After
    fun clean() {
        dir.deleteRecursively()
    }

    private fun message(junk: Boolean = false, vouched: Boolean = true): OpenedMessage {
        val blocks = listOf(
            B.p("Hello"),
            B.image(banner, "Banner", 560u, 120u),
            B.image(tracker, "", 1u, 1u),
            B.image("cid:logo", "Logo", 20u, 20u, picture = "cid:logo"),
            B.image("https://img.shop.example/second.png", "Second"),
        )
        val view = Corpus.plain.view.copy(id = 7uL, unread = true, blocks = blocks, remotePictures = 2u)
        return OpenedMessage(view, MessageFacts("news@shop.example", junk, vouched, known = false), emptyMap())
    }

    private class Rig(val source: FakeReadSource, val cache: PictureCache, val prefs: MemoryPicturePrefs, val c: ReadingController)

    // Unconfined: the fake source never suspends for real, so every launch runs to its end where it is made.
    private fun rig(opened: OpenedMessage? = message(), prefs: PicturePrefs = PicturePrefs()): Rig {
        val source = FakeReadSource(opened)
        source.web[banner] = B.png
        source.web["https://img.shop.example/second.png"] = B.png
        source.web[tracker] = B.png
        source.parts["cid:logo"] = B.png
        val cache = PictureCache(dir)
        val store = MemoryPicturePrefs(prefs)
        return Rig(source, cache, store, ReadingController(source, cache, store, CoroutineScope(Dispatchers.Unconfined), Dispatchers.Unconfined))
    }

    @Test
    fun aMessageOpensWithItsOwnPicturesAndNothingFromTheWeb() {
        val r = rig()
        r.c.open(7uL, 360, dark = false)
        val ui = r.c.ui.value
        assertEquals(LoadState.Ready, ui.load)
        assertEquals(PictureAccess.Blocked, ui.access)
        assertTrue(ui.offersPictures)
        assertTrue(ui.pictures["cid:logo"] is PictureState.Ready)
        assertTrue(r.source.downloads.isEmpty())
        assertEquals(listOf(Triple(7uL, 360, false)), r.source.opened)
        // Opening reads it: once.
        assertEquals(listOf(7uL), r.source.marked)
        r.c.open(7uL, 400, dark = true)
        assertEquals(listOf(7uL), r.source.marked)
    }

    @Test
    fun downloadPicturesForThisMessageFetchesEachAddressOnceAndNeverATrackingPixel() {
        val r = rig()
        r.c.open(7uL, 360, dark = false)
        r.c.allow(AllowScope.Message)
        assertEquals(listOf(banner, "https://img.shop.example/second.png"), r.source.downloads)
        assertTrue(r.c.ui.value.pictures[banner] is PictureState.Ready)
        assertNull(r.c.ui.value.pictures[tracker])
        assertTrue(r.c.isAsked(7uL))
        // The same message again (a new width) asks for nothing more.
        r.c.open(7uL, 500, dark = false)
        assertEquals(2, r.source.downloads.size)
        // And what is kept is not asked for again by another message.
        r.c.open(8uL, 360, dark = false)
        r.c.allow(AllowScope.Message)
        assertEquals(2, r.source.downloads.size)
    }

    @Test
    fun theSenderAndTheDomainAreRememberedAndAllowTheNextMessage() {
        val r = rig()
        r.c.open(7uL, 360, dark = false)
        r.c.allow(AllowScope.Sender)
        assertEquals(setOf("news@shop.example"), r.prefs.load().senders)
        assertEquals(PictureAccess.Allowed, r.c.ui.value.access)
        r.c.allow(AllowScope.Domain)
        assertEquals(setOf("shop.example"), r.prefs.load().domains)
    }

    @Test
    fun junkAllowsThisMessageAndNothingElse() {
        val r = rig(message(junk = true), PicturePrefs(all = true, senders = setOf("news@shop.example")))
        r.c.open(7uL, 360, dark = false)
        assertEquals(PictureAccess.Blocked, r.c.ui.value.access)
        assertEquals(listOf(AllowScope.Message), r.c.ui.value.choices)
        r.c.allow(AllowScope.Sender)
        r.c.allow(AllowScope.Domain)
        assertEquals(PictureAccess.Blocked, r.c.ui.value.access)
        assertTrue(r.source.downloads.isEmpty())
        r.c.allow(AllowScope.Message)
        assertEquals(PictureAccess.Allowed, r.c.ui.value.access)
        assertEquals(2, r.source.downloads.size)
    }

    @Test
    fun theSwitchForAllMailDownloadsAtOnce() {
        val r = rig(prefs = PicturePrefs(all = true))
        r.c.open(7uL, 360, dark = false)
        assertEquals(PictureAccess.Allowed, r.c.ui.value.access)
        assertFalse(r.c.ui.value.offersPictures)
        assertEquals(2, r.source.downloads.size)
    }

    @Test
    fun dataSaverHoldsDownloadsUntilTheOwnerTaps() {
        val r = rig(prefs = PicturePrefs(all = true))
        r.source.dataSaver = true
        r.c.open(7uL, 360, dark = false)
        val ui = r.c.ui.value
        assertTrue(ui.waiting)
        assertEquals(PictureState.Waiting, ui.pictures[banner])
        assertTrue(r.source.downloads.isEmpty())
        // What is already kept needs no network and is shown.
        r.cache.put("https://img.shop.example/second.png", B.png)
        r.c.open(7uL, 361, dark = false)
        assertTrue(r.c.ui.value.pictures["https://img.shop.example/second.png"] is PictureState.Ready)
        assertTrue(r.source.downloads.isEmpty())
        r.c.downloadNow()
        assertEquals(listOf(banner), r.source.downloads)
        assertTrue(r.c.ui.value.pictures[banner] is PictureState.Ready)
    }

    @Test
    fun aRefusedOrFalseDownloadIsAFailedPictureAndNothingIsKept() {
        val r = rig()
        r.source.web[banner] = "<html>a page, not a picture</html>".toByteArray()
        r.source.web.remove("https://img.shop.example/second.png")
        r.c.open(7uL, 360, dark = false)
        r.c.allow(AllowScope.Message)
        assertEquals(PictureState.Failed, r.c.ui.value.pictures[banner])
        assertEquals(PictureState.Failed, r.c.ui.value.pictures["https://img.shop.example/second.png"])
        assertEquals(0, r.cache.usage().first)
    }

    @Test
    fun aMessageIsHeldToItsPictureCountAndBytes() {
        val many = (1..MAX_PICTURES_PER_MESSAGE + 5).map { B.image("https://img.shop.example/$it.png", "p$it", 40u, 40u) }
        val view = Corpus.plain.view.copy(id = 9uL, unread = false, blocks = many, remotePictures = many.size.toUInt())
        val opened = OpenedMessage(view, MessageFacts("a@shop.example", junk = false, vouched = true, known = false), emptyMap())
        val r = rig(opened, PicturePrefs(all = true))
        many.forEach { block -> (block.kind as BlockKindView.Image).let { r.source.web[it.src] = B.png } }
        r.c.open(9uL, 360, dark = false)
        assertEquals(MAX_PICTURES_PER_MESSAGE, r.source.downloads.size)
        assertEquals(PictureState.LeftOut, r.c.ui.value.pictures["https://img.shop.example/${MAX_PICTURES_PER_MESSAGE + 1}.png"])
    }

    @Test
    fun aMessageThatCannotBeReadIsFailedAndAnOldAnswerIsDropped() {
        val r = rig(null)
        r.c.open(7uL, 360, dark = false)
        assertEquals(LoadState.Failed, r.c.ui.value.load)
        r.c.close(7uL)
        assertNull(r.c.ui.value.id)
        assertNull(r.c.ui.value.opened)
    }

    @Test
    fun theChoicesInSettingsChangeAnOpenMessage() {
        val r = rig()
        r.c.open(7uL, 360, dark = false)
        assertTrue(r.c.ui.value.offersPictures)
        r.prefs.save(PicturePrefs(all = true))
        r.c.prefsChanged()
        assertNotNull(r.c.ui.value.pictures[banner])
        assertFalse(r.c.ui.value.offersPictures)
    }

    @Test
    fun leavingTheMessageStopsEveryDownloadNotYetStartedAndKeepsNothingThatWasOnItsWay() {
        val r = rig()
        // The owner leaves while the first picture is coming.
        r.source.onDownload = { r.c.close(7uL) }
        r.c.open(7uL, 360, dark = false)
        r.c.allow(AllowScope.Message)
        assertEquals(listOf(banner), r.source.downloads)
        assertEquals(0, r.cache.usage().first)
        assertNull(r.c.ui.value.id)
        assertTrue(r.c.ui.value.pictures.isEmpty())
    }

    @Test
    fun closingAMessageThatIsNoLongerOpenChangesNothing() {
        val r = rig()
        r.c.open(7uL, 360, dark = false)
        r.c.close(99uL)
        assertEquals(7uL, r.c.ui.value.id)
        assertEquals(LoadState.Ready, r.c.ui.value.load)
    }
}
