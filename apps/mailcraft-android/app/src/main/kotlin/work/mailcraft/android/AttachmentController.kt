// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** What an attachment's bytes came to. */
internal sealed interface AttachmentBytes {
    class Got(val bytes: ByteArray, val saveName: String, val picture: Boolean) : AttachmentBytes

    /** It could not be had; [why] is the program's own words (English). */
    class Failed(val why: String) : AttachmentBytes
}

/** What a step of the core came to, as the owner is told. */
internal sealed interface AttachmentOutcome {
    data object Done : AttachmentOutcome

    /** The core refused: the kind is blocked or too large, or the bytes could not be read. */
    class Refused(val why: String) : AttachmentOutcome

    /** No app on the phone takes it. */
    data object NoApp : AttachmentOutcome

    /** A calendar file came in: how many events. */
    class Imported(val events: Int, val tasks: Int) : AttachmentOutcome
}

/** What the attachments' logic needs of the core, so that it runs without the native library. */
internal interface AttachmentSource {
    suspend fun bytes(id: ULong, index: UInt): AttachmentBytes

    /** Hands the file to another app under the opening verdict; [confirmed] says the owner was asked. */
    suspend fun open(id: ULong, index: UInt, confirmed: Boolean): AttachmentOutcome

    suspend fun importCalendar(id: ULong, index: UInt): AttachmentOutcome
}

/** What the attachments' logic needs of the phone. */
internal interface AttachmentDevice {
    /** Offers [bytes] to the system's share sheet with a one-off read-only grant; whether it could. */
    fun share(name: String, bytes: ByteArray): Boolean

    /** Writes [bytes] to the document the owner chose ([target], the picker's answer); whether it could. */
    fun write(target: String, bytes: ByteArray): Boolean
}

/** A question put to the owner before a file that can carry macros or scripts is opened. */
internal data class OpenQuestion(val id: ULong, val item: AttachmentItem)

/** The picture the owner is looking at, previewed inside the app. */
internal class Preview(val id: ULong, val item: AttachmentItem, val bytes: ByteArray)

/** What the owner is told after a step. */
internal sealed interface AttachmentNotice {
    data object NoApp : AttachmentNotice

    data object Saved : AttachmentNotice

    data object SaveFailed : AttachmentNotice

    data object ShareFailed : AttachmentNotice

    class Imported(val events: Int, val tasks: Int) : AttachmentNotice

    /** The core's own words. */
    class Said(val why: String) : AttachmentNotice
}

/** The place the owner is asked to choose for a save: the name the document is offered under. */
internal data class SaveRequest(val name: String)

/** What the reading pane draws of the attachments' doings. */
internal data class AttachmentUi(
    val question: OpenQuestion? = null,
    val preview: Preview? = null,
    val saving: SaveRequest? = null,
    val notice: AttachmentNotice? = null,
    /** Something is being read for the owner. */
    val busy: Boolean = false,
)

/**
 * What the reading pane does with attachments: open one in another app (only under the engine's
 * verdict, and after a question for the kinds that ask), preview a picture inside the app, save to a
 * document the owner chooses, share through the system's sheet, and bring a calendar file in. Each
 * step runs off the main thread and ends in a notice; nothing here reads a file by its name.
 */
