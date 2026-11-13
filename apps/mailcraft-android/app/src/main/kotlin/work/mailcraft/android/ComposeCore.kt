// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength", "TooManyFunctions")

package work.mailcraft.android

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.net.toUri
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.InputStream
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.AddressRecord
import uniffi.mailcraft_mobile.ComposeFormat
import uniffi.mailcraft_mobile.ComposeStart
import uniffi.mailcraft_mobile.ComposeView
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException
import uniffi.mailcraft_mobile.FromChoice
import uniffi.mailcraft_mobile.RecipientLine
import uniffi.mailcraft_mobile.SuggestionRecord

/** [ComposeSource] over the Rust core, which holds the message and every rule about it. */
internal class CoreCompose(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
) : ComposeSource {
    private suspend fun result(call: suspend () -> ComposeView): ComposeResult = withContext(dispatcher) {
        try {
            ComposeResult.Ok(call())
        } catch (e: CoreException) {
            ComposeResult.Failed(e.message.orEmpty())
        }
    }

    override suspend fun start(start: ComposeStart) = result { core.composeStart(start) }

    override suspend fun view(compose: ULong) = result { core.composeView(compose) }

    override suspend fun edit(compose: ULong, edit: BodyEdit) = result {
        core.composeEdit(compose, edit.from.toUInt(), edit.to.toUInt(), edit.text)
    }

    override suspend fun format(compose: ULong, format: ComposeFormat, from: Int, to: Int) = result {
        core.composeFormat(compose, format, from.toUInt(), to.toUInt())
    }

    override suspend fun recipients(compose: ULong, line: RecipientLine, people: List<AddressRecord>) =
        result { core.composeRecipients(compose, line, people) }

    override suspend fun subject(compose: ULong, subject: String) = result { core.composeSubject(compose, subject) }

    override suspend fun from(compose: ULong, account: ULong) = result { core.composeFrom(compose, account) }

    override suspend fun attach(compose: ULong, name: String, bytes: ByteArray) =
        result { core.composeAttach(compose, name, bytes) }

    override suspend fun removeFile(compose: ULong, index: UInt) = result { core.composeRemoveFile(compose, index) }

    override suspend fun picture(compose: ULong, bytes: ByteArray, alt: String, at: Int) =
        result { core.composePicture(compose, bytes, alt, at.toUInt()) }

    override suspend fun pictureBytes(compose: ULong, id: UInt): ByteArray? = withContext(dispatcher) {
        try {
            core.composePictureBytes(compose, id).bytes
        } catch (_: CoreException) {
            null
        }
    }

    override suspend fun suggest(query: String): List<SuggestionRecord> = withContext(dispatcher) {
        try {
            core.suggestRecipients(query, SUGGESTIONS)
        } catch (_: CoreException) {
            emptyList()
        }
    }

    override suspend fun accounts(): List<FromChoice> = withContext(dispatcher) {
        try {
            core.composeAccounts()
        } catch (_: CoreException) {
            emptyList()
        }
    }

    override suspend fun save(compose: ULong): Boolean = withContext(dispatcher) {
        try {
            core.composeSave(compose)
            true
        } catch (_: CoreException) {
            false
        }
    }

    override suspend fun send(compose: ULong): SendResult = withContext(dispatcher) {
        try {
            SendResult.Sent(core.composeSend(compose).queued)
        } catch (e: CoreException) {
            SendResult.Failed(e.message.orEmpty())
        }
    }

    override suspend fun close(compose: ULong, discard: Boolean): Boolean = withContext(dispatcher) {
        try {
            core.composeClose(compose, discard)
            true
        } catch (_: CoreException) {
            false
        }
    }

    private companion object {
        const val SUGGESTIONS = 6u
    }
}

/** A file the owner chose in the system's picker: the name it is under and what is in it. */
internal class PickedFile(val name: String, val bytes: ByteArray)

/** The most bytes read from one picked file (the core's own cap is the same: it refuses more). */
internal const val MAX_PICKED_BYTES = 25 * 1024 * 1024

/** Everything of [stream] up to [cap] bytes, or null when there is more: a file larger than the cap is never held. */
internal fun readCapped(stream: InputStream, cap: Int): ByteArray? {
    val out = ByteArrayOutputStream()
    val buffer = ByteArray(BUFFER)
    var total = 0
    while (true) {
        val n = stream.read(buffer)
        if (n < 0) break
        total += n
        if (total > cap) return null
        out.write(buffer, 0, n)
    }
    return out.toByteArray()
}

private const val BUFFER = 16 * 1024

/** Reads what the picker chose, through the system's own resolver (no storage permission is held). */
internal class PickedFiles(private val context: Context) {
    fun read(address: String): PickedFile? {
        val uri: Uri = runCatching { address.toUri() }.getOrNull() ?: return null
        return try {
            val bytes = context.contentResolver.openInputStream(uri)?.use { readCapped(it, MAX_PICKED_BYTES) } ?: return null
            PickedFile(displayName(uri), bytes)
        } catch (_: IOException) {
            null
        } catch (_: SecurityException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        } catch (_: IllegalStateException) {
            null
        }
    }

    private fun displayName(uri: Uri): String = try {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        }
    } catch (_: RuntimeException) {
        null
    } ?: "attachment"
}
