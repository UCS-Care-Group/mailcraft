// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.isActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** What a change of an account's window came to. */
internal data class WindowChange(val code: String, val shorter: Boolean)

/** What thinning older mail did in one call: how much is still to go. */
internal data class Thinned(val thinned: Int, val left: Int)

/** What Settings asks of the core and the phone: one seam, so that it runs without the library. */
internal interface SettingsApi {
    /** The windows the engine offers, shortest first. */
    fun codes(): List<String>

    suspend fun keepOffline(account: ULong): String?

    /** Sets the window; null when it didn't go through. */
    suspend fun setKeepOffline(account: ULong, code: String): WindowChange?

    /** Takes a few hundred messages outside their windows back to their headers. */
    suspend fun thin(): Thinned?

    /** About how many bytes of mail the window brings. */
    fun estimate(code: String): Long

    /** The bytes free on the storage the mailbox is on; negative when the phone can't say. */
    fun freeBytes(): Long

    suspend fun signature(account: ULong): SignatureState?

    /** Keeps the account's signature; false when it didn't go through. */
    suspend fun setSignature(account: ULong, text: String): Boolean

    fun switches(): Switches

    fun saveSwitches(switches: Switches)
}

/**
 * What happens on the Settings pages that change something: the windows of Mail to keep offline,
 * the signatures, the switches of this phone, and the question before a sync that needs more room
 * than there is. It holds the state they draw ([ui]) and talks to the core only through
 * [SettingsApi].
 *
 * [waitForRoom] is the one gate for work that fetches a lot: a widened window goes through it, and
 * so does the sync driver before a big first sync. It answers at once when there is room, and
 * otherwise puts the question to the owner and waits for the yes or the no.
 */
@Suppress("TooManyFunctions")
internal class SettingsController(
    private val api: SettingsApi,
    private val scope: CoroutineScope,
) {
    val ui = MutableStateFlow(SettingsUi(switches = api.switches(), codes = api.codes()))

    /** One question at a time. */
    private val asking = Mutex()
    private var answer: CompletableDeferred<Boolean>? = null

    /** The one run of thinning older mail: a second window choice, or Stop, ends it. */
    private var thinning: Job? = null

    fun actions(
        onChannelSettings: (String) -> Unit = {},
        onOpenLink: (String) -> Unit = {},
    ): SettingsActions = SettingsActions(
        onShown = ::shown,
        onSwitches = ::switches,
        onKeepOffline = { account, code -> chooseWindow(account.id, code) },
        onThinStop = ::stopThinning,
        onSpaceYes = { answered(true) },
        onSpaceNo = { answered(false) },
        onSignatureDraft = { account, text -> draft(account.id, text) },
        onSignatureSave = { account -> saveSignature(account.id) },
        onChannelSettings = onChannelSettings,
        onOpenLink = onOpenLink,
    )

    /** What is kept for each account is read once; an account removed is forgotten. */
    private fun shown(accounts: List<AccountItem>) {
        val ids = accounts.map { it.id }.toSet()
        ui.update { state ->
            state.copy(
                keepOffline = state.keepOffline.filterKeys { it in ids },
                signatures = state.signatures.filterKeys { it in ids },
            )
        }
        accounts.forEach { account ->
            if (account.id !in ui.value.keepOffline) readWindow(account.id)
            if (account.id !in ui.value.signatures) readSignature(account.id)
        }
    }

    private fun readWindow(account: ULong) {
        scope.launch {
            val code = api.keepOffline(account) ?: return@launch
            ui.update { state -> state.copy(keepOffline = state.keepOffline + (account to code)) }
        }
    }

    private fun readSignature(account: ULong) {
        scope.launch {
            val found = api.signature(account) ?: return@launch
            val read = found.copy(draft = found.saved)
            ui.update { state -> state.copy(signatures = state.signatures + (account to read)) }
        }
    }

    private fun switches(switches: Switches) {
        ui.update { it.copy(switches = switches) }
        api.saveSwitches(switches)
    }

    /** The owner picked a window: a longer one waits for room, a shorter one is applied at once. */
    private fun chooseWindow(account: ULong, code: String) {
        val state = ui.value
        val current = state.keepOffline[account] ?: DEFAULT_OFFLINE
        if (code == current || code !in state.codes) return
        scope.launch {
            val more = if (keepsMore(state.codes, code, current)) api.estimate(code) - api.estimate(current) else 0L
            // (A window that fetches nothing needs no room, however little there is.)
            if (more <= 0 || waitForRoom(account, more, code)) applyWindow(account, code)
        }
    }

    private suspend fun applyWindow(account: ULong, code: String) {
        val changed = api.setKeepOffline(account, code)
        ui.update { state ->
            val kept = if (changed != null) state.keepOffline + (account to changed.code) else state.keepOffline
            state.copy(keepOffline = kept, keepFailed = changed == null)
        }
        // A shorter window takes nothing off the disk by itself: older mail goes back to its
        // headers a few hundred messages a call, until none is left.
        // It stops at Stop and at the next choice; the sync driver takes it over when it exists.
        if (changed?.shorter == true) startThinning()
    }

    private fun startThinning() {
        thinning?.cancel()
        ui.update { state -> state.copy(thinning = true, thinned = 0L) }
        val run = scope.launch { thinAll() }
        thinning = run
        run.invokeOnCompletion { if (thinning === run) ui.update { state -> state.copy(thinning = false) } }
    }

    /** Stops taking older mail back to its headers; what was thinned stays thinned. */
    fun stopThinning() {
        thinning?.cancel()
    }

    private suspend fun thinAll() {
        repeat(MAX_THIN_CALLS) {
            if (!currentCoroutineContext().isActive) return
            val step = api.thin() ?: return
            ui.update { state -> state.copy(thinned = state.thinned + step.thinned.coerceAtLeast(0)) }
            if (step.left <= 0 || step.thinned <= 0) return
        }
    }

    /**
     * Whether a sync needing about [needed] bytes for [account] may start: true at once when there
     * is room, else the owner is asked, saying the two sizes, and this waits for the answer.
     */
    suspend fun waitForRoom(account: ULong, needed: Long, forWindow: String? = null): Boolean {
        if (needed <= 0) return true
        val warning = spaceCheck(needed, api.freeBytes()) ?: return true
        return asking.withLock {
            val reply = CompletableDeferred<Boolean>()
            answer = reply
            ui.update { it.copy(space = SpaceAsk(account, forWindow, warning)) }
            try {
                reply.await()
            } finally {
                answer = null
                ui.update { it.copy(space = null) }
            }
        }
    }

    private fun answered(yes: Boolean) {
        answer?.complete(yes)
    }

    private fun draft(account: ULong, text: String) = ui.update { state ->
        val now = state.signatures[account] ?: return@update state
        if (!now.editable) return@update state
        val typed = now.copy(draft = text.take(MAX_SIGNATURE), failed = false)
        state.copy(signatures = state.signatures + (account to typed))
    }

    private fun saveSignature(account: ULong) {
        val now = ui.value.signatures[account] ?: return
        if (!now.editable || !now.changed) return
        val text = now.draft
        scope.launch {
            val kept = api.setSignature(account, text)
            ui.update { state ->
                val here = state.signatures[account] ?: return@update state
                val next = if (kept) here.copy(saved = text, failed = false) else here.copy(failed = true)
                state.copy(signatures = state.signatures + (account to next))
            }
        }
    }

    private companion object {
        /** Calls of a few hundred messages each: a mailbox is far smaller than this reaches. */
        const val MAX_THIN_CALLS = 500
    }
}
