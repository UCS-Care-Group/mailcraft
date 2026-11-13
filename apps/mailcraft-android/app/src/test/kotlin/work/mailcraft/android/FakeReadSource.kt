// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
@file:Suppress("MaxLineLength")

package work.mailcraft.android

/** A read source over invented messages that records what it is asked, for the tests. No network, ever. */
internal class FakeReadSource(var message: OpenedMessage? = null) : ReadSource {
    val opened = mutableListOf<Triple<ULong, Int, Boolean>>()
    val downloads = mutableListOf<String>()
    val senderDownloads = mutableListOf<String>()
    val marked = mutableListOf<ULong>()
    val parts = mutableMapOf<String, ByteArray>()
    var dataSaver = false
    var factsAnswer: MessageFacts? = MessageFacts("ada@example.org", junk = false, vouched = true, known = true)

    /** What a download of an address gives; a missing address is refused. */
    val web = mutableMapOf<String, ByteArray>()
    val senderWeb = mutableMapOf<String, ByteArray>()

    override suspend fun open(id: ULong, widthDp: Int, dark: Boolean): OpenedMessage? {
        opened += Triple(id, widthDp, dark)
        return message
    }

    override suspend fun part(id: ULong, reference: String): ByteArray? = parts[reference]

    override suspend fun download(url: String): Fetched {
        downloads += url
        return web[url]?.let { Fetched.Got(it) } ?: Fetched.Refused("it is no longer there (404)")
    }

    override suspend fun facts(id: ULong): MessageFacts? = factsAnswer

    override fun senderPlaces(address: String): List<String> =
        listOf("https://gravatar.example/avatar/${address.hashCode()}", "https://favicon.example/${address.substringAfter('@')}")

    override suspend fun downloadSender(url: String): Fetched {
        senderDownloads += url
        return senderWeb[url]?.let { Fetched.Got(it) } ?: Fetched.Refused("404")
    }

    override suspend fun markRead(id: ULong): Boolean {
        marked += id
        return true
    }

    override suspend fun dataSaving(): Boolean = dataSaver
}
