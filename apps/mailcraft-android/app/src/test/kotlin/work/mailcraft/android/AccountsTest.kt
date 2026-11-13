// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** An [AccountsApi] that answers as the test says; every address and name is invented. */
private class FakeApi : AccountsApi {
    var signedIn: SignInResult = SignInResult.SignedIn(7u, "ada@example.org", "Ada Example")
    var granted: GrantResult = GrantResult.Granted
    var states = mutableMapOf<PermissionKind, PermissionStatus>()
    var removable = true
    var link: String? = "https://login.example.org/organizations/v2.0/adminconsent?client_id=x"
    val signIns = mutableListOf<String>()
    val grants = mutableListOf<Pair<ULong, PermissionKind>>()
    val removed = mutableListOf<ULong>()
    var cancelled = 0
    var gate: CompletableDeferred<Unit>? = null

    override suspend fun signIn(email: String): SignInResult {
        signIns.add(email)
        gate?.await()
        return signedIn
    }

    override suspend fun grant(account: ULong, permission: PermissionKind): GrantResult {
        grants.add(account to permission)
        gate?.await()
        return granted
    }

    override suspend fun status(account: ULong, permission: PermissionKind) = states[permission] ?: PermissionStatus.NotAllowed

    override suspend fun remove(account: ULong): Boolean {
        removed.add(account)
        return removable
    }

    override fun approvalLink(email: String): String? = link

    override fun cancel() {
        cancelled++
    }
}

private val ada = AccountItem(7u, "Ada Example", "ada@example.org", emptyList())

private class Rig(val api: FakeApi = FakeApi()) {
    var accounts = listOf(ada)
    var changes = 0
    val copied = mutableListOf<String>()
    val controller = AccountsController(
        api = api,
        scope = CoroutineScope(Dispatchers.Unconfined),
        texts = EnglishTexts,
        accounts = { accounts },
        changed = { changes++ },
        copy = { copied.add(it) },
    )
    val ui get() = controller.ui.value
    val actions = controller.actions()
}

class AccountsTest {
    @Test
    fun aSignInThatBringsAnAccountClosesTheScreenAndTellsTheAppTheAccountsChanged() {
        val rig = Rig()
        rig.actions.onEmail("ada@example.org")
        rig.actions.onSignIn()
        assertEquals(listOf("ada@example.org"), rig.api.signIns)
        assertEquals(AddAccountState.Done(7u), rig.ui.add)
        assertEquals(1, rig.changes)
    }

    @Test
    fun whileMicrosoftsPageIsOpenTheScreenWaitsAndCancelCallsItOff() {
        val api = FakeApi().apply { gate = CompletableDeferred() }
        val rig = Rig(api)
        rig.actions.onEmail("ada@example.org")
        rig.actions.onSignIn()
        assertEquals(AddAccountState.Waiting("ada@example.org"), rig.ui.add)
        // Typing and a second tap do nothing while it waits.
        rig.actions.onEmail("other@example.org")
        rig.actions.onSignIn()
        assertEquals(listOf("ada@example.org"), api.signIns)
        rig.actions.onCancelSignIn()
        assertEquals(1, api.cancelled)
        api.signedIn = SignInResult.Cancelled
        api.gate?.complete(Unit)
        assertEquals(AddAccountState.Form("ada@example.org", Notice.Cancelled), rig.ui.add)
        assertEquals(0, rig.changes)
    }

    @Test
    fun leavingWhileItWaitsLeavesTheFormAsNewAndTheLateAnswerIsNotShown() {
        val api = FakeApi().apply { gate = CompletableDeferred() }
        val rig = Rig(api)
        rig.actions.onEmail("ada@example.org")
        rig.actions.onSignIn()
        rig.actions.onLeaveAdd()
        assertEquals(AddAccountState.Form(), rig.ui.add)
        api.signedIn = SignInResult.Declined
        api.gate?.complete(Unit)
        assertEquals(AddAccountState.Form(), rig.ui.add)
    }

