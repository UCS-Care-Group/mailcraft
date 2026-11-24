// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import androidx.compose.runtime.saveable.Saver

/**
 * What the new-person form holds. Addresses and numbers are one text each, one per line (a comma or
 * semicolon also parts two addresses); [params] is what `people.new` is sent. A new person is kept on
 * this phone alone: the app's sign-in reads contacts but never writes them.
 */
internal data class PersonDraft(
    val first: String = "",
    val last: String = "",
    val emails: String = "",
    val phones: String = "",
    val company: String = "",
    val title: String = "",
    val notes: String = "",
) {
    val emailList: List<String> get() = entries(emails, ",;\\n")
    val phoneList: List<String> get() = entries(phones, ";\\n")

    /** An address the app would not write to is refused here, not left for the core to keep. */
    val badEmail: Boolean get() = emailList.any { mailtoLink(it) == null }

    /** More entries than the core keeps: refused here rather than cut without a word. */
    val tooMany: Boolean get() = emailList.size > MAX_ENTRIES || phoneList.size > MAX_ENTRIES

    /** The core wants a name, a company or an address; an address has to look like one. */
    val canSave: Boolean
        get() = !badEmail && !tooMany &&
            (listOf(first, last, company).any { it.isNotBlank() } || emailList.isNotEmpty())

    fun params(): String {
        val fields = ArrayList<String>()
        fun text(key: String, value: String) {
            val clean = oneLine(value)
            if (clean.isNotEmpty()) fields.add("\"$key\":" + jsonString(clean))
        }
        text("first", first)
        text("last", last)
        text("company", company)
        text("title", title)
        fun list(key: String, label: String, values: List<String>) {
            if (values.isEmpty()) return
            fields.add("\"$key\":" + values.joinToString(prefix = "[", postfix = "]", separator = ",") {
                "{\"label\":\"$label\",\"value\":" + jsonString(it) + "}"
            })
        }
        list("emails", "work", emailList)
        list("phones", "mobile", phoneList)
        val note = notes.trim().takeScalars(MAX_NOTES)
        if (note.isNotEmpty()) fields.add("\"notes\":" + jsonString(note))
        return fields.joinToString(prefix = "{", postfix = "}", separator = ",")
    }

    companion object {
        /** Kept by the system while the app is away: the seven texts, or nothing for no form. */
        val Saver: Saver<PersonDraft?, Any> = Saver(
            save = { d ->
                if (d == null) {
                    arrayListOf<String>()
                } else {
                    arrayListOf(d.first, d.last, d.emails, d.phones, d.company, d.title, d.notes)
                }
            },
            restore = { v ->
                val s = (v as? List<*>)?.filterIsInstance<String>()
                if (s != null && s.size == FIELDS) PersonDraft(s[0], s[1], s[2], s[3], s[4], s[5], s[6]) else null
            },
        )
    }
}

private const val FIELDS = 7
internal const val MAX_PERSON_LINE = 300
internal const val MAX_ENTRIES = 20

/** The text on one line, trimmed and cut as the core cuts it. */
internal fun oneLine(text: String): String =
    text.replace('\r', ' ').replace('\n', ' ').trim().takeScalars(MAX_PERSON_LINE)

internal fun entries(text: String, separators: String): List<String> =
    text.split(Regex("[$separators]")).map { oneLine(it) }.filter { it.isNotEmpty() }
