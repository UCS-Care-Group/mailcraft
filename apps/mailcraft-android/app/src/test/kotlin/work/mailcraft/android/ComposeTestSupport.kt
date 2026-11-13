// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "LongParameterList")

package work.mailcraft.android

import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.ComposeFile
import uniffi.mailcraft_mobile.ComposeFormat
import uniffi.mailcraft_mobile.ComposeKind
import uniffi.mailcraft_mobile.ComposeParagraph
import uniffi.mailcraft_mobile.ComposeSpan
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.FromChoice
import uniffi.mailcraft_mobile.ListMark
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.RecipientRecord
import uniffi.mailcraft_mobile.SuggestionRecord

/** Invented people, all on example.org. */
internal object Cast {
    val ada = RecipientRecord("Ada Quill", "ada@example.org", valid = true)
    val bo = RecipientRecord("Bo Marsh", "bo@example.org", valid = true)
    val broken = RecipientRecord("", "not an address", valid = false)
    val accounts = listOf(
        FromChoice(1uL, "Avery Stone", "avery@example.org", shared = false),
        FromChoice(2uL, "Support desk", "support@example.org", shared = true),
        FromChoice(3uL, "Avery (home)", "avery@home.example", shared = false),
    )
}

internal fun composeView(
    text: String = "",
    kind: ComposeKind = ComposeKind.NEW,
    account: ULong = 1uL,
    to: List<RecipientRecord> = emptyList(),
    cc: List<RecipientRecord> = emptyList(),
    subject: String = "",
    spans: List<ComposeSpan> = emptyList(),
    paragraphs: List<ComposeParagraph> = emptyList(),
    files: List<ComposeFile> = emptyList(),
    signature: String = "",
    quote: String = "",
    dirty: Boolean = false,
    html: Boolean = true,
    compose: ULong = 5uL,
) = ComposeView(
    rev = 1uL, compose = compose, kind = kind, source = 0uL, account = account, to = to, cc = cc, bcc = emptyList(),
    subject = subject, html = html, text = text, spans = spans, paragraphs = paragraphs, pictures = emptyList(),
    selectionStart = text.length.toUInt(), selectionEnd = text.length.toUInt(), files = files, signature = signature,
    quote = quote, dirty = dirty,
)

/** A core that holds one message being written, as far as the composer's logic needs: edits, lines, accounts, files, sending. */
internal class FakeCompose(var view: ComposeView = composeView()) : ComposeSource {
    val calls = mutableListOf<String>()
    var startAnswer: ComposeResult? = null
    var refuseEdits: String? = null
    var sendAnswer: SendResult = SendResult.Sent(queued = true)
    var saveWorks = true
    val suggestions = mutableListOf<SuggestionRecord>()
    val signatures = mapOf(1uL to "Avery", 2uL to "Support", 3uL to "")
    var onEdit: (ComposeView) -> ComposeView = { it }
    var accountList = Cast.accounts
    val pictureData = mutableMapOf<UInt, ByteArray>()

    override suspend fun start(start: ComposeStart): ComposeResult {
        calls += "start ${start::class.simpleName}"
        return startAnswer ?: ComposeResult.Ok(view)
    }

    override suspend fun view(compose: ULong): ComposeResult = ComposeResult.Ok(view)

    override suspend fun edit(compose: ULong, edit: BodyEdit): ComposeResult {
        calls += "edit ${edit.from}-${edit.to}:${edit.text}"
        refuseEdits?.let { return ComposeResult.Failed(it) }
        val text = view.text.replaceRange(edit.from, edit.to, edit.text)
        view = onEdit(view.copy(text = text, dirty = true, selectionStart = (edit.from + edit.text.length).toUInt(), selectionEnd = (edit.from + edit.text.length).toUInt()))
        return ComposeResult.Ok(view)
    }

    override suspend fun format(compose: ULong, format: ComposeFormat, from: Int, to: Int): ComposeResult {
        calls += "format ${format::class.simpleName} $from-$to"
        if (format == ComposeFormat.Bold) view = view.copy(spans = view.spans + ComposeSpan(from.toUInt(), to.toUInt(), bold = true, italic = false, underline = false, link = null), dirty = true)
        return ComposeResult.Ok(view)
    }

    override suspend fun recipients(compose: ULong, line: RecipientLine, people: List<AddressRecord>): ComposeResult {
        calls += "recipients $line ${people.joinToString { it.address }}"
        val made = people.map { RecipientRecord(it.name, it.address, it.address.contains('@')) }
        view = when (line) {
            RecipientLine.TO -> view.copy(to = made, dirty = true)
            RecipientLine.CC -> view.copy(cc = made, dirty = true)
            RecipientLine.BCC -> view.copy(dirty = true)
        }
        return ComposeResult.Ok(view)
    }

    override suspend fun subject(compose: ULong, subject: String): ComposeResult {
        calls += "subject $subject"
        view = view.copy(subject = subject, dirty = true)
        return ComposeResult.Ok(view)
    }

    override suspend fun from(compose: ULong, account: ULong): ComposeResult {
        calls += "from $account"
        view = view.copy(account = account, signature = signatures[account].orEmpty(), dirty = true)
        return ComposeResult.Ok(view)
    }

    override suspend fun attach(compose: ULong, name: String, bytes: ByteArray): ComposeResult {
        calls += "attach $name ${bytes.size}"
        view = view.copy(files = view.files + ComposeFile(view.files.size.toUInt(), name, "application/octet-stream", bytes.size.toULong()), dirty = true)
        return ComposeResult.Ok(view)
    }

    override suspend fun removeFile(compose: ULong, index: UInt): ComposeResult {
        calls += "removeFile $index"
        view = view.copy(files = view.files.filter { it.index != index })
        return ComposeResult.Ok(view)
    }

    override suspend fun picture(compose: ULong, bytes: ByteArray, alt: String, at: Int): ComposeResult {
        calls += "picture ${bytes.size} $alt $at"
        return ComposeResult.Ok(view)
    }

    override suspend fun pictureBytes(compose: ULong, id: UInt): ByteArray? = pictureData[id]

    override suspend fun suggest(query: String): List<SuggestionRecord> {
        calls += "suggest $query"
        return suggestions.filter { it.address.contains(query, ignoreCase = true) }
    }

    override suspend fun accounts(): List<FromChoice> = accountList

    override suspend fun save(compose: ULong): Boolean {
        calls += "save"
        return saveWorks
    }

    override suspend fun send(compose: ULong): SendResult {
        calls += "send"
        return sendAnswer
    }

    override suspend fun close(compose: ULong, discard: Boolean): Boolean {
        calls += "close discard=$discard"
        return true
    }
}
