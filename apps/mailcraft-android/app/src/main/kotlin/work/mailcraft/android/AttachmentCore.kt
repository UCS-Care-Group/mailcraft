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

/**
 * The records the reading pane lists: the files. A picture the body shows (`inline`, as the core
 * decides it) is the body's, never listed, even when the message has no other file.
 */
internal fun listedRecords(all: List<AttachmentRecord>): List<AttachmentRecord> = all.filter { !it.inline }

/** The attachments the reading pane lists ([listedRecords]). */
internal fun listedAttachments(all: List<AttachmentRecord>): List<AttachmentItem> =
    listedRecords(all).map { record ->
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
            val check = file.check
            AttachmentBytes.Got(file.bytes, check.saveName, file.picture.isNotEmpty(), check.verdict, check.blocked)
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
