// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

/**
 * Senders' pictures for the rows of the message list: a picture in place of the initials, only when
 * the owner turned sender images on, for the rows on screen, a few at a time, once while the app runs,
 * and under the desktop's rules ([PicturePolicy.senderImage]). Gravatar is asked first (told a hash of
 * the address), then favicon.im and Google (told the sender's site alone). With the system's data
 * saving on, only what is in the [cache] is shown and nothing is asked. Turning the switch off stops a
 * look-up before its next request; what it brings after that is not kept.
 *
 * The rules are applied to each row's own message, every time: a picture that one vouched message
 * brought is not drawn on another message "from" the same address that nothing vouches for, or that is
 * in Junk E-mail, so a forged sender never wears a real face. A picture is kept by address (it is the
 * address's), but whether a row shows it is the row's own.
 */
internal class SenderImages(
    private val source: ReadSource,
    private val cache: PictureCache,
    private val prefsStore: PicturePrefsStore,
    private val scope: CoroutineScope,
    private val asked: (ULong) -> Boolean = { false },
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    /** What the rules said of one row, and whose address it is. */
    private class Decision(val address: String, val allowed: Boolean)

    private val shown = MutableStateFlow<Map<ULong, ByteArray>>(emptyMap())
    private val flights = Semaphore(AT_A_TIME)
    private val generation = AtomicInteger()
    private val lock = Any()

    // Guarded by lock.
    private val pictures = LinkedHashMap<String, ByteArray?>()
    private val decisions = LinkedHashMap<ULong, Decision>()
    private val deciding = HashSet<ULong>()
    private val fetching = HashSet<String>()
    private var lastRows: List<InboxRow> = emptyList()

    /** By the row's id: the sender's picture, only for the rows whose own message may show it. */
    val images: StateFlow<Map<ULong, ByteArray>> get() = shown

    /** The rows now on screen: those not decided yet are looked at, if the owner allows. */
    fun onScreen(rows: List<InboxRow>) {
        synchronized(lock) { lastRows = rows.take(MAX_ROWS) }
        if (!prefsStore.load().senderImages) return
        val mine = generation.get()
        for (row in rows.take(MAX_ROWS)) {
            val address = PictureAddresses.sender(row.address) ?: continue
            if (claimRow(row.id)) scope.launch { decide(mine, row.id, address) }
        }
    }

    /** What the rules say may have changed (the owner allowed a message's pictures, or a choice changed): decide again. */
    fun redecide() {
        val rows = synchronized(lock) {
            decisions.clear()
            lastRows
        }
        shown.value = emptyMap()
        onScreen(rows)
    }

    /** The switch was turned off, or Clear was pressed: what is held goes, and what is in flight is not kept. */
    fun reset() {
        generation.incrementAndGet()
        synchronized(lock) {
            pictures.clear()
            decisions.clear()
            deciding.clear()
            fetching.clear()
        }
        shown.value = emptyMap()
    }

    private fun claimRow(id: ULong): Boolean = synchronized(lock) {
        if (id in decisions || id in deciding || deciding.size >= MAX_IN_FLIGHT) false else deciding.add(id)
    }

    private fun claimAddress(address: String): Boolean = synchronized(lock) {
        address !in pictures && fetching.size < MAX_IN_FLIGHT && fetching.add(address)
    }

    private suspend fun decide(mine: Int, id: ULong, address: String) {
        val facts = source.facts(id)
        val allowed = facts != null &&
            PicturePolicy.senderImage(prefsStore.load(), facts.copy(address = address), asked(id))
        val kept = synchronized(lock) {
            deciding.remove(id)
            if (mine != generation.get() || !prefsStore.load().senderImages) return
            decisions[id] = Decision(address, allowed)
            trimDecisions()
            address in pictures
        }
        if (!allowed || kept) return publish()
        if (!claimAddress(address)) return
        try {
            val answer = find(mine, address)
            // (What data saving kept from asking is asked again at the next look.)
            synchronized(lock) {
                if (answer.final && mine == generation.get() && prefsStore.load().senderImages) {
                    pictures[address] = answer.bytes
                    trimPictures()
                } else if (!answer.final) {
                    decisions.remove(id)
                }
            }
        } finally {
            synchronized(lock) { fetching.remove(address) }
        }
        publish()
    }

    /** The rows that may show a picture, with it: decided by their own message, held by their address. */
    private fun publish() {
        val now = synchronized(lock) {
            val out = HashMap<ULong, ByteArray>()
            for ((id, decision) in decisions) {
                val bytes = if (decision.allowed) pictures[decision.address] else null
                if (bytes != null) out[id] = bytes
            }
            out
        }
        shown.value = now
    }

    /** What a look-up came to: the picture or none; [final] unless data saving kept it from asking. */
    private class Answer(val bytes: ByteArray?, val final: Boolean)

    private suspend fun find(mine: Int, address: String): Answer {
        val places = source.senderPlaces(address)
        val kept = places.firstNotNullOfOrNull { withContext(io) { cache.get(it, ::looksLikeASenderPicture) } }
        if (kept != null || places.isEmpty()) return Answer(kept, final = true)
        // With data saving on only what is kept is shown; the rest is asked again at the next look.
        if (source.dataSaving()) return Answer(null, final = false)
        return Answer(askEach(mine, places), final = true)
    }

    /** Asks the places in turn until one has a picture, for as long as the owner wants sender images. */
    private suspend fun askEach(mine: Int, places: List<String>): ByteArray? {
        for (place in places) {
            if (mine != generation.get() || !prefsStore.load().senderImages) return null
            val got = flights.withPermit { source.downloadSender(place) }
            if (got is Fetched.Got && looksLikeASenderPicture(got.bytes) && got.bytes.size <= MAX_BYTES) {
                // (Not kept when the switch went off, or Clear was pressed, while it came.)
                if (mine == generation.get()) withContext(io) { cache.put(place, got.bytes, ::looksLikeASenderPicture) }
                return got.bytes
            }
        }
        return null
    }

    private fun trimPictures() {
        while (pictures.size > MAX_KEPT) pictures.remove(pictures.keys.first())
    }

    private fun trimDecisions() {
        while (decisions.size > MAX_DECISIONS) decisions.remove(decisions.keys.first())
    }

    private companion object {
        const val AT_A_TIME = 4
        const val MAX_IN_FLIGHT = 64
        const val MAX_ROWS = 64
        const val MAX_KEPT = 500
        const val MAX_DECISIONS = 2_000

        /** Largest picture taken: a sender's picture is small. */
        const val MAX_BYTES = 1024 * 1024
    }
}
