// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.security.GeneralSecurityException
import javax.crypto.Cipher
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Seals a secret and opens it again. What is sealed is bound to `aad` (the name the secret is
 * kept under), so a file copied over another name cannot be opened as that one.
 */
internal interface Sealer {
    @Throws(GeneralSecurityException::class)
    fun seal(aad: ByteArray, plain: ByteArray): ByteArray

    @Throws(GeneralSecurityException::class)
    fun open(aad: ByteArray, sealed: ByteArray): ByteArray
}

/**
 * AES-256-GCM over a key that is fetched for each call ([key]); on the phone the key is the
 * Keystore's and never leaves it. The sealed form is the version byte, the length of the
 * nonce, the nonce the cipher chose, and the ciphertext with its tag.
 */
internal class AesGcmSealer(private val key: () -> SecretKey) : Sealer {
    override fun seal(aad: ByteArray, plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(aad)
        val nonce = cipher.iv ?: throw GeneralSecurityException("no nonce")
        if (nonce.size !in 1..MAX_NONCE) throw GeneralSecurityException("odd nonce")
        val body = cipher.doFinal(plain)
        return byteArrayOf(VERSION, nonce.size.toByte()) + nonce + body
    }

    override fun open(aad: ByteArray, sealed: ByteArray): ByteArray {
        val version = sealed.getOrNull(0)
        val length = sealed.getOrNull(1)?.toInt() ?: 0
        if (version != VERSION || length !in 1..MAX_NONCE || sealed.size < HEADER + length + TAG_BYTES) {
            throw GeneralSecurityException("not a sealed secret")
        }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, sealed, HEADER, length))
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed, HEADER + length, sealed.size - HEADER - length)
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val VERSION: Byte = 1
        const val HEADER = 2
        const val MAX_NONCE = 32
        const val TAG_BITS = 128
        const val TAG_BYTES = 16
    }
}
