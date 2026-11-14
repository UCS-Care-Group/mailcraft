// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import java.io.File
import java.io.IOException
import java.io.OutputStream
import java.util.Locale
import java.util.UUID
import uniffi.mailcraft_mobile.HostException

/** A file name that is safe to write: one segment, no control or reserved characters, bounded. */
internal fun safeFileName(name: String): String {
    val last = name.substringAfterLast('/').substringAfterLast('\\')
    val cleaned = last.filter { it.code >= SPACE && it !in RESERVED }.trim().trim('.').trim()
    if (cleaned.isEmpty()) return FALLBACK
    if (utf8Length(cleaned) <= MAX_NAME_BYTES) return cleaned
    val dot = cleaned.lastIndexOf('.')
    val tail = if (dot > 0) cleaned.substring(dot) else ""
    val extension = if (utf8Length(tail) <= MAX_EXTENSION_BYTES) tail else ""
    val stem = cleaned.substring(0, cleaned.length - extension.length)
    val base = takeBytes(stem, MAX_NAME_BYTES - utf8Length(extension))
    return base.trim().trim('.').ifEmpty { FALLBACK } + extension
}

private fun utf8Length(s: String): Int = s.toByteArray(Charsets.UTF_8).size

/** The longest start of [s] that is at most [max] UTF-8 bytes, cut between code points. */
private fun takeBytes(s: String, max: Int): String {
    var bytes = 0
    var i = 0
    while (i < s.length) {
        val cp = s.codePointAt(i)
        val width = String(Character.toChars(cp)).toByteArray(Charsets.UTF_8).size
        if (bytes + width > max) break
        bytes += width
        i += Character.charCount(cp)
    }
    return s.substring(0, i)
}

/** Whether the system's package installers would take this file; they are never handed one. */
internal fun isInstallable(name: String): Boolean =
    name.substringAfterLast('.', "").lowercase(Locale.ROOT) in INSTALLABLE

/** The most bytes of one attachment the app writes for another program to open. */
internal const val MAX_OPEN_BYTES = 64 * 1024 * 1024

/** Why a file of this name and size is not written for another program, as the binding's error. */
internal fun refuseUnlessOpenable(safeName: String, size: Int) {
    if (isInstallable(safeName)) throw HostException.Failed()
    if (size > MAX_OPEN_BYTES) throw HostException.TooLong()
}

/** Removes everything under [dir], and [dir] itself. */
internal fun clearDirectory(dir: File) {
    dir.walkBottomUp().forEach { it.delete() }
}

/** Removes each copy folder under [dir] last written more than [maxAgeMillis] before [now]. */
internal fun clearOlderThan(dir: File, maxAgeMillis: Long, now: Long) {
    dir.listFiles()?.forEach { child ->
        if (now - child.lastModified() > maxAgeMillis) clearDirectory(child)
    }
}

private val RESERVED = setOf('/', '\\', ':', '*', '?', '"', '<', '>', '|')
private val INSTALLABLE = setOf("apk", "apks", "xapk", "aab")
private const val SPACE = 0x20
private const val MAX_NAME_BYTES = 255
private const val MAX_EXTENSION_BYTES = 12
private const val FALLBACK = "attachment"

/**
 * Opening an attachment in the program the system chooses for its kind. The bytes are written to a
 * folder of their own in the cache, and the other program is given one read-only grant to that one
 * file through a [FileProvider] address: it can read it for as long as it holds the grant and cannot
 * write it, and it never learns a path. The Rust side has already refused the kinds that are
 * blocked; installers are refused here too. Copies are cleared at start ([clear]).
 */
internal class OpenedFiles(private val context: Context) {
    private val root: File get() = File(context.cacheDir, FOLDER)

    fun open(name: String, bytes: ByteArray) {
        val safe = safeFileName(name)
        refuseUnlessOpenable(safe, bytes.size)
        val folder = File(root, UUID.randomUUID().toString())
        if (!folder.mkdirs() && !folder.isDirectory) throw IOException("no folder")
        val file = File(folder, safe)
        file.writeBytes(bytes)
        val uri = FileProvider.getUriForFile(context, authority(context), file)
        val mime = MimeTypeMap.getSingleton()
            .getMimeTypeFromExtension(safe.substringAfterLast('.', "").lowercase(Locale.ROOT))
            ?: "application/octet-stream"
        val intent = Intent(Intent.ACTION_VIEW)
        intent.setDataAndType(uri, mime)
        // The clip is what carries the grant to the program that answers.
        intent.clipData = ClipData.newRawUri(safe, uri)
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            throw HostException.Failed()
        }
    }

    /**
     * Takes away the copies older than a day. A viewer the owner left open may still be reading a
     * recent one after the app's process restarted, so only the old ones go.
     */
    fun clearOld(now: Long = System.currentTimeMillis()) {
        clearOlderThan(root, MAX_AGE_MILLIS, now)
    }

    internal companion object {
        const val FOLDER = "attachments"
        const val MAX_AGE_MILLIS = 24L * 60 * 60 * 1000

        fun authority(context: Context): String = context.packageName + ".files"
    }
}

/** Saving a file where the owner chooses, through the system's document picker (create-document). */
internal object SaveToFiles {
    /** The intent that asks the system for a new document named [name]. */
    fun createIntent(name: String, mime: String = "application/octet-stream"): Intent =
        Intent(Intent.ACTION_CREATE_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType(mime)
            .putExtra(Intent.EXTRA_TITLE, safeFileName(name))

    /** Writes [bytes] to what [open] gives (the chosen document); false when it could not. */
    fun write(open: () -> OutputStream?, bytes: ByteArray): Boolean =
        try {
            open()?.use { it.write(bytes) } != null
        } catch (_: IOException) {
            false
        } catch (_: SecurityException) {
            false
        }
}
