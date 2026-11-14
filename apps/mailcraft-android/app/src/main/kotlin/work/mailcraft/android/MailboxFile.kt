// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import org.json.JSONObject
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/**
 * Where the mailbox is kept between runs: one file, and the folder of message content beside it
 * (the core's `file.save`), in the app's private no-backup storage. What is in it is the accounts and
 * their mail; no password or token is (those are in the Keystore-sealed store). A file that can't be
 * opened is as if there were none.
 */
internal class MailboxFile(private val dir: File) {
    private val file = File(dir, NAME)

    /** Opens what was kept, when something was. */
    suspend fun restore(core: Core) {
        if (!file.isFile) return
        try {
            core.execute("file.open", """{"path":${JSONObject.quote(file.path)}}""")
        } catch (_: CoreException) {
            // Nothing to restore, and the next save must not write over what could not be read (a newer
            // build or a repair may still): it is put aside under a name of its own, and the mailbox
            // starts empty.
            file.renameTo(File(dir, "$NAME.unreadable-${System.currentTimeMillis()}"))
        }
    }

    /** Keeps the mailbox as it is now. */
    suspend fun keep(core: Core) {
        if (!dir.isDirectory && !dir.mkdirs()) return
        try {
            core.execute("file.save", """{"path":${JSONObject.quote(file.path)}}""")
        } catch (_: CoreException) {
            // Not kept this time; the next change tries again.
        }
    }

    private companion object {
        const val NAME = "mailbox.mailcraft"
    }
}
