// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import java.io.IOException
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.security.GeneralSecurityException
import java.security.MessageDigest
import java.util.Locale
import java.util.UUID
import uniffi.mailcraft_mobile.HostException
import uniffi.mailcraft_mobile.SecretStore
import uniffi.mailcraft_mobile.TroubleKind

/**
 * The credential store: each secret is one file in [dir] (the app's no-backup folder, so it is
 * never copied to the cloud or to another phone), sealed by [sealer]. The file is named by a
 * hash of the key, so that no address is written anywhere in the clear. The store never asks
 * for the screen to be unlocked.
 */
internal class SecretFiles(private val dir: File, private val sealer: Sealer) : SecretStore {
    @Volatile
    private var trouble: TroubleKind? = null

    init {
        sweepTemporaries()
    }

    override fun get(key: String): String? = guarded {
        val file = fileFor(key)
        if (!file.isFile) return@guarded null
        if (file.length() > MAX_FILE) {
            trouble = TroubleKind.UNAVAILABLE
            throw HostException.Failed()
        }
        val sealed = try {
            file.readBytes()
        } catch (e: IOException) {
            // A file that is there and cannot be read is a store in trouble, not a missing credential.
            trouble = TroubleKind.UNAVAILABLE
            throw e
        }
        try {
            String(sealer.open(aadOf(key), sealed), StandardCharsets.UTF_8)
        } catch (_: GeneralSecurityException) {
            trouble = TroubleKind.UNAVAILABLE
            throw HostException.Failed()
        } catch (_: RuntimeException) {
            trouble = TroubleKind.UNAVAILABLE
            throw HostException.Failed()
        }
    }

    override fun set(key: String, secret: String) {
        guarded {
            val plain = secret.toByteArray(StandardCharsets.UTF_8)
            if (plain.size > MAX_SECRET) throw HostException.TooLong()
            val file = fileFor(key)
            val sealed = try {
                sealer.seal(aadOf(key), plain)
            } catch (_: GeneralSecurityException) {
                trouble = TroubleKind.UNAVAILABLE
                throw HostException.Failed()
            } catch (_: RuntimeException) {
                // A provider or StrongBox failure: the Keystore is what is broken, not the secret.
                trouble = TroubleKind.UNAVAILABLE
                throw HostException.Failed()
            }
            if (!dir.isDirectory && !dir.mkdirs() && !dir.isDirectory) throw IOException("no folder")
            // Written beside the file and moved over it, so that a crash never leaves half a secret.
            val temp = File(dir, "${UUID.randomUUID()}.tmp")
            try {
                temp.writeBytes(sealed)
                Files.move(temp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE)
            } finally {
                temp.delete()
            }
        }
    }

    override fun delete(key: String) = guarded {
        val file = fileFor(key)
        if (file.exists() && !file.delete()) throw IOException("not deleted")
    }

    override fun takeTrouble(): TroubleKind? {
        val seen = trouble
        trouble = null
        return seen
    }

    private fun fileFor(key: String): File {
        if (key.isEmpty() || key.length > MAX_KEY) throw HostException.Failed()
        return File(dir, "${hex(sha256(aadOf(key)))}.sec")
    }

    private fun aadOf(key: String): ByteArray = key.toByteArray(StandardCharsets.UTF_8)

    private fun sha256(bytes: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").digest(bytes)

    private fun hex(bytes: ByteArray): String = bytes.joinToString("") { String.format(Locale.ROOT, "%02x", it) }

    // A write killed between the temporary file and the move leaves sealed bytes behind: take them away.
    private fun sweepTemporaries() {
        try {
            dir.listFiles { f -> f.name.endsWith(".tmp") }?.forEach { it.delete() }
        } catch (_: RuntimeException) {
            // Best effort: a folder that cannot be listed is dealt with when it is used.
        }
    }

    /** Takes away every file of a store (the owner removed the last account, or reset the app). */
    internal fun clear() {
        dir.listFiles()?.forEach { it.delete() }
    }

    internal companion object {
        /** The longest secret kept: a refresh token is about 2 KB. */
        const val MAX_SECRET = 16 * 1024
        const val MAX_KEY = 512
        private const val MAX_FILE = MAX_SECRET + 256
    }
}
