// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LanguagesTest {
    @Test
    fun theCoreSpeaksWhatTheLocaleAsksForOrEnglish() {
        assertEquals("fr", coreLanguage("fr"))
        assertEquals("de", coreLanguage("DE"))
        assertEquals("nl", coreLanguage("nl-NL"))
        assertEquals("ro", coreLanguage("ro_RO"))
        assertEquals("hu", coreLanguage("hu-HU"))
        assertEquals("pl", coreLanguage("pl-PL"))
        assertEquals("cs", coreLanguage("cs-CZ"))
        assertEquals("ka", coreLanguage("ka-GE"))
        assertEquals("pt", coreLanguage("pt"))
        assertEquals("pt", coreLanguage("pt-BR"))
        assertEquals("en", coreLanguage(""))
        assertEquals("en", coreLanguage(null))
        assertEquals("en", coreLanguage("\u0000￿"))
    }

    @Test
    fun theChoiceIsTheFirstLanguageThatIsOffered() {
        assertEquals("", chosenTag(emptyList()))
        assertEquals("fr", chosenTag(listOf("fr-CA")))
        assertEquals("pt", chosenTag(listOf("pt-BR", "es-MX", "fr")))
        assertEquals("es", chosenTag(listOf("ja", "es-MX", "fr")))
        assertEquals("", chosenTag(listOf("ja")))
        assertEquals("", chosenTag(listOf("")))
    }

    @Test
    fun theListOfLanguagesIsTheOneTheSystemIsTold() {
        // The test runs in the app's folder.
        val xml = File("src/main/res/xml/locales_config.xml").readText()
        val told = Regex("""<locale android:name="([^"]+)"""").findAll(xml).map { it.groupValues[1] }.toList()
        assertEquals(LANGUAGES.map { it.tag }, told)
        assertEquals(told.size, told.toSet().size)
    }

    @Test
    fun everyLanguageHasACatalogueOrIsEnglish() {
        // crates/i18n/po/<code>.po, from the app's folder. Brazilian Portuguese's catalogue is pt_BR.po.
        val po = File("../../../crates/i18n/po")
        LANGUAGES.filter { it.tag != "en" }.forEach { lang ->
            val file = if (lang.tag == "pt") "pt_BR.po" else "${lang.tag}.po"
            assertTrue(lang.tag, File(po, file).isFile)
        }
    }

    @Test
    fun namesAreTheLanguagesOwn() {
        assertEquals("Deutsch", LANGUAGES.first { it.tag == "de" }.nativeName)
        assertTrue(LANGUAGES.all { it.nativeName.isNotBlank() })
    }
}
