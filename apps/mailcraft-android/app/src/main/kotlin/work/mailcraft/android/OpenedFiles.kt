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
    if (cleaned.length <= MAX_NAME) return cleaned
    val dot = cleaned.lastIndexOf('.')
    val extension = if (dot > 0 && cleaned.length - dot <= MAX_EXTENSION) cleaned.substring(dot) else ""
    return cleaned.take(MAX_NAME - extension.length) + extension
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

/**
 * Where a temporary copy is written: a folder of its own ([folder], a random name made by the caller)
 * under [root], and [name] (already made safe) in it. Never anywhere else: a name that would leave the folder
 * is refused.
 */
internal fun stagedFile(root: File, folder: String, name: String): File {
    if (folder.any { it == '/' || it == '\\' } || folder.contains("..")) throw IOException("outside")
    val own = File(root, folder)
    val file = File(own, safeFileName(name))
    if (file.canonicalFile.parentFile?.canonicalFile != own.canonicalFile) throw IOException("outside")
    return file
}

/** Removes everything under [dir], and [dir] itself. */
internal fun clearDirectory(dir: File) {
    dir.walkBottomUp().forEach { it.delete() }
}

private val RESERVED = setOf('/', '\\', ':', '*', '?', '"', '<', '>', '|')
private val INSTALLABLE = setOf("apk", "apks", "xapk", "aab")
private const val SPACE = 0x20
private const val MAX_NAME = 100
private const val MAX_EXTENSION = 12
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

    /** Writes the temporary copy and gives back its read-only address and the name it is under. */
    private fun stage(name: String, bytes: ByteArray): Pair<android.net.Uri, String> {
        val safe = safeFileName(name)
        refuseUnlessOpenable(safe, bytes.size)
        val file = stagedFile(root, UUID.randomUUID().toString(), safe)
        file.parentFile?.let { if (!it.mkdirs() && !it.isDirectory) throw IOException("no folder") }
        file.writeBytes(bytes)
        return FileProvider.getUriForFile(context, authority(context), file) to safe
    }

    private fun mimeOf(safe: String): String =
        mimeFor(safe) { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it) }

    fun open(name: String, bytes: ByteArray) {
        val (uri, safe) = stage(name, bytes)
        val intent = Intent(Intent.ACTION_VIEW)
        intent.setDataAndType(uri, mimeOf(safe))
        // The clip is what carries the grant to the program that answers.
        intent.clipData = ClipData.newRawUri(safe, uri)
        intent.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (_: ActivityNotFoundException) {
            throw HostException.Failed()
        }
    }

    /** Offers the file to the system's share sheet: the program the owner picks gets one read-only grant. */
    fun share(name: String, bytes: ByteArray) {
        val (uri, safe) = stage(name, bytes)
        val send = Intent(Intent.ACTION_SEND).setType(mimeOf(safe)).putExtra(Intent.EXTRA_STREAM, uri)
        send.clipData = ClipData.newRawUri(safe, uri)
        send.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        val chooser = Intent.createChooser(send, null)
        chooser.clipData = ClipData.newRawUri(safe, uri)
        chooser.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(chooser)
        } catch (_: ActivityNotFoundException) {
            throw HostException.Failed()
        }
    }

    /** The copies left by an earlier run; none is needed once the app has been restarted. */
    fun clear() {
        clearDirectory(root)
    }

    internal companion object {
        const val FOLDER = "attachments"

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
        } catch (_: IllegalArgumentException) {
            false
        } catch (_: IllegalStateException) {
            false
        }
}
