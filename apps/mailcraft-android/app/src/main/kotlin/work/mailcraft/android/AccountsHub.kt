// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.Event
import uniffi.mailcraft_mobile.StorageNotice
import uniffi.mailcraft_mobile.SyncAnswer

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
 * for the owner in the browser, a grant, a removal) and what the screens are told of the mailbox's
 * storage. It is made once, with the application's scope, so a rotation, a dark-mode or language
 * switch, or the system reclaiming the activity while Microsoft's page is in front, doesn't cancel
 * the sign-in and lose what the owner just did there. The windows only read [controller]'s state.
 *
 * The mailbox is the core's to keep (`Core.open`): it opens it, saves it a few seconds after a
 * change and when it ends, and says when a save failed or the file couldn't be read. [save] asks it
 * to save now (it throws [CoreException] when it couldn't); a failed save is tried again by itself,
 * after [retryAfter] each time (the last one repeated), until one works, and [pause] waits between.
 *
 * [syncNow] is Send/Receive on the owner's word (the mail list's pull to refresh): [sync] starts a
 * round and says whether it did; [syncing] is true from the ask until the core says the round is
 * over, and [syncNotice] is why one didn't start, in the owner's language.
 *
 * A sign-in that worked starts a Send/Receive the same way ([firstSync]), so that a new account's
 * mail comes at once; a new account's first one waits for [roomFor] (the room gate, told the space
 * the owner chose for its mail in Add Account) and doesn't start on a no: the account is then held
 * back from the background's runs too ([holds]) until the owner says yes or pulls to refresh.
 */
