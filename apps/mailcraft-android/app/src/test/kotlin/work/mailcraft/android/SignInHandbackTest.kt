// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.time.Duration
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SignInHandbackTest {
    private val good = "work.mailcraft.android:/oauth2redirect?code=c&state=s"

    @Test
    fun onlyTheRegisteredAddressIsAccepted() {
        assertEquals(good, Redirect.accepted(good))
        val bare = "work.mailcraft.android:/oauth2redirect"
        assertEquals(bare, Redirect.accepted(bare))
        assertEquals("$bare#x", Redirect.accepted("$bare#x"))
        assertNull(Redirect.accepted(null))
        assertNull(Redirect.accepted(""))
        assertNull(Redirect.accepted("work.mailcraft.android:/oauth2redirectx?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android:/other?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android://oauth2redirect?code=c"))
        assertNull(Redirect.accepted("WORK.mailcraft.android:/oauth2redirect"))
        assertNull(Redirect.accepted("https://example.org/oauth2redirect"))
        assertNull(Redirect.accepted(good + "x".repeat(Redirect.MAX_ADDRESS)))
    }

    @Test
    fun anAddressWithNoSignInWaitingIsDropped() {
        val h = SignInHandback()
        assertFalse(h.deliver(good))
        h.arm()
        assertTrue(h.deliver(good))
        assertFalse("only the first is taken", h.deliver(good))
    }

    @Test
    fun theWaitGetsTheAddress() {
        val h = SignInHandback()
        h.arm()
        thread { h.deliver(good) }
        val got = h.await(AtomicBoolean(false), Duration.ofSeconds(5))
        assertEquals(good, (got as Handed.Address).address)
    }

    @Test
    fun aStaleAddressIsForgottenWhenAnotherSignInStarts() {
        val h = SignInHandback()
        h.arm()
        h.deliver(good)
        h.arm()
        assertEquals(Handed.TimedOut, h.await(AtomicBoolean(false), Duration.ofMillis(150)))
    }

    @Test
    fun theWaitEndsWhenCancelledOrTimedOut() {
        val h = SignInHandback()
        h.arm()
        assertEquals(Handed.Cancelled, h.await(AtomicBoolean(true), Duration.ofSeconds(5)))
        h.arm()
        assertEquals(Handed.TimedOut, h.await(AtomicBoolean(false), Duration.ofMillis(50)))
        h.arm()
        assertEquals(Handed.TimedOut, h.await(AtomicBoolean(false), Duration.ofSeconds(-3)))
    }

    @Test
    fun afterTheWaitNothingIsTaken() {
        val h = SignInHandback()
        h.arm()
        h.await(AtomicBoolean(false), Duration.ofMillis(10))
        assertFalse(h.deliver(good))
    }

    @Test
    fun theSignInPageIsHttpsOnly() {
        assertTrue(isSignInUrl("https://login.example.org/authorize?x=1&y=%20"))
        assertFalse(isSignInUrl("http://login.example.org/"))
        assertFalse(isSignInUrl("intent://login.example.org/#Intent;end"))
        assertFalse(isSignInUrl("file:///etc/passwd"))
        assertFalse(isSignInUrl("javascript:alert(1)"))
        assertFalse(isSignInUrl("https:///nohost"))
        // Built from parts: a URL literal with userinfo reads as a hard-coded credential.
        val userinfo = "someone" + ":" + "placeholder"
        assertFalse(isSignInUrl("https://" + userinfo + "@login.example.org/"))
        assertFalse(isSignInUrl("https://exa mple.org/"))
        assertFalse(isSignInUrl(""))
        assertFalse(isSignInUrl("https://login.example.org/" + "a".repeat(Redirect.MAX_ADDRESS)))
    }
}
