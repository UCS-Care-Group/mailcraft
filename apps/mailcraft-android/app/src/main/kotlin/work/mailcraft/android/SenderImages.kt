// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit

/**
 * Senders' pictures for the rows of the message list: a picture in place of the initials, only when
 * the owner turned sender images on, for the rows on screen, a few at a time, once while the app runs,
 * and under the desktop's rules ([PicturePolicy.senderImage]). Gravatar is asked first (told a hash of
 * the address), then favicon.im and Google (told the sender's site alone). With the system's data
 * saving on, only what is in the [cache] is shown and nothing is asked. Turning the switch off stops a
 * look-up before its next request; what it brings after that is not kept.
 */
internal class SenderImages(
    private val source: ReadSource,
    private val cache: PictureCache,
    private val prefsStore: PicturePrefsStore,
    private val scope: CoroutineScope,
    private val asked: (ULong) -> Boolean = { false },
) {
    private val state = MutableStateFlow<Map<String, ByteArray?>>(emptyMap())
    private val flights = Semaphore(AT_A_TIME)
    private val inFlight = LinkedHashSet<String>()
    private val generation = AtomicInteger()

    /** By the sender's address in lower case: the picture's bytes, or null once it is known there is none. */
    val images: StateFlow<Map<String, ByteArray?>> get() = state

    /** The rows now on screen: those whose sender is not known yet are looked up, if the owner allows. */
    fun onScreen(rows: List<InboxRow>) {
        if (!prefsStore.load().senderImages) return
        val mine = generation.get()
        for (row in rows.take(MAX_ROWS)) {
            val address = PictureAddresses.sender(row.address)
            if (address != null && claim(address)) scope.launch { lookUp(mine, row.id, address) }
        }
    }

    /** The switch was turned off, or Clear was pressed: what is held goes, and what is in flight is not kept. */
    fun reset() {
        generation.incrementAndGet()
        synchronized(inFlight) { inFlight.clear() }
        state.value = emptyMap()
    }

    private fun claim(address: String): Boolean {
        if (address in state.value) return false
        synchronized(inFlight) {
            if (address in inFlight || inFlight.size >= MAX_IN_FLIGHT) return false
            inFlight.add(address)
        }
        return true
    }

    private suspend fun lookUp(mine: Int, id: ULong, address: String) {
        val answer = try {
            find(mine, id, address)
        } finally {
            synchronized(inFlight) { inFlight.remove(address) }
        }
        // (What data saving kept from asking is asked again at the next look.)
        if (answer.final && mine == generation.get() && prefsStore.load().senderImages) {
            state.update { held -> trimmed(held) + (address to answer.bytes) }
        }
    }

    /** What a look-up came to: the picture or none; [final] unless data saving kept it from asking. */
    private class Answer(val bytes: ByteArray?, val final: Boolean)

    private suspend fun find(mine: Int, id: ULong, address: String): Answer {
        val facts = source.facts(id)
        val allowed = facts != null &&
            PicturePolicy.senderImage(prefsStore.load(), facts.copy(address = address), asked(id))
        val places = if (allowed) source.senderPlaces(address) else emptyList()
        val kept = places.firstNotNullOfOrNull { cache.get(it, ::looksLikeASenderPicture) }
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
                if (mine == generation.get()) cache.put(place, got.bytes, ::looksLikeASenderPicture)
                return got.bytes
            }
        }
        return null
    }

    private fun trimmed(held: Map<String, ByteArray?>): Map<String, ByteArray?> =
        if (held.size < MAX_KEPT) held else held.entries.drop(held.size - MAX_KEPT / 2).associate { it.toPair() }

    private companion object {
        const val AT_A_TIME = 4
        const val MAX_IN_FLIGHT = 64
        const val MAX_ROWS = 64
        const val MAX_KEPT = 1_000

        /** Largest picture taken: a sender's picture is small. */
        const val MAX_BYTES = 1024 * 1024
    }
}
