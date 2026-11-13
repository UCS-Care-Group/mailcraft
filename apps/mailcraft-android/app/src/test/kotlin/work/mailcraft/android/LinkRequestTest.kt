// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import android.content.Intent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.mailcraft_mobile.AddressRecord

class LinkRequestTest {
    @Test
    fun aWebAddressGoesToTheBrowserAndANumberToTheDialer() {
        val web = requestFor("https://example.org/a?b=c")
        assertEquals(Intent.ACTION_VIEW, web?.action)
        assertEquals("https://example.org/a?b=c", web?.address)
        assertEquals(Intent.ACTION_DIAL, requestFor("tel:+15550100")?.action)
    }

    @Test
    fun nothingElseIsHandedToTheSystem() {
        assertNull(requestFor("mailto:ada@example.org"))
        assertNull(requestFor("cid:logo"))
        for (shown in listOf("intent://x#Intent;end", "file:///etc/passwd", "javascript:alert(1)", "content://x/y", "ftp://x.example/")) {
            assertNull(shown, requestFor(shown))
        }
    }

    @Test
    fun peopleAreShownWithTheirAddressInFull() {
        assertEquals("Ada Example <ada@example.org>", personLabel(AddressRecord("Ada Example", "ada@example.org")))
        assertEquals("ada@example.org", personLabel(AddressRecord("", "ada@example.org")))
        assertEquals("ada@example.org", personLabel(AddressRecord("ada@example.org", "ada@example.org")))
    }

    @Test
    fun aMessageOpenedFromTheListIsSavedAndComesBack() {
        val reading = NavState().openMessage(42uL)
        assertEquals(Tab.Mail, reading.tab)
        assertEquals(42uL, reading.messageId)
        val saved = listOf<Any?>(reading.tab.ordinal, reading.settings, -1, reading.message)
        assertEquals(reading, NavState.fromSaved(saved))
        // Back closes the message first, and leaves the tab.
        assertEquals(NavState(), reading.back())
        assertNull(NavState().back())
        // A message with settings open, or on another tab, is not a thing: it is dropped.
        assertEquals(NavState(settings = true), NavState.fromSaved(listOf(1, true, -1, 5L)))
        assertEquals(NavState(tab = Tab.Tasks), NavState.fromSaved(listOf(4, false, -1, 5L)))
        assertEquals(NavState(), NavState.fromSaved(listOf(1, false, -1, -3L)))
        assertNull(NavState.fromSaved(listOf(1, false, -1, "x")).message)
    }
}
