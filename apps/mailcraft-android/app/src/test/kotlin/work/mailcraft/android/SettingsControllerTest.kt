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
    var gate: kotlinx.coroutines.CompletableDeferred<Unit>? = null
    var setCalls = 0
    var setGate: kotlinx.coroutines.CompletableDeferred<Unit>? = null
    var capGate: kotlinx.coroutines.CompletableDeferred<Unit>? = null

    override fun codes() = OFFLINE_CODES

    override suspend fun keepOffline(account: ULong) = windows[account]

    override suspend fun setKeepOffline(account: ULong, code: String): WindowChange? {
        setCalls++
        setGate?.await()
        if (refuse) return null
        val shorter = OFFLINE_CODES.indexOf(code) < OFFLINE_CODES.indexOf(windows[account])
        windows[account] = code
        return WindowChange(code, shorter)
    }

    override suspend fun thin(): Thinned? {
        thinCalls++
        gate?.await()
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

    var storage = StorageUse(500, 120L * 1024 * 1024, 40, listOf(250, 500, 1024, 2048))
    var storageReads = 0
    var removal: DataRemoval? = DataRemoval()
    var removals = 0
    var noticesRead = 0

    override suspend fun storage(): StorageUse? {
        storageReads++
        return storage
    }

    override suspend fun setStorageCap(megabytes: Long): StorageUse? {
        capGate?.await()
        if (refuse) return null
        storage = storage.copy(capMb = megabytes)
        return storage
    }

    override suspend fun removeData(): DataRemoval? {
        removals++
        return removal
    }

    override suspend fun removalNoticeRead() {
        noticesRead++
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
        c.actions().onSwitches(Switches(newMailOnly = true, primaryOnly = true))
        assertEquals(Switches(newMailOnly = true, primaryOnly = true), api.kept)
        assertEquals(api.kept, c.ui.value.switches)
        assertFalse(Switches().primaryOnly)
    }

    @Test
    fun remindersStartOnAndTurningThemOffIsKeptBesideTheOtherSwitches() {
        assertTrue(Switches().reminders)
        val api = FakeSettings()
        val c = controller(api)
        assertTrue(c.ui.value.switches.reminders)
        c.actions().onSwitches(Switches(newMailOnly = true, reminders = false))
        assertFalse(api.kept.reminders)
        assertTrue(api.kept.newMailOnly)
        assertFalse(c.ui.value.switches.reminders)
        // What the store kept is what the next start reads.
        val restarted = controller(FakeSettings().apply { kept = api.kept })
        assertFalse(restarted.ui.value.switches.reminders)
        assertTrue(restarted.ui.value.switches.newMailOnly)
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
    fun thinningSaysHowMuchWasDoneAndEndsWithNoneLeft() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "1w")
        assertEquals(720L, c.ui.value.thinned)
        assertEquals(false, c.ui.value.thinning)
    }

    @Test
    fun stopEndsThinningWhileItWaitsAndTheWindowStays() {
        val api = FakeSettings().apply { gate = kotlinx.coroutines.CompletableDeferred() }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "3d")
        assertEquals(true, c.ui.value.thinning)
        c.actions().onThinStop()
        assertEquals(false, c.ui.value.thinning)
        assertEquals(0L, c.ui.value.thinned)
        assertEquals("3d", c.ui.value.keepOffline[1uL])
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

    @Test
    fun theStorageCapIsReadChosenFromTheOfferedOnesAndAFailureLeavesItAsItWas() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onStorageShown()
        assertEquals(500L, c.ui.value.storage?.capMb)
        c.actions().onStorageCap(1024)
        assertEquals(1024L, c.ui.value.storage?.capMb)
        assertEquals(1024L, api.storage.capMb)
        // A cap that was not offered is not taken; nor is the one already there.
        c.actions().onStorageCap(77)
        assertEquals(1024L, api.storage.capMb)
        api.refuse = true
        c.actions().onStorageCap(250)
        assertEquals(1024L, c.ui.value.storage?.capMb)
        assertTrue(c.ui.value.storageFailed)
        api.refuse = false
        c.actions().onStorageCap(2048)
        assertFalse(c.ui.value.storageFailed)
    }

    @Test
    fun aCapChosenWhileAnotherIsGoingThroughIsNotLost() {
        val api = FakeSettings().apply { capGate = kotlinx.coroutines.CompletableDeferred() }
        val c = controller(api)
        c.actions().onStorageShown()
        c.actions().onStorageCap(1024)
        // The first is still waiting: the cap shown is the old one, and the way back to it is chosen.
        c.actions().onStorageCap(500)
        api.capGate?.complete(Unit)
        assertEquals(500L, api.storage.capMb)
        assertEquals(500L, c.ui.value.storage?.capMb)
    }

    @Test
    fun aWindowChosenWhileAnotherIsGoingThroughIsNotLost() {
        val api = FakeSettings().apply { setGate = kotlinx.coroutines.CompletableDeferred() }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "3m")
        c.actions().onKeepOffline(ada, "1m")
        api.setGate?.complete(Unit)
        assertEquals("1m", api.windows[1uL])
        assertEquals("1m", c.ui.value.keepOffline[1uL])
    }

    @Test
    fun aLongerWindowEndsTheThinningOfAnEarlierShorterOne() {
        val api = FakeSettings().apply { gate = kotlinx.coroutines.CompletableDeferred() }
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onKeepOffline(ada, "3d")
        assertEquals(true, c.ui.value.thinning)
        c.actions().onKeepOffline(ada, "3m")
        assertEquals(false, c.ui.value.thinning)
        assertEquals("3m", c.ui.value.keepOffline[1uL])
    }

    @Test
    fun aSignatureIsCutByCharactersAndNeverInTheMiddleOfOne() {
        val c = controller(FakeSettings())
        c.actions().onShown(listOf(ada))
        c.actions().onSignatureDraft(ada, "a".repeat(MAX_SIGNATURE - 1) + "\uD83D\uDE00tail")
        val draft = c.ui.value.signatures.getValue(1uL).draft
        assertEquals(MAX_SIGNATURE, draft.codePointCount(0, draft.length))
        assertTrue(draft.endsWith("\uD83D\uDE00"))
    }

    @Test
    fun aShorterWindowReadsWhatThePhoneHoldsAgain() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        val before = api.storageReads
        c.actions().onKeepOffline(ada, "1w")
        assertTrue(api.storageReads > before)
    }

    @Test
    fun removingTheDataAsksFirstAndGoesThroughTheCore() {
        val api = FakeSettings()
        val c = controller(api)
        c.actions().onShown(listOf(ada))
        c.actions().onRemoveConfirm()
        assertEquals(0, api.removals)
        c.actions().onRemoveAsk()
        assertEquals(Removal.Asking, c.ui.value.removal)
        c.actions().onRemoveCancel()
        assertEquals(Removal.Idle, c.ui.value.removal)
        assertEquals(0, api.removals)
        c.actions().onRemoveAsk()
        c.actions().onRemoveConfirm()
        assertEquals(1, api.removals)
        assertEquals(Removal.Done, c.ui.value.removal)
        // What was read for the accounts that were is gone with them.
        assertTrue(c.ui.value.keepOffline.isEmpty() && c.ui.value.signatures.isEmpty())
        assertNull(c.ui.value.storage)
        assertEquals(0, api.noticesRead)
    }

    @Test
    fun aRemovalThatLeftSomethingCanBeTriedAgain() {
        val api = FakeSettings().apply { removal = null }
        val c = controller(api)
        c.actions().onRemoveAsk()
        c.actions().onRemoveConfirm()
        assertEquals(Removal.Failed, c.ui.value.removal)
        api.removal = DataRemoval()
        c.actions().onRemoveConfirm()
        assertEquals(2, api.removals)
        assertEquals(Removal.Done, c.ui.value.removal)
    }

    @Test
    fun theCoresNoticeIsShownAndTheCoreToldOnceItIsRead() {
        val api = FakeSettings().apply { removal = DataRemoval("Some sign-ins may remain.") }
        val c = controller(api)
        c.actions().onRemoveAsk()
        c.actions().onRemoveConfirm()
        assertEquals(Removal.Notice("Some sign-ins may remain."), c.ui.value.removal)
        // Not told yet: it has only been put up.
        assertEquals(0, api.noticesRead)
        c.actions().onRemoveNoticeRead()
        assertEquals(Removal.Done, c.ui.value.removal)
        assertEquals(1, api.noticesRead)
        // Read twice, told once.
        c.actions().onRemoveNoticeRead()
        assertEquals(1, api.noticesRead)
    }
}
