// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "TooManyFunctions", "CyclomaticComplexMethod", "ComplexCondition", "LoopWithTooManyJumpStatements")

package work.mailcraft.android

import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.ComposeFormat
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.FromChoice
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.RecipientRecord
import uniffi.mailcraft_mobile.SuggestionRecord

/** What a call of the core about the message being written came to. */
internal sealed interface ComposeResult {
    class Ok(val view: ComposeView) : ComposeResult

    /** It was refused or failed; [why] is the program's own words, in the owner's language. */
    class Failed(val why: String) : ComposeResult
}

/** What the composer needs of the core, so that its logic runs without the native library. */
internal interface ComposeSource {
    suspend fun start(start: ComposeStart): ComposeResult

    suspend fun view(compose: ULong): ComposeResult

    suspend fun edit(compose: ULong, edit: BodyEdit): ComposeResult

    suspend fun format(compose: ULong, format: ComposeFormat, from: Int, to: Int): ComposeResult

    suspend fun recipients(compose: ULong, line: RecipientLine, people: List<AddressRecord>): ComposeResult

    suspend fun subject(compose: ULong, subject: String): ComposeResult

    suspend fun from(compose: ULong, account: ULong): ComposeResult

    suspend fun attach(compose: ULong, name: String, bytes: ByteArray): ComposeResult

    suspend fun removeFile(compose: ULong, index: UInt): ComposeResult

    suspend fun picture(compose: ULong, bytes: ByteArray, alt: String, at: Int): ComposeResult

    suspend fun pictureBytes(compose: ULong, id: UInt): ByteArray?

    suspend fun suggest(query: String): List<SuggestionRecord>

    suspend fun accounts(): List<FromChoice>

    /** Keeps the message in Drafts; whether it could. */
    suspend fun save(compose: ULong): Boolean

    /** Queues the message; the answer says whether it waits for the next check for mail. */
    suspend fun send(compose: ULong): SendResult

    suspend fun close(compose: ULong, discard: Boolean): Boolean
}

internal sealed interface SendResult {
    class Sent(val queued: Boolean) : SendResult

    class Failed(val why: String) : SendResult
}

/** What the owner is told on the composer. */
internal sealed interface ComposeNotice {
    class Said(val why: String) : ComposeNotice

    /** A file or picture could not be read from the phone. */
    data object Unreadable : ComposeNotice

    /** The line had nobody on it that could be sent to. */
    data object NoRecipient : ComposeNotice
}

/** How the composer ended. */
internal sealed interface ComposeDone {
    /** Queued for the next check for mail (else filed as sent). */
    class Sent(val queued: Boolean) : ComposeDone

    data object Saved : ComposeDone

    data object Discarded : ComposeDone

    /** Nothing was changed, so nothing was kept. */
    data object Closed : ComposeDone
}

/** What the composer draws. */
internal data class ComposeUi(
    val view: ComposeView? = null,
    /** What is in the body field now (the keyboard's, ahead of what the core has been told). */
    val body: TextFieldValue = TextFieldValue(""),
    val subject: String = "",
    /** What is typed on a line of recipients and not yet made a person. */
    val typed: Map<RecipientLine, String> = emptyMap(),
    /** The people on each line, as the owner has them (ahead of the core while its answer is awaited). */
    val people: Map<RecipientLine, List<RecipientRecord>> = emptyMap(),
    val suggestions: List<SuggestionRecord> = emptyList(),
    val suggestFor: RecipientLine? = null,
    val accounts: List<FromChoice> = emptyList(),
    /** The pictures in the body, by the number the core gave each, for drawing them. */
    val pictures: Map<UInt, ByteArray> = emptyMap(),
    /** Counts every change the owner makes: the autosave waits for it to stand still. */
    val edits: Int = 0,
    val sending: Boolean = false,
    val saved: Boolean = false,
    val notice: ComposeNotice? = null,
    /** What was left out of a share, in the program's words, kept on the screen until the owner has read it. */
    val skipped: String? = null,
    val done: ComposeDone? = null,
    /** The message could not be started; the owner's words. */
    val failed: String? = null,
) {
    val open: Boolean get() = view != null || failed != null
}

