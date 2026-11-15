// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** What happens to a download that is waiting, or on its way, when the owner's word changes under it. Invented mail; no network. */
class PictureRaceTest {
    private val dir: File = Files.createTempDirectory("races").toFile()
    private val urls = List(6) { "https://img.shop.example/p$it.png" }

    @After
    fun clean() {
        dir.deleteRecursively()
    }

    private fun message(): OpenedMessage {
        val view = Corpus.plain.view.copy(id = 7uL, unread = false, blocks = urls.map { B.image(it, "p") }, remotePictures = 6u)
        return OpenedMessage(view, MessageFacts("news@shop.example", junk = false, vouched = true, known = false), emptyMap())
    }

    private fun reading(prefs: PicturePrefs): Triple<FakeReadSource, MemoryPicturePrefs, ReadingController> {
        val source = FakeReadSource(message())
        urls.forEach { source.web[it] = B.png }
        val store = MemoryPicturePrefs(prefs)
        val controller = ReadingController(source, PictureCache(dir, SniffingCheck), store, CoroutineScope(Dispatchers.Unconfined), Dispatchers.Unconfined)
        return Triple(source, store, controller)
    }

    @Test
    fun aDownloadQueuedBehindOthersAsksNothingOnceAllMailIsBlockedAndIsAskedForAgainOnAnotherYes() {
        val (source, store, c) = reading(PicturePrefs(all = true))
        val gate = CompletableDeferred<Unit>()
        source.onDownload = { gate.await() }
        c.open(7uL, 360, dark = false)
        // Four are on their way (all the permits); two wait for one.
        assertEquals(4, source.stops.size)
        store.save(PicturePrefs(all = false))
        c.prefsChanged()
        gate.complete(Unit)
        assertEquals("the two that waited must not ask", 4, source.downloads.size)
        // A yes again in the same open message asks for the two that were let go of.
        store.save(PicturePrefs(all = true))
        c.prefsChanged()
        assertEquals(6, source.downloads.size)
        assertEquals(urls.toSet(), source.downloads.toSet())
    }

    @Test
    fun leavingTheMessageStopsWhatIsOnItsWayAndOpeningAgainStartsAfresh() {
        val (source, _, c) = reading(PicturePrefs(all = true))
        val gate = CompletableDeferred<Unit>()
        source.onDownload = { gate.await() }
        c.open(7uL, 360, dark = false)
        assertEquals(4, source.stops.size)
        assertTrue(source.stops.none { it.isStopped })
        c.close(7uL)
        assertTrue("every download on its way is told to stop", source.stops.all { it.isStopped })
        gate.complete(Unit)
        assertTrue("nothing of what came is kept", c.ui.value.pictures.isEmpty())
        // The next message's downloads are not stopped by the last one's.
        source.stops.clear()
        source.onDownload = {}
        c.open(7uL, 360, dark = false)
        assertTrue(source.stops.isNotEmpty() && source.stops.none { it.isStopped })
    }

    @Test
    fun aSignalGivenBeforeAHookIsRegisteredRunsTheHookAtOnce() {
        val stop = Stop()
        var ran = 0
        stop.onStop { ran++ }
        stop.give()
        stop.give()
        stop.onStop { ran++ }
        assertEquals(2, ran)
        assertTrue(stop.isStopped)
    }

    private val row = InboxRow(1u, "Ada Example", "Hi", "…", false, 0L, address = "ada@example.org")

    @Test
    fun aSenderLookUpQueuedBehindOthersAsksNothingOnceSenderImagesAreOff() {
        val source = FakeReadSource()
        val store = MemoryPicturePrefs(PicturePrefs(senderImages = true))
        val images = SenderImages(source, PictureCache(dir, SniffingCheck), store, CoroutineScope(Dispatchers.Unconfined), io = Dispatchers.Unconfined)
        val gate = CompletableDeferred<Unit>()
        source.onSenderDownload = { gate.await() }
        // Four addresses hold the four permits; the fifth waits for one.
        images.onScreen((1..5).map { row.copy(id = it.toULong(), address = "p$it@example.org") })
        assertEquals(4, source.senderDownloads.size)
        store.save(PicturePrefs(senderImages = false))
        images.reset()
        gate.complete(Unit)
        assertEquals(4, source.senderDownloads.size)
    }

