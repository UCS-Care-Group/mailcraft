// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PicturePolicyTest {
    private val facts = MessageFacts("News@Shop.Example", junk = false, vouched = true, known = false)
    private val off = PicturePrefs()

    @Test
    fun nothingIsAllowedUntilTheOwnerSaysSo() {
        assertEquals(PictureAccess.Blocked, PicturePolicy.access(off, facts, asked = false))
        assertFalse(off.all || off.senderImages)
        assertTrue(off.senders.isEmpty() && off.domains.isEmpty())
    }

    @Test
    fun theOwnersWordForAMessageIsEnoughEvenInJunk() {
        assertEquals(PictureAccess.Allowed, PicturePolicy.access(off, facts, asked = true))
        assertEquals(PictureAccess.Allowed, PicturePolicy.access(off, facts.copy(junk = true), asked = true))
    }

    @Test
    fun allMailIsAllowedExceptJunk() {
        val all = off.copy(all = true)
        assertEquals(PictureAccess.Allowed, PicturePolicy.access(all, facts, asked = false))
        assertEquals(PictureAccess.Blocked, PicturePolicy.access(all, facts.copy(junk = true), asked = false))
    }

    @Test
    fun aRememberedSenderCountsOnlyWhenTheServerVouchesForIt() {
        val prefs = off.withSender("news@shop.example")
        assertEquals(PictureAccess.Allowed, PicturePolicy.access(prefs, facts, asked = false))
        assertEquals(PictureAccess.Blocked, PicturePolicy.access(prefs, facts.copy(vouched = false), asked = false))
        assertEquals(PictureAccess.Blocked, PicturePolicy.access(prefs, facts.copy(junk = true), asked = false))
        assertTrue(PicturePolicy.rememberedButUnvouched(prefs, facts.copy(vouched = false)))
        assertFalse(PicturePolicy.rememberedButUnvouched(prefs, facts))
    }

    @Test
    fun aRememberedDomainIsThatDomainAndNoOther() {
        val prefs = off.withDomain("someone@shop.example")
        assertTrue(prefs.domainRemembered("other@SHOP.example"))
        assertFalse(prefs.domainRemembered("other@mail.shop.example"))
        assertFalse(prefs.domainRemembered("other@example"))
        assertEquals(PictureAccess.Allowed, PicturePolicy.access(prefs, facts, asked = false))
    }

    @Test
    fun theBannerOffersThreeWaysExceptInJunkOrWhenNothingVouches() {
        assertEquals(listOf(AllowScope.Message, AllowScope.Sender, AllowScope.Domain), PicturePolicy.choices(facts))
        assertEquals(listOf(AllowScope.Message), PicturePolicy.choices(facts.copy(junk = true)))
        assertEquals(listOf(AllowScope.Message), PicturePolicy.choices(facts.copy(vouched = false)))
        assertEquals(listOf(AllowScope.Message), PicturePolicy.choices(facts.copy(address = "")))
    }

    @Test
    fun senderImagesFollowTheDesktopsRules() {
        val on = off.copy(senderImages = true)
        // Off by default, and never without the server's word, never in Junk.
        assertFalse(PicturePolicy.senderImage(off, facts.copy(known = true), asked = false))
        assertFalse(PicturePolicy.senderImage(on, facts.copy(known = true, vouched = false), asked = false))
        assertFalse(PicturePolicy.senderImage(on, facts.copy(known = true, junk = true), asked = false))
        // Somebody the owner knows, or whose mail's pictures the owner lets come.
        assertTrue(PicturePolicy.senderImage(on, facts.copy(known = true), asked = false))
        assertFalse(PicturePolicy.senderImage(on, facts, asked = false))
        assertTrue(PicturePolicy.senderImage(on, facts, asked = true))
        assertTrue(PicturePolicy.senderImage(on.copy(all = true), facts, asked = false))
    }

    @Test
    fun dataSavingHoldsDownloadsBackUntilATap() {
        assertTrue(PicturePolicy.waits(dataSaving = true, tapped = false))
        assertFalse(PicturePolicy.waits(dataSaving = true, tapped = true))
        assertFalse(PicturePolicy.waits(dataSaving = false, tapped = false))
    }

    @Test
    fun addressesAreComparedInLowerCaseAndJunkIsNoAddress() {
        assertEquals("a@b.example", PictureAddresses.sender("  A@B.Example "))
        for (junk in listOf("", "a", "@b", "a@", "a b@c.example", "a@b\u0000.example", "x".repeat(400) + "@b.example")) {
            assertNull(junk, PictureAddresses.sender(junk))
        }
        assertEquals("b.example", PictureAddresses.domain("a@b.example"))
        assertEquals(off, off.withSender("junk").withDomain("junk"))
    }

    @Test
    fun whatIsRememberedIsCapped() {
        var prefs = off
        repeat(PicturePrefs.MAX_REMEMBERED + 10) { prefs = prefs.withSender("s$it@example.org") }
        assertEquals(PicturePrefs.MAX_REMEMBERED, prefs.senders.size)
    }
}
