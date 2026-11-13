// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import java.io.File
import java.nio.file.Files
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PictureCacheTest {
    private val dir: File = Files.createTempDirectory("pictures").toFile()
    private var now = 1_000_000_000_000L
    private val url = "https://img.shop.example/banner.png?secret=1"

    @After
    fun clean() {
        dir.deleteRecursively()
    }

    /** A valid PNG header and a tail, [extra] bytes long in all. */
    private fun png(extra: Int = 0, mark: Int = 0): ByteArray =
        B.png.copyOf(B.png.size + extra).also { if (extra > 0) it[it.size - 1] = mark.toByte() }

    private fun cache(maxPictures: Int = PictureCache.MAX_PICTURES, maxTotal: Long = PictureCache.MAX_TOTAL) =
        PictureCache(dir, { now }, maxPictures, maxTotal)

    @Test
    fun theCapsAreTheDesktops() {
        assertEquals(4_000, PictureCache.MAX_PICTURES)
        assertEquals(256L * 1024 * 1024, PictureCache.MAX_TOTAL)
        assertEquals(30L * 86_400_000L, PictureCache.MAX_AGE_MILLIS)
        assertEquals(8 * 1024 * 1024, PictureCache.MAX_PICTURE)
    }

    @Test
    fun aPictureComesBackAndIsNamedByAHashNeverItsAddress() {
        val c = cache()
        assertTrue(c.put(url, png()))
        assertArrayEquals(png(), c.get(url))
        val names = dir.listFiles().orEmpty().map { it.name }
        assertEquals(1, names.size)
        assertTrue(names.single().matches(Regex("[0-9a-f]{64}")))
        assertFalse(dir.walkTopDown().any { f -> f.isFile && f.readBytes().toString(Charsets.ISO_8859_1).contains("shop.example") })
        assertNull(c.get("https://img.shop.example/other.png"))
    }

    @Test
    fun whatIsKeptSurvivesARestart() {
        assertTrue(cache().put(url, png()))
        assertNotNull(cache().get(url))
        assertEquals(1 to png().size.toLong(), cache().usage())
    }

    @Test
    fun onlyPicturesAreKeptAndOnlyWithinTheSizeCap() {
        val c = cache()
        assertFalse(c.put(url, "<html>not a picture</html>".toByteArray()))
        assertFalse(c.put(url, ByteArray(0)))
        assertFalse(c.put(url, png(PictureCache.MAX_PICTURE)))
        assertFalse(c.put("https://x.example/" + "a".repeat(20_000), png()))
        assertEquals(0 to 0L, c.usage())
        assertTrue(c.put(url, byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 0)))
        assertTrue(c.put("$url#gif", "GIF89a....".toByteArray()))
        assertTrue(c.put("$url#webp", "RIFF\u0000\u0000\u0000\u0000WEBPVP8 ".toByteArray(Charsets.ISO_8859_1)))
        assertFalse(c.put("$url#ico", byteArrayOf(0, 0, 1, 0, 1, 0, 0, 0)))
        assertTrue(c.put("$url#ico", byteArrayOf(0, 0, 1, 0, 1, 0, 0, 0), ::looksLikeASenderPicture))
    }

    @Test
    fun aFileThatIsNoLongerAPictureIsNotReadBackAndIsRemoved() {
        val c = cache()
        c.put(url, png())
        val file = dir.listFiles().orEmpty().single()
        file.writeBytes("<script>alert(1)</script>".toByteArray())
        assertNull(c.get(url))
        assertFalse(file.exists())
        assertEquals(0 to 0L, c.usage())
    }

    @Test
    fun aPictureOlderThanThirtyDaysIsGone() {
        val c = cache()
        c.put(url, png())
        now += PictureCache.MAX_AGE_MILLIS
        assertNotNull(c.get(url))
        now += 1
        assertNull(c.get(url))
        assertTrue(dir.listFiles().orEmpty().isEmpty())
    }

    @Test
    fun theOldestGoFirstWhenThereAreTooMany() {
        val c = cache(maxPictures = 3)
        for (n in 1..5) {
            now += 1_000
            assertTrue(c.put("https://x.example/$n.png", png()))
        }
        assertEquals(3, c.usage().first)
        assertNull(c.get("https://x.example/1.png"))
        assertNull(c.get("https://x.example/2.png"))
        for (n in 3..5) assertNotNull(c.get("https://x.example/$n.png"))
    }

    @Test
    fun theOldestGoFirstWhenTheyTakeTooMuch() {
        val one = png(1000).size.toLong()
        val c = cache(maxTotal = one * 2 + 10)
        for (n in 1..4) {
            now += 1_000
            assertTrue(c.put("https://x.example/$n.png", png(1000, n)))
        }
        assertEquals(2, c.usage().first)
        assertTrue(c.usage().second <= one * 2 + 10)
        assertNull(c.get("https://x.example/1.png"))
        assertNotNull(c.get("https://x.example/4.png"))
    }

    @Test
    fun theWholeDesktopCountIsHeld() {
        val c = cache()
        val tiny = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte(), 1)
        for (n in 0 until PictureCache.MAX_PICTURES + 3) {
            now += 1
            c.put("https://x.example/$n", tiny)
        }
        assertEquals(PictureCache.MAX_PICTURES, c.usage().first)
        assertNull(c.get("https://x.example/0"))
        assertNotNull(c.get("https://x.example/${PictureCache.MAX_PICTURES + 2}"))
    }

    @Test
    fun clearEmptiesTheFolderAndForeignFilesAndLeftoversAreSwept() {
        val c = cache()
        c.put(url, png())
        File(dir, "leftover.tmp").writeText("x")
        File(dir, "notes.txt").writeText("x")
        val fresh = cache()
        assertEquals(1, fresh.usage().first)
        assertEquals(listOf(1), listOf(dir.listFiles().orEmpty().size))
        assertTrue(fresh.clear())
        assertEquals(0 to 0L, fresh.usage())
        assertTrue(dir.listFiles().orEmpty().isEmpty())
        assertNull(fresh.get(url))
    }

    @Test
    fun aFolderThatCannotBeUsedIsACacheThatIsAlwaysEmpty() {
        val blocked = File(dir, "file")
        blocked.writeText("a file where the folder should be")
        val c = PictureCache(File(blocked, "inside"), { now })
        assertFalse(c.put(url, png()))
        assertNull(c.get(url))
        assertEquals(0 to 0L, c.usage())
    }
}
