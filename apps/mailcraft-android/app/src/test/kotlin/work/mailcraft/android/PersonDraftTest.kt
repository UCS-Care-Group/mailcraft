// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import androidx.compose.runtime.saveable.SaverScope
import org.junit.Test

class PersonDraftTest {
    @Test
    fun anEmptyFormCannotBeSaved() {
        assertFalse(PersonDraft().canSave)
        assertFalse(PersonDraft(first = "  ", notes = "only notes").canSave)
    }

    @Test
    fun aNameACompanyOrAnAddressIsEnough() {
        assertTrue(PersonDraft(first = "Ada").canSave)
        assertTrue(PersonDraft(company = "Example Ltd").canSave)
        assertTrue(PersonDraft(emails = "ada@example.org").canSave)
    }

    @Test
    fun anAddressThatIsNotOneBlocksSaving() {
        assertTrue(PersonDraft(first = "Ada", emails = "ada at example").badEmail)
        assertFalse(PersonDraft(first = "Ada", emails = "ada@example.org, <x@example.org>").canSave)
        assertFalse(PersonDraft(first = "Ada", emails = "a@@example.org").canSave)
    }

    @Test
    fun moreEntriesThanTheCoreKeepsBlockSaving() {
        val many = { n: Int -> (1..n).joinToString("\n") { "a$it@example.org" } }
        assertTrue(PersonDraft(emails = many(20)).canSave)
        assertFalse(PersonDraft(emails = many(21)).canSave)
        assertTrue(PersonDraft(emails = many(21)).tooMany)
        assertFalse(PersonDraft(first = "Ada", phones = (1..21).joinToString("\n") { "$it" }).canSave)
        assertTrue(PersonDraft(first = "Ada", phones = (1..20).joinToString("\n") { "$it" }).canSave)
    }

    @Test
    fun theParametersAreTrimmedAndListed() {
        val d = PersonDraft(
            first = " Ada ", last = "Ex\nample", emails = "a@example.org, b@example.org;\n\n", phones = "123\n 456 ",
            company = "", notes = "  hi \"there\"  ",
        )
        assertEquals(
            "{\"first\":\"Ada\",\"last\":\"Ex ample\"," +
                "\"emails\":[{\"label\":\"work\",\"value\":\"a@example.org\"}," +
                "{\"label\":\"work\",\"value\":\"b@example.org\"}]," +
                "\"phones\":[{\"label\":\"mobile\",\"value\":\"123\"},{\"label\":\"mobile\",\"value\":\"456\"}]," +
                "\"notes\":\"hi \\\"there\\\"\"}",
            d.params(),
        )
    }

    @Test
    fun theChosenFolderIsSentAndNoneLeavesItToTheCore() {
        assertEquals("{\"first\":\"Ada\"}", PersonDraft(first = "Ada").params())
        assertEquals("{\"first\":\"Ada\",\"folder\":0}", PersonDraft(first = "Ada", folder = PHONE).params())
        assertEquals("{\"first\":\"Ada\",\"folder\":12}", PersonDraft(first = "Ada", folder = 12u).params())
    }

    @Test
    fun theFormSurvivesTheAppGoingAway() {
        val d = PersonDraft("Ada", "Example", "a@example.org", "1", "Co", "Dev", "n")
        val saved = PersonDraft.Saver.run { SaverScope { true }.save(d) }
        assertEquals(d, saved?.let { PersonDraft.Saver.restore(it) })
        val placed = d.copy(folder = 12u)
        val kept = PersonDraft.Saver.run { SaverScope { true }.save(placed) }
        assertEquals(placed, kept?.let { PersonDraft.Saver.restore(it) })
        assertNull(PersonDraft.Saver.restore(arrayListOf<String>()))
        assertNull(PersonDraft.Saver.restore("junk"))
    }
}
