// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private const val GIB = 1024L * 1024 * 1024

/** The core and the phone, as the settings see them, with invented accounts. */
private class FakeSettings(var free: Long = 50 * GIB) : SettingsApi {
    val windows = mutableMapOf(1uL to "1m", 2uL to "1m")
    val signatures = mutableMapOf<ULong, SignatureState>(1uL to SignatureState("Ada", "Ada"), 2uL to SignatureState("Bo", "Bo"))
    val saved = mutableListOf<String>()
    var kept = Switches()
    var refuse = false
    var thinning = mutableListOf(Thinned(300, 300), Thinned(300, 120), Thinned(120, 0))
    var thinCalls = 0
    var setCalls = 0

    override fun codes() = OFFLINE_CODES

    override suspend fun keepOffline(account: ULong) = windows[account]

    override suspend fun setKeepOffline(account: ULong, code: String): WindowChange? {
        setCalls++
        if (refuse) return null
        val shorter = OFFLINE_CODES.indexOf(code) < OFFLINE_CODES.indexOf(windows[account])
        windows[account] = code
        return WindowChange(code, shorter)
    }

    override suspend fun thin(): Thinned? {
        thinCalls++
        return if (thinning.isEmpty()) Thinned(0, 0) else thinning.removeAt(0)
    }

    // A month is 40 MB, as the core says.
    override fun estimate(code: String): Long = 40L * 1024 * 1024 * (OFFLINE_CODES.indexOf(code) + 1) * (OFFLINE_CODES.indexOf(code) + 1)

    override fun freeBytes() = free

    override suspend fun signature(account: ULong) = signatures[account]

    override suspend fun setSignature(account: ULong, text: String): Boolean {
        if (refuse) return false
        saved.add("$account:$text")
        return true
    }

    override fun switches() = kept

    override fun saveSwitches(switches: Switches) {
        kept = switches
    }
}

class SettingsControllerTest {
    private val app = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val ada = AccountItem(1u, "Ada Example", "ada@example.org", emptyList())
    private val bo = AccountItem(2u, "Bo Lumen", "bo@lumen.example", emptyList())

    private fun controller(api: SettingsApi): SettingsController = SettingsController(api, app)

    @Test
    fun whatIsKeptForEachAccountIsReadWhenTheyAreShownAndAnAccountThatGoesIsForgotten() {
        val c = controller(FakeSettings())
        c.actions().onShown(listOf(ada, bo))
        assertEquals(mapOf(1uL to "1m", 2uL to "1m"), c.ui.value.keepOffline)
        assertEquals("Ada", c.ui.value.signatures[1uL]?.saved)
        c.actions().onShown(listOf(bo))
        assertEquals(setOf(2uL), c.ui.value.keepOffline.keys)
        assertEquals(setOf(2uL), c.ui.value.signatures.keys)
    }

    @Test
    fun theSwitchesStartAsTheyWereKeptAndAreKeptWhenChanged() {
        val api = FakeSettings().apply { kept = Switches(newMailOnly = true) }
        val c = controller(api)
        assertTrue(c.ui.value.switches.newMailOnly)
        c.actions().onSwitches(Switches(newMailOnly = true, downloadPictures = true))
        assertEquals(Switches(downloadPictures = true, newMailOnly = true), api.kept)
        assertEquals(api.kept, c.ui.value.switches)
        // The picture switches are the ones the reading pane reads: the same record, off by default.
        assertFalse(Switches().downloadPictures)
        assertFalse(Switches().senderImages)
    }

