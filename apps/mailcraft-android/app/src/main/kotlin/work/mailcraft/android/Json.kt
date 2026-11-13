// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/**
 * Writes the parameters of a command. Only what the app itself builds goes through here (numbers,
 * truthy flags and names the owner chose); nothing that comes from outside is read as JSON in Kotlin.
 */
internal fun jsonText(value: Any?): String = when (value) {
    null -> "null"
    is Boolean -> value.toString()
    is ULong, is Long, is Int, is UInt -> value.toString()
    is String -> quoted(value)
    is List<*> -> value.joinToString(",", "[", "]") { jsonText(it) }
    is Map<*, *> -> value.entries.joinToString(",", "{", "}") { quoted(it.key.toString()) + ":" + jsonText(it.value) }
    else -> quoted(value.toString())
}

private const val FIRST_PRINTABLE = 0x20
private const val HEX = 16
private const val HEX_DIGITS = 4

private fun quoted(text: String): String = buildString {
    append('"')
    for (c in text) {
        when {
            c == '"' -> append("\\\"")
            c == '\\' -> append("\\\\")
            c == '\n' -> append("\\n")
            c == '\r' -> append("\\r")
            c == '\t' -> append("\\t")
            c.code < FIRST_PRINTABLE -> append("\\u").append(c.code.toString(HEX).padStart(HEX_DIGITS, '0'))
            else -> append(c)
        }
    }
    append('"')
}
