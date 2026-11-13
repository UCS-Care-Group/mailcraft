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
import uniffi.mailcraft_mobile.NetworkKind

/** Where a followed link goes. */
internal enum class LinkGoes { Browser, Composer, Dialer, Part }

/** A link of a message that may be followed: where it goes, and its address as it is shown and launched. */
internal data class LinkInfo(val goes: LinkGoes, val shown: String)

/** A message as the reading pane gets it: its view, what the rules need of its sender, and its links. */
internal class OpenedMessage(val view: MessageView, val facts: MessageFacts, val links: Map<String, LinkInfo>)

/** What a download came to. */
internal sealed interface Fetched {
    class Got(val bytes: ByteArray) : Fetched

    /** It was refused or failed; [why] is the program's own words, in English. */
    class Refused(val why: String) : Fetched
}

/**
 * What the reading pane needs of the core, so that its logic runs without the native library. Calls
 * may take long and are made off the main thread; a failure is an answer (null, Refused), never a throw.
 */
internal interface ReadSource {
    /** Message [id] read for a body [widthDp] wide in a dark or light window. */
    suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage?

    /** The bytes of a picture that is part of message [id] (by the reference a block names). */
    suspend fun part(id: ULong, reference: String): ByteArray?

    /** Downloads a picture from the web, through the checked downloader and nothing else. */
    suspend fun download(url: String): Fetched

    /** The facts the rules for message [id]'s sender need. */
    suspend fun facts(id: ULong): MessageFacts?

    /** Where a picture of [address] may be had, in the order asked. */
    fun senderPlaces(address: String): List<String>

    /** Downloads a sender's picture from one of [senderPlaces]. */
    suspend fun downloadSender(url: String): Fetched

    /** Marks [id] read; whether it was done. */
    suspend fun markRead(id: ULong): Boolean

    /** Whether the system's data saving holds back this connection. */
    suspend fun dataSaving(): Boolean
}

/** Most links of one message that are looked at for their destination. */
private const val MAX_LINKS = 2_000

/** [ReadSource] over the Rust core. */
internal class CoreReadSource(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) : ReadSource {
    override suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage? = withContext(dispatcher) {
        try {
            val view = core.message(id, widthDp.coerceIn(MIN_WIDTH, MAX_WIDTH).toFloat(), dark)
            // What the rules need of the sender: when the core cannot say, the safe answer (Junk E-mail, not vouched).
            val facts = try {
                core.senderFacts(id).toFacts()
            } catch (_: CoreException) {
                MessageFacts(view.from.address, junk = true, vouched = false, known = false)
            }
            OpenedMessage(view, facts, linksOf(view.blocks))
        } catch (_: CoreException) {
            null
        }
    }

    override suspend fun part(id: ULong, reference: String): ByteArray? = withContext(dispatcher) {
        try {
            core.picture(id, reference).bytes
        } catch (_: CoreException) {
            null
        }
    }

    override suspend fun download(url: String): Fetched = withContext(io) {
        try {
            Fetched.Got(uniffi.mailcraft_mobile.fetchPicture(url).bytes)
        } catch (e: CoreException) {
            Fetched.Refused(e.message.orEmpty())
        }
    }

    override suspend fun facts(id: ULong): MessageFacts? = withContext(dispatcher) {
        try {
            core.senderFacts(id).toFacts()
        } catch (_: CoreException) {
            null
        }
    }

    override fun senderPlaces(address: String): List<String> = uniffi.mailcraft_mobile.senderPictureUrls(address)

    override suspend fun downloadSender(url: String): Fetched = withContext(io) {
        try {
            Fetched.Got(uniffi.mailcraft_mobile.fetchSenderPicture(url).bytes)
        } catch (e: CoreException) {
            Fetched.Refused(e.message.orEmpty())
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

    override suspend fun dataSaving(): Boolean = withContext(dispatcher) {
        (core.network() as? NetworkKind.Online)?.dataSaving == true
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

/** What the rules need, or the safest facts (Junk, nobody vouches) when the core could not say. */
internal fun uniffi.mailcraft_mobile.SenderFacts?.toFacts() =
    this?.let { MessageFacts(it.address, it.junk, it.vouched, it.known) }
        ?: MessageFacts("", junk = true, vouched = false, known = false)

/** Calls [found] with every link in [blocks], the spans' and the pictures', inside tables too. */
internal fun walkLinks(blocks: List<BlockView>, found: (String?) -> Unit) {
    for (block in allBlocks(blocks)) {
        block.spans.forEach { found(it.link) }
        (block.kind as? BlockKindView.Image)?.let { found(it.link) }
    }
}
