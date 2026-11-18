// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/*
 * The addresses the app hands to other programs for a person: to write to them and to call them.
 * Both are built from what a contact card says, which anybody may have written, so each one is
 * checked and anything odd is no link at all. Only `mailto:` and `tel:` ever come out.
 */

/** The longest address and number that is linked. */
private const val MAX_ADDRESS = 320
private const val MAX_NUMBER = 20
private const val MIN_DIGITS = 3

/** Characters that go into a `mailto:` as they are; every other byte is written as `%XX`. */
private fun isPlain(c: Char): Boolean =
    c in 'a'..'z' || c in 'A'..'Z' || c in '0'..'9' || c == '@' || c == '.' || c == '+' || c == '-' || c == '_'

/**
 * `mailto:` and the address of a card, or null when it is not one address: nothing but a name
 * and a domain, no spaces, no line breaks, no second address, no header smuggled in.
 */
internal fun mailtoLink(address: String): String? {
    val text = address.trim()
    val at = text.indexOf('@')
    val one = at > 0 && at == text.lastIndexOf('@') && at < text.length - 1
    val clean = text.length <= MAX_ADDRESS && text.none { it.isWhitespace() || it.isISOControl() || it in "<>,;\"?&%" }
    if (!one || !clean) return null
    val hex = "0123456789ABCDEF"
    val out = StringBuilder("mailto:")
    for (byte in text.toByteArray(Charsets.UTF_8)) {
        val v = byte.toInt() and BYTE
        if (v < ASCII && isPlain(v.toChar())) {
            out.append(v.toChar())
        } else {
            out.append('%').append(hex[v shr NIBBLE]).append(hex[v and NIBBLE_MASK])
        }
    }
    return out.toString()
}

/**
 * `tel:` and the number of a card with its spaces, dashes, dots and brackets taken out, or null
 * when it holds anything else (a letter, an extension, a second number) or is too short or long.
 */
internal fun dialLink(phone: String): String? {
    val text = phone.trim()
    val plus = text.startsWith("+")
    val body = if (plus) text.drop(1) else text
    val digits = body.filter { it in '0'..'9' }
    val others = body.filterNot { it in '0'..'9' || it in " -.() " }
    val ok = others.isEmpty() && digits.length in MIN_DIGITS..MAX_NUMBER
    return if (ok) "tel:" + (if (plus) "+" else "") + digits else null
}

private const val BYTE = 0xFF
private const val ASCII = 0x80
private const val NIBBLE = 4
private const val NIBBLE_MASK = 0x0F
