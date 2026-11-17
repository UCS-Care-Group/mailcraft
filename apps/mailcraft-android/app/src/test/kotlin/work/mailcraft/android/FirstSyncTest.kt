// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.SyncAnswer

/** Signs in to account 7 (an invented address) as soon as it is asked, or fails as told. */
private class SigningIn(
    var result: SignInResult = SignInResult.SignedIn(7u, "ada@example.org", "Ada Example"),
) : AccountsApi {
    val kept = mutableListOf<KeepChoice>()

    /** What happens while the sign-in runs (the window reading the list again). */
    var meanwhile: () -> Unit = {}

    override suspend fun signIn(email: String, keep: KeepChoice): SignInResult {
        kept.add(keep)
        meanwhile()
        return result
    }

    override suspend fun grant(account: ULong, permission: PermissionKind): GrantResult = GrantResult.Granted

    override suspend fun status(account: ULong, permission: PermissionKind) = PermissionStatus.Allowed

    override suspend fun remove(account: ULong) = true

    override fun approvalLink(email: String): String? = null

    override fun cancel() = Unit
}

/** A sign-in that works starts the account's first Send/Receive at once, a new one's through the room gate. */
class FirstSyncTest {
    private val app = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private val api = SigningIn()
    private var syncs = 0

    /** Which rounds were asked for: the owner's ("owner", which lets the holds go) or the app's ("first"). */
    private val rounds = mutableListOf<String>()
    private val asked = mutableListOf<Pair<ULong, Long>>()
    private var room: suspend () -> Boolean = { true }
    private val kept = MemoryHolds()
    private val told = mutableListOf<Pair<ULong, Boolean>>()
    /** How many of the next hold calls the core refuses (its queue full). */
    private var refusals = 0
    private val holds = FirstSyncHolds(kept) { account, held ->
        if (refusals > 0) {
            refusals--
            throw CoreException.Failed("busy")
        }
        told.add(account to held)
    }

    private val hub = AccountsHub(
        api = api,
        texts = EnglishTexts,
        scope = app,
        save = {},
        copy = {},
        sync = {
            syncs++
            rounds.add("owner")
            SyncAnswer(started = true, message = "")
        },
        firstRound = {
            syncs++
            rounds.add("first")
            SyncAnswer(started = true, message = "")
        },
        roomFor = { account, storageMb ->
            asked.add(account to storageMb)
            room()
        },
        holds = holds,
    )

    private fun addAccount(keep: (AccountActions) -> Unit = {}) {
        val actions = hub.controller.actions()
        actions.onEmail("ada@example.org")
        keep(actions)
        actions.onSignIn()
    }

    @Test
    fun addingAnAccountStartsItsFirstSyncAtOnceAndTheListSpins() {
        addAccount { it.onStorage(1024L) }
        assertEquals(AddAccountState.Done(7u), hub.controller.ui.value.add)
        // The gate is asked for the new account, with the space the owner gave its mail.
        assertEquals(listOf(7uL to 1024L), asked)
        assertEquals(1, syncs)
        // The mail list's spinner ("updating") is on until the core says the round is over.
        assertTrue(hub.syncing.value)
        hub.handle(Event.SyncFinished(0u, true))
        assertFalse(hub.syncing.value)
    }

    @Test
    fun aFirstSyncThereIsNoRoomForDoesNotStart() {
        room = { false }
        addAccount()
        assertEquals(listOf(7uL to KeepChoice.DEFAULT_STORAGE_MB), asked)
        assertEquals(0, syncs)
        assertFalse(hub.syncing.value)
    }

    @Test
    fun theFirstSyncWaitsForTheOwnersAnswerToTheRoomQuestion() {
        val answer = CompletableDeferred<Boolean>()
        room = { answer.await() }
        addAccount()
        // The screen has closed while the question is open; nothing is fetched yet.
        assertEquals(AddAccountState.Done(7u), hub.controller.ui.value.add)
        assertEquals(0, syncs)
        answer.complete(true)
        assertEquals(1, syncs)
    }

    @Test
    fun anAccountSignedInAgainSyncsAtOnceWithoutTheRoomQuestion() {
        // Its mail is on the phone already: the window shows the account.
        hub.showing(listOf(AccountItem(7u, "Ada Example", "ada@example.org", emptyList())))
        addAccount()
        assertTrue(asked.isEmpty())
        assertEquals(1, syncs)
    }

    @Test
    fun aNewAccountTheListShowsBeforeTheSignInReturnsIsStillNew() {
        // The core announced the account and the window read the list again before the call returned.
        val ada = AccountItem(7u, "Ada Example", "ada@example.org", emptyList())
        api.meanwhile = { hub.showing(listOf(ada)) }
        addAccount()
        assertEquals(listOf(7uL to KeepChoice.DEFAULT_STORAGE_MB), asked)
        assertEquals(1, syncs)
    }