    @Test
    fun aLongerWindowIsAppliedAtOnceWhenThereIsRoom() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "3m")
        assertEquals("3m", c.ui.value.keepOffline[1uL])
        assertNull(c.ui.value.space)
        assertEquals(0, api.thinCalls)
    }

    @Test
    fun aLongerWindowWaitsForAYesWhenThePhoneIsShortOfRoom() {
        val api = FakeSettings(free = GIB / 2)
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "12m")
        // The question is up with the two sizes, and nothing has been asked of the core.
        val ask = c.ui.value.space
        assertEquals(1uL, ask?.account)
        assertEquals("12m", ask?.forWindow)
        assertEquals(GIB / 2, ask?.warning?.free)
        assertTrue((ask?.warning?.needed ?: 0) > 0)
        assertEquals(0, api.setCalls)
        assertEquals("1m", c.ui.value.keepOffline[1uL])
        c.actions().onSpaceYes()
        assertNull(c.ui.value.space)
        assertEquals("12m", c.ui.value.keepOffline[1uL])
        assertEquals(1, api.setCalls)
    }

    @Test
    fun aNoLeavesTheWindowAsItWasAndNothingIsAskedOfTheCore() {
        val api = FakeSettings(free = 0)
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "all")
        c.actions().onSpaceNo()
        assertNull(c.ui.value.space)
        assertEquals("1m", c.ui.value.keepOffline[1uL])
        assertEquals(0, api.setCalls)
    }

    @Test
    fun aShorterWindowNeedsNoRoomAndTakesOlderMailBackInSmallCallsUntilNoneIsLeft() {
        val api = FakeSettings(free = 0)
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "1w")
        assertNull(c.ui.value.space)
        assertEquals("1w", c.ui.value.keepOffline[1uL])
        assertEquals(3, api.thinCalls)
    }

    @Test
    fun thinningStopsWhenItGetsNowhere() {
        val api = FakeSettings().apply { thinning = MutableList(2000) { Thinned(0, 50) } }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "3d")
        assertEquals(1, api.thinCalls)
    }

    @Test
    fun aWindowThatDidNotGoThroughIsSaidAndKeptAsItWas() {
        val api = FakeSettings().apply { refuse = true }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "2w")
        assertTrue(c.ui.value.keepFailed)
        assertEquals("1m", c.ui.value.keepOffline[1uL])
    }

    @Test
    fun theSameWindowOrOneThatIsNotOneChangesNothing() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "1m")
        c.actions().onKeepOffline(ada, "forever")
        assertEquals(0, api.setCalls)
    }

    @Test
    fun aSyncThatAsksForRoomWaitsUntilTheOwnerAnswers() {
        val api = FakeSettings(free = GIB)
        val c = controller(api)
        var answer: Boolean? = null
        app.launch { answer = c.waitForRoom(1u, 4 * GIB) }
        assertNull(answer)
        assertEquals(4 * GIB, c.ui.value.space?.warning?.needed)
        assertNull(c.ui.value.space?.forWindow)
        c.actions().onSpaceYes()
        assertEquals(true, answer)
        assertNull(c.ui.value.space)
        // With room, it goes on at once.
        app.launch { answer = null; answer = c.waitForRoom(1u, 1024) }
        assertEquals(true, answer)
    }

    @Test
    fun twoSyncsThatAskAreAskedOneAfterTheOther() {
        val c = controller(FakeSettings(free = 0))
        val answers = mutableListOf<Boolean>()
        app.launch { answers.add(c.waitForRoom(1u, GIB)) }
        app.launch { answers.add(c.waitForRoom(2u, 2 * GIB)) }
        assertEquals(1uL, c.ui.value.space?.account)
        c.actions().onSpaceNo()
        assertEquals(listOf(false), answers)
        assertEquals(2uL, c.ui.value.space?.account)
        c.actions().onSpaceYes()
        assertEquals(listOf(false, true), answers)
        assertNull(c.ui.value.space)
    }

    @Test
    fun aSignatureIsSavedOnlyWhenChangedAndIsKeptUntilItIsSaved() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada, bo))
        c.actions().onSignatureSave(ada)
        assertEquals(emptyList<String>(), api.saved)
        c.actions().onSignatureDraft(ada, "Ada B.\nExample Org")
        assertEquals("Ada", c.ui.value.signatures[1uL]?.saved)
        assertEquals("Bo", c.ui.value.signatures[2uL]?.draft)
        c.actions().onSignatureSave(ada)
        assertEquals(listOf("1:Ada B.\nExample Org"), api.saved)
        assertEquals("Ada B.\nExample Org", c.ui.value.signatures[1uL]?.saved)
        assertFalse(c.ui.value.signatures.getValue(1uL).changed)
    }

    @Test
    fun aSignatureThatWasNotKeptIsSaidAndStaysTypedAndALongOneIsCut() {
        val api = FakeSettings().apply { refuse = true }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onSignatureDraft(ada, "x".repeat(MAX_SIGNATURE + 500))
        assertEquals(MAX_SIGNATURE, c.ui.value.signatures[1uL]?.draft?.length)
        c.actions().onSignatureSave(ada)
        assertTrue(c.ui.value.signatures.getValue(1uL).failed)
        assertTrue(c.ui.value.signatures.getValue(1uL).changed)
        // Typing again clears the notice.
        c.actions().onSignatureDraft(ada, "y")
        assertFalse(c.ui.value.signatures.getValue(1uL).failed)
    }

    @Test
    fun aSignatureWithAPictureIsNeverChangedFromTheBox() {
        val api = FakeSettings().apply { signatures[1uL] = SignatureState("Ada\n[logo]", "Ada\n[logo]", editable = false) }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onSignatureDraft(ada, "flat")
        c.actions().onSignatureSave(ada)
        assertEquals("Ada\n[logo]", c.ui.value.signatures[1uL]?.draft)
        assertEquals(emptyList<String>(), api.saved)
    }
}
