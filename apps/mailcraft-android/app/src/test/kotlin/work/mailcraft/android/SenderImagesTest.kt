// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import java.nio.file.Files
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SenderImagesTest {
    private val dir: File = Files.createTempDirectory("senders").toFile()
    private val source = FakeReadSource()
    private val prefs = MemoryPicturePrefs(PicturePrefs(senderImages = true))
    private val cache = PictureCache(dir)
    private val images = SenderImages(source, cache, prefs, CoroutineScope(Dispatchers.Unconfined))
    private val row = InboxRow(1u, "Ada Example", "Hi", "…", false, 0L, address = "Ada@Example.org")

    @After
    fun clean() {
        dir.deleteRecursively()
    }

    @Test
    fun offByDefaultAsksNobodyAnything() {
        val off = SenderImages(source, cache, MemoryPicturePrefs(), CoroutineScope(Dispatchers.Unconfined))
        off.onScreen(listOf(row))
        assertTrue(source.senderDownloads.isEmpty())
        assertTrue(off.images.value.isEmpty())
    }

    @Test
    fun theFirstPlaceThatHasAPictureWinsAndItIsKept() {
        val places = source.senderPlaces("ada@example.org")
        source.senderWeb[places[1]] = B.png
        images.onScreen(listOf(row))
        assertEquals(places, source.senderDownloads)
        assertArrayEquals(B.png, images.images.value["ada@example.org"])
        // Once while the app runs, and from the cache after.
        images.onScreen(listOf(row))
        assertEquals(2, source.senderDownloads.size)
        val again = SenderImages(source, cache, prefs, CoroutineScope(Dispatchers.Unconfined))
        again.onScreen(listOf(row))
        assertEquals(2, source.senderDownloads.size)
        assertArrayEquals(B.png, again.images.value["ada@example.org"])
    }

    @Test
    fun anAddressWithNoPictureKeepsItsInitialsAndIsNotAskedAgain() {
        images.onScreen(listOf(row))
        assertTrue(images.images.value.containsKey("ada@example.org"))
        assertNull(images.images.value["ada@example.org"])
        val asked = source.senderDownloads.size
        images.onScreen(listOf(row))
        assertEquals(asked, source.senderDownloads.size)
    }

    @Test
    fun theRulesStandBetweenARowAndTheNetwork() {
        for (facts in listOf(
            MessageFacts("ada@example.org", junk = true, vouched = true, known = true),
            MessageFacts("ada@example.org", junk = false, vouched = false, known = true),
            MessageFacts("ada@example.org", junk = false, vouched = true, known = false),
        )) {
            source.factsAnswer = facts
            val one = SenderImages(source, PictureCache(File(dir, "x")), prefs, CoroutineScope(Dispatchers.Unconfined))
            one.onScreen(listOf(row))
            assertTrue("$facts", source.senderDownloads.isEmpty())
        }
        source.factsAnswer = null
        images.onScreen(listOf(row))
        assertTrue(source.senderDownloads.isEmpty())
    }

    @Test
    fun dataSavingShowsWhatIsKeptAndAsksNobodyAndIsAskedAgainAfter() {
        val places = source.senderPlaces("ada@example.org")
        source.senderWeb[places[0]] = B.png
        source.dataSaver = true
        images.onScreen(listOf(row))
        assertTrue(source.senderDownloads.isEmpty())
        assertFalse(images.images.value.containsKey("ada@example.org"))
        cache.put(places[0], B.png, ::looksLikeASenderPicture)
        images.onScreen(listOf(row))
        assertArrayEquals(B.png, images.images.value["ada@example.org"])
        assertTrue(source.senderDownloads.isEmpty())
    }

    @Test
    fun turningItOffForgetsWhatIsHeld() {
        source.senderWeb[source.senderPlaces("ada@example.org")[0]] = B.png
        images.onScreen(listOf(row))
        assertFalse(images.images.value.isEmpty())
        prefs.save(PicturePrefs(senderImages = false))
        images.reset()
        assertTrue(images.images.value.isEmpty())
        images.onScreen(listOf(row))
        assertTrue(images.images.value.isEmpty())
    }

    @Test
    fun rowsWithNoAddressAreLeftAlone() {
        images.onScreen(listOf(row.copy(address = ""), row.copy(address = "not an address")))
        assertTrue(source.senderDownloads.isEmpty())
    }
}
