// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
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
@Suppress("TooManyFunctions", "LongParameterList")
internal class AccountsController(
    private val api: AccountsApi,
    private val scope: CoroutineScope,
    private val texts: Texts,
    private val accounts: () -> List<AccountItem>,
    private val changed: suspend () -> Unit,
    private val copy: (String) -> Unit,
    private val retrySave: suspend () -> Unit = {},
) {
    val ui = MutableStateFlow(AccountsUi())

    fun actions(): AccountActions = AccountActions(
        onEmail = ::typed,
        onSignIn = ::signIn,
        onCancelSignIn = ::cancelSignIn,
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
        onRestartInterrupted = ::restartInterrupted,
        onDismissInterrupted = ::dismissInterrupted,
        onRetrySave = { scope.launch { retrySave() } },
    )

    /** The kept mailbox can't be written this session: the screens say so. */
    fun setSavingOff(off: Boolean) = ui.update { it.copy(savingOff = off) }

    /** The last save of the mailbox failed (or worked again): the screens say it isn't saved yet. */
    fun setSaveFailed(failed: Boolean) = ui.update { it.copy(saveFailed = failed) }

    /**
     * Android closed the app while the provider's page was open and the owner came back: the sign-in
     * that was under way is gone with the process, so the screens say so and offer a new one.
     */
    fun signInInterrupted(email: String) = ui.update { state ->
        if (state.add is AddAccountState.Waiting) state else state.copy(interrupted = email.take(MAX_EMAIL))
    }

    private fun dismissInterrupted() = ui.update { it.copy(interrupted = null) }

    /** The form, its address as the hint, for the sign-in that was interrupted. */
    private fun restartInterrupted() = ui.update { state ->
        val email = state.interrupted ?: return@update state
        if (state.add is AddAccountState.Waiting) {
            state.copy(interrupted = null)
        } else {
            state.copy(add = AddAccountState.Form(email), interrupted = null)
        }
    }

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

    /** Which sign-in owns the Add Account screen: a result of an earlier one is never shown. */
    private var signInOperation = 0L

    /** The last sign-in started: the next waits for it to be over. */
    private var signInJob: Job? = null

    /** The sign-in that is waiting for an earlier one to be over, and hasn't opened its page; 0 when none is. */
    private var queued = 0L

    /** Cancel on the waiting screen: the sign-in under way is called off, and one still queued never begins. */
    private fun cancelSignIn() {
        api.cancel()
        if (queued != 0L && queued == signInOperation) {
            queued = 0L
            signInOperation++
            ui.update { state ->
                val add = state.add
                if (add is AddAccountState.Waiting) {
                    state.copy(add = AddAccountState.Form(add.email, Notice.Cancelled))
                } else {
                    state
                }
            }
        }
    }

    /** Opens Microsoft's page and waits for the owner. Only from the form or the approval help. */
    fun signIn() {
        val email = emailOf(ui.value.add) ?: return
        val operation = ++signInOperation
        ui.update { it.copy(add = AddAccountState.Waiting(email)) }
        val previous = signInJob
        signInJob = scope.launch {
            // A sign-in the owner left (leaving the screen calls it off) is over before this one begins:
            // one at a time, so that its end can't call off, or disarm, the new one.
            if (previous?.isActive == true) {
                queued = operation
                api.cancel()
                previous.join()
                if (queued == operation) queued = 0L
            }
            // (Called off, or left for a newer one, while it waited: it never opens its page.)
            if (operation != signInOperation) return@launch
            val result = api.signIn(email)
            val link = if (result is SignInResult.NeedsApproval) api.approvalLink(email) else null
            if (result is SignInResult.SignedIn) {
                // The sign-in is new: where this account's permissions stand is asked again, now (the
                // Accounts page may stay on screen with the same list, which asks nothing by itself).
                PermissionKind.entries.forEach { setStatus(result.account, it) }
                changed()
            }
            ui.update { state ->
                // (The owner may have left the screen meanwhile: what is there is left as it is.)
                val current = operation == signInOperation && state.add is AddAccountState.Waiting
                if (current) state.copy(add = afterSignIn(email, result, link)) else state
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

    /** The form for an account the owner has, its address as the hint; a sign-in that waits is left as it is. */
    private fun signInAgain(account: AccountItem) = ui.update { state ->
        if (state.add is AddAccountState.Waiting) state else state.copy(add = AddAccountState.Form(account.email))
    }

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
        // (An answer still on its way is older than this and must not replace it.)
        over.keys.forEach { generations[it] = (generations[it] ?: 0) + 1 }
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

    private fun removeAsk(account: AccountItem) = ui.update { it.copy(removing = account, removeFailed = false) }

    private fun removeCancel() = ui.update { it.copy(removing = null, removeFailed = false) }

    /** The accounts whose removal is under way: a second tap on Remove for one of them does nothing. */
    private val removals = HashSet<ULong>()

    private fun remove() {
        val account = ui.value.removing ?: return
        if (!removals.add(account.id)) return
        scope.launch {
            var removed = false
            try {
                removed = api.remove(account.id)
            } finally {
                removals.remove(account.id)
            }
            // The confirmation is this removal's only while it is still the one for this account: one
            // the owner dismissed, or another account's opened meanwhile, is left as it is.
            fun owns(state: AccountsUi) = state.removing?.id == account.id
            // (Only a removal that happened cleans up; a failed one says so and can be tried again.)
            if (!removed) {
                ui.update { state -> if (owns(state)) state.copy(removeFailed = true) else state }
                return@launch
            }
            ui.update { state ->
                val closed = if (owns(state)) state.copy(removing = null, removeFailed = false) else state
                closed.copy(statuses = closed.statuses.filterKeys { it.account != account.id })
            }
            changed()
        }
    }

    /** Asks the core where each permission of each account stands (the Accounts page is on screen). */
    private fun refreshStatuses(shown: List<AccountItem>) {
        shown.forEach { account ->
            PermissionKind.entries.forEach { kind ->
                val key = StatusKey(account.id, kind)
                // (Not known yet, or couldn't be told the last time.)
                val known = ui.value.statuses[key]
                if (known == null || known == PermissionStatus.Unknown) setStatus(account.id, kind)
            }
        }
    }

    private fun setStatus(account: ULong, permission: PermissionKind) {
        val key = StatusKey(account, permission)
        val generation = (generations[key] ?: 0) + 1
        generations[key] = generation
        ui.update { it.copy(statuses = it.statuses + (key to PermissionStatus.Checking)) }
        scope.launch {
            val status = api.status(account, permission)
            // (An account removed meanwhile has no row to keep, and a newer word about it stands.)
            ui.update { state ->
                val current = key in state.statuses && generations[key] == generation
                if (current) state.copy(statuses = state.statuses + (key to status)) else state
            }
        }
    }

    /** How many times each status was asked or overridden: a late answer to an older ask is dropped. */
    private val generations = HashMap<StatusKey, Int>()

    private companion object {
        const val MAX_EMAIL = 254
    }
}
