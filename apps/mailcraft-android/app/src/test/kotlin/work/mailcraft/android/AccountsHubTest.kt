// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.StorageNotice

private class HubApi(val gate: CompletableDeferred<Unit>) : AccountsApi {
    override suspend fun signIn(email: String): SignInResult {
        gate.await()
        return SignInResult.SignedIn(7u, "ada@example.org", "Ada Example")
    }

    override suspend fun grant(account: ULong, permission: PermissionKind): GrantResult = GrantResult.Granted

    override suspend fun status(account: ULong, permission: PermissionKind) = PermissionStatus.Allowed

    override suspend fun remove(account: ULong) = true

    override fun approvalLink(email: String): String? = null

    override fun cancel() = Unit
}

/** The account work lives in the application's scope: a window that goes away takes none of it with it. */
class AccountsHubTest {
    private val app = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
    private var kept = 0
    private val gate = CompletableDeferred<Unit>()
    private val hub = AccountsHub(
        api = HubApi(gate),
        texts = EnglishTexts,
        scope = app,
        save = { kept++ },
        copy = {},
    )

    @Test
    fun aSignInThatWaitsInTheBrowserSurvivesTheLossOfTheWindowThatStartedIt() {
        // The window: it reads the controller's state in a scope of its own, which the system cancels.
        val window = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined)
        val seen = mutableListOf<AddAccountState>()
        window.launch { hub.controller.ui.collect { seen.add(it.add) } }
        hub.controller.actions().onEmail("ada@example.org")
        hub.controller.actions().onSignIn()
        assertEquals(AddAccountState.Waiting("ada@example.org"), hub.controller.ui.value.add)
        // The activity is recreated (rotation, dark mode, language) while Microsoft's page is in front.
        window.cancel()
        // The owner finishes in the browser.
        gate.complete(Unit)
        assertEquals(AddAccountState.Done(7u), hub.controller.ui.value.add)
        assertEquals(1, kept)
        assertEquals(1, hub.reloads.value)
        // The window that was lost saw the wait, and nothing after.
        assertEquals(AddAccountState.Waiting("ada@example.org"), seen.last())
    }

    @Test
    fun aRevisionTheCoreAnnouncesHasTheMailboxReadAgain() {
        // A synchronisation that ends after the window read the outline changes the revision: the
        // window reads the outline, and with it the folder's messages, again.
        val before = hub.reloads.value
        hub.handle(Event.RevChanged(42u))
        hub.handle(Event.RevChanged(43u))
        assertEquals(before + 2, hub.reloads.value)
        // Events that are not the mailbox's change nothing.
        hub.handle(Event.SyncProgress(7u, "mail", 1u, 2u))
        assertEquals(before + 2, hub.reloads.value)
    }

    @Test
    fun aFailedSaveIsShownTriedAgainByItselfAndClearedWhenItWorks() {
        val pauses = mutableListOf<Long>()
        var failures = 2
        var saves = 0
        val saving = AccountsHub(
            api = HubApi(CompletableDeferred()),
            texts = EnglishTexts,
            scope = app,
            save = {
                saves++
                if (failures-- > 0) throw CoreException.Failed("disk full")
            },
            copy = {},
            retryAfter = listOf(1L, 2L),
            pause = { pauses.add(it) },
        )
        // The core says a save failed (it tries again too): the notice is up, and the hub tries.
        saving.handle(Event.SaveFailed("disk full"))
        // (Unconfined: the retries ran at once, the first two failing, the third working.)
        assertEquals(3, saves)
        assertEquals(listOf(1L, 2L, 2L), pauses)
        assertFalse(saving.controller.ui.value.saveFailed)
    }

    @Test
    fun theRetryOfTheNoticeSavesThroughTheCoreAndSaysWhetherItWorked() = runBlocking {
        var fail = true
        val saving = AccountsHub(
            api = HubApi(CompletableDeferred()),
            texts = EnglishTexts,
            scope = CoroutineScope(SupervisorJob() + Dispatchers.Unconfined),
            save = { if (fail) throw CoreException.Failed("disk full") },
            copy = {},
            pause = { kotlinx.coroutines.awaitCancellation() },
        )
        assertFalse(saving.saveNow())
        assertTrue(saving.controller.ui.value.saveFailed)
        fail = false
        assertTrue(saving.saveNow())
        assertFalse(saving.controller.ui.value.saveFailed)
    }

    @Test
    fun aSaveThatFailsAfterSavingWasTurnedOffLeavesThatNoticeAlone() = runBlocking {
        val gate = CompletableDeferred<Unit>()
        lateinit var saving: AccountsHub
        saving = AccountsHub(
            api = HubApi(CompletableDeferred()),
            texts = EnglishTexts,
            scope = app,
            save = {
                gate.await()
                throw CoreException.Failed("nothing is saved")
            },
            copy = {},
        )
        val save = app.launch { saving.saveNow() }
        // The file turns out to be unkeepable while that save runs.
        saving.handle(Event.StorageNotice(StorageNotice.NOT_KEPT, ""))
        gate.complete(Unit)
        save.join()
        assertTrue(saving.controller.ui.value.savingOff)
        assertFalse(saving.controller.ui.value.saveFailed)
    }

    @Test
    fun aMailboxThatCouldNotBeKeptAsideIsNeverSavedOver() = runBlocking {
        var saves = 0
        val saving = AccountsHub(
            api = HubApi(CompletableDeferred()),
            texts = EnglishTexts,
            scope = app,
            save = { saves++ },
            copy = {},
        )
        saving.handle(Event.StorageNotice(StorageNotice.NOT_KEPT, ""))
        assertTrue(saving.controller.ui.value.savingOff)
        assertFalse(saving.saveNow())
        saving.handle(Event.SaveFailed("nothing is saved"))
        assertEquals(0, saves)
        assertFalse(saving.controller.ui.value.saveFailed)
    }

    @Test
    fun anAccountsQuestionIsPutWhileNoWindowIsOpenAndIsThereWhenOneComes() {
        hub.showing(listOf(AccountItem(7u, "Ada Example", "ada@example.org", emptyList())))
        hub.controller.askFor(7u, PermissionKind.Tasks)
        assertEquals(QuestionState.Asking(PermissionAsk(7u, PermissionKind.Tasks)), hub.controller.ui.value.question)
    }

    @Test
    fun theOneReaderOfTheStreamGivesEveryEventToEveryHandler() {
        val dispatcher = EventDispatcher()
        val a = mutableListOf<uniffi.mailcraft_mobile.Event>()
        val b = mutableListOf<uniffi.mailcraft_mobile.Event>()
        dispatcher.register { a.add(it) }
        dispatcher.register { b.add(it) }
        val events = ArrayDeque(listOf<uniffi.mailcraft_mobile.Event?>(uniffi.mailcraft_mobile.Event.SignInNeeded(7u), null))
        dispatcher.launchIn(app) { events.removeFirst() }
        assertEquals(1, a.size)
        assertEquals(a, b)
    }

    @Test
    fun theApplicationsScopeIsAJobNoWindowOwns() {
        assertTrue(app.coroutineContext[Job]?.isActive == true)
    }
}
