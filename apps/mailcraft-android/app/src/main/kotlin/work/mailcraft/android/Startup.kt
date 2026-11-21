// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicBoolean
import kotlin.coroutines.CoroutineContext
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Where opening something slow stands: under way, open, or failed. */
internal sealed interface Phase<out T> {
    data object Loading : Phase<Nothing>

    class Ready<T>(val value: T) : Phase<T>

    data object Failed : Phase<Nothing>
}

/**
 * Opens one thing once, off the main thread: [begin] runs the opening on [io] (reading a mailbox
 * from disk can take seconds, and the main thread must never wait for it), then hands what it
 * opened to its `ready` step on [scope]'s own dispatcher (the main thread), and only then says it
 * is [Phase.Ready]. Until it is, [readyOrNull] is null, so nothing reaches what is still opening.
 */
internal class Startup<T : Any>(private val scope: CoroutineScope, private val io: CoroutineContext = Dispatchers.IO) {
    private val phase = MutableStateFlow<Phase<T>>(Phase.Loading)
    private val begun = AtomicBoolean(false)

    /** Where the opening stands; the window shows a loading state while it is [Phase.Loading]. */
    val state: StateFlow<Phase<T>> = phase.asStateFlow()

    /**
     * Begins the opening, the first time only. [open] gives null when it could not open; [ready]
     * wires what was opened (false: it could not be, and the opening counts as failed).
     */
    fun begin(open: () -> T?, ready: (T) -> Boolean) {
        if (!begun.compareAndSet(false, true)) return
        scope.launch {
            val opened = withContext(io) { open() }
            phase.value = if (opened != null && ready(opened)) Phase.Ready(opened) else Phase.Failed
        }
    }

    /** What was opened, once it is ready; null while it is opening or when it failed. */
    fun readyOrNull(): T? = (phase.value as? Phase.Ready)?.value
}
