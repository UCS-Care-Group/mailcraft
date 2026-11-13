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
import kotlinx.coroutines.CoroutineScope
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
internal class ComposeHost(private val core: Core, private val texts: Texts, private val scope: CoroutineScope) {
    private var controller by mutableStateOf<ComposeController?>(null)

    /** What is to be said once. */
    var snack by mutableStateOf<String?>(null)
        private set

    fun begin(start: ComposeStart) {
        if (controller != null) return
        controller = ComposeController(CoreCompose(core), scope).also { it.start(start) }
    }

    fun newMessage() = begin(ComposeStart.New(0uL, null))

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
