// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

/** A read source over invented messages that records what it is asked, for the tests. */
internal class FakeReadSource(var message: OpenedMessage? = null) : ReadSource {
    val opened = mutableListOf<Triple<ULong, Int, Boolean>>()
    val marked = mutableListOf<ULong>()

    override suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage? {
        opened += Triple(id, widthDp, dark)
        return message
    }

    override suspend fun markRead(id: ULong): Boolean {
        marked += id
        return true
    }
}
