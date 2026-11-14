// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.Duration
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** The address the provider's page sent the owner back to, as the system opened the app with it. */
internal object Redirect {
    /** The reverse-domain scheme registered with the provider; the Rust side holds the same words. */
    const val SCHEME = "work.mailcraft.android"

    /** What follows `SCHEME:` in the registered address (the Rust side's default path). */
    const val PATH = "/oauth2redirect"

    /** The longest address handed over; the Rust side refuses a longer one, so none is passed. */
    const val MAX_ADDRESS = 8 * 1024

    /**
     * [address] when it is the registered one (scheme and path to the letter, within the cap), else
     * null. This only keeps strangers' intents out of the queue; the Rust side checks the state, the
     * fields and the rest, strictly, and every address is hostile until it has.
     */
    fun accepted(address: String?): String? {
        if (address == null || address.length > MAX_ADDRESS) return null
        val head = "$SCHEME:$PATH"
        if (!address.startsWith(head)) return null
        val next = address.getOrNull(head.length)
        return if (next == null || next == '?' || next == '#') address else null
    }
}

/** What waiting for the owner's return came to. */
internal sealed interface Handed {
    /** The whole address the page came back to. */
    class Address(val address: String) : Handed

    /** The owner gave up, or the sign-in was cancelled. */
    data object Cancelled : Handed

    data object TimedOut : Handed
}

/**
 * Where the redirect lands. A sign-in [arm]s it, opens the page, and [await]s; the redirect's
 * activity [deliver]s what the system gave it. Only an armed sign-in takes an address, and only
 * the first: a second, or one with no sign-in waiting (another program may name our scheme), is
 * dropped. Waiting is for no longer than asked and stops at once when the flag is set, as the
 * core's hand-back asks.
 */
internal class SignInHandback {
    private val queue = LinkedBlockingQueue<String>(1)

    @Volatile
    private var armed = false

    /** A sign-in is about to open its page: forget anything left over and listen. */
    fun arm() {
        queue.clear()
        armed = true
    }

    /** Nobody waits any more. */
    fun disarm() {
        armed = false
        queue.clear()
    }

    /** What the redirect's activity got; false when it was not taken. */
    fun deliver(address: String?): Boolean {
        val accepted = Redirect.accepted(address)
        if (!armed || accepted == null) return false
        return queue.offer(accepted)
    }

    fun await(cancel: AtomicBoolean, timeout: Duration): Handed {
        val deadline = System.nanoTime() + timeout.toNanos().coerceAtLeast(0)
        try {
            var result: Handed? = null
            while (result == null) result = step(cancel, deadline)
            return result
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            return Handed.Cancelled
        } finally {
            disarm()
        }
    }

    // One short wait: the answer when there is one, null to go round again.
    private fun step(cancel: AtomicBoolean, deadline: Long): Handed? {
        if (cancel.get()) return Handed.Cancelled
        val left = deadline - System.nanoTime()
        if (left <= 0) return Handed.TimedOut
        val slice = minOf(left, TimeUnit.MILLISECONDS.toNanos(SLICE_MS))
        return queue.poll(slice, TimeUnit.NANOSECONDS)?.let { Handed.Address(it) }
    }

    private companion object {
        const val SLICE_MS = 100L
    }
}
