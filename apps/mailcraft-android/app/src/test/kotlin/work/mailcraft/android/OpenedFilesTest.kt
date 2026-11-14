// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.OutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.mailcraft_mobile.HostException

class OpenedFilesTest {
    @get:Rule
    val tmp = TemporaryFolder()

    @Test
    fun aNameIsOneSafeSegment() {
        assertEquals("report.pdf", safeFileName("report.pdf"))
        assertEquals("passwd", safeFileName("../../etc/passwd"))
        assertEquals("evil.txt", safeFileName("C:\\Users\\x\\evil.txt"))
        assertEquals("ab.txt", safeFileName("a\u0000b\n.txt"))
        assertEquals("attachment", safeFileName(""))
        assertEquals("attachment", safeFileName("..."))
        assertEquals("attachment", safeFileName("a/"))
        assertEquals("ab", safeFileName("a:b*?"))
    }

    @Test
    fun aLongNameKeepsItsExtension() {
        val name = safeFileName("x".repeat(500) + ".pdf")
        assertEquals(255, name.length)
        assertTrue(name.endsWith(".pdf"))
        assertEquals(255, safeFileName("y".repeat(500)).length)
        assertEquals(255, safeFileName("z".repeat(300) + "." + "e".repeat(50)).length)
    }

    @Test
    fun aLongNameIsBoundedByUtf8BytesAtACodePointBoundary() {
        val name = safeFileName("界".repeat(96) + ".pdf")
        assertTrue(name.toByteArray(Charsets.UTF_8).size <= 255)
        assertTrue(name.endsWith(".pdf"))
        assertTrue(name.startsWith("界"))
        // Four-byte code points are never split into a lone surrogate.
        val emoji = safeFileName("\uD83D\uDE00".repeat(100) + ".txt")
        assertTrue(emoji.toByteArray(Charsets.UTF_8).size <= 255)
        assertTrue(emoji.endsWith(".txt"))
        assertEquals(emoji, String(emoji.toByteArray(Charsets.UTF_8), Charsets.UTF_8))
        assertFalse(emoji.contains('\uFFFD'))
    }

    @Test
    fun installersAreRecognised() {
        listOf("a.apk", "A.APK", "b.apks", "c.xapk", "d.aab").forEach { assertTrue(it, isInstallable(it)) }
        listOf("a.pdf", "apk", "a.apk.txt", "").forEach { assertFalse(it, isInstallable(it)) }
    }

    @Test
    fun installersAndHugeFilesAreRefusedByName() {
        assertThrowsHost<HostException.Failed> { refuseUnlessOpenable("a.apk", 1) }
        assertThrowsHost<HostException.TooLong> { refuseUnlessOpenable("a.pdf", MAX_OPEN_BYTES + 1) }
        refuseUnlessOpenable("a.pdf", MAX_OPEN_BYTES)
    }

    @Test
    fun clearingTakesAFolderAndItsFilesAway() {
        val root = tmp.newFolder("attachments")
        File(root, "one/two").mkdirs()
        File(root, "one/two/f.txt").writeText("x")
        clearDirectory(root)
        assertFalse(root.exists())
        clearDirectory(root)
    }

    @Test
    fun savingWritesTheBytes() {
        val out = ByteArrayOutputStream()
        assertTrue(SaveToFiles.write({ out }, byteArrayOf(1, 2, 3)))
        assertEquals(listOf<Byte>(1, 2, 3), out.toByteArray().toList())
    }

    @Test
    fun savingWhereNothingOpensOrWritingFailsIsFalse() {
        assertFalse(SaveToFiles.write({ null }, byteArrayOf(1)))
        val failing = object : OutputStream() {
            override fun write(b: Int) = throw IOException("full")
        }
        assertFalse(SaveToFiles.write({ failing }, byteArrayOf(1)))
        assertFalse(SaveToFiles.write({ throw SecurityException("revoked") }, byteArrayOf(1)))
    }

    @Suppress("ThrowsCount")
    @Test
    fun guardTurnsWhatTheSystemThrowsIntoTheBindingsError() {
        assertThrowsHost<HostException.Failed> { guarded { error("boom $SECRET") } }
        assertThrowsHost<HostException.Io> { guarded { throw IOException("disk $SECRET") } }
        assertThrowsHost<HostException.TooLong> { guarded { throw HostException.TooLong() } }
        assertThrowsHost<HostException.Failed> { guarded { throw SecurityException("no") } }
        assertEquals(5, guarded { 5 })
    }

    private inline fun <reified T : HostException> assertThrowsHost(body: () -> Unit) {
        val thrown = runCatching(body).exceptionOrNull()
        assertTrue("got $thrown", thrown is T)
        assertFalse(thrown?.message.orEmpty().contains(SECRET))
    }

    private companion object {
        const val SECRET = "hunter2"
    }
}