    @Test
    fun eachWayASignInEndsHasItsOwnScreenOrNotice() {
        val ends = listOf(
            SignInResult.Declined to Notice.Declined,
            SignInResult.TimedOut to Notice.TimedOut,
            SignInResult.Cancelled to Notice.Cancelled,
            SignInResult.OrganisationBlocks to Notice.OrganisationBlocks,
            SignInResult.NoBrowser to Notice.NoBrowser,
        )
        ends.forEach { (result, notice) ->
            assertEquals(AddAccountState.Form("a@example.org", notice), afterSignIn("a@example.org", result, null))
        }
        val failed = afterSignIn("a@example.org", SignInResult.Failed("why"), null) as AddAccountState.Form
        assertEquals("The sign-in didn't go through.\nwhy", failed.notice?.text(EnglishTexts))
        val approval = afterSignIn("a@example.org", SignInResult.NeedsApproval, "https://x.example/")
        assertEquals(AddAccountState.Approval("a@example.org", "https://x.example/"), approval)
    }

    @Test
    fun theConditionalAccessNoticeSaysItIsThePolicyAndNotAFault() {
        val text = Notice.OrganisationBlocks.text(EnglishTexts)
        assertTrue(text.contains("Conditional Access"))
        assertTrue(text.contains("not a fault"))
        assertTrue(text.contains("Ask"))
    }

    @Test
    fun anApprovalNeededSignInShowsTheHelpWithTheMessageAndSignInAgainTriesAgain() {
        val api = FakeApi().apply { signedIn = SignInResult.NeedsApproval }
        val rig = Rig(api)
        rig.actions.onEmail("ada@lumen.example")
        rig.actions.onSignIn()
        val help = rig.ui.add as AddAccountState.Approval
        assertEquals(api.link, help.link)
        assertFalse(help.copied)
        val message = adminMessage(EnglishTexts, help.link.orEmpty())
        rig.actions.onCopyMessage(message)
        assertEquals(listOf(message), rig.copied)
        assertTrue((rig.ui.add as AddAccountState.Approval).copied)
        // Sign in again, now that an administrator has approved.
        api.signedIn = SignInResult.SignedIn(7u, "ada@lumen.example", "Ada")
        rig.actions.onSignIn()
        assertEquals(listOf("ada@lumen.example", "ada@lumen.example"), api.signIns)
        assertEquals(AddAccountState.Done(7u), rig.ui.add)
    }

    @Test
    fun theMessageForAnAdministratorHasThePageAndNothingOfTheOwner() {
        val link = "https://login.example.org/lumen.example/v2.0/adminconsent?client_id=abc"
        val message = adminMessage(EnglishTexts, link)
        assertTrue(message.contains(link))
        assertFalse(message.contains("{link}"))
        assertTrue(message.contains("my own phone"))
        assertFalse(message.contains("ada"))
    }

    @Test
    fun aPermissionQuestionWaitsForTheOwnerAndOnlyTheOwnersYesOpensThePage() {
        val rig = Rig()
        rig.controller.askFor(7u, PermissionKind.Tasks)
        assertEquals(QuestionState.Asking(PermissionAsk(7u, PermissionKind.Tasks)), rig.ui.question)
        assertTrue("nothing is opened before a yes", rig.api.grants.isEmpty())
        // Not now: nothing is asked of Microsoft.
        rig.actions.onNotNow()
        assertNull(rig.ui.question)
        assertTrue(rig.api.grants.isEmpty())
        // A question about an account that isn't there is not put.
        rig.controller.askFor(99u, PermissionKind.Tasks)
        assertNull(rig.ui.question)
        // A yes opens Microsoft's page for that permission of that account.
        rig.controller.askFor(7u, PermissionKind.SharedSend)
        rig.actions.onAllow()
        assertEquals(listOf(7uL to PermissionKind.SharedSend), rig.api.grants)
        assertNull(rig.ui.question)
        assertEquals(PermissionStatus.NotAllowed, rig.ui.statuses[StatusKey(7u, PermissionKind.SharedSend)])
    }

