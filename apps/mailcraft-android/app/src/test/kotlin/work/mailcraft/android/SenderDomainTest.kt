// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import uniffi.mailcraft_mobile.AddressRecord

class SenderDomainTest {
    @Test
    fun theDomainComesFromTheAddressAndNeverTheName() {
        assertEquals("example.org", senderDomain(AddressRecord("Ada@evil.example", "Ada@Example.ORG")))
        assertEquals("example.org", senderDomain(AddressRecord("", "a@b@Example.org")))
        assertNull(senderDomain(AddressRecord("Ada", "")))
        assertNull(senderDomain(AddressRecord("Ada", "no-at-sign")))
        assertNull(senderDomain(AddressRecord("Ada", "trailing@")))
    }
}
