// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import java.security.GeneralSecurityException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeFalse
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import uniffi.mailcraft_mobile.HostException
import uniffi.mailcraft_mobile.TroubleKind

class SecretFilesTest {
    @get:Rule
    val tmp = TemporaryFolder()

    private fun newKey(): SecretKey = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()

    private fun store(key: SecretKey = newKey(), dir: File = File(tmp.root, "secrets")) =
        SecretFiles(dir, AesGcmSealer { key })

    @Test
    fun aSecretComesBack() {
        val s = store()
        s.set("refresh:ada@example.org", "tok\u00e9n-\u2026")
        assertEquals("tok\u00e9n-\u2026", s.get("refresh:ada@example.org"))
    }

    @Test
    fun noSecretIsNull() {
        assertNull(store().get("password:nobody@example.org"))
    }

    @Test
    fun aSecretIsReplacedAndDeleted() {
        val s = store()
        s.set("password:a@example.org", "one")
        s.set("password:a@example.org", "two")
        assertEquals("two", s.get("password:a@example.org"))
        s.delete("password:a@example.org")
        assertNull(s.get("password:a@example.org"))
        s.delete("password:a@example.org")
    }

    @Test
    fun nothingIsWrittenInTheClear() {
        val dir = File(tmp.root, "secrets")
        store(dir = dir).set("password:ada@example.org", "hunter2-hunter2")
        val files = dir.listFiles().orEmpty()
        assertEquals(1, files.size)
        assertFalse(files[0].name.contains("ada"))
        val bytes = files[0].readBytes()
        assertFalse(String(bytes, Charsets.ISO_8859_1).contains("hunter2"))
        assertTrue(dir.listFiles().orEmpty().none { it.name.endsWith(".tmp") })
    }

    @Test
    fun aFileCopiedOverAnotherNameIsNotOpened() {
        val dir = File(tmp.root, "secrets")
        val s = store(dir = dir)
        s.set("password:a@example.org", "alpha")
        s.set("password:b@example.org", "bravo")
        val files = dir.listFiles().orEmpty().sortedBy { it.name }
        files[0].copyTo(files[1], overwrite = true)
        val results = listOf("password:a@example.org", "password:b@example.org").map {
            runCatching { s.get(it) }
        }
        assertTrue(results.any { it.exceptionOrNull() is HostException.Failed })
        assertEquals(TroubleKind.UNAVAILABLE, s.takeTrouble())
        assertNull(s.takeTrouble())
    }

    @Test
    fun aDamagedFileIsAFailureNotACrash() {
        val dir = File(tmp.root, "secrets")
        val s = store(dir = dir)
        s.set("refresh:a@example.org", "alpha")
        val file = dir.listFiles().orEmpty().single()
        file.writeBytes(byteArrayOf(1, 12, 0, 0))
        assertThrows(HostException.Failed::class.java) { s.get("refresh:a@example.org") }
        file.writeBytes(ByteArray(0))
        assertThrows(HostException.Failed::class.java) { s.get("refresh:a@example.org") }
        s.takeTrouble()
        file.writeBytes(ByteArray(MAX_FILE_PLUS))
        assertThrows(HostException.Failed::class.java) { s.get("refresh:a@example.org") }
        assertEquals(TroubleKind.UNAVAILABLE, s.takeTrouble())
        assertNull(s.takeTrouble())
    }

    @Test
    fun aDifferentKeyCannotOpenIt() {
        val dir = File(tmp.root, "secrets")
        store(dir = dir).set("refresh:a@example.org", "alpha")
        assertThrows(HostException.Failed::class.java) { store(dir = dir).get("refresh:a@example.org") }
    }

    @Test
    fun aKeystoreThatFailsIsAFailure() {
        val broken = SecretFiles(File(tmp.root, "x"), AesGcmSealer { throw GeneralSecurityException("no") })
        assertThrows(HostException.Failed::class.java) { broken.set("k", "v") }
        assertEquals(TroubleKind.UNAVAILABLE, broken.takeTrouble())
    }

    @Test
    fun aSecretTooLongIsRefusedByName() {
        val s = store()
        assertThrows(HostException.TooLong::class.java) { s.set("k", "x".repeat(SecretFiles.MAX_SECRET + 1)) }
        s.set("k", "x".repeat(SecretFiles.MAX_SECRET))
    }

    @Test
    fun badKeysAreRefused() {
        val s = store()
        assertThrows(HostException.Failed::class.java) { s.set("", "v") }
        assertThrows(HostException.Failed::class.java) { s.get("k".repeat(SecretFiles.MAX_KEY + 1)) }
    }

    @Test
    fun aFolderThatCannotBeMadeIsAnIoError() {
        val blocker = tmp.newFile("blocker")
        val s = store(dir = File(blocker, "inside"))
        assertThrows(HostException.Io::class.java) { s.set("k", "v") }
    }

    @Test
    fun clearTakesEverythingAway() {
        val s = store()
        s.set("a", "1")
        s.set("b", "2")
        s.clear()
        assertNull(s.get("a"))
    }

    @Test
    fun leftoverTemporaryFilesAreSweptAtStart() {
        val dir = File(tmp.root, "secrets").apply { mkdirs() }
        val stray = File(dir, "abc.tmp").apply { writeBytes(byteArrayOf(1, 2, 3)) }
        store(dir = dir)
        assertFalse(stray.exists())
    }

    @Test
    fun aSealerThatThrowsAtRuntimeIsUnavailableNotACrash() {
        val broken = SecretFiles(File(tmp.root, "y"), AesGcmSealer { throw IllegalStateException("provider") })
        assertThrows(HostException.Failed::class.java) { broken.set("k", "v") }
        assertEquals(TroubleKind.UNAVAILABLE, broken.takeTrouble())
    }

    @Test
    fun aFileThatCannotBeReadIsStoreTroubleNotAMissingSecret() {
        val dir = File(tmp.root, "secrets")
        val s = store(dir = dir)
        s.set("k", "v")
        val file = dir.listFiles { f -> f.name.endsWith(".sec") }.orEmpty().single()
        file.setReadable(false, false)
        try {
            assumeFalse("running as a user that ignores file modes", file.canRead())
            assertThrows(HostException::class.java) { s.get("k") }
            assertEquals(TroubleKind.UNAVAILABLE, s.takeTrouble())
        } finally {
            file.setReadable(true, false)
        }
    }

    private companion object {
        const val MAX_FILE_PLUS = SecretFiles.MAX_SECRET + 1024
    }
}