    @Test
    fun aSecondQuestionDoesNotReplaceTheOneOnScreen() {
        val rig = Rig()
        rig.controller.askFor(7u, PermissionKind.Tasks)
        rig.controller.askFor(7u, PermissionKind.SharedMail)
        assertEquals(PermissionKind.Tasks, rig.ui.question?.ask?.permission)
    }

    @Test
    fun aPermissionThatIsRefusedIsExplainedAndTheOwnerDismissesIt() {
        val api = FakeApi().apply { granted = GrantResult.NeedsApproval }
        val rig = Rig(api)
        rig.controller.askFor(7u, PermissionKind.Tasks)
        rig.actions.onAllow()
        val told = rig.ui.question as QuestionState.Told
        assertTrue(told.text.contains("administrator"))
        rig.actions.onNotNow()
        assertNull(rig.ui.question)
        // A call off is not a refusal: nothing is said.
        api.granted = GrantResult.Cancelled
        rig.controller.askFor(7u, PermissionKind.Tasks)
        rig.actions.onAllow()
        assertNull(rig.ui.question)
    }

    @Test
    fun whileThePermissionPageIsOpenTheQuestionCannotBeDismissedButCanBeCancelled() {
        val api = FakeApi().apply { gate = CompletableDeferred() }
        val rig = Rig(api)
        rig.controller.askFor(7u, PermissionKind.Tasks)
        rig.actions.onAllow()
        assertTrue(rig.ui.question is QuestionState.Waiting)
        rig.actions.onNotNow()
        assertTrue(rig.ui.question is QuestionState.Waiting)
        rig.actions.onCancelGrant()
        assertEquals(1, api.cancelled)
    }

    @Test
    fun everyWayAPermissionEndsHasWordsExceptYesAndCancel() {
        val noText = listOf(GrantResult.Granted, GrantResult.Cancelled)
        val withText = listOf(
            GrantResult.NeedsApproval,
            GrantResult.OrganisationBlocks,
            GrantResult.NotForThisAccount,
            GrantResult.Declined,
            GrantResult.TimedOut,
            GrantResult.NoBrowser,
            GrantResult.Failed("because"),
        )
        noText.forEach { assertNull(grantNotice(EnglishTexts, it)) }
        withText.forEach { assertTrue(it.toString(), !grantNotice(EnglishTexts, it).isNullOrBlank()) }
        assertTrue(grantNotice(EnglishTexts, GrantResult.OrganisationBlocks).orEmpty().contains("not a fault"))
    }

    @Test
    fun theAccountsPageAsksWhereEachPermissionStandsOnceAndShowsWhatTheCoreSays() {
        val api = FakeApi().apply { states[PermissionKind.Tasks] = PermissionStatus.Allowed }
        val rig = Rig(api)
        rig.actions.onShown(listOf(ada))
        assertEquals(PermissionStatus.Allowed, rig.ui.statuses[StatusKey(7u, PermissionKind.Tasks)])
        assertEquals(PermissionStatus.NotAllowed, rig.ui.statuses[StatusKey(7u, PermissionKind.SharedMail)])
        assertEquals(PermissionKind.entries.size, rig.ui.statuses.size)
        // Shown again: not asked again.
        api.states[PermissionKind.Tasks] = PermissionStatus.NotAllowed
        rig.actions.onShown(listOf(ada))
        assertEquals(PermissionStatus.Allowed, rig.ui.statuses[StatusKey(7u, PermissionKind.Tasks)])
    }

    @Test
    fun anAccountsSignInBeingOverIsShownOnAllItsPermissionsAndSignInAgainOpensTheFormWithItsAddress() {
        val rig = Rig()
        rig.controller.signInNeeded(7u)
        assertEquals(PermissionKind.entries.size, rig.ui.statuses.values.count { it == PermissionStatus.SignInNeeded })
        rig.actions.onSignInAgain(ada)
        assertEquals(AddAccountState.Form("ada@example.org"), rig.ui.add)
    }

