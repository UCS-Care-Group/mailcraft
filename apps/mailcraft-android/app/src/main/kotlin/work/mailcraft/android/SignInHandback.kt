// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicBoolean

/** The address the provider's page sent the owner back to, as the system opened the app with it. */
internal object Redirect {
    /** The reverse-domain scheme registered with the provider; the Rust side holds the same words. */
    const val SCHEME = "work.mailcraft.android"

    /**
     * What follows `SCHEME://` in the registered address: a host and no path (the Rust side's
     * default host). Microsoft Entra only accepts the `//` form, and matches it to the letter.
     */
    const val HOST = "oauth2redirect"

    /** The longest address handed over; the Rust side refuses a longer one, so none is passed. */
    const val MAX_ADDRESS = 8 * 1024

    /**
     * [address] when it is the registered one (scheme and host to the letter, within the cap), else
     * null. This only keeps strangers' intents out of the queue; the Rust side checks the state, the
     * fields and the rest, strictly, and every address is hostile until it has.
     */
    fun accepted(address: String?): String? {
        if (address == null || address.length > MAX_ADDRESS) return null
        val head = "$SCHEME://$HOST"
        if (!address.startsWith(head)) return null
        // Entra sends a registration with no path back with one `/` after the host.
        val at = if (address.getOrNull(head.length) == '/') head.length + 1 else head.length
        val next = address.getOrNull(at)
        return if (next == null || next == '?' || next == '#') address else null
    }
}

/**
 * Where the redirect lands, on the way to the core. A sign-in [arm]s it before its page opens; the
 * redirect's activity [deliver]s what the system gave it, and the address goes, whole and unread, to
 * [sink] (the core's `handBackRedirect`), where the strategy in Rust judges all of it: the scheme, the
 * path, the state, the fields. Only an armed sign-in takes an address, and only the first: a second,
 * or one with no sign-in waiting (another program may name our scheme), is dropped here and costs
 * nothing. The state check is not here and must not be: Kotlin never holds it.
 */
internal class SignInHandback {
    private val armed = AtomicBoolean(false)

    /** Where an accepted address goes; false from it means no sign-in took it. Set once the core exists. */
    @Volatile
    var sink: (String) -> Boolean = { false }

    /** A sign-in is about to open its page: the next address is for it. */
    fun arm() {
        armed.set(true)
    }

    /** Nobody waits any more. */
    fun disarm() {
        armed.set(false)
    }

    /** What the redirect's activity got; false when it was not taken. */
    fun deliver(address: String?): Boolean {
        val accepted = Redirect.accepted(address) ?: return false
        // (Only the first: whoever clears the flag is the one that passes the address on.)
        if (!armed.compareAndSet(true, false)) return false
        return sink(accepted)
    }
}
