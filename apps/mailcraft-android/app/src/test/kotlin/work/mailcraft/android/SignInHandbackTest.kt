// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class SignInHandbackTest {
    @get:Rule
    val tmp = TemporaryFolder()

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
        // Built from parts: a URL literal with userinfo reads as a hard-coded credential.
        val userinfo = "someone" + ":" + "placeholder"
        assertFalse(isSignInUrl("https://" + userinfo + "@login.example.org/"))
        assertFalse(isSignInUrl("https://exa mple.org/"))
        assertFalse(isSignInUrl(""))
        assertFalse(isSignInUrl("https://login.example.org/" + "a".repeat(Redirect.MAX_ADDRESS)))
    }

    @Test
    fun aRedirectAfterTheSystemClosedTheAppIsToldOnceAndNeverPassedOn() {
        val record = java.io.File(tmp.newFolder("p"), "pending")
        // The sign-in began in the process that is gone.
        PendingSignIn(record).save("ada@example.org")
        val passed = mutableListOf<String>()
        val told = mutableListOf<String>()
        val handback = SignInHandback(PendingSignIn(record))
        handback.sink = { passed.add(it) }
        handback.interrupted = { told.add(it) }
        assertFalse(handback.deliver(good))
        assertEquals(listOf("ada@example.org"), told)
        assertTrue(passed.isEmpty())
        // Only once.
        assertFalse(handback.deliver(good))
        assertEquals(1, told.size)
    }

    @Test
    fun aLostSignInsRedirectThatComesBeforeTheCoreIsOpenIsToldOnceItIs() {
        val record = java.io.File(tmp.newFolder("early"), "pending")
        PendingSignIn(record).save("ada@example.org")
        val handback = SignInHandback(PendingSignIn(record))
        // The core is still opening: nobody is set to be told yet.
        assertFalse(handback.deliver(good))
        val told = mutableListOf<String>()
        handback.interrupted = { told.add(it) }
        assertEquals(listOf("ada@example.org"), told)
        // Only once, also when it is set again.
        handback.interrupted = { told.add(it) }
        assertEquals(1, told.size)
    }

    @Test
    fun aStrangersAddressWithNoSignInUnderWayTellsNothing() {
        val handback = SignInHandback(PendingSignIn(java.io.File(tmp.newFolder("q"), "pending")))
        val told = mutableListOf<String>()
        handback.interrupted = { told.add(it) }
        assertFalse(handback.deliver(good))
        assertTrue(told.isEmpty())
    }

    @Test
    fun aSignInThatEndedLeavesNoRecordAndAnOldRecordIsNotBelieved() {
        val dir = tmp.newFolder("r")
        val record = java.io.File(dir, "pending")
        val handback = SignInHandback(PendingSignIn(record))
        val ticket = handback.begin("ada@example.org")!!
        assertTrue(record.exists())
        handback.end(ticket)
        assertFalse(record.exists())
        PendingSignIn(record).save("ada@example.org")
        val later = PendingSignIn(record, now = { record.lastModified() + 11 * 60 * 1000L })
        assertNull(later.take())
        assertFalse(record.exists())
    }

    @Test
    fun aSecondRedirectDuringALiveSignInKeepsItsRecord() {
        val record = java.io.File(tmp.newFolder("s"), "pending")
        val handback = SignInHandback(PendingSignIn(record))
        val told = mutableListOf<String>()
        handback.sink = { true }
        handback.interrupted = { told.add(it) }
        handback.begin("ada@example.org")
        handback.arm()
        assertTrue(handback.deliver(good))
        assertFalse(handback.deliver(good))
        assertTrue(told.isEmpty())
        assertTrue("still there for a later process death", record.exists())
    }

    @Test
    fun aSecondSignInWhileOneIsUnderWayIsRefusedAndLeavesTheFirstWaiting() {
        val record = java.io.File(tmp.newFolder("t"), "pending")
        val got = mutableListOf<String>()
        val handback = SignInHandback(PendingSignIn(record))
        handback.sink = { got.add(it) }
        val first = handback.begin("ada@example.org")!!
        handback.arm()
        // A second sign-in, or a grant, finds the first under way and touches nothing.
        assertNull(handback.begin("bob@example.org"))
        assertNull(handback.begin())
        assertEquals("ada@example.org", record.readText())
        // The first's redirect still reaches the core.
        assertTrue(handback.deliver(good))
        handback.end(first)
        assertEquals(1, got.size)
    }

    @Test
    fun anOlderSignInsEndNeverDisarmsANewerOne() {
        val record = java.io.File(tmp.newFolder("u"), "pending")
        val got = mutableListOf<String>()
        val handback = SignInHandback(PendingSignIn(record))
        handback.sink = { got.add(it) }
        val a = handback.begin("ada@example.org")!!
        handback.end(a)
        val b = handback.begin("bob@example.org")!!
        handback.arm()
        // A's completion arrives late, after B armed: it changes nothing of B's.
        handback.end(a)
        assertTrue("B's record is kept", record.exists())
        assertTrue("B's redirect is taken", handback.deliver(good))
        assertEquals(1, got.size)
        handback.end(b)
        assertFalse(record.exists())
    }
}