    @Test
    fun aRoundThatCouldNotStartLeavesNoFirstSyncWaiting() {
        val answers = ArrayDeque(
            listOf(SyncAnswer(started = false, message = "No network"), SyncAnswer(started = true, message = "")),
        )
        val hub = AccountsHub(
            api = api,
            texts = EnglishTexts,
            scope = app,
            save = {},
            copy = {},
            sync = {
                syncs++
                answers.removeFirst()
            },
            firstRound = {
                syncs++
                answers.removeFirst()
            },
        )
        val actions = hub.controller.actions()
        actions.onEmail("ada@example.org")
        actions.onSignIn()
        // The first ask was refused; nothing is left asked for to start later by itself.
        assertEquals(1, syncs)
        assertFalse(hub.syncing.value)
        hub.handle(Event.SyncFinished(0u, true))
        assertEquals(1, syncs)
    }

    @Test
    fun aSignInThatDidNotWorkStartsNoSync() {
        api.result = SignInResult.Declined
        addAccount()
        assertTrue(asked.isEmpty())
        assertEquals(0, syncs)
    }

    @Test
    fun aRoundAlreadyUnderWayIsFollowedByTheNewAccountsFirstSync() {
        // A pull to refresh began before the account was in the mailbox.
        hub.syncNow()
        assertEquals(1, syncs)
        addAccount()
        assertEquals(1, syncs)
        // When it ends, the round with the new account starts.
        hub.handle(Event.SyncFinished(0u, true))
        assertEquals(2, syncs)
        assertTrue(hub.syncing.value)
    }

    @Test
    fun notNowHoldsTheNewAccountBackAndKeepsItOverARestart() {
        room = { false }
        addAccount()
        assertEquals(listOf(7uL to true), told)
        assertEquals(setOf(7uL), kept.load())
    }

    @Test
    fun syncAnywayLetsTheAccountGo() {
        kept.save(setOf(7uL))
        val answer = CompletableDeferred<Boolean>()
        room = { answer.await() }
        addAccount()
        // Nothing changes while the question is open.
        assertTrue(told.isEmpty())
        answer.complete(true)
        assertEquals(listOf(7uL to false), told)
        assertTrue(kept.load().isEmpty())
        assertEquals(1, syncs)
    }

    @Test
    fun aPullToRefreshLetsEveryAccountHeldBackGo() {
        room = { false }
        addAccount()
        assertEquals(setOf(7uL), kept.load())
        hub.syncNow()
        // The core let go of them as it began the round; nothing is held here either.
        assertTrue(kept.load().isEmpty())
        assertEquals(1, syncs)
    }

    @Test
    fun aPullToRefreshThatDidNotStartKeepsTheHold() {
        kept.save(setOf(7uL))
        val hub = AccountsHub(
            api = api,
            texts = EnglishTexts,
            scope = app,
            save = {},
            copy = {},
            sync = { SyncAnswer(started = false, message = "No network") },
            holds = holds,
        )
        hub.syncNow()
        assertEquals(setOf(7uL), kept.load())
    }

    @Test
    fun aNewAccountsFirstRoundLetsNoOtherHoldGo() {
        // A pull to refresh is under way when the owner says yes to account 7's first sync...
        hub.syncNow()
        addAccount()
        // ...and then "Not now" to account 8's, before that round ends.
        runBlocking { holds.answered(8u, yes = false) }
        hub.handle(Event.SyncFinished(0u, true))
        // Account 7's round is the app's, not the owner's: 8 stays held, in the core and here.
        assertEquals(listOf("owner", "first"), rounds)
        assertEquals(setOf(8uL), kept.load())
        // (8's hold is said again before that round, as before every round the app starts.)
        assertEquals(listOf(7uL to false, 8uL to true, 8uL to true), told)
    }

    @Test
    fun aHoldTheCoreMissedIsSaidAgainBeforeANewAccountsFirstRound() {
        // "Not now" to account 8, which the core didn't hear (its queue was full); kept here.
        refusals = 1
        runBlocking { holds.answered(8u, yes = false) }
        assertTrue(told.isEmpty())
        addAccount()
        // Account 7's round starts only once 8's hold has been said again.
        assertEquals(listOf(7uL to false, 8uL to true), told)
        assertEquals(listOf("first"), rounds)
    }

    @Test
    fun aNewAccountsFirstRoundDoesNotStartWhileAHoldCannotBeSaid() {
        kept.save(setOf(8uL))
        // The yes for account 7 is heard; 8's hold, said again before the round, is not.
        val hub = AccountsHub(
            api = api,
            texts = EnglishTexts,
            scope = app,
            save = {},
            copy = {},
            firstRound = {
                rounds.add("first")
                SyncAnswer(started = true, message = "")
            },
            holds = FirstSyncHolds(kept) { account, held ->
                if (account == 8uL) throw CoreException.Failed("busy")
                told.add(account to held)
            },
        )
        val actions = hub.controller.actions()
        actions.onEmail("ada@example.org")
        actions.onSignIn()
        assertTrue(rounds.isEmpty())
        assertFalse(hub.syncing.value)
        assertEquals("Send/Receive couldn't start. Try again.", hub.syncNotice.value)
        assertEquals(setOf(8uL), kept.load())
    }
}
