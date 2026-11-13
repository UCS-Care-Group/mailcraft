// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.flow.MutableStateFlow
import androidx.compose.runtime.collectAsState
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.Core

/** What is said when a message is done with: it is queued (not yet sent from the phone), filed as sent, or kept as a draft. */
internal fun doneText(texts: Texts, done: ComposeDone): String? = when (done) {
    is ComposeDone.Sent ->
        if (done.queued) texts.t("Message queued. Sending from this phone is not available yet.") else texts.t("Message sent.")
    ComposeDone.Saved -> texts.t("Draft saved.")
    ComposeDone.Discarded, ComposeDone.Closed -> null
}

/** What the composer's actions are, answered by [c] (none when no message is open). */
internal fun composerActions(c: ComposeController?): ComposerActions = if (c == null) {
    ComposerActions.None
} else {
    ComposerActions(
        onClose = c::close,
        onDiscard = c::discard,
        onSend = c::send,
        onSave = c::save,
        onNoticeShown = c::noticeShown,
        onSkippedDismissed = c::dismissSkipped,
        onFrom = c::from,
        onSubject = c::onSubject,
        onBody = c::onBody,
        onFormat = c::format,
        lines = LineActions(c::typed, c::commit, c::remove, c::pick),
        files = FileActions(onAttach = c::attach, onPicture = { c.picture(it, "") }, onRemove = c::removeFile),
    )
}

private val Idle = MutableStateFlow(ComposeUi())

/**
 * Where the message being written lives for the window: one at a time, started from a reply button, the new
 * message button or a `mailto:` link, ended when it is sent, kept as a draft or discarded, with what is said
 * about that left for the shell to show once.
 */
internal class ComposeHost(
    private val core: Core,
    private val texts: Texts,
    private val scope: CoroutineScope,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) {
    private var controller by mutableStateOf<ComposeController?>(null)

    /** What is to be said once. */
    var snack by mutableStateOf<String?>(null)
        private set

    fun begin(start: ComposeStart) {
        if (controller != null) return
        controller = ComposeController(CoreCompose(core), scope).also { it.start(start) }
    }

    fun newMessage() = begin(ComposeStart.New(0uL, null))

    /**
     * Another app asked for a message: a `mailto:` link (the core reads it, and the message is only ever
     * started, never sent), or something shared (text, links, files, read off the main thread under the caps).
     * With a message already open nothing is taken in, and the owner is told so.
     */
    fun beginIncoming(incoming: Incoming, reader: ShareReader, ownAuthority: String) {
        when (incoming) {
            is Incoming.Mailto -> if (controller == null) begin(ComposeStart.New(0uL, incoming.link)) else busy()
            is Incoming.Share -> {
                if (controller != null) return busy()
                scope.launch {
                    val load = withContext(io) { loadShared(incoming.files, reader, ownAuthority) }
                    if (controller != null) return@launch busy()
                    val start = if (incoming.text == null && incoming.subject == null) {
                        ComposeStart.New(0uL, null)
                    } else {
                        ComposeStart.Shared(0uL, incoming.subject, incoming.text.orEmpty())
                    }
                    val told = load.refusals.takeIf { it.isNotEmpty() }?.let { refusalText(texts, it) }
                    controller = ComposeController(CoreCompose(core), scope).also { it.start(start, load.files, told) }
                }
            }
        }
    }

    private fun busy() {
        snack = texts.t("Finish or close the message you are writing, then try again.")
    }

    fun snackShown() {
        snack = null
    }

    fun actions(): ComposerActions = composerActions(controller)

    /** The composer's state, and the end of a message taken in: it is said and let go of. */
    @Composable
    fun ui(): ComposeUi {
        val current = controller
        val state by (current?.ui ?: Idle).collectAsState()
        LaunchedEffect(state.done) {
            val done = state.done ?: return@LaunchedEffect
            snack = doneText(texts, done)
            current?.reset()
            controller = null
        }
        return state
    }
}

@Composable
internal fun rememberComposeHost(core: Core, texts: Texts, scope: CoroutineScope): ComposeHost =
    remember(core) { ComposeHost(core, texts, scope) }
