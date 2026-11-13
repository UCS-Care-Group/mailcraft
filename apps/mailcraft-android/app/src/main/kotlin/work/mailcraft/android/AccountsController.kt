// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * What happens on the account screens: Add Account, the permission questions, Remove account. It
 * holds the state they draw ([ui]) and talks to the core only through [AccountsApi], so it runs
 * without the library. The core keeps every token; nothing here sees one.
 *
 * [accounts] is the list as the app has it now, [changed] is called after the mailbox's accounts
 * changed (to keep the mailbox and read the list again), [copy] puts text on the clipboard, and
 * [texts] says the words of a notice.
 */
@Suppress("TooManyFunctions")
internal class AccountsController(
    private val api: AccountsApi,
    private val scope: CoroutineScope,
    private val texts: Texts,
    private val accounts: () -> List<AccountItem>,
    private val changed: suspend () -> Unit,
    private val copy: (String) -> Unit,
) {
    val ui = MutableStateFlow(AccountsUi())

    fun actions(): AccountActions = AccountActions(
        onEmail = ::typed,
        onSignIn = ::signIn,
        onCancelSignIn = api::cancel,
        onCopyMessage = ::copyMessage,
        onLeaveAdd = ::leaveAdd,
        onSignInAgain = ::signInAgain,
        onAsk = { account, permission -> askFor(account.id, permission) },
        onAllow = ::allow,
        onNotNow = ::notNow,
        onCancelGrant = api::cancel,
        onRemoveAsk = ::removeAsk,
        onRemove = ::remove,
        onRemoveCancel = ::removeCancel,
        onShown = ::refreshStatuses,
    )

    private fun typed(email: String) = ui.update { state ->
        val add = state.add
        if (add is AddAccountState.Form) {
            state.copy(add = add.copy(email = email.take(MAX_EMAIL), notice = null))
        } else {
            state
        }
    }

    private fun emailOf(add: AddAccountState): String? = when (add) {
        is AddAccountState.Form -> add.email
        is AddAccountState.Approval -> add.email
        is AddAccountState.Waiting, is AddAccountState.Done -> null
    }

    /** Opens Microsoft's page and waits for the owner. Only from the form or the approval help. */
    fun signIn() {
        val email = emailOf(ui.value.add) ?: return
        ui.update { it.copy(add = AddAccountState.Waiting(email)) }
        scope.launch {
            val result = api.signIn(email)
            val link = if (result is SignInResult.NeedsApproval) api.approvalLink(email) else null
            if (result is SignInResult.SignedIn) changed()
            ui.update { state ->
                // (The owner may have left the screen meanwhile: what is there is left as it is.)
                if (state.add is AddAccountState.Waiting) state.copy(add = afterSignIn(email, result, link)) else state
            }
        }
    }

    private fun copyMessage(text: String) {
        copy(text)
        ui.update { state ->
            val add = state.add
            if (add is AddAccountState.Approval) state.copy(add = add.copy(copied = true)) else state
        }
    }

    private fun leaveAdd() = ui.update { it.copy(add = AddAccountState.Form()) }

    private fun signInAgain(account: AccountItem) = ui.update { it.copy(add = AddAccountState.Form(account.email)) }

    /** Puts the question for a permission to the owner (an event from the core, or the owner's own tap). */
    fun askFor(account: ULong, permission: PermissionKind) = ui.update { state ->
        if (state.question != null || accounts().none { it.id == account }) {
            state
        } else {
            state.copy(question = QuestionState.Asking(PermissionAsk(account, permission)))
        }
    }

    /** The core says an account's sign-in is over. */
    fun signInNeeded(account: ULong) = ui.update { state ->
        val over = PermissionKind.entries.associate { StatusKey(account, it) to PermissionStatus.SignInNeeded }
        state.copy(statuses = state.statuses + over)
    }

    /** The owner said yes: Microsoft's page opens. */
    private fun allow() {
        val asking = ui.value.question as? QuestionState.Asking ?: return
        val ask = asking.ask
        ui.update { it.copy(question = QuestionState.Waiting(ask)) }
        scope.launch {
            val result = api.grant(ask.account, ask.permission)
            val told = grantNotice(texts, result)
            ui.update { state -> state.copy(question = told?.let { QuestionState.Told(ask, it) }) }
            if (result is GrantResult.Granted) setStatus(ask.account, ask.permission)
        }
    }

    private fun notNow() = ui.update { state ->
        if (state.question is QuestionState.Waiting) state else state.copy(question = null)
    }

    private fun removeAsk(account: AccountItem) = ui.update { it.copy(removing = account) }

    private fun removeCancel() = ui.update { it.copy(removing = null) }

    private fun remove() {
        val account = ui.value.removing ?: return
        ui.update { it.copy(removing = null) }
        scope.launch {
            api.remove(account.id)
            ui.update { state -> state.copy(statuses = state.statuses.filterKeys { it.account != account.id }) }
            changed()
        }
    }

    /** Asks the core where each permission of each account stands (the Accounts page is on screen). */
    private fun refreshStatuses(shown: List<AccountItem>) {
        shown.forEach { account ->
            PermissionKind.entries.forEach { kind ->
                val key = StatusKey(account.id, kind)
                if (ui.value.statuses[key] == null) setStatus(account.id, kind)
            }
        }
    }

    private fun setStatus(account: ULong, permission: PermissionKind) {
        val key = StatusKey(account, permission)
        ui.update { it.copy(statuses = it.statuses + (key to PermissionStatus.Checking)) }
        scope.launch {
            val status = api.status(account, permission)
            // (An account removed meanwhile has no row to keep.)
            ui.update { state ->
                if (key in state.statuses) state.copy(statuses = state.statuses + (key to status)) else state
            }
        }
    }

    private companion object {
        const val MAX_EMAIL = 254
    }
}
