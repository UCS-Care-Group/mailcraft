// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
package work.mailcraft.android

import java.io.File
import java.io.IOException
import java.security.MessageDigest

/** Whether bytes begin as a PNG, JPEG, GIF or WebP does: what a downloaded picture must be. */
internal fun looksLikeAPicture(bytes: ByteArray): Boolean = startsWith(bytes, PNG) ||
    startsWith(bytes, JPEG) ||
    startsWith(bytes, GIF87) ||
    startsWith(bytes, GIF89) ||
    (startsWith(bytes, RIFF) && bytes.size >= WEBP_END && startsWith(bytes.copyOfRange(WEBP_AT, WEBP_END), WEBP))

/** Whether bytes begin as a sender's picture does: a message's kinds, or an ICO that holds a picture or more. */
internal fun looksLikeASenderPicture(bytes: ByteArray): Boolean = looksLikeAPicture(bytes) || isIcon(bytes)

private fun isIcon(bytes: ByteArray): Boolean = startsWith(bytes, ICO) && bytes.size > ICO_COUNT_END &&
    (bytes[ICO_COUNT_AT].toInt() != 0 || bytes[ICO_COUNT_AT + 1].toInt() != 0)

private fun startsWith(bytes: ByteArray, head: ByteArray): Boolean =
    bytes.size >= head.size && head.indices.all { bytes[it] == head[it] }

private val PNG = byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A)
private val JPEG = byteArrayOf(0xFF.toByte(), 0xD8.toByte(), 0xFF.toByte())
private val GIF87 = "GIF87a".toByteArray(Charsets.US_ASCII)
private val GIF89 = "GIF89a".toByteArray(Charsets.US_ASCII)
private val RIFF = "RIFF".toByteArray(Charsets.US_ASCII)
private val WEBP = "WEBP".toByteArray(Charsets.US_ASCII)
private val ICO = byteArrayOf(0, 0, 1, 0)
private const val WEBP_AT = 8
private const val WEBP_END = 12
private const val ICO_COUNT_AT = 4
private const val ICO_COUNT_END = 5

/**
 * The pictures downloaded for mail, kept in the app's cache folder so that they are not asked for
 * twice. A picture is a file named by the SHA-256 of its address (the address itself is never
 * written), and is read back through the same check a download passes. At most [MAX_PICTURES]
 * pictures, [MAX_TOTAL] bytes and [MAX_AGE_MILLIS] old: the oldest go first. [clear] removes all.
 * Nothing here throws: a folder that cannot be used is a cache that is always empty.
 */
internal class PictureCache(
    private val dir: File,
    private val clock: () -> Long = System::currentTimeMillis,
    /** The caps; the desktop's unless a test says otherwise. */
    private val maxPictures: Int = MAX_PICTURES,
    private val maxTotal: Long = MAX_TOTAL,
) {
    private class Entry(val size: Long, val stored: Long)

    private val entries = HashMap<String, Entry>()
    private var total = 0L
    private var scanned = false

    /** The picture kept for [url] and still good, or null; one past its time, damaged or not a picture is removed. */
    @Synchronized
    fun get(url: String, looks: (ByteArray) -> Boolean = ::looksLikeAPicture): ByteArray? {
        scan()
        val name = nameOf(url)
        val entry = entries[name] ?: return null
        val bytes = if (clock() - entry.stored > MAX_AGE_MILLIS) null else readFile(File(dir, name))
        val good = bytes != null && bytes.size <= MAX_PICTURE && looks(bytes)
        if (!good) drop(name)
        return bytes.takeIf { good }
    }

    /** Keeps [bytes] for [url] when they are a picture within the size cap; whether they were kept. */
    @Synchronized
    fun put(url: String, bytes: ByteArray, looks: (ByteArray) -> Boolean = ::looksLikeAPicture): Boolean {
        if (!acceptable(url, bytes, looks)) return false
        scan()
        val name = nameOf(url)
        val written = runCatching {
            dir.mkdirs()
            val temporary = File(dir, "$name$TEMPORARY")
            temporary.writeBytes(bytes)
            val target = File(dir, name)
            temporary.renameTo(target).also { if (!it) temporary.delete() }
        }.getOrDefault(false)
        if (!written) return false
        entries.remove(name)?.let { total -= it.size }
        entries[name] = Entry(bytes.size.toLong(), clock())
        total += bytes.size
        trim()
        return entries.containsKey(name)
    }

    private fun acceptable(url: String, bytes: ByteArray, looks: (ByteArray) -> Boolean): Boolean =
        bytes.isNotEmpty() && bytes.size <= MAX_PICTURE && url.length <= MAX_ADDRESS && looks(bytes)

    /** How many pictures are kept, and how many bytes they take. */
    @Synchronized
    fun usage(): Pair<Int, Long> {
        scan()
        return entries.size to total
    }

    /** Removes every picture; whether the folder is empty after. */
    @Synchronized
    fun clear(): Boolean {
        val listed = runCatching { dir.listFiles() }.getOrNull()
        listed?.forEach { runCatching { it.delete() } }
        entries.clear()
        total = 0L
        scanned = true
        return runCatching { dir.listFiles()?.isEmpty() ?: true }.getOrDefault(false)
    }

    /** The files that are there, read once; a leftover of an interrupted write goes. */
    private fun scan() {
        if (scanned) return
        scanned = true
        val listed = runCatching { dir.listFiles() }.getOrNull() ?: return
        for (file in listed.take(MAX_ENTRIES)) {
            if (file.name.endsWith(TEMPORARY) || !isOurName(file.name)) {
                runCatching { file.delete() }
            } else {
                entries[file.name] = Entry(file.length(), file.lastModified())
                total += file.length()
            }
        }
    }

    /** Drops what is past its time, then the oldest, until the caps are met. */
    private fun trim() {
        val now = clock()
        entries.filter { now - it.value.stored > MAX_AGE_MILLIS }.keys.forEach { drop(it) }
        if (entries.size <= maxPictures && total <= maxTotal) return
        for ((name, _) in entries.entries.sortedBy { it.value.stored }) {
            if (entries.size <= maxPictures && total <= maxTotal) break
            drop(name)
        }
    }

    private fun drop(name: String) {
        runCatching { File(dir, name).delete() }
        entries.remove(name)?.let { total -= it.size }
    }

    private fun readFile(file: File): ByteArray? = try {
        if (file.length() > MAX_PICTURE) null else file.readBytes()
    } catch (_: IOException) {
        null
    }

    private fun nameOf(url: String): String =
        MessageDigest.getInstance("SHA-256").digest(url.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(java.util.Locale.ROOT, it) }

    private fun isOurName(name: String): Boolean =
        name.length == HASH_CHARS && name.all { it in '0'..'9' || it in 'a'..'f' }

    companion object {
        /** The desktop's caps: 4,000 pictures, 256 MB, 30 days. */
        const val MAX_PICTURES = 4_000
        const val MAX_TOTAL = 256L * 1024 * 1024
        const val MAX_AGE_MILLIS = 30L * 86_400_000L

        /** The most bytes of one picture, as the downloader's own cap. */
        const val MAX_PICTURE = 8 * 1024 * 1024
        private const val MAX_ADDRESS = 16 * 1024
        private const val MAX_ENTRIES = 100_000
        private const val HASH_CHARS = 64
        private const val TEMPORARY = ".tmp"
    }
}
