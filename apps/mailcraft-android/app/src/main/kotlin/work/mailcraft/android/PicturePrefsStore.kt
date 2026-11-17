// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context

/** Where the owner's say-so about pictures is kept: on this device only (nothing is backed up). */
internal interface PicturePrefsStore {
    fun load(): PicturePrefs

    fun save(prefs: PicturePrefs)
}

/** [PicturePrefsStore] in the app's private preferences. A store that cannot be read gives the defaults: all off. */
internal class DevicePicturePrefs(context: Context) : PicturePrefsStore {
    private val prefs = runCatching { context.getSharedPreferences(FILE, Context.MODE_PRIVATE) }.getOrNull()

    override fun load(): PicturePrefs = runCatching {
        PicturePrefs(
            all = prefs?.getBoolean(ALL, false) ?: false,
            senders = capped(prefs?.getStringSet(SENDERS, null)),
            domains = capped(prefs?.getStringSet(DOMAINS, null)),
            senderImages = prefs?.getBoolean(SENDER_IMAGES, false) ?: false,
        )
    }.getOrDefault(PicturePrefs())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    override fun save(prefs: PicturePrefs) {
        runCatching {
            this.prefs?.edit()
                ?.putBoolean(ALL, prefs.all)
                ?.putStringSet(SENDERS, prefs.senders.toSet())
                ?.putStringSet(DOMAINS, prefs.domains.toSet())
                ?.putBoolean(SENDER_IMAGES, prefs.senderImages)
                ?.apply()
        }
    }

    /** What was read, held to the cap and to what is an address. */
    private fun capped(kept: Set<String>?): Set<String> =
        kept.orEmpty().filter { it.length <= MAX_KEPT_LENGTH }.take(PicturePrefs.MAX_REMEMBERED).toSet()

    private companion object {
        const val FILE = "pictures"
        const val ALL = "download_all"
        const val SENDERS = "senders"
        const val DOMAINS = "domains"
        const val SENDER_IMAGES = "sender_images"
        const val MAX_KEPT_LENGTH = 320
    }
}

/** Keeps the choices in memory: for tests and screenshots. */
internal class MemoryPicturePrefs(private var prefs: PicturePrefs = PicturePrefs()) : PicturePrefsStore {
    override fun load(): PicturePrefs = prefs

    override fun save(prefs: PicturePrefs) {
        this.prefs = prefs
    }
}
