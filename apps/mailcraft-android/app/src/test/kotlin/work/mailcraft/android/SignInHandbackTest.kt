// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SignInHandbackTest {
    private val good = "work.mailcraft.android://oauth2redirect?code=c&state=s"

    @Test
    fun onlyTheRegisteredAddressIsAccepted() {
        assertEquals(good, Redirect.accepted(good))
        val bare = "work.mailcraft.android://oauth2redirect"
        assertEquals(bare, Redirect.accepted(bare))
        assertEquals("$bare#x", Redirect.accepted("$bare#x"))
        assertNull(Redirect.accepted(null))
        assertNull(Redirect.accepted(""))
        assertNull(Redirect.accepted("work.mailcraft.android://oauth2redirectx?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android://other?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android:/oauth2redirect?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android://oauth2redirect/x?code=c"))
        assertNull(Redirect.accepted("work.mailcraft.android://oauth2redirect//?code=c"))
        assertEquals(
            "work.mailcraft.android://oauth2redirect/?code=c",
            Redirect.accepted("work.mailcraft.android://oauth2redirect/?code=c"),
        )
        assertNull(Redirect.accepted("work.mailcraft.android://OAUTH2REDIRECT?code=c"))
        assertNull(Redirect.accepted("WORK.mailcraft.android://oauth2redirect"))
        assertNull(Redirect.accepted("https://example.org/oauth2redirect"))
        assertNull(Redirect.accepted(good + "x".repeat(Redirect.MAX_ADDRESS)))
    }

    @Test
    fun anAddressWithNoSignInWaitingIsDropped() {
        val got = mutableListOf<String>()
        val h = SignInHandback()
        h.sink = { got.add(it) }
        assertFalse(h.deliver(good))
        h.arm()
        assertTrue(h.deliver(good))
        assertFalse("only the first is taken", h.deliver(good))
        assertEquals(listOf(good), got)
    }

    @Test
    fun theAddressGoesToTheCoreWholeAndUnreadWhatItSays() {
        val got = mutableListOf<String>()
        val h = SignInHandback()
        h.sink = { got.add(it) }
        // The state, the code and the rest are the core's to judge:
        // any address with the registered scheme and path goes.
        val odd = "work.mailcraft.android://oauth2redirect?state=not-ours&code=%00&evil=1#frag"
        h.arm()
        assertTrue(h.deliver(odd))
        assertEquals(listOf(odd), got)
    }

    @Test
    fun aStrangersAddressIsNotPassedOnAndLeavesTheSignInWaiting() {
        val got = mutableListOf<String>()
        val h = SignInHandback()
        h.sink = { got.add(it) }
        h.arm()
        assertFalse(h.deliver("https://example.org/oauth2redirect?code=c&state=s"))
        assertFalse(h.deliver("work.mailcraft.android:/other?code=c"))
        assertFalse(h.deliver(null))
        assertTrue(got.isEmpty())
        assertTrue("the real one still gets through", h.deliver(good))
        assertEquals(listOf(good), got)
    }

    @Test
    fun whatTheCoreSaysOfAnAddressIsWhatDeliverSays() {
        val h = SignInHandback()
        h.sink = { false }
        h.arm()
        assertFalse(h.deliver(good))
        // (The address was offered once; nothing waits for a second.)
        h.sink = { true }
        assertFalse(h.deliver(good))
    }

    @Test
    fun aSignInThatEndedTakesNothing() {
        val got = mutableListOf<String>()
        val h = SignInHandback()
        h.sink = { got.add(it) }
        h.arm()
        h.disarm()
        assertFalse(h.deliver(good))
        assertTrue(got.isEmpty())
    }

    @Test
    fun anotherSignInArmsItAfresh() {
        val got = mutableListOf<String>()
        val h = SignInHandback()
        h.sink = { got.add(it) }
        h.arm()
        h.deliver(good)
        h.arm()
        assertTrue(h.deliver(good))
        assertEquals(2, got.size)
    }

    @Test
    fun onlyOneOfTwoAddressesAtTheSameTimeIsPassedOn() {
        val count = java.util.concurrent.atomic.AtomicInteger()
        val h = SignInHandback()
        h.sink = {
            count.incrementAndGet()
            true
        }
        repeat(200) {
            count.set(0)
            h.arm()
            val workers = List(4) { thread { h.deliver(good) } }
            workers.forEach { it.join() }
            assertEquals(1, count.get())
        }
    }

    @Test
    fun theSignInPageIsHttpsOnly() {
        assertTrue(isSignInUrl("https://login.example.org/authorize?x=1&y=%20"))
        assertFalse(isSignInUrl("http://login.example.org/"))
        assertFalse(isSignInUrl("intent://login.example.org/#Intent;end"))
        assertFalse(isSignInUrl("file:///etc/passwd"))
        assertFalse(isSignInUrl("javascript:alert(1)"))
        assertFalse(isSignInUrl("https:///nohost"))
        assertFalse(isSignInUrl("https://user:pw@login.example.org/"))
        assertFalse(isSignInUrl("https://exa mple.org/"))
        assertFalse(isSignInUrl(""))
        assertFalse(isSignInUrl("https://login.example.org/" + "a".repeat(Redirect.MAX_ADDRESS)))
    }
}
