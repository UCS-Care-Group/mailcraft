// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "TooManyFunctions", "ComplexCondition")

package work.mailcraft.android

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextDecoration
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.FromChoice
import uniffi.mailcraft_mobile.ListMark
import uniffi.mailcraft_mobile.RecipientRecord

/** The one replacement ([from] until [to] of the old text becomes [text]) that turns one text into another. */
internal data class BodyEdit(val from: Int, val to: Int, val text: String)

/**
 * What changed between [old] and [new]: the part in the middle, never cutting between the two halves of a
 * character the keyboard counts as two (an emoji), so that the core is never given half of one. Null when nothing changed.
 */
internal fun diffEdit(old: String, new: String): BodyEdit? {
    if (old == new) return null
    var start = 0
    val shortest = minOf(old.length, new.length)
    while (start < shortest && old[start] == new[start]) start++
    if (start > 0 && start <= shortest && old[start - 1].isHighSurrogate()) start--
    var endOld = old.length
    var endNew = new.length
    while (endOld > start && endNew > start && old[endOld - 1] == new[endNew - 1]) {
        endOld--
        endNew--
    }
    if (endOld < old.length && old[endOld].isLowSurrogate()) {
        endOld++
        endNew++
    }
    return BodyEdit(start, endOld, new.substring(start, endNew))
}

/** [text] (the body as the keyboard has it) with the look the core gave it, as far as the text reaches: bold, italic, underline and links, and the signature in a quieter colour. */
internal fun styledBody(text: String, view: ComposeView, link: Color, quiet: Color): AnnotatedString = buildAnnotatedString {
    append(text)
    val length = text.length
    fun range(start: UInt, end: UInt): IntRange? {
        val from = start.toInt().coerceIn(0, length)
        val to = end.toInt().coerceIn(0, length)
        return if (to > from) from until to else null
    }
    for (p in view.paragraphs) {
        if (p.signature) range(p.start, p.end)?.let { addStyle(SpanStyle(color = quiet), it.first, it.last + 1) }
    }
    for (s in view.spans) {
        val at = range(s.start, s.end) ?: continue
        val underline = s.underline || s.link != null
        addStyle(
            SpanStyle(
                fontWeight = if (s.bold) FontWeight.Bold else null,
                fontStyle = if (s.italic) FontStyle.Italic else null,
                textDecoration = if (underline) TextDecoration.Underline else null,
                color = if (s.link != null) link else Color.Unspecified,
            ),
            at.first,
            at.last + 1,
        )
    }
}

/** What is put in front of a list item: its marker, indented by how deep it is. */
internal fun markerOf(mark: ListMark, level: Int, number: UInt): String {
    val indent = "    ".repeat(level.coerceIn(0, MAX_LEVEL))
    return when (mark) {
        ListMark.BULLET -> "$indent• "
        ListMark.NUMBER -> "$indent$number. "
        ListMark.NONE -> ""
    }
}

private const val MAX_LEVEL = 8

/** The markers a body's list items show: where in the text each goes and what it says. */
internal fun markersOf(view: ComposeView): List<Pair<Int, String>> = view.paragraphs
    .filter { it.list != ListMark.NONE }
    .map { it.start.toInt().coerceIn(0, view.text.length) to markerOf(it.list, it.level.toInt(), it.number) }

/**
 * Shows the markers of a list's items in front of them without putting them in the text: the core's text
 * has none, so what is typed and what is sent are only the words. The caret never lands inside a marker.
 */
