// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SettingsModelTest {
    private val gib = 1024L * 1024 * 1024
    private val mib = 1024L * 1024

    @Test
    fun everySwitchOfThePhoneStartsOff() {
        val s = Switches()
        assertFalse(s.downloadPictures || s.senderImages || s.newMailOnly || s.primaryOnly)
    }

    @Test
    fun aPhoneKeepsAMonthUntilTheOwnerSaysOtherwiseAndEveryWindowHasWords() {
        assertEquals("1m", DEFAULT_OFFLINE)
        assertTrue(DEFAULT_OFFLINE in OFFLINE_CODES)
        OFFLINE_CODES.forEach { code -> assertTrue(code, offlineLabel(EnglishTexts, code) != code) }
        assertEquals("1 month", offlineLabel(EnglishTexts, "1m"))
        assertEquals("All mail", offlineLabel(EnglishTexts, "all"))
        // What isn't a window is said as it is, never blank.
        assertEquals("soon", offlineLabel(EnglishTexts, "soon"))
    }

    @Test
    fun aLongerWindowIsOneFurtherAlongTheList() {
        assertTrue(keepsMore(OFFLINE_CODES, "3m", "1m"))
        assertFalse(keepsMore(OFFLINE_CODES, "1w", "1m"))
        assertFalse(keepsMore(OFFLINE_CODES, "1m", "1m"))
        assertTrue(keepsMore(OFFLINE_CODES, "all", "10y"))
        // A code the list doesn't have keeps nothing more, either way round.
        assertFalse(keepsMore(OFFLINE_CODES, "forever", "1m"))
        assertFalse(keepsMore(OFFLINE_CODES, "3m", "forever"))
    }

    @Test
    fun aSyncIsAskedAboutOnlyWhenThereIsNotRoomForItAndTheReserve() {
        assertNull(spaceCheck(needed = 100 * mib, free = 10 * gib))
        assertEquals(SpaceWarning(2 * gib, 2 * gib), spaceCheck(needed = 2 * gib, free = 2 * gib))
        // Exactly enough with the reserve left over is room; a byte less is not.
        assertNull(spaceCheck(needed = gib, free = gib + FREE_RESERVE))
        assertEquals(SpaceWarning(gib, gib + FREE_RESERVE - 1), spaceCheck(needed = gib, free = gib + FREE_RESERVE - 1))
    }

    @Test
    fun aPhoneThatCannotSayHowMuchItHasIsNotAskedAndNothingIsNegative() {
        assertNull(spaceCheck(needed = 50 * gib, free = -1))
        assertEquals(SpaceWarning(0, 0), spaceCheck(needed = -5, free = 0))
    }

    @Test
    fun sizesAreSaidShortAndTheSameInEveryLocale() {
        assertEquals("0 B", sizeText(0))
        assertEquals("0 B", sizeText(-3))
        assertEquals("900 B", sizeText(900))
        assertEquals("2 KB", sizeText(2048))
        assertEquals("40 MB", sizeText(40 * mib))
        assertEquals("1.5 GB", sizeText(gib + gib / 2))
        val before = java.util.Locale.getDefault()
        try {
            java.util.Locale.setDefault(java.util.Locale.GERMANY)
            assertEquals("1.5 GB", sizeText(gib + gib / 2))
        } finally {
            java.util.Locale.setDefault(before)
        }
    }

    @Test
    fun aSignatureIsChangedOnlyWhenWhatIsTypedIsNotWhatIsKept() {
        assertFalse(SignatureState(saved = "Ada", draft = "Ada").changed)
        assertTrue(SignatureState(saved = "Ada", draft = "Ada B.").changed)
    }

    @Test
    fun theSummariesSayWhereEachSettingStands() {
        val ada = AccountItem(1u, "Ada Example", "ada@example.org", emptyList())
        val ui = SettingsUi(keepOffline = mapOf(1uL to "3m"), signatures = mapOf(1uL to SignatureState("Ada", "Ada")))
        assertEquals("3 months", offlineSummary(EnglishTexts, ui, listOf(ada)))
        assertEquals("No account yet", offlineSummary(EnglishTexts, ui, emptyList()))
        assertEquals("1 month", offlineSummary(EnglishTexts, SettingsUi(), listOf(ada)))
        assertEquals("1 account has one", signaturesSummary(EnglishTexts, ui, listOf(ada)))
        assertEquals("None", signaturesSummary(EnglishTexts, SettingsUi(), listOf(ada)))
        assertEquals("Asked first", picturesSummary(EnglishTexts, Switches()))
        assertEquals("Downloaded automatically", picturesSummary(EnglishTexts, Switches(downloadPictures = true)))
        assertEquals("Off", senderImagesSummary(EnglishTexts, Switches()))
        assertEquals("On", senderImagesSummary(EnglishTexts, Switches(senderImages = true)))
        assertEquals("Mail and reminders", notificationsSummary(EnglishTexts, Switches()))
        assertEquals("New mail only", notificationsSummary(EnglishTexts, Switches(newMailOnly = true)))
        val both = Switches(newMailOnly = true, primaryOnly = true)
        assertEquals("New mail, Primary or Focused only", notificationsSummary(EnglishTexts, both))
    }

    @Test
    fun theLicencesNameTheProgramAndEveryPartHasAllThree() {
        assertEquals("MailCraft", BUNDLED_LICENCES.first().name)
        BUNDLED_LICENCES.forEach { assertTrue(it.name.isNotBlank() && it.part.isNotBlank() && it.licence.isNotBlank()) }
        assertEquals(BUNDLED_LICENCES.size, BUNDLED_LICENCES.map { it.name }.toSet().size)
    }

    @Test
    fun onlyHttpsLinksOfTheOwnAreOpened() {
        assertTrue(isWebLink(PRIVACY_URL))
        assertFalse(isWebLink("http://example.org/"))
        assertFalse(isWebLink("javascript:alert(1)"))
        assertFalse(isWebLink("https://example.org/a b"))
        assertFalse(isWebLink("https://example.org/\n"))
        assertFalse(isWebLink("intent://x#Intent;end"))
    }
}
