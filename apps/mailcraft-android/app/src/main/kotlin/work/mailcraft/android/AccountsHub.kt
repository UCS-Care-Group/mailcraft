// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.Event

/**
 * The one reader of the core's event stream. The stream has a single consumer, so it is read here,
 * in the application's scope (not a window's, which is gone when the system recreates the activity
 * or reclaims it while the browser is in front), and each event goes to every handler that was
 * registered; a handler leaves alone the events that are not its own.
 */
internal class EventDispatcher {
    private val handlers = CopyOnWriteArrayList<(Event) -> Unit>()

    fun register(handler: (Event) -> Unit) {
        handlers.add(handler)
    }

    fun dispatch(event: Event) {
        handlers.forEach { it(event) }
    }

    /** Reads [next] until the stream is over (null), in [scope], and hands each event on. */
    fun launchIn(scope: CoroutineScope, next: suspend () -> Event?) {
        scope.launch {
            while (true) {
                val event = next() ?: break
                dispatch(event)
            }
        }
    }
}

/**
 * What the account screens need that must outlive the window: the controller (a sign-in that waits
 * for the owner in the browser, a grant, a removal) and the mailbox's first opening. It is made
 * once, with the application's scope, so a rotation, a dark-mode or language switch, or the
 * system reclaiming the activity while Microsoft's page is in front, doesn't cancel the sign-in
 * and lose what the owner just did there. The windows only read [controller]'s state.
 *
 * [restore] opens what was kept and runs once, here; [keep] keeps the mailbox as it is now.
 */
@Suppress("LongParameterList")
internal class AccountsHub(
    api: AccountsApi,
    texts: Texts,
    scope: CoroutineScope,
    restore: suspend () -> Unit,
    private val keep: suspend () -> Unit,
    copy: (String) -> Unit,
    /** True while changes can't be kept (the kept mailbox could not be read or put aside). */
    savingOff: StateFlow<Boolean> = MutableStateFlow(false),
) {
    /** True once the kept mailbox was opened (or there was none). */
    val restored = MutableStateFlow(false)

    /** Bumped when the mailbox's accounts changed: the outline is read again. */
    val reloads = MutableStateFlow(0)

    @Volatile
    private var shown: List<AccountItem> = emptyList()

    val controller = AccountsController(
        api = api,
        scope = scope,
        texts = texts,
        accounts = { shown },
        changed = {
            keep()
            reloads.update { it + 1 }
        },
        copy = copy,
    )

    init {
        scope.launch {
            restore()
            restored.value = true
        }
        scope.launch { savingOff.collect { off -> controller.setSavingOff(off) } }
    }

    /** The accounts as the window has them now. */
    fun showing(accounts: List<AccountItem>) {
        shown = accounts
    }

    /** Takes the events that are the account screens' (a permission question, a sign-in that is over). */
    fun handle(event: Event) {
        when (event) {
            is Event.PermissionQuestion -> controller.askFor(event.account, PermissionKind.of(event.permission))
            is Event.SignInNeeded -> controller.signInNeeded(event.account)
            else -> Unit
        }
    }
}