    @Test
    fun removingAnAccountIsAskedFirstAndThenTheAppReadsTheListAgain() {
        val rig = Rig()
        rig.actions.onShown(listOf(ada))
        rig.actions.onRemoveAsk(ada)
        assertEquals(ada, rig.ui.removing)
        assertTrue("nothing is removed before a yes", rig.api.removed.isEmpty())
        rig.actions.onRemoveCancel()
        assertNull(rig.ui.removing)
        assertTrue(rig.api.removed.isEmpty())
        rig.actions.onRemoveAsk(ada)
        rig.actions.onRemove()
        assertEquals(listOf(7uL), rig.api.removed)
        assertNull(rig.ui.removing)
        assertTrue(rig.ui.statuses.isEmpty())
        assertEquals(1, rig.changes)
    }

    @Test
    fun aTypedAddressIsBoundedAndTypingClearsTheNotice() {
        val rig = Rig()
        rig.actions.onEmail("a".repeat(5000))
        assertEquals(254, (rig.ui.add as AddAccountState.Form).email.length)
        rig.api.signedIn = SignInResult.Declined
        rig.actions.onSignIn()
        assertTrue((rig.ui.add as AddAccountState.Form).notice != null)
        rig.actions.onEmail("b@example.org")
        assertNull((rig.ui.add as AddAccountState.Form).notice)
    }

    @Test
    fun theTextsOfTheAccountScreensAreEnglishAsWritten() {
        assertEquals("To Do tasks", PermissionKind.Tasks.label(EnglishTexts))
        assertEquals("Allowed", statusText(EnglishTexts, PermissionStatus.Allowed))
        assertEquals("Couldn't check", statusText(EnglishTexts, PermissionStatus.Unknown))
        PermissionKind.entries.forEach {
            assertTrue(it.question(EnglishTexts).endsWith("?"))
            assertTrue(it.explanation(EnglishTexts).contains("Microsoft's page"))
        }
    }

    @Test
    fun aSignInAgainThatWorksDropsWhatWasKnownOfTheAccountSoItIsAskedAgain() {
        val api = FakeApi()
        val rig = Rig(api)
        rig.controller.signInNeeded(7u)
        rig.actions.onSignInAgain(ada)
        rig.actions.onSignIn()
        assertEquals(AddAccountState.Done(7u), rig.ui.add)
        assertTrue(rig.ui.statuses.isEmpty())
        // The page asks again when it is shown, and now says what the core says.
        api.states[PermissionKind.Tasks] = PermissionStatus.Allowed
        rig.actions.onShown(listOf(ada))
        assertEquals(PermissionStatus.Allowed, rig.ui.statuses[StatusKey(7u, PermissionKind.Tasks)])
    }

    @Test
    fun aSignInAgainDoesNotReplaceASignInThatIsWaiting() {
        val api = FakeApi().apply { gate = CompletableDeferred() }
        val rig = Rig(api)
        rig.actions.onEmail("ada@example.org")
        rig.actions.onSignIn()
        rig.actions.onSignInAgain(ada)
        assertEquals(AddAccountState.Waiting("ada@example.org"), rig.ui.add)
    }

    @Test
    fun aPermissionThatCouldNotBeCheckedIsAskedAgainTheNextTimeThePageIsShown() {
        val api = FakeApi().apply { states[PermissionKind.Tasks] = PermissionStatus.Unknown }
        val rig = Rig(api)
        rig.actions.onShown(listOf(ada))
        assertEquals(PermissionStatus.Unknown, rig.ui.statuses[StatusKey(7u, PermissionKind.Tasks)])
        api.states[PermissionKind.Tasks] = PermissionStatus.Allowed
        rig.actions.onShown(listOf(ada))
        assertEquals(PermissionStatus.Allowed, rig.ui.statuses[StatusKey(7u, PermissionKind.Tasks)])
    }
}
