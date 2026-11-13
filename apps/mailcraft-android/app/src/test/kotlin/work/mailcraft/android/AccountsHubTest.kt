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
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

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
    private var restores = 0
    private var kept = 0
    private val gate = CompletableDeferred<Unit>()
    private val hub = AccountsHub(
        api = HubApi(gate),
        texts = EnglishTexts,
        scope = app,
        restore = { restores++ },
        keep = { kept++ },
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
    fun theKeptMailboxIsOpenedOnceWhateverTheWindowsDo() {
        assertEquals(1, restores)
        assertTrue(hub.restored.value)
        // Windows come and go and read the same state; none opens the mailbox again.
        repeat(3) { hub.showing(listOf(AccountItem(7u, "Ada Example", "ada@example.org", emptyList()))) }
        assertEquals(1, restores)
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
