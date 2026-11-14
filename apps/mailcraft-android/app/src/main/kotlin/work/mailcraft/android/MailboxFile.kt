// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONObject
import uniffi.mailcraft_mobile.Core
import uniffi.mailcraft_mobile.CoreException

/**
 * Where the mailbox is kept between runs: one file, and the folder of message content beside it
 * (the core's `file.save`), in the app's private no-backup storage. What is in it is the accounts and
 * their mail; no password or token is (those are in the Keystore-sealed store). A file that can't be
 * opened is put aside, and if it can't be put aside either, nothing is written to its place this
 * session ([savingOff]): the original is never overwritten.
 *
 * [setAside] moves a file that can't be read out of the way; true when it was moved.
 */
internal class MailboxFile(
    private val dir: File,
    private val setAside: (File, File) -> Boolean = { from, to -> from.renameTo(to) },
) {
    private val file = File(dir, NAME)
    private val off = MutableStateFlow(false)

    /** True when the file could neither be read nor put aside: nothing is saved over it. */
    val savingOff: StateFlow<Boolean> = off

    /** Opens what was kept, when something was. */
    suspend fun restore(core: Core) = restore { path ->
        core.execute("file.open", """{"path":${JSONObject.quote(path)}}""")
    }

    /** Keeps the mailbox as it is now. */
    suspend fun keep(core: Core) = keep { path ->
        core.execute("file.save", """{"path":${JSONObject.quote(path)}}""")
    }

    internal suspend fun restore(open: suspend (String) -> Unit) {
        if (!file.isFile) return
        try {
            open(file.path)
        } catch (_: CoreException) {
            // Nothing to restore, and the next save must not write over what could not be read (a newer
            // build or a repair may still): it is put aside under a name of its own, and the mailbox
            // starts empty. When it can't be put aside it stays where it is, and nothing is saved there.
            val moved = try {
                setAside(file, File(dir, "$NAME.unreadable-${System.currentTimeMillis()}"))
            } catch (_: SecurityException) {
                false
            }
            if (!moved && file.exists()) off.value = true
        }
    }

    internal suspend fun keep(save: suspend (String) -> Unit) {
        if (off.value) return
        if (!dir.isDirectory && !dir.mkdirs()) return
        try {
            save(file.path)
        } catch (_: CoreException) {
            // Not kept this time; the next change tries again.
        }
    }

    private companion object {
        const val NAME = "mailbox.mailcraft"
    }
}
