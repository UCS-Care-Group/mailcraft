// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import java.io.File
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.Core

/**
 * What the pictures of mail are made of, built once for a core: the owner's choices (on this device),
 * the cache in the app's cache folder, the reading pane's logic and the sender images'. One cache and
 * one set of choices serve both, as on the desktop.
 */
internal class PictureStack(
    core: Core,
    context: Context,
    private val scope: CoroutineScope,
    /** Where the cache's folder is read and written: never the main thread. */
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    val prefs: PicturePrefsStore = DevicePicturePrefs(context)
    val cache = PictureCache(File(context.cacheDir, FOLDER))
    private val source = CoreReadSource(core)
    val reading = ReadingController(source, cache, prefs, scope, io)
    val senderImages = SenderImages(source, cache, prefs, scope, reading::isAsked, io)

    private val state = MutableStateFlow(PicturesUi(prefs.load()))

    init {
        refreshUsage()
    }

    /** What Settings shows: the owner's choices and what the cache holds (the cache is counted off the main thread). */
    val settings: StateFlow<PicturesUi> get() = state

    private fun refreshUsage() {
        scope.launch {
            val (kept, bytes) = withContext(io) { cache.usage() }
            state.update { it.copy(kept = kept, keptBytes = bytes) }
        }
    }

    /** The owner changed a choice: what is open and what is on the list follow it. */
    fun changed(next: PicturePrefs) {
        val before = prefs.load()
        prefs.save(next)
        if (before.senderImages && !next.senderImages) senderImages.reset() else senderImages.redecide()
        reading.prefsChanged()
        state.update { it.copy(prefs = next) }
        refreshUsage()
    }

    /** Clear: the pictures kept go, and the senders' pictures held in memory with them. */
    fun clear() {
        senderImages.reset()
        scope.launch {
            withContext(io) { cache.clear() }
            refreshUsage()
        }
    }

    /** What the reading pane asks, answered by [reading], with links followed by [links]. */
    fun readingActions(
        links: LinkOpener,
        attachments: AttachmentActions = AttachmentActions.None,
        onLeft: () -> Unit = {},
        onCompose: (ComposeStart) -> Unit = {},
    ) = ReadingActions(
        onOpen = { id, width, dark -> reading.open(id, width, dark) },
        onClose = {
            reading.close(it)
            onLeft()
        },
        onAllow = {
            reading.allow(it)
            senderImages.redecide()
        },
        onDownloadNow = reading::downloadNow,
        onLink = links::open,
        attachments = attachments,
        onCompose = onCompose,
    )

    private companion object {
        const val FOLDER = "pictures.cache"
    }
}

/** What the reading pane is made of: pictures, the links it follows, and attachments (open, save, share). */
internal class ReadingStack(core: Core, context: Context, scope: CoroutineScope) {
    val pictures = PictureStack(core, context, scope)
    val attachments = AttachmentController(CoreAttachments(core), AndroidAttachmentDevice(context), scope)
    private val links = CoreLinks(core)

    /** What the reading pane asks; leaving a message lets go of what its attachments held. */
    fun actions(onCompose: (ComposeStart) -> Unit) =
        pictures.readingActions(links, attachments.actions(), attachments::reset, onCompose)
}