/**
 * The composer's logic, apart from any screen. The body field is the keyboard's and answers at once;
 * every change goes to the core in order, as one replacement of a range of the text, and the core's view
 * (its spans, lists and pictures) is taken in when nothing is waiting. The core does the quoting, the
 * signature, the drafts and the sending; here nothing is built from a message's words but what the owner writes.
 */
internal class ComposeController(
    private val source: ComposeSource,
    private val scope: CoroutineScope,
) {
    private val state = MutableStateFlow(ComposeUi())
    val ui: StateFlow<ComposeUi> get() = state

    private sealed interface Op {
        class Edit(val edit: BodyEdit) : Op

        class Format(val format: ComposeFormat, val from: Int, val to: Int) : Op

        class Recipients(val line: RecipientLine, val people: List<AddressRecord>) : Op

        class Subject(val text: String) : Op

        class From(val account: ULong) : Op

        class Attach(val name: String, val bytes: ByteArray) : Op

        class RemoveFile(val index: UInt) : Op

        class Picture(val bytes: ByteArray, val alt: String, val at: Int) : Op

        data object Save : Op

        data object Send : Op

        class Close(val discard: Boolean) : Op
    }

    private val ops = Channel<Op>(Channel.UNLIMITED)
    private val waiting = AtomicInteger()
    private var compose: ULong = 0uL
    private var started = false

    /** The message was sent or closed: what is still waiting is let go of. */
    @Volatile
    private var finished = false

    /**
     * Opens a message: nothing else is open here at once. [files] are attached once it is open (shared from
     * another app, already read under the caps), and [told] is what is said about what was refused.
     */
    fun start(start: ComposeStart, files: List<SharedFile> = emptyList(), told: String? = null) {
        if (started) return
        started = true
        scope.launch {
            when (val first = source.start(start)) {
                is ComposeResult.Ok -> {
                    compose = first.view.compose
                    val accounts = source.accounts()
                    state.value = shown(first.view, ComposeUi(accounts = accounts))
                    loadPictures(first.view)
                    run()
                    files.forEach { enqueue(Op.Attach(it.name, it.bytes)) }
                    if (files.isNotEmpty() || told != null) {
                        state.update { it.copy(edits = it.edits + files.size, saved = false, skipped = told) }
                    }
                }
                is ComposeResult.Failed -> {
                    started = false
                    state.value = ComposeUi(failed = first.why)
                }
            }
        }
    }

    /** What the screen shows for [view]: nothing is waiting behind it, so it is the core's body, lines and subject. */
    private fun shown(view: ComposeView, base: ComposeUi): ComposeUi {
        val body = if (base.body.text == view.text) {
            base.body
        } else {
            val end = view.text.length
            TextFieldValue(view.text, TextRange(view.selectionStart.toInt().coerceIn(0, end), view.selectionEnd.toInt().coerceIn(0, end)))
        }
        val lines = mapOf(RecipientLine.TO to view.to, RecipientLine.CC to view.cc, RecipientLine.BCC to view.bcc)
        return base.copy(view = view, body = body, people = lines, subject = if (base.edits == 0) view.subject else base.subject)
    }

    private fun run() {
        scope.launch {
            for (op in ops) {
                handle(op)
                waiting.decrementAndGet()
            }
        }
    }

    private fun enqueue(op: Op) {
        if (state.value.view == null) return
        waiting.incrementAndGet()
        ops.trySend(op)
    }

    /** The keyboard changed the body. */
    fun onBody(value: TextFieldValue) {
        val current = state.value
        if (current.view == null) return
        val edit = diffEdit(current.body.text, value.text)
        state.update { it.copy(body = value, saved = if (edit != null) false else it.saved, edits = if (edit != null) it.edits + 1 else it.edits) }
        if (edit != null) enqueue(Op.Edit(edit))
    }

    fun onSubject(text: String) {
        state.update { it.copy(subject = text, edits = it.edits + 1, saved = false) }
        enqueue(Op.Subject(text))
    }

    /** Bold, italic, underline, a list, a link or no look at all, on what is selected. */
    fun format(format: ComposeFormat) {
        val body = state.value.body
        enqueue(Op.Format(format, body.selection.min, body.selection.max))
        state.update { it.copy(edits = it.edits + 1, saved = false) }
    }

    /** What is typed on [line] changes; commas, semicolons and line ends make what came before a person. */
    fun typed(line: RecipientLine, text: String) {
        val ends = text.indexOfFirst { it == ',' || it == ';' || it == '\n' }
        if (ends >= 0) {
            val before = text.substring(0, ends + 1)
            val rest = text.substring(ends + 1)
            addPeople(line, parseTyped(before))
            state.update { it.copy(typed = it.typed + (line to rest)) }
            suggest(line, rest)
        } else {
            state.update { it.copy(typed = it.typed + (line to text)) }
            suggest(line, text)
        }
    }

    /** The owner left [line] or pressed done: what is typed there becomes people. */
    fun commit(line: RecipientLine) {
        val text = state.value.typed[line].orEmpty()
        if (text.isBlank()) return
        state.update { it.copy(typed = it.typed + (line to ""), suggestions = emptyList(), suggestFor = null) }
        addPeople(line, parseTyped(text))
    }

    /** The owner took one of the suggestions. */
    fun pick(line: RecipientLine, who: SuggestionRecord) {
        state.update { it.copy(typed = it.typed + (line to ""), suggestions = emptyList(), suggestFor = null) }
        addPeople(line, listOf(AddressRecord(who.name, who.address)))
    }

    fun remove(line: RecipientLine, index: Int) {
        val now = state.value.people[line].orEmpty()
        if (index !in now.indices) return
        setPeople(line, now.filterIndexed { i, _ -> i != index })
    }

    private fun addPeople(line: RecipientLine, more: List<AddressRecord>) {
        if (more.isEmpty()) return
        setPeople(line, state.value.people[line].orEmpty() + more.map { RecipientRecord(it.name, it.address, looksDeliverable(it.address)) })
    }

    /** Puts [people] on [line] at once, and tells the core, which says which of them it can send to. */
    private fun setPeople(line: RecipientLine, people: List<RecipientRecord>) {
        state.update { it.copy(people = it.people + (line to people), edits = it.edits + 1, saved = false) }
        enqueue(Op.Recipients(line, people.map { it.toAddress() }))
    }

    private var lookup: kotlinx.coroutines.Job? = null

    private fun suggest(line: RecipientLine, text: String) {
        lookup?.cancel()
        val query = text.trim()
        if (query.length < MIN_SUGGEST) {
            state.update { it.copy(suggestions = emptyList(), suggestFor = null) }
            return
        }
        lookup = scope.launch {
            val found = source.suggest(query)
            state.update { it.copy(suggestions = found, suggestFor = line) }
        }
    }

    fun from(account: ULong) = enqueue(Op.From(account))

    /** A file the owner chose, read from the phone. */
    fun attach(name: String, bytes: ByteArray?) {
        if (bytes == null) {
            state.update { it.copy(notice = ComposeNotice.Unreadable) }
            return
        }
        enqueue(Op.Attach(name, bytes))
        state.update { it.copy(edits = it.edits + 1, saved = false) }
    }

    fun removeFile(index: UInt) {
        enqueue(Op.RemoveFile(index))
        state.update { it.copy(edits = it.edits + 1, saved = false) }
    }

    /** A picture the owner chose, put in the text where the caret is. */
    fun picture(bytes: ByteArray?, alt: String) {
        if (bytes == null) {
            state.update { it.copy(notice = ComposeNotice.Unreadable) }
            return
        }
        enqueue(Op.Picture(bytes, alt, state.value.body.selection.min))
        state.update { it.copy(edits = it.edits + 1, saved = false) }
    }

    /** Keeps the message in Drafts (the autosave, and when the app is left). */
    fun save() = enqueue(Op.Save)

    fun send() {
        if (state.value.view == null || state.value.sending) return
        if (state.value.people.values.all { it.isEmpty() } && state.value.typed.values.all { it.isBlank() }) {
            state.update { it.copy(notice = ComposeNotice.NoRecipient) }
            return
        }
        // What is typed on a line and was not made a person yet is a person.
        RecipientLine.entries.forEach { commit(it) }
        state.update { it.copy(sending = true) }
        enqueue(Op.Send)
    }

    /** Leaves the message: kept in Drafts when it has changes. */
    fun close() = leave(discard = false)

    fun discard() = leave(discard = true)

    /** With no message open (it could not be started) there is nothing to keep: the screen just goes. */
    private fun leave(discard: Boolean) {
        if (state.value.view == null) state.update { it.copy(done = ComposeDone.Closed) } else enqueue(Op.Close(discard))
    }

    fun dismissSkipped() = state.update { it.copy(skipped = null) }

    fun noticeShown() = state.update { it.copy(notice = null) }

    /** The screen was left: what is held goes. */
    fun reset() {
        ops.close()
        state.value = ComposeUi()
    }

    private suspend fun handle(op: Op) {
        val id = compose
        if (finished) return
        when (op) {
            is Op.Edit -> take(source.edit(id, op.edit))
            is Op.Format -> take(source.format(id, op.format, op.from, op.to))
            is Op.Recipients -> take(source.recipients(id, op.line, op.people))
            is Op.Subject -> take(source.subject(id, op.text))
            is Op.From -> take(source.from(id, op.account))
            is Op.Attach -> take(source.attach(id, op.name, op.bytes))
            is Op.RemoveFile -> take(source.removeFile(id, op.index))
            is Op.Picture -> take(source.picture(id, op.bytes, op.alt, op.at))
            Op.Save -> if (source.save(id)) state.update { it.copy(saved = true) }
            Op.Send -> when (val sent = source.send(id)) {
                is SendResult.Sent -> {
                    finished = true
                    ops.close()
                    state.update { it.copy(sending = false, done = ComposeDone.Sent(sent.queued)) }
                }
                is SendResult.Failed -> state.update { it.copy(sending = false, notice = ComposeNotice.Said(sent.why)) }
            }
            is Op.Close -> finish(id, op.discard)
        }
    }

    private suspend fun finish(id: ULong, discard: Boolean) {
        val had = state.value.view?.dirty == true
        val saved = source.close(id, discard)
        finished = true
        ops.close()
        val done = when {
            discard -> ComposeDone.Discarded
            had && saved -> ComposeDone.Saved
            else -> ComposeDone.Closed
        }
        state.update { it.copy(done = done) }
    }

    /** The core's view comes in once nothing more is waiting behind this step; a refusal is told and the view is read again. */
    private suspend fun take(result: ComposeResult) {
        when (result) {
            is ComposeResult.Ok -> if (waiting.get() <= 1) {
                state.update { shown(result.view, it) }
                loadPictures(result.view)
            }
            is ComposeResult.Failed -> {
                val again = source.view(compose)
                state.update {
                    val told = it.copy(notice = ComposeNotice.Said(result.why))
                    // What the keyboard shows is not what the core holds any more: the core's is right.
                    if (again is ComposeResult.Ok && waiting.get() <= 1) shown(again.view, told.copy(body = TextFieldValue("\u0000"))) else told
                }
            }
        }
    }

    /** The bytes of the pictures in the text that have not been read yet (a few, and each once). */
    private suspend fun loadPictures(view: ComposeView) {
        for (picture in view.pictures.take(MAX_SHOWN_PICTURES)) {
            if (picture.id in state.value.pictures) continue
            val bytes = source.pictureBytes(compose, picture.id) ?: continue
            state.update { it.copy(pictures = it.pictures + (picture.id to bytes)) }
        }
    }

    private companion object {
        const val MAX_SHOWN_PICTURES = 24
        const val MIN_SUGGEST = 2
    }
}

/** Whether an address looks like `local@domain` (the core says for certain; this is for what is shown before it has). */
internal fun looksDeliverable(address: String): Boolean {
    val at = address.indexOf('@')
    return at > 0 && address.indexOf('.', at) > at + 1 && !address.endsWith('.') && address.none { it.isWhitespace() }
}
