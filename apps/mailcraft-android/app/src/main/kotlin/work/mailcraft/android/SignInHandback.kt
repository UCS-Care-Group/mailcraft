// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
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
internal class SignInHandback(private val pending: PendingSignIn? = null) {
    private val armed = AtomicBoolean(false)

    /** This process started a sign-in or grant that has not ended: its recovery record is not lost. */
    private val live = AtomicBoolean(false)

    /** Where an accepted address goes; false from it means no sign-in took it. Set once the core exists. */
    @Volatile
    var sink: (String) -> Boolean = { false }

    /**
     * Told the address of a sign-in that was under way when the system closed the app, once, when
     * its redirect arrives to a process that knows nothing of it.
     */
    @Volatile
    var interrupted: (String) -> Unit = {}

    /** A sign-in for [email] begins: should the app be closed meanwhile, the next start knows. */
    fun remember(email: String) {
        live.set(true)
        pending?.save(email)
    }

    /** A sign-in is about to open its page: the next address is for it. */
    fun arm() {
        live.set(true)
        armed.set(true)
    }

    /** Nobody waits any more. */
    fun disarm() {
        live.set(false)
        armed.set(false)
        pending?.clear()
    }

    /** What the redirect's activity got; false when it was not taken. */
    fun deliver(address: String?): Boolean {
        val accepted = Redirect.accepted(address) ?: return false
        // (Only the first: whoever clears the flag is the one that passes the address on.)
        if (!armed.compareAndSet(true, false)) {
            // (None waits here. If one did before the system closed the app, it is told, once; a stranger's
            // address finds no such record and costs nothing.)
            // (Not while this process's own sign-in is still under way, e.g. past its first redirect:
            // its record stays for a later process death.)
            if (!live.get()) pending?.take()?.let(interrupted)
            return false
        }
        return sink(accepted)
    }
}

/**
 * The record that a sign-in was under way: the address it is for, in a file in the app's private
 * no-backup storage, and nothing else (no state, no verifier, no token: the Rust side keeps those in
 * memory, so a sign-in the system cut short can't be finished, only started again). It is made when the
 * sign-in begins and removed when it ends; one still there at the next start is a lost sign-in. A
 * record is believed for ten minutes (the sign-in itself waits five).
 */
internal class PendingSignIn(private val file: File, private val now: () -> Long = System::currentTimeMillis) {
    fun save(email: String) {
        try {
            file.parentFile?.mkdirs()
            file.writeText(email.take(MAX_EMAIL))
        } catch (_: java.io.IOException) {
            // No record: a lost sign-in is then dropped as before.
        }
    }

    fun clear() {
        try {
            file.delete()
        } catch (_: SecurityException) {
            // Left; it ages out.
        }
    }

    /** The address of the lost sign-in, once: the record is taken. Null when there is none or it is too old. */
    fun take(): String? =
        try {
            if (!file.isFile) {
                null
            } else {
                val fresh = now() - file.lastModified() in 0..MAX_AGE_MILLIS
                val email = file.readText().take(MAX_EMAIL)
                file.delete()
                email.takeIf { fresh }
            }
        } catch (_: java.io.IOException) {
            null
        } catch (_: SecurityException) {
            null
        }

    private companion object {
        const val MAX_EMAIL = 254
        const val MAX_AGE_MILLIS = 10 * 60 * 1000L
    }
}
