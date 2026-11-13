// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.ContentResolver
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import java.io.IOException
import java.io.InputStream

/** A calendar file another app handed over: its name and its bytes, at most [ICS_MAX_BYTES]. */
internal class IcsFile(val name: String, val bytes: ByteArray)

/** What reading a handed-over file came to. */
internal sealed interface IcsRead {
    class Got(val file: IcsFile) : IcsRead

    /** The file is bigger than a calendar file may be, or could not be read. */
    data object Unreadable : IcsRead

    /** The intent is not a calendar file for us. */
    data object NotOurs : IcsRead
}

/** Whether [type] is what calendar files are called. */
internal fun isCalendarType(type: String?): Boolean {
    val base = type?.substringBefore(';')?.trim()?.lowercase().orEmpty()
    return base == "text/calendar" || base == "application/ics"
}

/** Reads at most [max] bytes of [stream]; null when it holds more (nothing of it is kept then). */
internal fun readCapped(stream: InputStream, max: Int): ByteArray? {
    val out = java.io.ByteArrayOutputStream()
    val buffer = ByteArray(BUFFER)
    while (true) {
        val n = stream.read(buffer)
        if (n < 0) break
        if (out.size() + n > max) return null
        out.write(buffer, 0, n)
    }
    return out.toByteArray()
}

private const val BUFFER = 8192

/**
 * The calendar file an "open with" or "view" intent carries, read from its `content:` address with
 * no permission of ours but the one the sender granted. Nothing else is looked at; a file is read
 * to its cap and no further, and everything that can go wrong is an [IcsRead.Unreadable].
 */
internal fun readIcs(resolver: ContentResolver, intent: Intent): IcsRead {
    val uri: Uri = intent.data ?: return IcsRead.NotOurs
    val type = intent.type ?: resolver.getType(uri)
    if (intent.action != Intent.ACTION_VIEW || uri.scheme != ContentResolver.SCHEME_CONTENT || !isCalendarType(type)) {
        return IcsRead.NotOurs
    }
    return try {
        val bytes = resolver.openInputStream(uri)?.use { readCapped(it, ICS_MAX_BYTES) }
        if (bytes == null) IcsRead.Unreadable else IcsRead.Got(IcsFile(nameOf(resolver, uri), bytes))
    } catch (_: IOException) {
        IcsRead.Unreadable
    } catch (_: SecurityException) {
        IcsRead.Unreadable
    }
}

private fun nameOf(resolver: ContentResolver, uri: Uri): String = try {
    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
        if (c.moveToFirst()) c.getString(0).orEmpty() else ""
    }.orEmpty()
} catch (_: SecurityException) {
    ""
}