    @Test
    fun aMessageThatMovedToJunkLosesItsSenderPictureWhenTheListIsAtANewerRevision() {
        val source = FakeReadSource()
        val store = MemoryPicturePrefs(PicturePrefs(senderImages = true))
        val images = SenderImages(source, PictureCache(dir, SniffingCheck), store, CoroutineScope(Dispatchers.Unconfined), io = Dispatchers.Unconfined)
        source.senderWeb[source.senderPlaces("ada@example.org")[0]] = B.png
        images.revChanged(1uL)
        images.onScreen(listOf(row))
        assertArrayEquals(B.png, images.images.value[1uL])
        // Nothing changed: the decision stands, and nothing is asked again.
        images.onScreen(listOf(row))
        assertEquals(1, source.senderDownloads.size)
        // The same message, now in Junk E-mail: a newer revision, and the rules say no.
        source.factsAnswer = MessageFacts("ada@example.org", junk = true, vouched = true, known = true)
        images.onScreen(listOf(row))
        assertArrayEquals("still the same revision", B.png, images.images.value[1uL])
        images.revChanged(2uL)
        assertNull(images.images.value[1uL])
    }

    @Test
    fun aDownloadQueuedBehindOthersWaitsForATapOnceDataSaverIsOn() {
        val (source, _, c) = reading(PicturePrefs(all = true))
        val gate = CompletableDeferred<Unit>()
        source.onDownload = { gate.await() }
        c.open(7uL, 360, dark = false)
        assertEquals(4, source.stops.size)
        source.dataSaver = true
        gate.complete(Unit)
        assertEquals("the two that waited must not ask", 4, source.downloads.size)
        assertTrue(c.ui.value.waiting)
        assertEquals(2, c.ui.value.pictures.values.count { it is PictureState.Waiting })
    }

    @Test
    fun aCacheWriteQueuedWhenTheMessageIsClosedIsNotMade() {
        val source = FakeReadSource(message())
        urls.forEach { source.web[it] = B.png }
        val store = MemoryPicturePrefs(PicturePrefs(all = true))
        val queued = mutableListOf<Runnable>()
        val deferred = object : kotlinx.coroutines.CoroutineDispatcher() {
            override fun dispatch(context: kotlin.coroutines.CoroutineContext, block: Runnable) {
                queued += block
            }
        }
        val cache = PictureCache(dir, SniffingCheck)
        val c = ReadingController(source, cache, store, CoroutineScope(Dispatchers.Unconfined), deferred)
        c.open(7uL, 360, dark = false)
        // Run what is queued (the cache reads) until downloads have come and their writes are queued.
        var guard = 0
        while (source.downloads.size < 4 && queued.isNotEmpty() && guard++ < 100) queued.removeAt(0).run()
        c.close(7uL)
        while (queued.isNotEmpty() && guard++ < 1000) queued.removeAt(0).run()
        assertNull("closed before the write ran", cache.get(urls[0]))
    }

    @Test
    fun aSenderLookUpQueuedBehindOthersAsksNothingOnceDataSaverIsOn() {
        val source = FakeReadSource()
        val store = MemoryPicturePrefs(PicturePrefs(senderImages = true))
        val images = SenderImages(source, PictureCache(dir, SniffingCheck), store, CoroutineScope(Dispatchers.Unconfined), io = Dispatchers.Unconfined)
        val gate = CompletableDeferred<Unit>()
        source.onSenderDownload = { gate.await() }
        images.onScreen((1..5).map { row.copy(id = it.toULong(), address = "p$it@example.org") })
        assertEquals(4, source.senderDownloads.size)
        source.dataSaver = true
        gate.complete(Unit)
        assertEquals(4, source.senderDownloads.size)
    }

    @Test
    fun aRowTurnedAwayForWantOfRoomIsLookedAtAgain() {
        val source = FakeReadSource()
        val store = MemoryPicturePrefs(PicturePrefs(senderImages = true))
        val images = SenderImages(source, PictureCache(dir, SniffingCheck), store, CoroutineScope(Dispatchers.Unconfined), io = Dispatchers.Unconfined)
        val gate = CompletableDeferred<Unit>()
        source.onSenderDownload = { gate.await() }
        // 64 addresses fill the room (four hold permits, the rest wait); a 65th is turned away.
        val rows = (1..64).map { row.copy(id = it.toULong(), address = "p$it@example.org") }
        images.onScreen(rows)
        val late = row.copy(id = 99u, address = "late@example.org")
        images.onScreen(listOf(late))
        assertTrue("the 65th is not asked for while the room is full", source.senderDownloads.none { it.contains("late") })
        source.onSenderDownload = {}
        gate.complete(Unit)
        val before = source.senderDownloads.size
        images.onScreen(listOf(late))
        assertTrue("the turned-away row is asked for after room is made", source.senderDownloads.size > before)
    }
}