internal class AttachmentController(
    private val source: AttachmentSource,
    private val device: AttachmentDevice,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(AttachmentUi())
    val ui: StateFlow<AttachmentUi> get() = state

    /** What is to be saved once the owner has chosen the place: held here, never in the UI state. */
    private var pending: ByteArray? = null

    /** The owner chose [action] for attachment [item] of message [id] (a tap does [primaryOf]). */
    fun act(id: ULong, item: AttachmentItem, action: AttachmentAction) {
        // Only what the row offers: a blocked kind is never opened or shared, whatever asks.
        if (state.value.busy || action !in actionsOf(item)) return
        when (action) {
            AttachmentAction.Open -> openFile(id, item)
            AttachmentAction.Save -> save(id, item)
            AttachmentAction.Share -> share(id, item)
            AttachmentAction.AddToCalendar -> addToCalendar(id, item)
        }
    }

    private fun openFile(id: ULong, item: AttachmentItem) {
        if (asksFirst(item.verdict)) {
            state.update { it.copy(question = OpenQuestion(id, item)) }
        } else {
            launchOpen(id, item, confirmed = false)
        }
    }

    /** The owner said yes to the question. */
    fun confirm() {
        val q = state.value.question ?: return
        state.update { it.copy(question = null) }
        launchOpen(q.id, q.item, confirmed = true)
    }

    /** The owner closed the question or the picture without going on. */
    fun dismiss() = state.update { it.copy(question = null, preview = null) }

    private fun launchOpen(id: ULong, item: AttachmentItem, confirmed: Boolean) {
        busy {
            // A picture is previewed in the app, by its own bytes; every other kind goes to another app.
            if (looksLikeAPictureByName(item.name)) {
                val got = source.bytes(id, item.index)
                if (got is AttachmentBytes.Got && got.picture) {
                    state.update { it.copy(preview = Preview(id, item, got.bytes)) }
                    return@busy
                }
            }
            when (val out = source.open(id, item.index, confirmed)) {
                AttachmentOutcome.Done -> Unit
                AttachmentOutcome.NoApp -> tell(AttachmentNotice.NoApp)
                is AttachmentOutcome.Refused -> tell(AttachmentNotice.Said(out.why))
                is AttachmentOutcome.Imported -> Unit
            }
        }
    }

    private fun save(id: ULong, item: AttachmentItem) {
        busy {
            when (val got = source.bytes(id, item.index)) {
                is AttachmentBytes.Got -> {
                    pending = got.bytes
                    state.update { it.copy(saving = SaveRequest(got.saveName)) }
                }
                is AttachmentBytes.Failed -> tell(AttachmentNotice.Said(got.why))
            }
        }
    }

    /** The picker answered: [target] is the document the owner chose, or null when they backed out. */
    fun saveTo(target: String?) {
        val bytes = pending
        pending = null
        state.update { it.copy(saving = null) }
        if (target == null || bytes == null) return
        scope.launch {
            tell(if (device.write(target, bytes)) AttachmentNotice.Saved else AttachmentNotice.SaveFailed)
        }
    }

    private fun share(id: ULong, item: AttachmentItem) {
        busy {
            when (val got = source.bytes(id, item.index)) {
                is AttachmentBytes.Got -> if (!device.share(got.saveName, got.bytes)) tell(AttachmentNotice.ShareFailed)
                is AttachmentBytes.Failed -> tell(AttachmentNotice.Said(got.why))
            }
        }
    }

    private fun addToCalendar(id: ULong, item: AttachmentItem) {
        busy {
            when (val out = source.importCalendar(id, item.index)) {
                is AttachmentOutcome.Imported -> tell(AttachmentNotice.Imported(out.events, out.tasks))
                is AttachmentOutcome.Refused -> tell(AttachmentNotice.Said(out.why))
                AttachmentOutcome.NoApp -> tell(AttachmentNotice.NoApp)
                AttachmentOutcome.Done -> Unit
            }
        }
    }

    /** Save or share the picture being previewed. */
    fun previewAct(action: AttachmentAction) {
        val p = state.value.preview ?: return
        state.update { it.copy(preview = null) }
        act(p.id, p.item, action)
    }

    fun noticeShown() = state.update { it.copy(notice = null) }

    /** The message was left: nothing held for it stays. */
    fun reset() {
        pending = null
        state.value = AttachmentUi()
    }

    private fun busy(step: suspend () -> Unit) {
        state.update { it.copy(busy = true) }
        scope.launch {
            try {
                step()
            } finally {
                state.update { it.copy(busy = false) }
            }
        }
    }

    private fun tell(notice: AttachmentNotice) = state.update { it.copy(notice = notice) }
}
