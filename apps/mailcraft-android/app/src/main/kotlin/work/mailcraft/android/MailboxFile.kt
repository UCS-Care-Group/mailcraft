// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/**
 * Where the mailbox is kept between runs: one file, and the folder of message content beside it
 * (the core's `file.save`), in the app's private no-backup storage. What is in it is the accounts and
 * their mail; no password or token is (those are in the Keystore-sealed store). A file that can't be
 * opened is put aside, and if it can't be put aside either, nothing is written to its place this
 * session ([savingOff]): the original is never overwritten.
 *
 * A save that fails (no space, a folder that can't be made) leaves the change in memory, sets
 * [saveFailed] so the screens say it isn't saved yet, and is tried again by itself, after
 * [retryAfter] each time (the last one repeated) while it keeps failing; a save that works clears
 * it. Saves never overlap.
 *
 * [setAside] moves a file that can't be read out of the way; true when it was moved. [scope] runs
 * the retries (none: no retry by itself, [keepAgain] only), and [pause] waits between them.
 */
internal class MailboxFile(
    private val dir: File,
    private val setAside: (File, File) -> Boolean = { from, to -> from.renameTo(to) },
    private val scope: CoroutineScope? = null,
    private val retryAfter: List<Long> = DEFAULT_RETRY,
    private val pause: suspend (Long) -> Unit = { delay(it) },
) {
    private val file = File(dir, NAME)
    private val off = MutableStateFlow(false)

    /** True when the file could neither be read nor put aside: nothing is saved over it. */
    val savingOff: StateFlow<Boolean> = off

    private val failed = MutableStateFlow(false)
    private val one = Mutex()
    private val guard = Any()
    private var retrying: Job? = null
    private var lastSave: (suspend (String) -> Unit)? = null

    /** True while the last save failed: what the owner changed is in memory only. */
    val saveFailed: StateFlow<Boolean> = failed

    /** Opens what was kept, when something was. */
    suspend fun restore(core: Core) = restore { path ->
        core.execute("file.open", """{"path":${JSONObject.quote(path)}}""")
    }

    /** Keeps the mailbox as it is now. */
    suspend fun keep(core: Core) = keep { path ->
        core.execute("file.save", """{"path":${JSONObject.quote(path)}}""")
    }

    /** Tries the last save again now (the Retry of the notice). */
    suspend fun keepAgain() {
        val save = synchronized(guard) { lastSave } ?: return
        keep(save)
    }

    internal suspend fun restore(open: suspend (String) -> Unit) {
        if (!file.isFile) return
        try {
            open(file.path)
        } catch (_: CoreException) {
            // Nothing to restore, and the next save must not write over what could not be read (a newer
            // build or a repair may still): it is put aside under a name of its own, and the mailbox
            // starts empty. When it can't be put aside it stays where it is, and nothing is saved there.
            val moved = try {
                setAside(file, File(dir, "$NAME.unreadable-${System.currentTimeMillis()}"))
            } catch (_: SecurityException) {
                false
            }
            if (!moved && file.exists()) off.value = true
        }
    }

    internal suspend fun keep(save: suspend (String) -> Unit) {
        synchronized(guard) { lastSave = save }
        if (!attempt(save)) retryLater()
    }

    /** One save, alone; false when it failed (and [saveFailed] says so). */
    private suspend fun attempt(save: suspend (String) -> Unit): Boolean = one.withLock {
        if (off.value) return@withLock true
        val kept = try {
            (dir.isDirectory || dir.mkdirs()) && run {
                save(file.path)
                true
            }
        } catch (_: CoreException) {
            false
        } catch (_: SecurityException) {
            false
        }
        failed.value = !kept
        kept
    }

    private fun retryLater() {
        val scope = scope ?: return
        synchronized(guard) {
            if (retrying != null) return
            retrying = scope.launch {
                var tries = 0
                var done = false
                while (!done) {
                    pause(retryAfter.getOrElse(tries) { retryAfter.lastOrNull() ?: 0L })
                    tries++
                    val save = synchronized(guard) { lastSave }
                    // (Done only if nothing failed since: a failure after this save keeps the loop.)
                    done = save == null || (attempt(save) && finished())
                }
            }
        }
    }

    /** The retry loop is over if no save has failed since; false keeps it going. */
    private fun finished(): Boolean = synchronized(guard) {
        val over = !failed.value
        if (over) retrying = null
        over
    }

    private companion object {
        val DEFAULT_RETRY = listOf(10_000L, 60_000L)
        const val NAME = "mailbox.mailcraft"
    }
}
