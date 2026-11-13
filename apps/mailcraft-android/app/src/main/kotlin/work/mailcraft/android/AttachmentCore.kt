// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import android.content.Context
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.core.net.toUri
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.mailcraft_mobile.AttachmentRecord
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/** The attachments the reading pane lists: the files, else (when there is no other) the pictures too. */
internal fun listedAttachments(all: List<AttachmentRecord>): List<AttachmentItem> =
    all.filter { !it.inline }.ifEmpty { all }.map { record ->
        val check = uniffi.mailcraft_mobile.attachmentCheck(record.name)
        AttachmentItem(
            index = record.index,
            name = check.name,
            saveName = check.saveName,
            size = record.size.toLong(),
            verdict = check.verdict,
            blocked = check.blocked,
            calendar = check.calendar,
        )
    }

/** [AttachmentSource] over the Rust core, which holds every rule: the verdict, the caps, the names. */
internal class CoreAttachments(
    private val core: Core,
    private val dispatcher: CoroutineDispatcher = Dispatchers.Default,
) : AttachmentSource {
    override suspend fun bytes(id: ULong, index: UInt): AttachmentBytes = withContext(dispatcher) {
        try {
            val file = core.attachment(id, index)
            AttachmentBytes.Got(file.bytes, file.check.saveName, file.picture.isNotEmpty())
        } catch (e: CoreException) {
            AttachmentBytes.Failed(e.message.orEmpty())
        }
    }

    override suspend fun open(id: ULong, index: UInt, confirmed: Boolean): AttachmentOutcome = withContext(dispatcher) {
        try {
            core.openAttachment(id, index, confirmed)
            AttachmentOutcome.Done
        } catch (e: CoreException) {
            AttachmentOutcome.Refused(e.message.orEmpty())
        }
    }

    override suspend fun importCalendar(id: ULong, index: UInt): AttachmentOutcome = withContext(dispatcher) {
        try {
            val done = core.importCalendarAttachment(id, index)
            AttachmentOutcome.Imported(done.events.toInt(), done.tasks.toInt())
        } catch (e: CoreException) {
            AttachmentOutcome.Refused(e.message.orEmpty())
        }
    }
}

/** [AttachmentDevice] over the phone: the share sheet through a one-off grant, and the chosen document. */
internal class AndroidAttachmentDevice(
    private val context: Context,
    private val files: OpenedFiles = OpenedFiles(context),
) : AttachmentDevice {
    override fun share(name: String, bytes: ByteArray): Boolean = runCatching { files.share(name, bytes) }.isSuccess

    override fun write(target: String, bytes: ByteArray): Boolean {
        val uri: Uri = runCatching { target.toUri() }.getOrNull() ?: return false
        return SaveToFiles.write({ context.contentResolver.openOutputStream(uri) }, bytes)
    }
}
