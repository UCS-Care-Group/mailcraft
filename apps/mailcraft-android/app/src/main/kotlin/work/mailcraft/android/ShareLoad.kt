// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.net.toUri
import java.io.IOException

/** What reading one shared address came to. */
internal sealed interface ShareRead {
    class Bytes(val name: String?, val bytes: ByteArray) : ShareRead

    /** More than the cap allows (the size the provider gave, or what was read before the cap was reached). */
    class TooLarge(val name: String?) : ShareRead

    data object Unreadable : ShareRead
}

/** Reads a shared file's bytes, at most [cap] of them; blocking, so it is called off the main thread. */
internal interface ShareReader {
    fun read(uri: String, cap: Int): ShareRead
}

/** Why a shared file was not attached. */
internal enum class Why { TooMany, TooLarge, TotalTooLarge, Unreadable, NotAllowed }

/** A shared file that was not attached: [name] is the safe name when it is known; [count] is how many this stands for. */
internal class Refusal(val why: Why, val name: String? = null, val count: Int = 1)

internal class SharedFile(val name: String, val bytes: ByteArray, val kind: ShareKind)

internal class ShareLoad(val files: List<SharedFile>, val refusals: List<Refusal>)

/**
 * Reads the files of a share in order under the caps: [MAX_SHARED_FILES] of them, [MAX_SHARED_FILE] each and
 * [MAX_SHARED_TOTAL] together, with no more than the room that is left ever read. Whatever is refused is
 * listed with why, and what can be attached still is.
 */
internal fun loadShared(uris: List<String>, reader: ShareReader, ownAuthority: String): ShareLoad {
    val files = mutableListOf<SharedFile>()
    val refusals = mutableListOf<Refusal>()
    var total = 0L
    var extra = 0
    uris.forEachIndexed { index, uri ->
        if (index >= MAX_SHARED_FILES) {
            extra++
            return@forEachIndexed
        }
        if (!shareableUri(uri, ownAuthority)) {
            refusals += Refusal(Why.NotAllowed)
            return@forEachIndexed
        }
        val room = MAX_SHARED_TOTAL - total
        val cap = minOf(MAX_SHARED_FILE.toLong(), room).toInt()
        val squeezed = cap < MAX_SHARED_FILE
        if (cap <= 0) {
            refusals += Refusal(Why.TotalTooLarge)
            return@forEachIndexed
        }
        when (val read = reader.read(uri, cap)) {
            is ShareRead.Bytes -> {
                val kind = kindOf(read.bytes)
                total += read.bytes.size
                files += SharedFile(safeShareName(read.name, kind), read.bytes, kind)
            }
            is ShareRead.TooLarge -> refusals += Refusal(if (squeezed) Why.TotalTooLarge else Why.TooLarge, read.name?.let { safeShareName(it) })
            ShareRead.Unreadable -> refusals += Refusal(Why.Unreadable)
        }
    }
    if (extra > 0) refusals += Refusal(Why.TooMany, count = extra)
    return ShareLoad(files, refusals)
}

/** The refusals in plain words, one sentence each, in the owner's language. */
internal fun refusalText(texts: Texts, refusals: List<Refusal>): String = refusals.joinToString(" ") { refusal ->
    val name = refusal.name ?: texts.t("A file")
    when (refusal.why) {
        Why.TooMany -> texts.tn(
            refusal.count.toULong(),
            "{n} more file was not attached: a message takes at most {max} files from a share.",
            "{n} more files were not attached: a message takes at most {max} files from a share.",
        ).replace("{max}", MAX_SHARED_FILES.toString())
        Why.TooLarge -> texts.t("{name} was not attached: it is larger than 25 MB.").replace("{name}", name)
        Why.TotalTooLarge -> texts.t("{name} was not attached: the files on a message can be 50 MB together.").replace("{name}", name)
        Why.Unreadable -> texts.t("{name} could not be read, so it was not attached.").replace("{name}", name)
        Why.NotAllowed -> texts.t("{name} was not attached: it is not a file this app may read.").replace("{name}", name)
    }
}

/** Reads shared files through the system's resolver; no storage permission is held, only what the sender granted. */
internal class ResolverShareReader(private val context: Context) : ShareReader {
    override fun read(uri: String, cap: Int): ShareRead {
        val address: Uri = runCatching { uri.toUri() }.getOrNull() ?: return ShareRead.Unreadable
        return try {
            val (name, size) = describe(address)
            if (size != null && size > cap) {
                ShareRead.TooLarge(name)
            } else {
                val stream = context.contentResolver.openInputStream(address)
                val bytes = stream?.use { readCapped(it, cap) }
                when {
                    stream == null -> ShareRead.Unreadable
                    bytes == null -> ShareRead.TooLarge(name)
                    else -> ShareRead.Bytes(name, bytes)
                }
            }
        } catch (_: IOException) {
            ShareRead.Unreadable
        } catch (_: SecurityException) {
            ShareRead.Unreadable
        } catch (_: IllegalArgumentException) {
            ShareRead.Unreadable
        } catch (_: IllegalStateException) {
            ShareRead.Unreadable
        } catch (_: UnsupportedOperationException) {
            ShareRead.Unreadable
        }
    }

    private fun describe(uri: Uri): Pair<String?, Long?> = try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
            if (!c.moveToFirst()) return@use null to null
            val name = c.getString(0)
            val size = if (c.isNull(1)) null else c.getLong(1).takeIf { it >= 0 }
            name to size
        } ?: (null to null)
    } catch (_: RuntimeException) {
        null to null
    }
}
