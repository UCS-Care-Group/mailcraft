// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import uniffi.mailcraft_mobile.Core

/**
 * What the pictures of mail are made of, built once for a core: the owner's choices (on this device),
 * the cache in the app's cache folder, the reading pane's logic and the sender images'. One cache and
 * one set of choices serve both, as on the desktop.
 */
internal class PictureStack(core: Core, context: Context, scope: CoroutineScope) {
    val prefs: PicturePrefsStore = DevicePicturePrefs(context)
    val cache = PictureCache(File(context.cacheDir, FOLDER))
    private val source = CoreReadSource(core)
    val reading = ReadingController(source, cache, prefs, scope)
    val senderImages = SenderImages(source, cache, prefs, scope, reading::isAsked)

    private val state = MutableStateFlow(snapshot())

    /** What Settings shows: the owner's choices and what the cache holds. */
    val settings: StateFlow<PicturesUi> get() = state

    private fun snapshot() = PicturesUi(prefs.load(), cache.usage().first, cache.usage().second)

    /** The owner changed a choice: what is open and what is on the list follow it. */
    fun changed(next: PicturePrefs) {
        val before = prefs.load()
        prefs.save(next)
        if (before.senderImages && !next.senderImages) senderImages.reset()
        reading.prefsChanged()
        state.value = snapshot()
    }

    /** Clear: the pictures kept go, and the senders' pictures held in memory with them. */
    fun clear() {
        cache.clear()
        senderImages.reset()
        state.value = snapshot()
    }

    /** What the reading pane asks, answered by [reading], with links followed by [links]. */
    fun readingActions(links: LinkOpener) = ReadingActions(
        onOpen = { id, width, dark -> reading.open(id, width, dark) },
        onAllow = reading::allow,
        onDownloadNow = reading::downloadNow,
        onLink = links::open,
    )

    private companion object {
        const val FOLDER = "pictures.cache"
    }
}