@Suppress("LongParameterList")
internal class AccountsHub(
    api: AccountsApi,
    private val texts: Texts,
    private val scope: CoroutineScope,
    private val save: suspend () -> Unit,
    copy: (String) -> Unit,
    private val retryAfter: List<Long> = DEFAULT_RETRY,
    private val pause: suspend (Long) -> Unit = { delay(it) },
    private val sync: suspend () -> SyncAnswer = { SyncAnswer(started = false, message = "") },
    private val firstRound: suspend () -> SyncAnswer = { SyncAnswer(started = false, message = "") },
    private val roomFor: suspend (account: ULong, storageMb: Long) -> Boolean = { _, _ -> true },
    private val holds: FirstSyncHolds? = null,
) {
    private val syncingNow = MutableStateFlow(false)
    private val notice = MutableStateFlow<String?>(null)

    /** A first sync waits for the round under way to end (it began without the new account). */
    private val followUp = AtomicBoolean(false)

    /** True while a Send/Receive the owner asked for is under way (the pull-to-refresh spinner). */
    val syncing: StateFlow<Boolean> = syncingNow.asStateFlow()

    /** Why the last Send/Receive the owner asked for didn't start; null when it did. */
    val syncNotice: StateFlow<String?> = notice.asStateFlow()

    /** Bumped when the mailbox changed (its accounts, or a revision the core announced): the outline is read again. */
    val reloads = MutableStateFlow(0)

    @Volatile
    private var shown: List<AccountItem> = emptyList()

    /** True while nothing can be saved this run (the file could be neither read nor kept aside). */
    @Volatile
    private var savingOff = false

    private val guard = Any()
    private var retrying: Job? = null

    val controller = AccountsController(
        api = api,
        scope = scope,
        texts = texts,
        accounts = { shown },
        changed = {
            saveNow()
            reloads.update { it + 1 }
        },
        copy = copy,
        retrySave = { saveNow() },
        signedIn = { account, keep, added -> firstSync(account, keep, added) },
    )

    /** The accounts as the window has them now. */
    fun showing(accounts: List<AccountItem>) {
        shown = accounts
    }

    /**
     * Send/Receive now, the owner having asked (pull to refresh). A second ask while one is under way
     * changes nothing; the round's end comes as the core's `SyncFinished`.
     */
    fun syncNow() = start(owner = true)

    /**
     * A round the owner asked for ([owner]) lets go of every account held back (the core does as it
     * begins), and the holds kept here go in the same step ([FirstSyncHolds.ownerRound]). A new
     * account's first round ([firstRound]) is the app's, not the owner's: the accounts held back
     * stay held, in the core and here (that account's own hold the owner's yes let go already).
     */
    private fun start(owner: Boolean) {
        if (!syncingNow.compareAndSet(expect = false, update = true)) return
        notice.value = null
        scope.launch {
            val answer = try {
                // (The app's round waits for every hold kept to be said again, as the background's
                // does: one the core didn't hear when it was answered must not be fetched.)
                if (owner) holds?.ownerRound(sync) ?: sync() else afterHolds(holds, firstRound)
            } catch (_: CoreException) {
                SyncAnswer(started = false, message = texts.t("Send/Receive couldn't start. Try again."))
            }
            if (!answer.started) {
                notice.value = answer.message.ifEmpty { null }
                syncingNow.value = false
                startFollowUp()
            }
        }
    }

    /**
     * The first Send/Receive after a sign-in to [account] worked: true when it was asked for. A new
     * ([added]) account's waits for the room gate first, for what its window brings within the space
     * the owner chose ([KeepChoice.storageMb]); on a no nothing starts (the gate's question was the
     * owner's word). A round the owner asked for that is under way began without this account, so
     * this one stays asked for and starts when that round ends, however long it takes.
     */
    suspend fun firstSync(account: ULong, keep: KeepChoice, added: Boolean): Boolean {
        if (added) {
            val yes = roomFor(account, keep.storageMb)
            // A "Not now" holds the account back from every sync that starts by itself, kept over a
            // restart; a yes lets it go ([holds]).
            holds?.answered(account, yes)
            if (!yes) return false
        }
        followUp.set(true)
        startFollowUp()
        return true
    }

    /** Starts the first sync that waits, once no round the owner asked for is under way. */
    private fun startFollowUp() {
        if (!syncingNow.value && followUp.getAndSet(false)) start(owner = false)
    }

    /** The owner has read why Send/Receive didn't start. */
    fun syncNoticeShown() {
        notice.value = null
    }

    /** Saves the mailbox now; true when it is on disk. A failure is shown, and tried again by itself. */
    suspend fun saveNow(): Boolean {
        if (savingOff) return false
        val saved = try {
            save()
            true
        } catch (_: CoreException) {
            false
        }
        // (Saving may have been turned off while it ran: that notice stands, and nothing is retried.)
        if (savingOff) return false
        controller.setSaveFailed(!saved)
        if (!saved) retryLater()
        return saved
    }

    private fun retryLater() = synchronized(guard) {
        if (retrying?.isActive == true) return@synchronized
        retrying = scope.launch {
            var tries = 0
            do {
                pause(retryAfter.getOrElse(tries) { retryAfter.lastOrNull() ?: 0L })
                tries++
            } while (!savingOff && !trySave())
        }
    }

    /** One save from the retry loop: true when it worked. */
    private suspend fun trySave(): Boolean {
        val saved = try {
            save()
            true
        } catch (_: CoreException) {
            false
        }
        if (saved) controller.setSaveFailed(false)
        return saved
    }

    /** Takes the events that are the account screens' and the mailbox's: a question, a sign-in over, a save. */
    fun handle(event: Event) {
        when (event) {
            is Event.PermissionQuestion -> controller.askFor(event.account, PermissionKind.of(event.permission))
            is Event.SignInNeeded -> controller.signInNeeded(event.account)
            // (Synchronisation and every other change: what the window shows is read again.)
            is Event.RevChanged -> reloads.update { it + 1 }
            is Event.SyncFinished -> {
                syncingNow.value = false
                startFollowUp()
            }
            is Event.SaveFailed -> if (!savingOff) {
                controller.setSaveFailed(true)
                retryLater()
            }
            is Event.StorageNotice -> if (event.notice == StorageNotice.NOT_KEPT) {
                savingOff = true
                controller.setSaveFailed(false)
                controller.setSavingOff(true)
            }
            else -> Unit
        }
    }

    private companion object {
        val DEFAULT_RETRY = listOf(10_000L, 60_000L)
    }
}
