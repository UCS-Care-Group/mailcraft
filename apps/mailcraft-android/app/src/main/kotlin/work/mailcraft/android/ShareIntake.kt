// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

/** The most files one share takes; more are refused, and said so. */
internal const val MAX_SHARED_FILES = 20

/** The most bytes of one shared file, and of all of a share's files together (the composer's own caps). */
internal const val MAX_SHARED_FILE = 25 * 1024 * 1024
internal const val MAX_SHARED_TOTAL = 50 * 1024 * 1024

/** The most shared text taken, in characters (the core cuts at the same number). */
internal const val MAX_SHARED_TEXT = 100_000

/** The most addresses read off one share: a hostile sender can list thousands, and only the first few are looked at. */
private const val MAX_SHARED_ADDRESSES = 200

/** The longest file name kept, in characters, extension included. */
internal const val MAX_NAME_CHARS = 100

private const val ACTION_SENDTO = "android.intent.action.SENDTO"
private const val ACTION_VIEW = "android.intent.action.VIEW"
private const val ACTION_SEND = "android.intent.action.SEND"
private const val ACTION_SEND_MULTIPLE = "android.intent.action.SEND_MULTIPLE"

/** What another app asked MailCraft to do, in the app's own terms and without any Android type. */
internal sealed interface Incoming {
    /** A `mailto:` link; the core reads it (RFC 6068) and only ever starts a message from it. */
    class Mailto(val link: String) : Incoming

    /** Text, a link, and files (as the addresses the system gave) to start a message with. */
    class Share(val subject: String?, val text: String?, val files: List<String>) : Incoming
}

/**
 * Which of the things MailCraft is registered for an intent is, from its parts: nothing in it is trusted
 * and nothing in it is read here but what routes it. The link of a mailto is passed on as it is for the core
 * to read; the text is cut to its cap; the files are only addresses, read later, one at a time, under caps.
 * The kind the sender says its data is (the MIME type) is never used.
 */
internal fun routeIncoming(
    action: String?,
    data: String?,
    text: String?,
    subject: String?,
    streams: List<String>,
): Incoming? = when (action) {
    ACTION_SENDTO, ACTION_VIEW -> data?.takeIf { isMailto(it) }?.let { Incoming.Mailto(it.trim()) }
    ACTION_SEND, ACTION_SEND_MULTIPLE -> {
        val words = text?.take(MAX_SHARED_TEXT)?.takeIf { it.isNotBlank() }
        val files = streams.take(MAX_SHARED_ADDRESSES)
        if (words == null && files.isEmpty()) null else Incoming.Share(subject?.takeIf { it.isNotBlank() }, words, files)
    }
    else -> null
}

/** Whether [text] is a `mailto:` link, by its scheme and whatever its case. */
internal fun isMailto(text: String): Boolean = text.trim().startsWith("mailto:", ignoreCase = true)

/**
 * Whether the system's address [uri] may be read as a shared file: a `content:` address of some other
 * program's provider. A `file:` address (which would read this program's own private files for a hostile
 * sender), an address of this program's own provider, and anything else are refused.
 */
internal fun shareableUri(uri: String, ownAuthority: String): Boolean {
    val trimmed = uri.trim()
    if (!trimmed.startsWith("content://", ignoreCase = true)) return false
    val rest = trimmed.substring("content://".length)
    val authority = rest.takeWhile { it != '/' && it != '?' && it != '#' }.substringAfterLast('@').lowercase()
    return authority.isNotEmpty() && authority != ownAuthority.lowercase()
}

/** What a shared file is, read from its own bytes. */
internal enum class ShareKind(val extension: String?) {
    Png("png"),
    Jpeg("jpg"),
    Gif("gif"),
    Webp("webp"),
    Other(null),
    ;

    val picture: Boolean get() = this != Other
}

private fun latin1(text: String): ByteArray = text.toByteArray(Charsets.ISO_8859_1)

private val PNG = latin1("\u0089PNG\r\n\u001A\n")
private val JPEG = latin1("\u00FF\u00D8\u00FF")
private val GIF87 = latin1("GIF87a")
private val GIF89 = latin1("GIF89a")
private val RIFF = latin1("RIFF")
private val WEBP = latin1("WEBP")

/** Where WebP's own mark sits in its RIFF wrapper, after the tag and the size. */
private const val WEBP_AT = 8

private fun ByteArray.has(sign: ByteArray, at: Int = 0): Boolean =
    size >= at + sign.size && sign.indices.all { this[at + it] == sign[it] }

/** The kind of [bytes] by their first bytes (PNG, JPEG, GIF, WebP): a name or a MIME type that says more is no evidence. */
internal fun kindOf(bytes: ByteArray): ShareKind = when {
    bytes.has(PNG) -> ShareKind.Png
    bytes.has(JPEG) -> ShareKind.Jpeg
    bytes.has(GIF87) || bytes.has(GIF89) -> ShareKind.Gif
    bytes.has(RIFF) && bytes.has(WEBP, WEBP_AT) -> ShareKind.Webp
    else -> ShareKind.Other
}

private const val FALLBACK_NAME = "attachment"
private const val MAX_EXTENSION = 10

/**
 * A name a file can be attached under: the last part of whatever path was given, with control, line-break
 * and direction-changing characters taken out, no leading or trailing dots and spaces, and at most
 * [MAX_NAME_CHARS] characters with the extension kept. A name with nothing left is "picture" (with its
 * extension) when [kind] says the bytes are a picture, else "attachment".
 */
internal fun safeShareName(raw: String?, kind: ShareKind = ShareKind.Other): String {
    val last = raw.orEmpty().substringAfterLast('/').substringAfterLast('\\')
    val cleaned = last.filterNot { it.isISOControl() || it in HIDDEN }.trim().trim('.', ' ')
    if (cleaned.isEmpty()) return kind.extension?.let { "picture.$it" } ?: FALLBACK_NAME
    if (cleaned.length <= MAX_NAME_CHARS) return cleaned
    val dot = cleaned.lastIndexOf('.')
    val extension = if (dot > 0 && cleaned.length - dot - 1 in 1..MAX_EXTENSION) cleaned.substring(dot) else ""
    val stem = cleaned.substring(0, cleaned.length - extension.length).take(MAX_NAME_CHARS - extension.length)
    // Never leave half of a pair of surrogates.
    val whole = if (stem.lastOrNull()?.isHighSurrogate() == true) stem.dropLast(1) else stem
    return whole.trim().ifEmpty { FALLBACK_NAME } + extension
}

private const val BOM = '\uFEFF'

/** Characters that change how text reads without showing: direction overrides and isolates, zero-width marks, the BOM. */
private val HIDDEN: Set<Char> = buildSet {
    addAll('​'..'‏')
    addAll('‪'..'‮')
    addAll('⁠'..'⁤')
    addAll('⁦'..'⁩')
    add(BOM)
}
