// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.annotation.SuppressLint
import android.content.Context
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import uniffi.mailcraft_mobile.CoreException

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
 * The core keeps the hold in memory only, so it is kept here too, in [store], and said to the core
 * again whenever a core starts ([reapply]: at the app's start, and before the background worker
 * runs, as the worker's process may start without a window). [hold] is the core's call; it throws
 * [CoreException] when the core refuses (an account that is gone is forgotten here too).
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
     * The owner asked for a Send/Receive (a pull to refresh): the core let go of every account held
     * back when it started the round, and nothing is held here either.
     */
    suspend fun releaseAll() = lock.withLock {
        if (store.load().isNotEmpty()) store.save(emptySet())
    }

    /** Says every hold kept to a core that has just started (its own are gone with the last one). */
    suspend fun reapply() = lock.withLock {
        for (account in store.load()) tell(account, held = true)
    }

    /** Tells the core; an account it refuses (gone from the mailbox) is held no more. */
    private suspend fun tell(account: ULong, held: Boolean) {
        try {
            hold(account, held)
        } catch (_: CoreException.BadRequest) {
            store.save(store.load() - account)
        } catch (_: CoreException) {
            // (Another failure may pass: the hold stays kept and is said again at the next start.)
        }
    }
}
