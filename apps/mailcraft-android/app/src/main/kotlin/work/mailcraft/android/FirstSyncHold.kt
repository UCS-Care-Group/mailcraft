// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.SyncAnswer

/** Where the accounts whose first sync waits are kept: their ids and nothing else. */
internal interface HoldStore {
    fun load(): Set<ULong>

    fun save(held: Set<ULong>)
}

/**
 * [HoldStore] in a small preference of the app's: account ids only, no mail, no address, no secret.
 * A preference that can't be read is an empty one (nothing held), never a crash.
 */
internal class DeviceHolds(context: Context) : HoldStore {
    private val prefs = context.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    override fun load(): Set<ULong> = runCatching {
        prefs.getStringSet(KEY, emptySet()).orEmpty().mapNotNull { it.toULongOrNull() }.take(MAX_HELD).toSet()
    }.getOrDefault(emptySet())

    // (The KTX `edit {}` would be a dependency for one line.)
    @SuppressLint("UseKtx")
    override fun save(held: Set<ULong>) {
        runCatching { prefs.edit().putStringSet(KEY, held.take(MAX_HELD).map { it.toString() }.toSet()).apply() }
    }

    private companion object {
        const val FILE = "first_sync_holds"
        const val KEY = "held"
    }
}

/** [HoldStore] in memory, for tests and screenshots. */
internal class MemoryHolds(private var held: Set<ULong> = emptySet()) : HoldStore {
    override fun load(): Set<ULong> = held

    override fun save(held: Set<ULong>) {
        this.held = held
    }
}

/** The most accounts kept held: more than a phone has accounts, so a damaged preference stays small. */
private const val MAX_HELD = 64

/**
 * A new account's first sync that the owner said "Not now" to, for want of room (the "Not much
 * room" question of [SettingsController.roomForFirstSync]): the core holds the account back from
 * everything that starts by itself (`Core.holdBackgroundSync`), the background's runs included,
 * until the owner says yes or pulls to refresh.
 *
 * The core keeps the hold in memory only, so it is kept here too, in [store]: given to the core
 * as it opens (`Core.openHolding`, before anything of its own starts), and said again
 * ([reapply]) once it has (which forgets an account that is gone) and before the background
 * worker's round. [hold] is the core's call; it throws [CoreException] when the core refuses (an
 * account that is gone is forgotten here too).
 */
internal class FirstSyncHolds(
    private val store: HoldStore,
    private val hold: suspend (account: ULong, held: Boolean) -> Unit,
) {
    private val lock = Mutex()

    /** The accounts held now, as kept. */
    fun held(): Set<ULong> = store.load()

    /** The owner answered the room question for [account]'s first sync: [yes] lets it go, a no holds it. */
    suspend fun answered(account: ULong, yes: Boolean) = lock.withLock {
        val kept = store.load()
        store.save(if (yes) kept - account else kept + account)
        tell(account, held = !yes)
    }

    /**
     * A Send/Receive the owner asked for (a pull to refresh): [start] asks the core for the round,
     * and the core lets go of every account held back as it begins one, so nothing is held here
     * either once it has. Both happen under the lock, so that a "Not now" answered while the ask is
     * on its way lands either before the round (and is let go with the rest) or after it (and is
     * kept, as the core keeps it), never between the core's letting go and this one's.
     */
    suspend fun ownerRound(start: suspend () -> SyncAnswer): SyncAnswer = lock.withLock {
        start().also { if (it.started && store.load().isNotEmpty()) store.save(emptySet()) }
    }

    /**
     * The owner removed the mail and data on the phone ([remove], null when that failed): the
     * accounts held back went with the mailbox (the core let go of them), so none is kept here
     * either; a new account may come to have the same number. Under the lock, so that no answer
     * lands between the removal and the forgetting.
     */
    suspend fun <T : Any> removing(remove: suspend () -> T?): T? = lock.withLock {
        remove()?.also { if (store.load().isNotEmpty()) store.save(emptySet()) }
    }

    /**
     * Says every hold kept to a core that has just started (its own are gone with the last one).
     * Every hold is tried; the first failure that may pass is returned (null when every hold was
     * said), so that a round that would start by itself doesn't start without it.
     */
    suspend fun reapply(): CoreException? = lock.withLock {
        var failed: CoreException? = null
        for (account in store.load()) {
            val failure = tell(account, held = true)
            if (failed == null) failed = failure
        }
        failed
    }

    /**
     * Tells the core; an account it refuses (gone from the mailbox) is held no more. Another failure
     * may pass: the hold stays kept, is said again at the next start, and is returned.
     */
    private suspend fun tell(account: ULong, held: Boolean): CoreException? =
        try {
            hold(account, held)
            null
        } catch (_: CoreException.BadRequest) {
            store.save(store.load() - account)
            null
        } catch (failure: CoreException) {
            failure
        }
}

/**
 * A round that starts by itself (the background worker's): every hold kept is said to the core
 * first, and when one couldn't be, the round doesn't start (the failure is thrown, and the worker
 * tries again later), so that no account held back is fetched against the owner's "Not now".
 */
internal suspend fun <T> afterHolds(holds: FirstSyncHolds?, round: suspend () -> T): T {
    holds?.reapply()?.let { throw it }
    return round()
}
