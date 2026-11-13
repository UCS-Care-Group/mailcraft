// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.BlockKindView
import uniffi.mailcraft_mobile.BlockView
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.LinkKind
import uniffi.mailcraft_mobile.MessageView

/** Where a followed link goes. */
internal enum class LinkGoes { Browser, Composer, Dialer, Part }

/** A link of a message that may be followed: where it goes, and its address as it is shown and launched. */
internal data class LinkInfo(val goes: LinkGoes, val shown: String)

/** A message as the reading pane gets it: its view and its links. */
internal class OpenedMessage(val view: MessageView, val links: Map<String, LinkInfo>)

/**
 * What the reading pane needs of the core, so that its logic runs without the native library. Calls
 * may take long and are made off the main thread; a failure is an answer (null, false), never a throw.
 */
internal interface ReadSource {
    /** Message [id] read for a body [widthDp] wide in a dark or light window. */
    suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage?

    /** Marks [id] read; whether it was done. */
    suspend fun markRead(id: ULong): Boolean
}

/** Most links of one message that are looked at for their destination. */
private const val MAX_LINKS = 2_000

/** [ReadSource] over the Rust core. */
internal class CoreReadSource(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
) : ReadSource {
    override suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage? = withContext(dispatcher) {
        try {
            val view = core.message(id, widthDp.coerceIn(MIN_WIDTH, MAX_WIDTH).toFloat(), dark)
            OpenedMessage(view, linksOf(view.blocks))
        } catch (_: CoreException) {
            null
        }
    }

    override suspend fun markRead(id: ULong): Boolean = withContext(dispatcher) {
        try {
            core.execute("mail.markRead", jsonText(mapOf("ids" to listOf(id))))
            true
        } catch (_: CoreException) {
            false
        }
    }

    private fun linksOf(blocks: List<BlockView>): Map<String, LinkInfo> {
        val found = LinkedHashMap<String, LinkInfo>()
        fun add(url: String?) {
            if (url == null || found.size >= MAX_LINKS || url in found) return
            val target = uniffi.mailcraft_mobile.linkTarget(url) ?: return
            found[url] = LinkInfo(target.kind.goes(), target.shown)
        }
        walkLinks(blocks, ::add)
        return found
    }

    private companion object {
        const val MIN_WIDTH = 100
        const val MAX_WIDTH = 4_000
    }
}

internal fun LinkKind.goes() = when (this) {
    LinkKind.WEB -> LinkGoes.Browser
    LinkKind.MAIL -> LinkGoes.Composer
    LinkKind.PHONE -> LinkGoes.Dialer
    LinkKind.PART -> LinkGoes.Part
}

/** Calls [found] with every link in [blocks], the spans' and the pictures', inside tables too. */
internal fun walkLinks(blocks: List<BlockView>, found: (String?) -> Unit) {
    for (block in allBlocks(blocks)) {
        block.spans.forEach { found(it.link) }
        (block.kind as? BlockKindView.Image)?.let { found(it.link) }
    }
}
