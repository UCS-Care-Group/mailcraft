// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Calendar files other apps hand over, read off the main thread ([io]: a slow sender must not hold
 * up the window) and kept until the window has taken them in. An earlier read that comes back after a
 * later one is not kept, and an intent that is not a calendar file leaves a waiting file as it is.
 * [pending] is what the system kept of an earlier window: a file that was handed over and never taken
 * in; [remember] tells the system whether one is waiting now.
 */
internal class Handoff(
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher,
    pending: Boolean,
    private val remember: (Boolean) -> Unit,
) {
    private val _file = MutableStateFlow<IcsRead?>(null)

    /** The file read and waiting to be taken in, if any. */
    val file: StateFlow<IcsRead?> = _file

    private var started = 0L
    private var applied = 0L
    private var reading = 0

    /**
     * A file was handed over to a window before this one and never taken in, and nothing has been read
     * since: the window reads its intent again.
     */
    var unfinished: Boolean = pending
        private set

    val readAgain: Boolean get() = unfinished && started == 0L

    /** Reads what an intent carries, by [read], which does the reading (and may block). */
    fun take(read: () -> IcsRead) {
        started += 1
        val mine = started
        reading += 1
        mark(true)
        scope.launch {
            val got = withContext(io) { read() }
            reading -= 1
            if (got !is IcsRead.NotOurs && mine > applied) {
                applied = mine
                _file.value = got
            }
            if (reading == 0 && _file.value == null) mark(false)
        }
    }

    /** The window took the file in. */
    fun handled() {
        _file.value = null
        if (reading == 0) mark(false)
    }

    private fun mark(waiting: Boolean) {
        if (unfinished != waiting) {
            unfinished = waiting
            remember(waiting)
        }
    }
}
