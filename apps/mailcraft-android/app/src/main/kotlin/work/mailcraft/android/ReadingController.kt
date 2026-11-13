// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * The reading pane's logic, apart from any screen: which message is open and how it came. An unread
 * message is marked read once it has been shown.
 */
internal class ReadingController(
    private val source: ReadSource,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(ReadingUi())

    val ui: StateFlow<ReadingUi> get() = state

    /** Moves on when another message is opened, so that an answer to an earlier question is dropped. */
    private val generation = AtomicInteger()
    private val marked = LinkedHashSet<ULong>()

    /** Opens message [id] for a body [widthDp] wide in a dark or light window (again, when either changed). */
    fun open(id: ULong, widthDp: Int, dark: Boolean) {
        val mine = generation.incrementAndGet()
        state.update { if (it.id == id) it else ReadingUi(id = id) }
        scope.launch {
            val opened = source.open(id, widthDp, dark)
            if (mine != generation.get()) return@launch
            if (opened == null) {
                state.update { it.copy(load = LoadState.Failed) }
                return@launch
            }
            state.update { it.copy(load = LoadState.Ready, opened = opened) }
            if (opened.view.unread && remember(id)) source.markRead(id)
        }
    }

    /** The owner left the message. */
    fun close() {
        generation.incrementAndGet()
        state.value = ReadingUi()
    }

    @Synchronized
    private fun remember(id: ULong): Boolean {
        val added = marked.add(id)
        if (marked.size > MAX_REMEMBERED_MESSAGES) marked.remove(marked.first())
        return added
    }

    private companion object {
        const val MAX_REMEMBERED_MESSAGES = 500
    }
}