internal class ListMarks(private val marks: List<Pair<Int, String>>) : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val sorted = marks.filter { it.first in 0..text.length }.sortedBy { it.first }
        if (sorted.isEmpty()) return TransformedText(text, OffsetMapping.Identity)
        val shown = buildAnnotatedString {
            var at = 0
            for ((start, marker) in sorted) {
                append(text.subSequence(at, start))
                append(marker)
                at = start
            }
            append(text.subSequence(at, text.length))
        }
        val mapping = object : OffsetMapping {
            override fun originalToTransformed(offset: Int): Int =
                offset + sorted.filter { it.first <= offset }.sumOf { it.second.length }

            override fun transformedToOriginal(offset: Int): Int {
                var added = 0
                for ((start, marker) in sorted) {
                    val markerStart = start + added
                    // Before this marker: nothing later can matter. Inside it: the item's first character.
                    if (offset < markerStart) break
                    if (offset < markerStart + marker.length) return start
                    added += marker.length
                }
                return (offset - added).coerceAtLeast(0)
            }
        }
        return TransformedText(shown, mapping)
    }
}

/**
 * People typed into a line of recipients: separated by `,` or `;` outside quotes and angle brackets, each as
 * `Name <address>` or just an address. Nothing is checked here: the core says which look deliverable.
 */
internal fun parseTyped(text: String): List<AddressRecord> {
    val parts = mutableListOf<String>()
    val part = StringBuilder()
    var quoted = false
    var angle = false
    for (c in text) {
        when {
            c == '"' -> quoted = !quoted
            c == '<' && !quoted -> angle = true
            c == '>' && !quoted -> angle = false
        }
        if ((c == ',' || c == ';' || c == '\n') && !quoted && !angle) {
            parts += part.toString()
            part.clear()
        } else {
            part.append(c)
        }
    }
    parts += part.toString()
    return parts.map { it.trim() }.filter { it.isNotEmpty() }.take(MAX_TYPED).map { entry ->
        val open = entry.lastIndexOf('<')
        val close = entry.lastIndexOf('>')
        if (open >= 0 && close > open) {
            AddressRecord(entry.substring(0, open).trim().trim('"').trim(), entry.substring(open + 1, close).trim())
        } else {
            AddressRecord("", entry.trim('"'))
        }
    }
}

private const val MAX_TYPED = 200

internal fun RecipientRecord.toAddress() = AddressRecord(name, address)

/** How a person on a line of recipients is shown. */
internal fun recipientLabel(who: RecipientRecord): String = who.name.ifBlank { who.address }

/** Which accounts a message can be sent from, the one it is from first, and which of them are shared mailboxes. */
internal fun fromOrder(choices: List<FromChoice>, current: ULong): List<FromChoice> =
    choices.sortedWith(compareByDescending<FromChoice> { it.account == current }.thenBy { it.shared })

/** The line the From choice shows: the name and the address, and "shared mailbox" for one. */
internal fun fromLabel(texts: Texts, choice: FromChoice): String {
    val who = if (choice.name.isBlank() || choice.name == choice.address) choice.address else "${choice.name} <${choice.address}>"
    return if (choice.shared) "$who (${texts.t("shared mailbox")})" else who
}

/** The words of the title of the screen. */
internal fun composeTitle(texts: Texts, kind: uniffi.mailcraft_mobile.ComposeKind): String = when (kind) {
    uniffi.mailcraft_mobile.ComposeKind.NEW -> texts.t("New message")
    uniffi.mailcraft_mobile.ComposeKind.REPLY, uniffi.mailcraft_mobile.ComposeKind.REPLY_ALL -> texts.t("Reply")
    uniffi.mailcraft_mobile.ComposeKind.FORWARD -> texts.t("Forward")
    uniffi.mailcraft_mobile.ComposeKind.DRAFT -> texts.t("Draft")
}

/** The look at the caret: what the toggles on the bar show as on. */
internal data class LookAt(val bold: Boolean = false, val italic: Boolean = false, val underline: Boolean = false, val link: Boolean = false)

/** The look of the character just before [caret] (the one typing goes on from), or of the first when the caret is at the start. */
internal fun lookAt(view: ComposeView, caret: Int): LookAt {
    val at = (if (caret > 0) caret - 1 else 0).toLong()
    val on = view.spans.filter { it.start.toLong() <= at && at < it.end.toLong() }
    return LookAt(on.any { it.bold }, on.any { it.italic }, on.any { it.underline }, on.any { it.link != null })
}
