// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.security.ProviderException
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The one AES key that seals the owner's secrets, in the Android Keystore (in the StrongBox
 * chip when the device has one). It is made to work whenever the phone is on: it asks for no
 * unlock of the screen and no authentication (a background sync that runs before the first
 * unlock after a restart must still be able to open the refresh token), and it is never exported.
 */
internal class KeystoreKey(private val alias: String = ALIAS) {
    /** The key, made on first use. Fails with a [GeneralSecurityException] when the Keystore won't. */
    @Throws(GeneralSecurityException::class)
    fun get(): SecretKey = synchronized(LOCK) { lookupOrCreate() }

    private fun lookupOrCreate(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER)
        store.load(null)
        val existing = store.getKey(alias, null)
        if (existing is SecretKey) return existing
        return try {
            generate(strongBox = true)
        } catch (_: StrongBoxUnavailableException) {
            generate(strongBox = false)
        } catch (_: ProviderException) {
            // Some devices say so by a plain ProviderException; the key in the TEE is as good.
            generate(strongBox = false)
        }
    }

    private fun generate(strongBox: Boolean): SecretKey {
        val spec = KeyGenParameterSpec.Builder(
            alias,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(KEY_BITS)
            .setIsStrongBoxBacked(strongBox)
            .build()
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER)
        generator.init(spec)
        return generator.generateKey()
    }

    private companion object {
        /** One lock for every instance: lookup and creation must not interleave, whoever asks. */
        val LOCK = Any()
        const val PROVIDER = "AndroidKeyStore"
        const val ALIAS = "work.mailcraft.secrets.v1"
        const val KEY_BITS = 256
    }
}
